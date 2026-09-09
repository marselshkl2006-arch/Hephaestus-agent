//! Doc Generator — генерация Markdown API-справки из doc-комментариев и
//! сигнатур публичных функций/классов. Аналог `doc_generator.py`.
//!
//! В отличие от `diagram_tool.rs` (структура кода — кто от кого зависит),
//! этот инструмент вытаскивает ИМЕННО документацию: `///`/`//!` в Rust,
//! docstring после `def`/`class` в Python, `/** ... */` в JS/TS/Java —
//! и собирает по ним читаемый reference, а не диаграмму связей.
//!
//! Как и `class_diagram`/`file_tree` в `diagram_tools.rs` — разбор
//! регулярками, без полноценного парсера AST для каждого языка. Это
//! осознанный компромисс: агенту не нужен идеальный AST, достаточно
//! вытащить "какие есть публичные функции/классы и что о них написано" за
//! доли секунды на файл, без тяжёлых зависимостей на пять разных парсеров.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use walkdir::WalkDir;

use super::{Tool, ToolResult};

#[derive(Debug, Clone)]
struct DocEntry {
    kind: &'static str, // "fn" | "class"
    name: String,
    signature: String,
    doc: String,
}

enum Lang {
    Rust,
    Python,
    JsLike, // JS/TS/Java/C#/Go — общий стиль /** */ и function/class
    Unknown,
}

fn detect_lang(path: &Path) -> Lang {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => Lang::Rust,
        Some("py") => Lang::Python,
        Some("js" | "ts" | "tsx" | "jsx" | "java" | "cs" | "go") => Lang::JsLike,
        _ => Lang::Unknown,
    }
}

/// Rust: `/// doc` строки прямо перед `pub fn` / `pub struct` / `pub enum`.
fn extract_rust(source: &str) -> Vec<DocEntry> {
    let item_re = Regex::new(
        r"(?m)^\s*pub\s+(fn|struct|enum|trait)\s+([A-Za-z_][A-Za-z0-9_]*)\s*[<\(]?[^\n{;]*",
    )
    .unwrap();
    let mut entries = Vec::new();
    let lines: Vec<&str> = source.lines().collect();

    for cap in item_re.captures_iter(source) {
        let full_match = cap.get(0).unwrap();
        let kind = if &cap[1] == "fn" { "fn" } else { "class" };
        let name = cap[2].to_string();
        let signature = full_match.as_str().trim().trim_end_matches('{').trim().to_string();

        // Найти номер строки совпадения и собрать блок `///` непосредственно над ней.
        let line_no = source[..full_match.start()].matches('\n').count();
        let mut doc_lines = Vec::new();
        let mut i = line_no;
        while i > 0 {
            let prev = lines[i - 1].trim();
            if let Some(rest) = prev.strip_prefix("///") {
                doc_lines.push(rest.trim().to_string());
                i -= 1;
            } else {
                break;
            }
        }
        doc_lines.reverse();
        entries.push(DocEntry { kind, name, signature, doc: doc_lines.join(" ") });
    }
    entries
}

/// Python: docstring — первая `"""..."""`/`'''...'''` сразу после
/// `def ...:` / `class ...:`.
fn extract_python(source: &str) -> Vec<DocEntry> {
    let def_re = Regex::new(r"(?m)^(\s*)(def|class)\s+([A-Za-z_][A-Za-z0-9_]*)\s*(\([^\n]*\))?\s*:").unwrap();
    // ВНИМАНИЕ: крейт `regex` НЕ поддерживает backreference (\1), поэтому
    // нельзя написать ("""|''')...\1 — Regex::new вернёт Err и unwrap()
    // запаниковал бы при каждом вызове на .py файле. Вместо этого — две
    // явные альтернативы, какая группа совпала, определяем по номеру.
    let doc_re = Regex::new(r#"^\s*(?:"""([\s\S]*?)"""|'''([\s\S]*?)''')"#).unwrap();
    let mut entries = Vec::new();

    for cap in def_re.captures_iter(source) {
        let indent = cap[1].len();
        let kind = if &cap[2] == "def" { "fn" } else { "class" };
        let name = cap[3].to_string();
        let args = cap.get(4).map(|m| m.as_str()).unwrap_or("()");
        // Пропускаем "приватные" по конвенции (_name), кроме __init__.
        if name.starts_with('_') && name != "__init__" {
            continue;
        }
        let signature = format!("{} {}{}", &cap[2], name, args);

        let after = &source[cap.get(0).unwrap().end()..];
        let after_trimmed = after.trim_start_matches(['\n', '\r']);
        let doc = doc_re
            .captures(after_trimmed)
            .and_then(|m| m.get(1).or_else(|| m.get(2)))
            .map(|m| m.as_str().trim().replace('\n', " "))
            .unwrap_or_default();

        let _ = indent; // используется только для потенциального будущего вложенного разбора
        entries.push(DocEntry { kind, name, signature, doc });
    }
    entries
}

/// JS/TS/Java/Go и т.п.: `/** ... */` непосредственно перед
/// `function`/`class`/публичным методом.
fn extract_jslike(source: &str) -> Vec<DocEntry> {
    let block_re = Regex::new(
        r"/\*\*([\s\S]*?)\*/\s*\n\s*(export\s+)?(function|class)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*([^\n{]*)",
    )
    .unwrap();
    let mut entries = Vec::new();
    for cap in block_re.captures_iter(source) {
        let kind = if &cap[3] == "function" { "fn" } else { "class" };
        let name = cap[4].to_string();
        let signature = format!("{} {}{}", &cap[3], name, cap[5].trim());
        let doc_raw = &cap[1];
        let doc = doc_raw
            .lines()
            .map(|l| l.trim().trim_start_matches('*').trim())
            .filter(|l| !l.is_empty() && !l.starts_with('@'))
            .collect::<Vec<_>>()
            .join(" ");
        entries.push(DocEntry { kind, name, signature, doc });
    }
    entries
}

fn render_markdown(file_label: &str, entries: &[DocEntry]) -> String {
    if entries.is_empty() {
        return format!("## {file_label}\n\n_Публичных элементов с документацией не найдено._\n");
    }
    let mut out = format!("## {file_label}\n\n");
    for e in entries {
        let heading = if e.kind == "class" { "###" } else { "####" };
        out.push_str(&format!("{heading} `{}`\n\n", e.signature));
        if !e.doc.is_empty() {
            out.push_str(&format!("{}\n\n", e.doc));
        } else {
            out.push_str("_(без описания)_\n\n");
        }
    }
    out
}

fn generate_for_file(path: &Path) -> Result<String, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("не удалось прочитать {}: {e}", path.display()))?;
    let entries = match detect_lang(path) {
        Lang::Rust => extract_rust(&source),
        Lang::Python => extract_python(&source),
        Lang::JsLike => extract_jslike(&source),
        Lang::Unknown => return Err(format!("неподдерживаемый тип файла: {}", path.display())),
    };
    Ok(render_markdown(&path.display().to_string(), &entries))
}

