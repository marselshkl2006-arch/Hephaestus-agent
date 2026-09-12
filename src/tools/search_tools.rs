//! grep/glob инструменты первого класса для модели (по образцу opencode).
//!
//! ЗАЧЕМ: без них модель ищет по коду через `bash` — это медленнее,
//! небезопаснее (произвольный shell) и не обрезается по лимитам контекста.
//! Здесь: чистый Rust (walkdir + regex, зависимости уже есть), результаты
//! ОБРЕЗАЮТСЯ до разумного лимита с подсказкой сузить запрос, вывод в
//! ripgrep-стиле `путь:строка: текст`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolResult};
use crate::workdir::WorkDir;

/// Лимит совпадений grep (строк вывода).
const GREP_MAX_MATCHES: usize = 200;
/// Лимит файлов glob.
const GLOB_MAX_FILES: usize = 300;

/// Директории, которые никогда не обходим (артефакты и VCS).
fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | ".venv" | "venv" | "__pycache__"
            | "dist" | "build" | ".idea" | ".vscode" | ".pytest_cache"
    )
}

/// Бинарный файл? Грубая проверка: NUL в первых 8 КБ.
fn looks_binary(path: &Path) -> bool {
    use std::io::Read;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return true,
    };
    let mut buf = [0u8; 8192];
    let n = f.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

// ══════════════════════════════════════════════════════════════ grep ──

pub struct GrepTool {
    workdir: WorkDir,
}

impl GrepTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for GrepTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        if pattern.is_empty() {
            return ToolResult::error("нужен параметр 'pattern' — регулярное выражение (Rust syntax)");
        }
        let re = match regex::Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("неверное регулярное выражение: {e}")),
        };
        let root_raw = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = self.workdir.resolve(root_raw);
        if !root.is_dir() {
            return ToolResult::error(format!("не директория: {}", root.display()));
        }
        // glob-фильтр имён файлов (например "*.rs"), необязательный.
        let file_glob = args
            .get("file_glob")
            .and_then(|v| v.as_str())
            .and_then(|g| glob::Pattern::new(g).ok());
        let case_sensitive = args.get("case_sensitive").and_then(|v| v.as_bool()).unwrap_or(true);
        let re = if case_sensitive {
            re
        } else {
            match regex::Regex::new(&format!("(?i){pattern}")) {
                Ok(r) => r,
                Err(_) => re,
            }
        };

        let mut out: Vec<String> = Vec::new();
        let mut files_matched = 0usize;
        let mut truncated = false;
        for entry in walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| e.path().file_name().map(|n| !is_skipped_dir(n.to_str().unwrap_or(""))).unwrap_or(true))
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            if looks_binary(p) {
                continue;
            }
            if let Some(g) = &file_glob {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if !g.matches(&name) {
                    continue;
                }
            }
            let text: String = match std::fs::read_to_string(p) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let rel = p
                .strip_prefix(&root)
                .map(|r| r.display().to_string())
                .unwrap_or_else(|_| p.display().to_string());
            let mut file_hit = false;
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    if out.len() >= GREP_MAX_MATCHES {
                        truncated = true;
                        break;
                    }
                    // Строку в выводе обрезаем — длинные строки не тащим.
                    let trimmed = line.trim();
                    let short: String = trimmed.chars().take(240).collect();
                    out.push(format!("{rel}:{}: {short}", i + 1));
                    file_hit = true;
                }
            }
            if file_hit {
                files_matched += 1;
            }
            if truncated {
                break;
            }
        }

        if out.is_empty() {
            return ToolResult::success(format!("Совпадений не найдено (pattern: {pattern})."));
        }
        let mut report = format!(
            "Найдено {} совпадений в {} файлах:\n{}",
            out.len(),
            files_matched,
            out.join("\n")
        );
        if truncated {
            report.push_str(&format!(
                "\n\n…[вывод ОБРЕЗАН до {GREP_MAX_MATCHES} совпадений — сузьте pattern или добавьте file_glob]"
            ));
        }
        ToolResult::success(report)
    }

    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Поиск по содержимому файлов рабочей директории (regex, ripgrep-стиль вывода путь:строка:текст). Быстрее и безопаснее bash-grep: обрезается по лимиту, пропускает бинарные и артефакты (target/node_modules/.git)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Регулярное выражение (Rust regex syntax, например \"fn \\w+\\(\" или \"TODO\"})"},
                "path": {"type": "string", "description": "Поддиректория поиска, по умолчанию вся рабочая директория"},
                "file_glob": {"type": "string", "description": "Фильтр имён файлов, например \"*.rs\" или \"*.py\""},
                "case_sensitive": {"type": "boolean", "description": "Учитывать регистр (по умолчанию true)"}
            },
            "required": ["pattern"]
        })
    }
}

// ══════════════════════════════════════════════════════════════ glob ──

pub struct GlobTool {
    workdir: WorkDir,
}

