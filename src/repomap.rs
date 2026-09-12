//! Repomap — карта репозитория для LLM (как в Aider). Порт `repomap.py`
//! (итерация 6). Оригинал парсит только `*.py` через модуль `ast`
//! (настоящий AST-парсер Python). Здесь эквивалента `ast` для
//! произвольного языка в чистом Rust нет (нужен был бы tree-sitter —
//! тяжёлая зависимость, которой нет в дереве), поэтому классы/функции
//! находятся простыми построчными regex — как и `diagram_tools.rs`
//! (`extract_classes` там) в этом же дереве делает для того же класса
//! задач. Расширено по сравнению с оригиналом: сканирует не только
//! `*.py`, а `.py/.rs/.go/.js/.ts/.java/.rb`.

use regex::Regex;
use std::path::Path;
use walkdir::WalkDir;

use crate::tools::ToolResult;

fn class_and_fn_patterns(ext: &str) -> Option<(Regex, Regex)> {
    match ext {
        "py" => Some((
            Regex::new(r"(?m)^class\s+(\w+)").unwrap(),
            Regex::new(r"(?m)^def\s+(\w+)\s*\(").unwrap(),
        )),
        "rs" => Some((
            Regex::new(r"(?m)^\s*(?:pub\s+)?(?:struct|enum|trait)\s+(\w+)").unwrap(),
            Regex::new(r"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)").unwrap(),
        )),
        "go" => Some((
            Regex::new(r"(?m)^type\s+(\w+)\s+struct").unwrap(),
            Regex::new(r"(?m)^func\s+(?:\([^)]*\)\s*)?(\w+)\s*\(").unwrap(),
        )),
        "js" | "ts" | "jsx" | "tsx" => Some((
            Regex::new(r"(?m)^\s*(?:export\s+)?class\s+(\w+)").unwrap(),
            Regex::new(r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+(\w+)").unwrap(),
        )),
        "java" | "kt" => Some((
            Regex::new(r"(?m)^\s*(?:public|private|protected)?\s*(?:class|interface)\s+(\w+)").unwrap(),
            Regex::new(r"(?m)^\s*(?:public|private|protected)?\s*(?:static\s+)?\w+\s+(\w+)\s*\([^)]*\)\s*\{").unwrap(),
        )),
        "rb" => Some((
            Regex::new(r"(?m)^\s*class\s+(\w+)").unwrap(),
            Regex::new(r"(?m)^\s*def\s+(\w+)").unwrap(),
        )),
        _ => None,
    }
}

pub fn scan(workspace: &Path, max_files: usize, save_to: Option<&str>) -> ToolResult {
    let mut lines = vec![format!(
        "🗺️  Карта репозитория: {}\n",
        workspace.file_name().and_then(|n| n.to_str()).unwrap_or(".")
    )];

    let source_exts = ["py", "rs", "go", "js", "ts", "jsx", "tsx", "java", "kt", "rb"];
    let mut files: Vec<_> = WalkDir::new(workspace)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let p = e.path().to_string_lossy();
            !p.contains("/venv/")
                && !p.contains("/__pycache__/")
                && !p.contains("/node_modules/")
                && !p.contains("/target/")
                && !p.contains("/.git/")
        })
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|ext| source_exts.contains(&ext))
                .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|e| e.path().to_path_buf());
    files.truncate(max_files);

    let mut total_classes = 0;
    let mut total_fns = 0;
    let files_scanned = files.len();

    for entry in &files {
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let Some((class_re, fn_re)) = class_and_fn_patterns(ext) else {
            continue;
        };
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let rel = path.strip_prefix(workspace).unwrap_or(path);

        let classes: Vec<String> = class_re
            .captures_iter(&content)
            .map(|c| format!("  📦 {}", &c[1]))
            .collect();
        let fns: Vec<String> = fn_re
            .captures_iter(&content)
            .map(|c| format!("  ƒ {}", &c[1]))
            .collect();

        total_classes += classes.len();
        total_fns += fns.len();

        if !classes.is_empty() || !fns.is_empty() {
            lines.push(format!("📄 {}", rel.display()));
            lines.extend(classes.into_iter().take(3));
            lines.extend(fns.into_iter().take(5));
            lines.push(String::new());
        }
    }

    lines.push(format!(
        "\n📊 Итого: {} файлов, {} классов, {} функций",
        files_scanned, total_classes, total_fns
    ));
    let mut output = lines.join("\n");

    if let Some(save_path) = save_to {
        if let Some(parent) = Path::new(save_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(save_path, &output).is_ok() {
            output.push_str(&format!("\n💾 Сохранено: {}", save_path));
        }
    }

    ToolResult::success(output)
}

pub fn file_summary(file_path: &str) -> ToolResult {
    let path = Path::new(file_path);
    if !path.exists() {
        return ToolResult::error(format!("Файл не найден: {}", file_path));
    }
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let Some((class_re, fn_re)) = class_and_fn_patterns(ext) else {
        return ToolResult::error(format!("Неподдерживаемое расширение файла: .{}", ext));
    };
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Не удалось прочитать файл: {}", e)),
    };

    let mut lines = vec![format!("📄 {}\n", path.file_name().and_then(|n| n.to_str()).unwrap_or(file_path))];
    for cap in class_re.captures_iter(&content) {
        lines.push(format!("  📦 class {}", &cap[1]));
    }
    for cap in fn_re.captures_iter(&content) {
        lines.push(format!("  ƒ def {}", &cap[1]));
    }
    ToolResult::success(lines.join("\n"))
}