fn generate_for_dir(dir: &Path, extensions: &[&str]) -> String {
    let mut out = format!("# API-справка: {}\n\n", dir.display());
    let mut files: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|ext| extensions.contains(&ext))
                .unwrap_or(false)
        })
        .filter(|p| !p.components().any(|c| matches!(c.as_os_str().to_str(), Some("target" | "node_modules" | ".git" | "venv" | "__pycache__"))))
        .collect();
    files.sort();

    if files.is_empty() {
        return format!("{out}_В директории не найдено файлов с расширениями {:?}._\n", extensions);
    }

    for f in files {
        match generate_for_file(&f) {
            Ok(section) => out.push_str(&section),
            Err(e) => out.push_str(&format!("## {}\n\n_Ошибка: {e}_\n\n", f.display())),
        }
    }
    out
}

pub struct DocGeneratorTool;

impl DocGeneratorTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DocGeneratorTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let file_path = args.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string());
        let directory = args.get("directory").and_then(|v| v.as_str()).map(|s| s.to_string());
        let save_to = args.get("save_to").and_then(|v| v.as_str()).map(|s| s.to_string());
        let extensions: Vec<String> = args
            .get("extensions")
            .and_then(|v| v.as_str())
            .map(|s| s.split(',').map(|e| e.trim().trim_start_matches('.').to_string()).collect())
            .unwrap_or_else(|| vec!["rs".to_string(), "py".to_string()]);

        // fs::read_to_string/WalkDir — синхронные, уводим на blocking-пул,
        // чтобы не держать executor на больших деревьях.
        let markdown = tokio::task::spawn_blocking(move || -> Result<String, String> {
            if let Some(fp) = file_path {
                generate_for_file(Path::new(&fp))
            } else if let Some(dir) = directory {
                let ext_refs: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
                Ok(generate_for_dir(Path::new(&dir), &ext_refs))
            } else {
                Err("doc_generator: нужен либо 'file_path', либо 'directory'".to_string())
            }
        })
        .await;

        let markdown = match markdown {
            Ok(Ok(md)) => md,
            Ok(Err(e)) => return ToolResult::error(e),
            Err(e) => return ToolResult::error(format!("задача паникнула: {e}")),
        };

        if let Some(path) = save_to {
            if let Err(e) = std::fs::write(&path, &markdown) {
                return ToolResult::error(format!("не удалось сохранить в {path}: {e}"));
            }
            return ToolResult::success(format!("Справка сохранена в {path} ({} симв.)", markdown.len()));
        }

        ToolResult::success(markdown)
    }

    fn name(&self) -> &'static str {
        "doc_generator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Регрессия: раньше extract_python паниковал на Regex::new с
    /// backreference \1 (крейт regex их не поддерживает) — то есть любой
    /// вызов doc_generator на .py файле падал. Плюс проверяем, что сам
    /// docstring теперь реально вытаскивается.
    #[test]
    fn test_extract_python_docstrings() {
        let src = "\ndef greet(name):\n\
                   \"\"\"Возвращает приветствие.\n\nВторая строка.\"\"\"\n\
                   return name\n\n\
                   class Widget:\n\
                   '''Класс виджета'''\n\
                   pass\n";

        let entries = extract_python(src);
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].kind, "fn");
        assert_eq!(entries[0].name, "greet");
        assert_eq!(entries[0].signature, "def greet(name)");
        assert!(entries[0].doc.contains("Возвращает приветствие"));
        assert!(entries[0].doc.contains("Вторая строка."));

        assert_eq!(entries[1].kind, "class");
        assert_eq!(entries[1].name, "Widget");
        assert_eq!(entries[1].doc, "Класс виджета");
    }

    #[test]
    fn test_extract_rust_docs() {
        let src = "/// Складывает числа.\npub fn add(a: u32, b: u32) -> u32 {\n    a + b\n}\n";
        let entries = extract_rust(src);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "add");
        assert_eq!(entries[0].doc, "Складывает числа.");
    }
}

// ══════════════════════════════════════════ document_create ════════════
// Генератор ПОЛНОЦЕННЫХ документов из Markdown: DOCX (MS Word), ODT
// (LibreOffice), XLSX/CSV (таблицы), HTML, MD. Чистый Rust: DOCX/ODT/XLSX —
// это ZIP-пакеты со стандартизированным XML внутри (OOXML / OpenDocument),
// собираем их сами — никаких внешних конвертеров и установленного офиса.
//
// Почему Markdown на входе: LLM идеально владеет им; модель описывает
// документ обычной разметкой (# заголовки, - списки, |таблицы|, ```код```),
// инструмент рендерит в выбранный формат. Плюс прямой JSON-blocks вход
// не нужен — md покрывает всё.

use std::io::Write as _;

#[derive(Debug, Clone)]
enum DocBlock {
    H(u8, String),
    Para(String),
    Bullets(Vec<String>),
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
    Code(String),
}

struct ParsedDoc {
    title: String,
    blocks: Vec<DocBlock>,
}