impl GlobTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for GlobTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        if pattern.is_empty() {
            return ToolResult::error("нужен параметр 'pattern' — glob-маска, например \"src/**/*.rs\"");
        }
        let full = if pattern.starts_with('/') || pattern.starts_with('~') {
            crate::workdir::expand_home(pattern)
        } else {
            self.workdir.get().join(pattern)
        };
        let matcher = match glob::glob(&full.to_string_lossy()) {
            Ok(g) => g,
            Err(e) => return ToolResult::error(format!("неверная glob-маска: {e}")),
        };

        let mut files: Vec<PathBuf> = Vec::new();
        let mut truncated = false;
        for entry in matcher {
            match entry {
                Ok(p) => {
                    if p.is_file() {
                        if files.len() >= GLOB_MAX_FILES {
                            truncated = true;
                            break;
                        }
                        files.push(p);
                    }
                }
                Err(_) => continue,
            }
        }
        if files.is_empty() {
            return ToolResult::success(format!("Файлов по маске {pattern} не найдено."));
        }
        // Сортировка по времени изменения (свежие сверху), как полезнее.
        let mut with_time: Vec<(std::time::SystemTime, PathBuf)> = files
            .into_iter()
            .map(|p| {
                let t = std::fs::metadata(&p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (t, p)
            })
            .collect();
        with_time.sort_by(|a, b| b.0.cmp(&a.0));
        let rel = self.workdir.get();
        let lines: Vec<String> = with_time
            .iter()
            .map(|(t, p)| {
                let age = t
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| {
                        let secs = d.as_secs();
                        chrono::DateTime::from_timestamp(secs as i64, 0)
                            .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let shown = p.strip_prefix(&rel).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string());
                format!("{shown}  ({age})")
            })
            .collect();
        let mut report = format!("Найдено {} файлов:\n{}", lines.len(), lines.join("\n"));
        if truncated {
            report.push_str(&format!("\n\n…[ОБРЕЗАНО до {GLOB_MAX_FILES} файлов — сузьте маску]"));
        }
        ToolResult::success(report)
    }

    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Найти файлы по glob-маске (\"src/**/*.rs\", \"**/*.md\"). Вывод: пути + время изменения, свежие сверху. Результаты обрезаются до 300 файлов."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Glob-маска относительно рабочей директории"}
            },
            "required": ["pattern"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, WorkDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let wd = WorkDir::new(tmp.path().to_path_buf());
        (tmp, wd)
    }

    #[tokio::test]
    async fn grep_finds_and_truncates_lines() {
        let (_t, wd) = setup();
        std::fs::write(wd.get().join("a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        let sub = wd.get().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("b.rs"), "fn gamma() {}\n").unwrap();

        let tool = GrepTool::new(wd);
        let res = tool.execute(&serde_json::json!({"pattern": "fn \\w+\\(", "file_glob": "*.rs"})).await;
        assert!(res.success, "{}", res.error.unwrap_or_default());
        let out = res.output;
        assert!(out.contains("a.rs:1: fn alpha()"));
        assert!(out.contains("b.rs:1: fn gamma()"));
        assert!(out.contains("3 совпадени"));
    }

    #[tokio::test]
    async fn grep_skips_target_and_binary() {
        let (_t, wd) = setup();
        std::fs::write(wd.get().join("ok.rs"), "secret_marker\n").unwrap();
        let tgt = wd.get().join("target");
        std::fs::create_dir(&tgt).unwrap();
        std::fs::write(tgt.join("gen.rs"), "secret_marker\n").unwrap();
        std::fs::write(wd.get().join("bin.dat"), b"secret_marker\x00\x01").unwrap();

        let tool = GrepTool::new(wd);
        let res = tool.execute(&serde_json::json!({"pattern": "secret_marker"})).await;
        let out = res.output;
        assert!(out.contains("ok.rs:1"));
        assert!(!out.contains("target"));
        assert!(!out.contains("bin.dat"));
    }

    #[tokio::test]
    async fn grep_respects_case_flag() {
        let (_t, wd) = setup();
        std::fs::write(wd.get().join("f.txt"), "Hello World\n").unwrap();
        let tool = GrepTool::new(wd);
        let cs = tool.execute(&serde_json::json!({"pattern": "hello"})).await;
        assert!(cs.output.contains("не найдено"));
        let ci = tool.execute(&serde_json::json!({"pattern": "hello", "case_sensitive": false})).await;
        assert!(ci.output.contains("f.txt:1"));
    }

    #[tokio::test]
    async fn glob_lists_matching_files() {
        let (_t, wd) = setup();
        std::fs::create_dir_all(wd.get().join("src/deep")).unwrap();
        std::fs::write(wd.get().join("src/main.rs"), "x").unwrap();
        std::fs::write(wd.get().join("src/deep/mod.rs"), "x").unwrap();
        std::fs::write(wd.get().join("readme.md"), "x").unwrap();

        let tool = GlobTool::new(wd);
        let res = tool.execute(&serde_json::json!({"pattern": "src/**/*.rs"})).await;
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert!(res.output.contains("main.rs"));
        assert!(res.output.contains("mod.rs"));
        assert!(!res.output.contains("readme.md"));
    }

    #[tokio::test]
    async fn file_read_pagination() {
        let (_t, wd) = setup();
        let content: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(wd.get().join("big.txt"), &content).unwrap();
        let read = super::super::file_tools::FileReadTool::new(wd.clone());

        // Дефолт: первые 2000 строк с маркером обрезки.
        let r1 = read.execute(&serde_json::json!({"file_path": "big.txt"})).await;
        assert!(r1.output.contains("line 0"));
        assert!(r1.output.contains("показаны строки 1–2000 из 3000"));
        assert!(r1.output.contains("offset=2000"));
        assert!(!r1.output.contains("line 2999"));

        // Дочитка.
        let r2 = read.execute(&serde_json::json!({"file_path": "big.txt", "offset": 2000})).await;
        assert!(r2.output.contains("line 2999"));
        assert!(r2.output.contains("конец файла"));

        // Маленький файл — без маркеров.
        std::fs::write(wd.get().join("small.txt"), "just one\n").unwrap();
        let r3 = read.execute(&serde_json::json!({"file_path": "small.txt"})).await;
        assert_eq!(r3.output, "just one\n");
    }
}