/// Мини-парсер Markdown-подмножества: #/##/### , "- " списки,
/// ```код```, таблицы |a|b| с разделителем |---|---|, абзацы.
fn parse_markdown_doc(md: &str) -> ParsedDoc {
    let mut blocks = Vec::new();
    let mut title = String::new();

    #[derive(PartialEq)]
    enum Mode {
        Normal,
        Code,
        List,
        Table,
    }
    let mut mode = Mode::Normal;
    let mut para: Vec<String> = Vec::new();
    let mut code: Vec<String> = Vec::new();
    let mut list: Vec<String> = Vec::new();
    let mut tbl_rows: Vec<Vec<String>> = Vec::new();
    let mut pending_hdr: Option<Vec<String>> = None;

    fn flush_para(p: &mut Vec<String>, out: &mut Vec<DocBlock>) {
        if !p.is_empty() {
            out.push(DocBlock::Para(p.join(" ").trim().to_string()));
            p.clear();
        }
    }
    let split_row = |l: &str| -> Vec<String> {
        l.trim()
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(|c| c.trim().to_string())
            .collect()
    };

    for raw in md.lines() {
        let line = raw.to_string();
        if mode == Mode::Code {
            if raw.trim_start().starts_with("```") {
                blocks.push(DocBlock::Code(code.join("\n")));
                code.clear();
                mode = Mode::Normal;
            } else {
                code.push(raw.to_string());
            }
            continue;
        }
        if raw.trim_start().starts_with("```") {
            flush_para(&mut para, &mut blocks);
            mode = Mode::Code;
            continue;
        }

        // Таблицы: заголовочная строка в пайпах, затем разделитель |---|.
        // ИСПРАВЛЕНО: раньше заголовок терялся — сепаратор включал режим
        // таблицы ДО того, как строка заголовка куда-то попадала.
        if line.trim().starts_with('|') && line.contains('|') {
            let cells = split_row(&line);
            let is_sep = cells.iter().all(|c| c.is_empty() || c.chars().all(|ch| ch == '-' || ch == ':'));
            if is_sep {
                if mode != Mode::Table {
                    if let Some(h) = pending_hdr.take() {
                        tbl_rows.push(h);
                    }
                    mode = Mode::Table;
                }
                continue;
            }
            if mode == Mode::Table {
                tbl_rows.push(cells);
                continue;
            }
            // Возможный заголовок будущей таблицы — запомним; если дальше
            // не будет сепаратора, вернём строку как обычный абзац.
            flush_para(&mut para, &mut blocks);
            pending_hdr = Some(cells);
            continue;
        }
        if mode == Mode::Table {
            // Таблица закончилась (строка без пайпов)
            if !tbl_rows.is_empty() {
                let headers = tbl_rows.remove(0);
                blocks.push(DocBlock::Table { headers, rows: tbl_rows.clone() });
                tbl_rows.clear();
            }
            mode = Mode::Normal;
        }

        let t = line.trim();
        if let Some(h) = t.strip_prefix("### ") {
            flush_para(&mut para, &mut blocks);
            blocks.push(DocBlock::H(3, h.trim().to_string()));
        } else if let Some(h) = t.strip_prefix("## ") {
            flush_para(&mut para, &mut blocks);
            blocks.push(DocBlock::H(2, h.trim().to_string()));
        } else if let Some(h) = t.strip_prefix("# ") {
            flush_para(&mut para, &mut blocks);
            if title.is_empty() {
                title = h.trim().to_string();
            }
            blocks.push(DocBlock::H(1, h.trim().to_string()));
        } else if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            flush_para(&mut para, &mut blocks);
            list.push(item.trim().to_string());
            mode = Mode::List;
        } else if t.is_empty() {
            flush_para(&mut para, &mut blocks);
            if mode == Mode::List && !list.is_empty() {
                blocks.push(DocBlock::Bullets(std::mem::take(&mut list)));
            }
        } else {
            if mode == Mode::List && !list.is_empty() {
                blocks.push(DocBlock::Bullets(std::mem::take(&mut list)));
            }
            para.push(t.to_string());
        }
    }
    if mode == Mode::Table && !tbl_rows.is_empty() {
        let headers = tbl_rows.remove(0);
        blocks.push(DocBlock::Table { headers, rows: tbl_rows.clone() });
        tbl_rows.clear();
    }
    if let Some(h) = pending_hdr.take() {
        blocks.push(DocBlock::Para(h.join(" | ")));
    }
    if mode == Mode::Code && !code.is_empty() {
        blocks.push(DocBlock::Code(code.join("\n")));
    }
    if mode == Mode::Table && !tbl_rows.is_empty() {
        let headers = tbl_rows.remove(0);
        blocks.push(DocBlock::Table { headers, rows: tbl_rows.clone() });
    }
    if !list.is_empty() {
        blocks.push(DocBlock::Bullets(list));
    }
    flush_para(&mut para, &mut blocks);

    ParsedDoc { title, blocks }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ───────────────────────────────────────────────────────────── ODT ──

fn write_odt(path: &std::path::Path, doc: &ParsedDoc) -> Result<(), String> {
    use xml_escape as esc;

    let mut body = String::new();
    let mut table_no = 0;
    for b in &doc.blocks {
        match b {
            DocBlock::H(l, t) => body.push_str(&format!(
                "<text:h text:outline-level=\"{l}\">{}</text:h>",
                esc(t)
            )),
            DocBlock::Para(t) => body.push_str(&format!("<text:p>{}</text:p>", esc(t))),
            DocBlock::Bullets(items) => {
                body.push_str("<text:list><text:list-item>");
                for it in items {
                    body.push_str(&format!("<text:p>• {}</text:p>", esc(it)));
                }
                body.push_str("</text:list-item></text:list>");
            }
            DocBlock::Code(c) => body.push_str(&format!(
                "<text:p text:style-name=\"Preformatted_Text\">{}</text:p>",
                esc(c)
            )),
            DocBlock::Table { headers, rows } => {
                table_no += 1;
                body.push_str(&format!(
                    "<table:table table:name=\"T{table_no}\"><table:table-column table:number-columns-repeated=\"{}\"/>",
                    headers.len().max(1)
                ));
                body.push_str("<table:table-row>");
                for h in headers {
                    body.push_str(&format!(
                        "<table:table-cell office:value-type=\"string\"><text:p><text:span text:style-name=\"Strong_20_Emphasis\">{}</text:span></text:p></table:table-cell>",
                        esc(h)
                    ));
                }
                body.push_str("</table:table-row>");
                for r in rows {
                    body.push_str("<table:table-row>");
                    for c in r {
                        body.push_str(&format!(
                            "<table:table-cell office:value-type=\"string\"><text:p>{}</text:p></table:table-cell>",
                            esc(c)
                        ));
                    }
                    body.push_str("</table:table-row>");
                }
                body.push_str("</table:table>");
            }
        }
    }

    let content_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" \
xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" \
xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" \
office:version=\"1.2\"><office:body><office:text>{body}</office:text></office:body></office:document-content>"
    );

    let manifest = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.2\">\
<manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"application/vnd.oasis.opendocument.text\"/>\
<manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/>\
</manifest:manifest>";

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    // mimetype обязан быть ПЕРВОЙ записью и БЕЗ сжатия (спецификация ODF).
    z.start_file(
        "mimetype",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored),
    )
    .map_err(|e| e.to_string())?;
    z.write_all(b"application/vnd.oasis.opendocument.text").map_err(|e| e.to_string())?;
    z.start_file("META-INF/manifest.xml", zip::write::SimpleFileOptions::default()).map_err(|e| e.to_string())?;
    z.write_all(manifest.as_bytes()).map_err(|e| e.to_string())?;
    z.start_file("content.xml", zip::write::SimpleFileOptions::default()).map_err(|e| e.to_string())?;
    z.write_all(content_xml.as_bytes()).map_err(|e| e.to_string())?;
    z.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ──────────────────────────────────────────────────────────── DOCX ──

fn write_docx(path: &std::path::Path, doc: &ParsedDoc) -> Result<(), String> {
    use xml_escape as esc;

    let mut body = String::new();
    for b in &doc.blocks {
        match b {
            DocBlock::H(l, t) => body.push_str(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"Heading{l}\"/></w:pPr><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
                esc(t)
            )),
            DocBlock::Para(t) => body.push_str(&format!(
                "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
                esc(t)
            )),
            DocBlock::Bullets(items) => {
                for it in items {
                    body.push_str(&format!(
                        "<w:p><w:pPr><w:ind w:left=\"360\"/></w:pPr><w:r><w:t xml:space=\"preserve\">• {}</w:t></w:r></w:p>",
                        esc(it)
                    ));
                }
            }
            DocBlock::Code(c) => {
                for l in c.lines() {
                    body.push_str(&format!(
                        "<w:p><w:pPr><w:shd w:val=\"clear\" w:fill=\"F2F2F2\"/></w:pPr><w:r><w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
                        esc(l)
                    ));
                }
            }
            DocBlock::Table { headers, rows } => {
                let cols = headers.len().max(1);
                let mut borders = String::from("<w:tblBorders>");
                for s in ["top", "left", "bottom", "right", "insideH", "insideV"] {
                    borders.push_str(&format!(
                        "<w:{s} w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>"
                    ));
                }
                borders.push_str("</w:tblBorders>");
                body.push_str(&format!(
                    // ИСПРАВЛЕНО: </w:tblGrid> не закрывался — LibreOffice
                    // показывал заставку и вылетал ("mismatched tag").
                    "<w:tbl><w:tblPr><w:tblW w:w=\"5000\" w:type=\"pct\"/>{borders}</w:tblPr>\
<w:tblGrid>{}{}</w:tblGrid>",
                    "<w:gridCol/>".repeat(cols),
                    ""
                ));
                let cell = |t: &str, bold: bool| {
                    let rpr = if bold { "<w:rPr><w:b/></w:rPr>" } else { "" };
                    format!(
                        "<w:tc><w:tcPr/><w:p><w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p></w:tc>",
                        esc(t)
                    )
                };
                body.push_str("<w:tr>");
                for h in headers {
                    body.push_str(&cell(h, true));
                }
                body.push_str("</w:tr>");
                for r in rows {
                    body.push_str("<w:tr>");
                    for c in r {
                        body.push_str(&cell(c, false));
                    }
                    body.push_str("</w:tr>");
                }
                body.push_str("</w:tbl><w:p/>");
            }
        }
    }

    let document_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}\
<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:body></w:document>"
    );

    let styles_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:style w:type=\"paragraph\" w:styleId=\"Normal\" w:default=\"1\"><w:name w:val=\"Normal\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"36\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"30\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:outlineLvl w:val=\"2\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"26\"/></w:rPr></w:style>\
</w:styles>";

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
</Types>";

    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
</Relationships>";

    let doc_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
</Relationships>";

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    let parts: Vec<(&str, Vec<u8>)> = vec![
        ("[Content_Types].xml", content_types.as_bytes().to_vec()),
        ("_rels/.rels", root_rels.as_bytes().to_vec()),
        ("word/document.xml", document_xml.into_bytes()),
        ("word/styles.xml", styles_xml.as_bytes().to_vec()),
        ("word/_rels/document.xml.rels", doc_rels.as_bytes().to_vec()),
    ];
    for (name, data) in parts {
        z.start_file(name, opts).map_err(|e| e.to_string())?;
        z.write_all(&data).map_err(|e| e.to_string())?;
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ──────────────────────────────────────────────────────────── XLSX ──

fn write_xlsx(path: &std::path::Path, doc: &ParsedDoc) -> Result<(), String> {
    use xml_escape as esc;

    // Все таблицы документа -> отдельные листы Sheet1..N.
    let tables: Vec<&DocBlock> = doc
        .blocks
        .iter()
        .filter(|b| matches!(b, DocBlock::Table { .. }))
        .collect();
    if tables.is_empty() {
        return Err("в документе нет таблиц для XLSX — используйте docx/odt/html/md".into());
    }

    let col_letter = |mut n: usize| -> String {
        let mut s = String::new();
        loop {
            s.insert(0, (b'A' + (n % 26) as u8) as char);
            if n < 26 { break; }
            n = n / 26 - 1;
        }
        s
    };

    let mut sheets_xml = String::new();
    let mut sheet_files = String::new();
    let mut sheet_rels = String::new();
    let count = tables.len();
    for (i, t) in tables.iter().enumerate() {
        let sid = i + 1;
        sheets_xml.push_str(&format!(
            "<sheet name=\"Sheet{sid}\" sheetId=\"{sid}\" r:id=\"rId{sid}\"/>"
        ));
        sheet_rels.push_str(&format!(
            "<Relationship Id=\"rId{sid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{sid}.xml\"/>"
        ));
        let DocBlock::Table { headers, rows } = t else { unreachable!() };
        let mut rows_xml = String::new();
        let mut row_no = 1usize;
        let emit_row = |rows_xml: &mut String, row_no: &mut usize, cells: &[String]| {
            let mut r = format!("<row r=\"{}\">", *row_no);
            for (ci, val) in cells.iter().enumerate() {
                let letter = col_letter(ci + 1);
                r.push_str(&format!(
                    "<c r=\"{letter}{row_no}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    esc(val)
                ));
            }
            r.push_str("</row>");
            rows_xml.push_str(&r);
            *row_no += 1;
        };
        emit_row(&mut rows_xml, &mut row_no, headers);
        for r in rows {
            emit_row(&mut rows_xml, &mut row_no, r);
        }
        sheet_files.push_str(&format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows_xml}</sheetData></worksheet>"
        ));
    }

    let workbook = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>{sheets_xml}</sheets></workbook>"
    );
    let wb_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{sheet_rels}</Relationships>"
    );
    let content_types = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
{}\
</Types>",
        (1..=count)
            .map(|sid| format!(
                "<Override PartName=\"/xl/worksheets/sheet{sid}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
            ))
            .collect::<String>()
    );
    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
</Relationships>";

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    let static_parts: Vec<(&str, Vec<u8>)> = vec![
        ("[Content_Types].xml", content_types.as_bytes().to_vec()),
        ("_rels/.rels", root_rels.as_bytes().to_vec()),
        ("xl/workbook.xml", workbook.into_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.into_bytes()),
    ];
    for (name, data) in static_parts {
        z.start_file(name, opts).map_err(|e| e.to_string())?;
        z.write_all(&data).map_err(|e| e.to_string())?;
    }
    // Листы с данными (пересобираем содержимое — данные уже в tables).
    for (i, t) in tables.iter().enumerate() {
        let DocBlock::Table { headers, rows } = t else { unreachable!() };
        let mut rows_xml = String::new();
        let mut row_no = 1usize;
        {
            let emit = |rows_xml: &mut String, row_no: &mut usize, cells: &[String]| {
                let mut r = format!("<row r=\"{}\">", *row_no);
                for (ci, val) in cells.iter().enumerate() {
                    let letter = col_letter(ci + 1);
                    r.push_str(&format!(
                        "<c r=\"{letter}{row_no}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                        esc(val)
                    ));
                }
                r.push_str("</row>");
                rows_xml.push_str(&r);
                *row_no += 1;
            };
            emit(&mut rows_xml, &mut row_no, headers);
            for r in rows {
                emit(&mut rows_xml, &mut row_no, r);
            }
        }
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows_xml}</sheetData></worksheet>"
        );
        let sid = i + 1;
        z.start_file(&format!("xl/worksheets/sheet{sid}.xml"), opts).map_err(|e| e.to_string())?;
        z.write_all(xml.as_bytes()).map_err(|e| e.to_string())?;
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ─────────────────────────────────────── HTML / CSV / Markdown ──

fn write_html(path: &std::path::Path, doc: &ParsedDoc) -> Result<(), String> {
    use xml_escape as esc;
    let mut body = String::new();
    for b in &doc.blocks {
        match b {
            DocBlock::H(l, t) => body.push_str(&format!("<h{l}>{}</h{l}>{}", esc(t), "\n")),
            DocBlock::Para(t) => body.push_str(&format!("<p>{}</p>\n", esc(t))),
            DocBlock::Bullets(items) => {
                body.push_str("<ul>\n");
                for it in items {
                    body.push_str(&format!("<li>{}</li>\n", esc(it)));
                }
                body.push_str("</ul>\n");
            }
            DocBlock::Code(c) => body.push_str(&format!("<pre><code>{}</code></pre>\n", esc(c))),
            DocBlock::Table { headers, rows } => {
                body.push_str("<table border=\"1\">\n<tr>");
                for h in headers {
                    body.push_str(&format!("<th>{}</th>", esc(h)));
                }
                body.push_str("</tr>\n");
                for r in rows {
                    body.push_str("<tr>");
                    for c in r {
                        body.push_str(&format!("<td>{}</td>", esc(c)));
                    }
                    body.push_str("</tr>\n");
                }
                body.push_str("</table>\n");
            }
        }
    }
    let html = format!(
        "<!DOCTYPE html><html lang=\"ru\"><head><meta charset=\"utf-8\"><title>{}</title>\
<style>body{{font-family:sans-serif;max-width:900px;margin:2rem auto;line-height:1.5}}\
pre{{background:#f4f4f4;padding:.7rem;overflow:auto}}</style></head><body>{}</body></html>",
        esc(&doc.title),
        body
    );
    std::fs::write(path, html).map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────── PPTX ──

/// Слайд презентации: заголовок + строки контента (без инлайн-разметки —
/// PPTX-текст рендерим простыми параграфами, буллеты как "• ").
struct Slide {
    title: String,
    lines: Vec<String>,
}

/// ParsedDoc → слайды. Правила (markdown-презентации как в Marp):
/// первый `# ` — титульный слайд; каждый `## ` — заголовок нового слайда;
/// буллеты/абзацы/код/таблицы — контент текущего слайда; `### ` — строка
/// подзаголовка. Если H1/H2 нет вообще — один слайд со всем содержимым.
fn doc_to_slides(doc: &ParsedDoc) -> Vec<Slide> {
    let mut slides: Vec<Slide> = Vec::new();
    let mut cur: Option<Slide> = None;
    let mut title_used = false;

    fn flush(cur: &mut Option<Slide>, slides: &mut Vec<Slide>) {
        if let Some(s) = cur.take() {
            slides.push(s);
        }
    }
    fn push_line(cur: &mut Option<Slide>, slides: &mut Vec<Slide>, line: String) {
        if line.trim().is_empty() {
            return;
        }
        if cur.is_none() {
            *cur = Some(Slide { title: String::new(), lines: Vec::new() });
        }
        if let Some(s) = cur.as_mut() {
            s.lines.push(line);
        }
        let _ = slides;
    }

    for b in &doc.blocks {
        match b {
            DocBlock::H(1, t) => {
                // Первый H1 — титульный слайд (подзаголовки после него
                // попадают в его же контент, до первого H2).
                flush(&mut cur, &mut slides);
                cur = Some(Slide { title: t.clone(), lines: Vec::new() });
                if !title_used {
                    title_used = true;
                }
            }
            DocBlock::H(2, t) => {
                flush(&mut cur, &mut slides);
                cur = Some(Slide { title: t.clone(), lines: Vec::new() });
            }
            DocBlock::H(3, t) => push_line(&mut cur, &mut slides, format!("▸ {}", t)),
            DocBlock::Para(t) => push_line(&mut cur, &mut slides, t.clone()),
            DocBlock::Bullets(items) => {
                for it in items {
                    push_line(&mut cur, &mut slides, format!("• {}", it));
                }
            }
            DocBlock::Code(c) => {
                for l in c.lines() {
                    push_line(&mut cur, &mut slides, l.to_string());
                }
            }
            DocBlock::Table { headers, rows } => {
                push_line(&mut cur, &mut slides, headers.join(" | "));
                for r in rows {
                    push_line(&mut cur, &mut slides, r.join(" | "));
                }
            }
            DocBlock::H(_, t) => push_line(&mut cur, &mut slides, format!("▸ {}", t)),
        }
    }
    flush(&mut cur, &mut slides);

    if slides.is_empty() {
        // Ни H1, ни H2 — всё в один слайд с заголовком документа.
        let mut s = Slide { title: doc.title.clone(), lines: Vec::new() };
        for b in &doc.blocks {
            match b {
                DocBlock::Para(t) => s.lines.push(t.clone()),
                DocBlock::Bullets(items) => {
                    for it in items {
                        s.lines.push(format!("• {}", it));
                    }
                }
                _ => {}
            }
        }
        slides.push(s);
    }
    slides
}

/// Минимальный ВАЛИДНЫЙ PPTX (открывается PowerPoint и LibreOffice):
/// OOXML-презентация = ZIP с [Content_Types].xml, presentation.xml,
/// slideMaster, slideLayout, theme и слайдами. Ничего внешнего не нужно —
/// тот же принцип, что у write_docx.
fn write_pptx(path: &std::path::Path, doc: &ParsedDoc) -> Result<(), String> {
    use xml_escape as esc;

    let slides = doc_to_slides(doc);
    let n = slides.len();
    if n == 0 {
        return Err("нет контента для слайдов".into());
    }

    // ── слайды ──
    let mut slide_parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut sld_id_list = String::new();
    let mut pres_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"slideMasters/slideMaster1.xml\"/>",
    );
    let mut content_overrides = String::new();

    for (i, s) in slides.iter().enumerate() {
        let idx = i + 1;
        sld_id_list.push_str(&format!(
            "<p:sldId id=\"{}\" r:id=\"rId{}\"/>",
            256 + i,
            idx + 1
        ));
        pres_rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{}.xml\"/>",
            idx + 1,
            idx
        ));
        content_overrides.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
            idx
        ));

        // Титульный слайд (i == 0 и заголовок есть, контента нет) — крупный
        // текст по центру; остальные — заголовок сверху + контент.
        let is_title_only = i == 0 && s.lines.is_empty();
        let (t_off_y, t_size, t_bold) = if is_title_only {
            (2400300i64, 5400, true)
        } else {
            (365125i64, 3200, true)
        };
        let body_sp = if is_title_only {
            // Подзаголовок титульного — из doc.title не берём; пусто.
            String::new()
        } else {
            let mut paras = String::new();
            for line in &s.lines {
                paras.push_str(&format!(
                    "<a:p><a:pPr marL=\"0\" indent=\"0\"/><a:r><a:rPr lang=\"ru-RU\" sz=\"1800\"/>\
<a:t>{}</a:t></a:r></a:p>",
                    esc(line)
                ));
            }
            format!(
                "<p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Content\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr/></p:nvSpPr>\
<p:spPr><a:xfrm><a:off x=\"838200\" y=\"1825625\"/><a:ext cx=\"10515600\" cy=\"4572000\"/></a:xfrm></p:spPr>\
<p:txBody><a:bodyPr/><a:lstStyle/>{paras}</p:txBody></p:sp>"
            )
        };
        let slide_xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
<p:cSld><p:spTree>\
<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>\
<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr/></p:nvSpPr>\
<p:spPr><a:xfrm><a:off x=\"838200\" y=\"{t_off_y}\"/><a:ext cx=\"10515600\" cy=\"1325563\"/></a:xfrm></p:spPr>\
<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"ru-RU\" sz=\"{t_size}\" b=\"{t_bold}\"/><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>\
{body_sp}\
</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>",
            esc(&s.title)
        );

        let slide_rels = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>\
</Relationships>"
        );
        slide_parts.push((format!("ppt/slides/slide{}.xml", idx), slide_xml.into_bytes()));
        slide_parts.push((
            format!("ppt/slides/_rels/slide{}.xml.rels", idx),
            slide_rels.into_bytes(),
        ));
    }
    pres_rels.push_str("</Relationships>");

    let content_types = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>\
<Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/>\
<Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/>\
<Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>\
{content_overrides}\
</Types>"
    );

    let presentation_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<p:presentation xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
<p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>\
<p:sldIdLst>{sld_id_list}</p:sldIdLst>\
<p:sldSz cx=\"12192000\" cy=\"6858000\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/></p:presentation>"
    );

    // Тема: clrScheme/fontScheme/fmtScheme обязательны (fmtScheme — минимум
    // по 3 стиля в каждом списке, иначе PowerPoint считает файл битым).
    let theme_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Hephaestus">
<a:themeElements>
<a:clrScheme name="Hephaestus"><a:dk1><a:srgbClr val="1F1F1F"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2>
<a:accent1><a:srgbClr val="C0504D"/></a:accent1><a:accent2><a:srgbClr val="E8A33D"/></a:accent2>
<a:accent3><a:srgbClr val="4472C4"/></a:accent3><a:accent4><a:srgbClr val="70AD47"/></a:accent4>
<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="7030A0"/></a:accent6>
<a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="Hephaestus"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>
<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>
<a:fmtScheme name="Hephaestus">
<a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"><a:tint val="60000"/></a:schemeClr></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"><a:shade val="80000"/></a:schemeClr></a:solidFill></a:fillStyleLst>
<a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
<a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
<a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle>
<a:effectStyle><a:effectLst/></a:effectStyle>
<a:effectStyle><a:effectLst><a:outerShdw blurRad="40000" dist="23000" dir="5400000" algn="tl"><a:srgbClr val="000000"><a:alpha val="35000"/></a:srgbClr></a:outerShdw></a:effectLst></a:effectStyle></a:effectStyleLst>
<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/></a:schemeClr></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"><a:shade val="90000"/></a:schemeClr></a:solidFill></a:bgFillStyleLst>
</a:fmtScheme></a:themeElements></a:theme>"#;

    let master_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld>
<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst>
</p:sldMaster>"#;

    let master_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>"#;

    let layout_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank" preserve="1">
<p:cSld name="Blank"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld>
<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#;

    let layout_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>"#;

    let root_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#;

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    let mut parts: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), content_types.into_bytes()),
        ("_rels/.rels".into(), root_rels.as_bytes().to_vec()),
        ("ppt/presentation.xml".into(), presentation_xml.into_bytes()),
        ("ppt/_rels/presentation.xml.rels".into(), pres_rels.into_bytes()),
        ("ppt/slideMasters/slideMaster1.xml".into(), master_xml.as_bytes().to_vec()),
        ("ppt/slideMasters/_rels/slideMaster1.xml.rels".into(), master_rels.as_bytes().to_vec()),
        ("ppt/slideLayouts/slideLayout1.xml".into(), layout_xml.as_bytes().to_vec()),
        ("ppt/slideLayouts/_rels/slideLayout1.xml.rels".into(), layout_rels.as_bytes().to_vec()),
        ("ppt/theme/theme1.xml".into(), theme_xml.as_bytes().to_vec()),
    ];
    parts.extend(slide_parts);
    for (name, data) in parts {
        z.start_file(name, opts).map_err(|e| e.to_string())?;
        z.write_all(&data).map_err(|e| e.to_string())?;
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ───────────────────────────────────────────────────────────── RTF ──

/// RTF-эскейп: ASCII как есть (с \{ \} \\), не-ASCII (кириллица!) —
/// юникод-эскейпы \uN? (signed 16-bit; всё выше 32767 заменяем '?').
fn rtf_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\line "),
            _ if cp < 128 => out.push(c),
            _ if cp <= 32767 => out.push_str(&format!("\\u{}?", cp as i16)),
            _ => out.push('?'),
        }
    }
    out
}

/// RTF-документ: открывается Word, LibreOffice, Google Docs — универсальный
/// текстовый формат без XML и ZIP (одна строка escape-последовательностей).
fn write_rtf(path: &std::path::Path, doc: &ParsedDoc) -> Result<(), String> {
    let mut body = String::new();
    for b in &doc.blocks {
        match b {
            DocBlock::H(1, t) => body.push_str(&format!("\\fs40\\b {}\\b0\\fs22\\par\n", rtf_escape(t))),
            DocBlock::H(2, t) => body.push_str(&format!("\\fs32\\b {}\\b0\\fs22\\par\n", rtf_escape(t))),
            DocBlock::H(3, t) => body.push_str(&format!("\\fs26\\b {}\\b0\\fs22\\par\n", rtf_escape(t))),
            DocBlock::H(_, t) => body.push_str(&format!("\\fs26\\b {}\\b0\\fs22\\par\n", rtf_escape(t))),
            DocBlock::Para(t) => body.push_str(&format!("{}\\par\n", rtf_escape(t))),
            DocBlock::Bullets(items) => {
                for it in items {
                    body.push_str(&format!("\\bullet  {}\\par\n", rtf_escape(it)));
                }
            }
            DocBlock::Code(c) => {
                body.push_str("\\f1\\fs20 ");
                for l in c.lines() {
                    body.push_str(&format!("{}\\line\n", rtf_escape(l)));
                }
                body.push_str("\\f0\\fs22\\par\n");
            }
            DocBlock::Table { headers, rows } => {
                let cols = headers.len().max(1);
                // Ширина колонок: равные, по 2000 twips на колонку.
                let mut rowdef = String::from("\\trowd\\trgaph108");
                for i in 1..=cols {
                    rowdef.push_str(&format!("\\cellx{}", 2000 * i));
                }
                let cell = |t: &str, bold: bool| {
                    format!("\\intbl {}{}\\cell ", if bold { "\\b " } else { "" }, rtf_escape(t))
                };
                body.push_str(&rowdef);
                for h in headers {
                    body.push_str(&cell(h, true));
                }
                body.push_str("\\row\n");
                for r in rows {
                    body.push_str(&rowdef);
                    for c in r {
                        body.push_str(&cell(c, false));
                    }
                    body.push_str("\\row\n");
                }
                body.push_str("\\par\n");
            }
        }
    }
    let rtf = format!(
        "{{\\rtf1\\ansi\\deff0\\uc1\n{{\\fonttbl{{\\f0 Arial;}}{{\\f1 Consolas;}}}}\n\\f0\\fs22 {body}\n}}",
    );
    std::fs::write(path, rtf).map_err(|e| e.to_string())
}

fn write_csv(path: &std::path::Path, doc: &ParsedDoc) -> Result<(), String> {
    let table = doc.blocks.iter().find_map(|b| match b {
        DocBlock::Table { headers, rows } => Some((headers, rows)),
        _ => None,
    });
    let Some((headers, rows)) = table else {
        return Err("в документе нет таблиц для CSV — добавьте markdown-таблицу".into());
    };
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut out = String::new();
    out.push_str(&headers.iter().map(|h| q(h)).collect::<Vec<_>>().join(";"));
    out.push('\n');
    for r in rows {
        out.push_str(&r.iter().map(|c| q(c)).collect::<Vec<_>>().join(";"));
        out.push('\n');
    }
    std::fs::write(path, out).map_err(|e| e.to_string())
}

pub struct DocumentCreateTool {
    workdir: crate::workdir::WorkDir,
}

impl DocumentCreateTool {
    pub fn new(workdir: crate::workdir::WorkDir) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for DocumentCreateTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let Some(markdown) = args.get("markdown").and_then(|m| m.as_str()) else {
            return ToolResult::error("нужен параметр 'markdown' — содержимое документа в разметке");
        };
        let fmt = args
            .get("format")
            .and_then(|f| f.as_str())
            .unwrap_or("docx")
            .to_lowercase();
        let path_raw = args
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or("document");
        let ext = match fmt.as_str() {
            "docx" | "odt" | "xlsx" | "csv" | "html" | "md" | "pptx" | "rtf" => fmt.as_str(),
            other => {
                return ToolResult::error(format!(
                    "неизвестный формат '{other}'. Доступно: docx, odt, xlsx, csv, html, md, pptx (презентация), rtf"
                ))
            }
        };
        let final_path = if path_raw.ends_with(&format!(".{ext}")) {
            path_raw.to_string()
        } else {
            format!("{path_raw}.{ext}")
        };
        let path = self.workdir.resolve(&final_path);

        // ШАБЛОН report: титул + дата + гарантированные стандартные секции.
        let markdown_owned;
        let markdown = if args.get("template").and_then(|t| t.as_str()) == Some("report") {
            let title_from_args = args.get("title").and_then(|t| t.as_str());
            let body = markdown.to_string();
            let (h1, rest) = match body.strip_prefix("# ") {
                Some(after) => {
                    let end = after.find('\n').unwrap_or(after.len());
                    (after[..end].trim().to_string(), after[end..].trim_start_matches('\n').to_string())
                }
                None => (
                    title_from_args.unwrap_or("Отчёт").to_string(),
                    body,
                ),
            };
            let date = chrono::Local::now().format("%d.%m.%Y").to_string();
            let mut md = format!("# {h1}\n\n_Дата: {date}_\n\n");
            if !rest.contains("\n## ") && !rest.starts_with("## ") {
                md.push_str("## Краткое резюме\n\n_(заполните)_\n\n## Детали\n\n");
                md.push_str(&rest);
                md.push_str("\n\n## Результаты и выводы\n\n_(заполните)_");
            } else {
                md.push_str(&rest);
            }
            markdown_owned = md;
            markdown_owned.as_str()
        } else {
            markdown
        };

        let doc = parse_markdown_doc(markdown);
        let result = match ext {
            "odt" => write_odt(&path, &doc),
            "docx" => write_docx(&path, &doc),
            "xlsx" => write_xlsx(&path, &doc),
            "csv" => write_csv(&path, &doc),
            "html" => write_html(&path, &doc),
            "pptx" => write_pptx(&path, &doc),
            "rtf" => write_rtf(&path, &doc),
            "md" => std::fs::write(&path, markdown).map_err(|e| e.to_string()),
            _ => unreachable!(),
        };
        match result {
            Ok(_) => ToolResult::success(format!(
                "✅ Документ создан: {} ({} байт)",
                path.display(),
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            )),
            Err(e) => ToolResult::error(format!("Не удалось создать {ext}: {e}")),
        }
    }

    fn name(&self) -> &'static str {
        "document_create"
    }

    fn description(&self) -> &'static str {
        "Создать ДОКУМЕНТ или ПРЕЗЕНТАЦИЮ. Форматы: docx (Word), odt (LibreOffice), xlsx (Excel, таблицы становятся листами), csv, html, md, pptx (PowerPoint-презентация: первый '# ' — титульный слайд, каждый '## ' — новый слайд, '- ' списки и таблицы — контент слайда), rtf (универсальный текст). Содержимое параметром 'markdown': # заголовки, абзацы, '- ' списки, ```код```, таблицы |колонка|колонка| с разделителем |---|---|. Параметры 'path' (например reports/report.docx) и 'format'."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "markdown": {"type": "string", "description": "Содержимое в Markdown; для pptx каждый '## ' начинает новый слайд"},
                "title": {"type": "string", "description": "Заголовок документа (для template=report, если нет # в markdown)"},
                "format": {"type": "string", "enum": ["docx", "odt", "xlsx", "csv", "html", "md", "pptx", "rtf"], "description": "Формат файла, по умолчанию docx; pptx — презентация"},
                "path": {"type": "string", "description": "Путь сохранения (относительный или абсолютный)"},
                "template": {"type": "string", "enum": ["none", "report"], "description": "report — структура отчёта: титул, дата, секции Краткое резюме/Детали/Результаты"}
            },
            "required": ["markdown"]
        })
    }
}

// ════════════════════════════════════════ тесты генератора ═════════════
#[cfg(test)]
mod docgen_tests {
    use super::*;

    fn sample_md() -> String {
        "# Отчёт\n\nАбзац описания.\n\n- пункт один\n- пункт два\n\n\
         | Имя | Число |\n|---|---|\n| альфа | 1 |\n| бета | 2 |\n\n\
         ```rust\nfn hello() {}\n```"
            .to_string()
    }

    fn unzip_entry(path: &std::path::Path, name: &str) -> String {
        use std::io::Read;
        let f = std::fs::File::open(path).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn test_md_parse_blocks() {
        let d = parse_markdown_doc(&sample_md());
        assert_eq!(d.title, "Отчёт");
        assert!(d.blocks.iter().any(|b| matches!(b, DocBlock::H(1, t) if t == "Отчёт")));
        assert!(d.blocks.iter().any(|b| matches!(b, DocBlock::Bullets(v) if v.len() == 2)));
        match d.blocks.iter().find(|b| matches!(b, DocBlock::Table { .. })) {
            Some(DocBlock::Table { headers, rows }) => {
                assert_eq!(headers[0], "Имя");
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("таблица не распознана"),
        }
    }

    #[test]
    fn test_odt_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("t.odt");
        write_odt(&p, &parse_markdown_doc(&sample_md())).unwrap();
        // mimetype первой записью и без сжатия — требование ODF.
        let f = std::fs::File::open(&p).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        assert_eq!(z.by_index(0).unwrap().name(), "mimetype");
        let mt = unzip_entry(&p, "mimetype");
        assert_eq!(mt, "application/vnd.oasis.opendocument.text");
        let content = unzip_entry(&p, "content.xml");
        assert!(content.contains("Отчёт"));
        assert!(content.contains("table:table"));
    }

    #[test]
    fn test_docx_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("t.docx");
        write_docx(&p, &parse_markdown_doc(&sample_md())).unwrap();
        let doc = unzip_entry(&p, "word/document.xml");
        assert!(doc.contains("Heading1"));
        assert!(doc.contains("пункт один"));
        assert!(doc.contains("<w:tbl>"));
        let styles = unzip_entry(&p, "word/styles.xml");
        assert!(styles.contains("Heading1"));
    }

    #[test]
    fn test_xlsx_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("t.xlsx");
        write_xlsx(&p, &parse_markdown_doc(&sample_md())).unwrap();
        let sheet = unzip_entry(&p, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("inlineStr"));
        assert!(sheet.contains("Имя"));
        let wb = unzip_entry(&p, "xl/workbook.xml");
        assert!(wb.contains("Sheet1"));
    }

    #[test]
    fn test_csv_html() {
        let dir = tempfile::TempDir::new().unwrap();
        let doc = parse_markdown_doc(&sample_md());
        let csv_p = dir.path().join("t.csv");
        write_csv(&csv_p, &doc).unwrap();
        let csv = std::fs::read_to_string(&csv_p).unwrap();
        assert!(csv.starts_with("\"Имя\";\"Число\""));

        let html_p = dir.path().join("t.html");
        write_html(&html_p, &doc).unwrap();
        let html = std::fs::read_to_string(&html_p).unwrap();
        assert!(html.contains("<h1>Отчёт</h1>"));
        assert!(html.contains("<li>пункт два</li>"));
    }

    #[test]
    fn test_tool_execute_docx_end_to_end() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tool = DocumentCreateTool::new(crate::workdir::WorkDir::from_cwd());
            let dir = tempfile::TempDir::new().unwrap();
            let out = dir.path().join("report.docx").display().to_string();
            let res = tool
                .execute(&serde_json::json!({
                    "markdown": "# Тестовый документ\n\nСтрока.\n\n| A | B |\n|---|---|\n| 1 | 2 |",
                    "format": "docx",
                    "path": out,
                }))
                .await;
            assert!(res.success, "{}", res.error.unwrap_or_default());
            assert!(std::path::Path::new(&out).exists());
            // Регрессия "mismatched tag": grid обязан быть закрыт.
            let doc_xml = unzip_entry(std::path::Path::new(&out), "word/document.xml");
            assert!(doc_xml.contains("</w:tblGrid>"));
        });
    }

    #[test]
    fn test_pptx_structure() {
        let dir = tempfile::TempDir::new().unwrap();
        let md = "# Титульный\n\n## Слайд раз\n\n- пункт один\n- пункт два\n\n## Слайд два\n\nТекст слайда.\n\n| A | B |\n|---|---|\n| 1 | 2 |";
        let p = dir.path().join("t.pptx");
        write_pptx(&p, &parse_markdown_doc(md)).unwrap();

        // Обязательные части минимального PPTX.
        let ct = unzip_entry(&p, "[Content_Types].xml");
        assert!(ct.contains("presentationml.presentation.main+xml"));
        assert!(ct.contains("/ppt/slides/slide3.xml")); // титульный + 2 контентных
        let pres = unzip_entry(&p, "ppt/presentation.xml");
        assert_eq!(pres.matches("<p:sldId ").count(), 3);
        assert!(pres.contains("sldSz"));
        let rels = unzip_entry(&p, "ppt/_rels/presentation.xml.rels");
        assert!(rels.contains("slideMaster1.xml"));
        let s1 = unzip_entry(&p, "ppt/slides/slide1.xml");
        assert!(s1.contains("Титульный"));
        let s2 = unzip_entry(&p, "ppt/slides/slide2.xml");
        assert!(s2.contains("пункт один"));
        assert!(s2.contains("Слайд раз"));
        let s3 = unzip_entry(&p, "ppt/slides/slide3.xml");
        assert!(s3.contains("A | B"));
        // Мастер/лейаут/тема на месте и связаны.
        assert!(unzip_entry(&p, "ppt/slideMasters/slideMaster1.xml").contains("sldLayoutIdLst"));
        assert!(unzip_entry(&p, "ppt/slideLayouts/slideLayout1.xml").contains("masterClrMapping"));
        assert!(unzip_entry(&p, "ppt/theme/theme1.xml").contains("fmtScheme"));
    }

    #[test]
    fn test_doc_to_slides_rules() {
        let doc = parse_markdown_doc("# Титул\n\nПодзаголовок титула.\n\n## Один\n\n- a\n- b\n\n## Два\n\nТекст.");
        let slides = doc_to_slides(&doc);
        assert_eq!(slides.len(), 3);
        assert_eq!(slides[0].title, "Титул");
        // Контент после титульного H1 — строки титульного слайда.
        assert_eq!(slides[0].lines.len(), 1);
        assert_eq!(slides[1].title, "Один");
        assert_eq!(slides[1].lines.len(), 2);
        assert_eq!(slides[2].title, "Два");
        // Без заголовков вообще — один слайд с содержимым.
        let flat = parse_markdown_doc("Просто текст.\n\n- пункт");
        let s2 = doc_to_slides(&flat);
        assert_eq!(s2.len(), 1);
        assert_eq!(s2[0].lines.len(), 2);
    }

    #[test]
    fn test_rtf_output() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("t.rtf");
        write_rtf(&p, &parse_markdown_doc(&sample_md())).unwrap();
        let rtf = std::fs::read_to_string(&p).unwrap();
        assert!(rtf.starts_with("{\\rtf1"));
        // «О» из «Отчёт» = U+041E (1054).
        assert!(rtf.contains("\\u1054?"));
        assert!(rtf.contains("\\trowd")); // таблица есть
        assert!(rtf.contains("\\bullet"));
        assert!(rtf.trim_end().ends_with('}'));
    }
}

#[cfg(test)]
mod docgen_user_samples {
    use super::*;

        #[test]
        fn test_generate_user_samples_for_manual_check() {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let tool = DocumentCreateTool::new(crate::workdir::WorkDir::new(
                    std::path::PathBuf::from("/home/marsel/projects/reports"),
                ));
                let md = "# Отчёт Гефеста\n\nЭтот файл создан генератором документов.\n\n## Раздел\n\n- первый пункт\n- второй пункт\n\n| Показатель | Значение |\n|---|---|\n| Скорость | 42 |\n| Качество | 100 |";
                for fmt in ["docx", "odt"] {
                    let out = format!("/home/marsel/projects/reports/gefest_sample.{fmt}");
                    let res = tool.execute(&serde_json::json!({"markdown": md, "format": fmt, "path": out})).await;
                    assert!(res.success, "{fmt}: {}", res.error.unwrap_or_default());
                }
            });
        }
}
