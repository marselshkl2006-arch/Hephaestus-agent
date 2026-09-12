use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use serde_json::Value;
use walkdir::WalkDir;
use regex::Regex;

/// Результат выполнения диаграммы
#[derive(Clone)]
pub struct DiagramResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

impl DiagramResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self { success: true, output: output.into(), error: None }
    }
    pub fn err(error: impl Into<String>) -> Self {
        Self { success: false, output: String::new(), error: Some(error.into()) }
    }
}

/// Детектирует язык по расширению файла
fn detect_language(file_path: &Path) -> &'static str {
    match file_path.extension().and_then(|e| e.to_str()) {
        Some("py") => "python",
        Some("rs") => "rust",
        Some("go") => "go",
        Some("js") => "javascript",
        Some("ts") => "typescript",
        Some("jsx") => "jsx",
        Some("tsx") => "tsx",
        Some("java") => "java",
        Some("cpp") | Some("cc") | Some("cxx") => "cpp",
        Some("c") => "c",
        Some("h") | Some("hpp") => "c_header",
        Some("rb") => "ruby",
        Some("php") => "php",
        Some("swift") => "swift",
        Some("kt") | Some("kts") => "kotlin",
        _ => "unknown",
    }
}

/// Определение классов/структур через tree-sitter
fn extract_classes(content: &str, _lang: &str) -> Vec<HashMap<String, Value>> {
    let mut classes = Vec::new();
    
    // Простой regex-based парсинг для всех языков
    // Для полноценной поддержки нужно tree-sitter, но для базовых случаев хватит regex
    // ФИКС (живой e2e): Rust-структуры почти всегда `pub struct`/`pub enum`/
    // `pub trait` — без опционального pub-префикса regex не находил НИ ОДНОЙ
    // структуры в типичных Rust-файлах.
    let re_class = Regex::new(r#"(?m)^\s*(?:pub(\([^)]*\))?\s+)?(?:class|struct|interface|trait|type|enum|object|case class)\s+(\w+)(?:\s*(?:extends|implements|:|<|\{|where)\s*([^{]+))?"#).unwrap();
    
    for cap in re_class.captures_iter(content) {
        let name = cap[2].to_string();
        let bases = cap.get(3).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let mut class_info = HashMap::new();
        class_info.insert("name".to_string(), Value::String(name));
        class_info.insert("bases".to_string(), Value::String(bases));
        classes.push(class_info);
    }
    
    classes
}

/// Извлечение импортов
fn extract_imports(content: &str, lang: &str) -> HashMap<String, Vec<String>> {
    let mut imports: HashMap<String, Vec<String>> = HashMap::new();
    imports.insert("stdlib".to_string(), Vec::new());
    imports.insert("third_party".to_string(), Vec::new());
    imports.insert("local".to_string(), Vec::new());
    
    let patterns = match lang {
        "python" => vec![r#"^\s*(?:from\s+(\S+)\s+import|import\s+(\S+))"#],
        "rust" => vec![r#"^\s*(?:use\s+([^;]+);)"#],
        "go" => vec![r#"^\s*import\s+(?:"([^"]+)"|(\S+))"#],
        "javascript" | "typescript" | "jsx" | "tsx" => {
            vec![r#"^\s*(?:import\s+.*?from\s+['"]([^'"]+)['"]|require\s*\(['"]([^'"]+)['"]\))"#]
        }
        _ => vec![r#"^\s*#\s*include\s*[<"]([^>"]+)[>"]"#],
    };
    
    for pattern in patterns {
        let re = Regex::new(pattern).unwrap();
        for cap in re.captures_iter(content) {
            for i in 1..cap.len() {
                if let Some(m) = cap.get(i) {
                    let module = m.as_str().to_string();
                    let module_lower = module.to_lowercase();
                    
                    // Определяем тип импорта (упрощённо)
                    if module_lower.contains("std") || module_lower.contains("core") {
                        imports.get_mut("stdlib").unwrap().push(module);
                    } else if module.starts_with('.') || module.contains("/") || module.contains("\\") {
                        imports.get_mut("local").unwrap().push(module);
                    } else {
                        imports.get_mut("third_party").unwrap().push(module);
                    }
                }
            }
        }
    }
    
    imports
}

/// Найти `mod X;` (не `mod X { ... }` — то уже в этом же файле, regex
/// извлечения классов и так его видит) и разрешить в путь файла: сперва
/// `{dir}/X.rs`, затем `{dir}/X/mod.rs` (обе схемы раскладки модулей
/// в Rust). Не разбирает `#[path = "..."]` — редкий случай, не стал
/// усложнять регексом то, что реально требует полноценного парсера.
fn find_mod_declarations(content: &str, dir: &Path) -> Vec<PathBuf> {
    let re_mod = Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;").unwrap();
    let mut found = Vec::new();
    for cap in re_mod.captures_iter(content) {
        let name = &cap[1];
        let flat = dir.join(format!("{}.rs", name));
        let nested = dir.join(name).join("mod.rs");
        if flat.exists() {
            found.push(flat);
        } else if nested.exists() {
            found.push(nested);
        }
    }
    found
}

/// Рекурсивно собрать все файлы модуля, начиная с `entry_file` — следуя
/// за `mod X;`. ИСПРАВЛЕНО (жалоба из аудита и скриншота: "diagram ищет
/// структуры только в корневом файле main.rs, игнорируя остальные
/// модули... не учитывает mod declarations и не сканирует вложенные
/// модули"): раньше `class_diagram` читал РОВНО один переданный файл —
/// если в проекте типичная для Rust раскладка "main.rs объявляет mod
/// tools; а сама структура — в src/tools/*.rs", диаграмма показывала
/// только main.rs, то есть почти ничего. Теперь при обходе `.rs`-файла
/// дополнительно разворачиваются все его `mod X;` — рекурсивно, с
/// защитой от циклов через `visited`.
fn collect_rust_module_files(entry_file: &Path) -> Vec<PathBuf> {
    let mut visited = std::collections::HashSet::new();
    let mut result = Vec::new();
    let mut stack = vec![entry_file.to_path_buf()];

    while let Some(path) = stack.pop() {
        let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !visited.insert(canonical) {
            continue; // уже обходили — либо дубликат, либо цикл mod-объявлений
        }
        let Ok(content) = fs::read_to_string(&path) else { continue };
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        for sub in find_mod_declarations(&content, dir) {
            stack.push(sub);
        }
        result.push(path);
    }
    result
}

/// Диаграмма классов из файла
pub fn class_diagram(file_path: &str, save_to: Option<&str>) -> DiagramResult {
    let path = Path::new(file_path);
    if !path.exists() {
        return DiagramResult::err(format!("Файл не найден: {}", file_path));
    }

    let lang = detect_language(path);

    // Для Rust — следуем за `mod X;` по всему дереву модулей, а не
    // только по одному переданному файлу (см. collect_rust_module_files).
    // Для остальных языков пока честно ограничено одним файлом — их
    // системы модулей (import/require) устроены иначе, обобщать вслепую
    // не стал.
    let files = if lang == "rust" {
        collect_rust_module_files(path)
    } else {
        vec![path.to_path_buf()]
    };

    let mut output = format!("\n Диаграмма классов: {} (язык: {})\n", path.file_name().unwrap_or_default().to_string_lossy(), lang);
    if files.len() > 1 {
        output.push_str(&format!(" (включая {} связанных модулей через 'mod')\n", files.len() - 1));
    }
    output.push('\n');

    let mut total_classes = 0;
    for f in &files {
        let content = match fs::read_to_string(f) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let classes = extract_classes(&content, lang);
        if classes.is_empty() {
            continue;
        }
        if files.len() > 1 {
            output.push_str(&format!("── {} ──\n", f.display()));
        }
        for cls in classes {
            total_classes += 1;
            let name = cls.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let bases = cls.get("bases").and_then(|v| v.as_str()).unwrap_or("");

            output.push_str(&format!("┌─  {}", name));
            if !bases.is_empty() {
                output.push_str(&format!(" ← {}", bases));
            }
            output.push('\n');
            output.push_str(&("└".to_owned() + &"─".repeat(40)));
            output.push('\n');
        }
    }

    if total_classes == 0 {
        return DiagramResult::err("Классы/структуры не найдены (проверены файлы: ".to_string() + &files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", ") + ")");
    }

    if let Some(path_save) = save_to {
        if let Err(e) = fs::write(path_save, &output) {
            return DiagramResult::err(format!("Не удалось сохранить: {}", e));
        }
        output.push_str(&format!("\n\n Сохранено: {}", path_save));
    }

    DiagramResult::ok(output)
}

/// Дерево файлов и папок
pub fn file_tree(directory: Option<&str>, max_depth: Option<usize>, save_to: Option<&str>) -> DiagramResult {
    let dir = directory.unwrap_or(".");
    let depth = max_depth.unwrap_or(3);
    let root = Path::new(dir);
    
    if !root.exists() {
        return DiagramResult::err(format!("Папка не найдена: {}", dir));
    }
    
    let mut output = format!("\n {}\n\n", root.display());
    
    let mut entries: Vec<(PathBuf, usize)> = Vec::new();
    for entry in WalkDir::new(root).max_depth(depth).into_iter().filter_entry(|e| {
        !e.file_name().to_string_lossy().starts_with('.') &&
        e.file_name().to_string_lossy() != "venv" &&
        e.file_name().to_string_lossy() != "__pycache__" &&
        e.file_name().to_string_lossy() != "node_modules" &&
        e.file_name().to_string_lossy() != "target"
    }) {
        if let Ok(e) = entry {
            let path = e.path();
            let depth = path.components().count() - 1;
            entries.push((path.to_path_buf(), depth));
        }
    }
    
    // Сортировка: сначала папки, потом файлы
    entries.sort_by(|a, b| {
        let a_is_dir = a.0.is_dir();
        let b_is_dir = b.0.is_dir();
        if a_is_dir && !b_is_dir { return std::cmp::Ordering::Less; }
        if !a_is_dir && b_is_dir { return std::cmp::Ordering::Greater; }
        a.0.file_name().cmp(&b.0.file_name())
    });
    

    for (i, (path, depth)) in entries.iter().enumerate() {
        let is_last = i == entries.len() - 1;
        let is_dir = path.is_dir();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        
        let prefix = if *depth > 0 {
            let mut p = String::new();
            for _d in 0..*depth-1 {
                p.push_str("│   ");
            }
            if is_last {
                p.push_str("└── ");
            } else {
                p.push_str("├── ");
            }
            p
        } else {
            String::new()
        };
        
        let icon = if is_dir { " " } else { " " };
        let size = if path.is_file() {
            if let Ok(metadata) = fs::metadata(path) {
                let sz = metadata.len();
                if sz < 1024 {
                    format!(" ({:.0} B)", sz)
                } else {
                    format!(" ({:.0} KB)", sz as f64 / 1024.0)
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        
        output.push_str(&format!("{}{}{}{}\n", prefix, icon, name, size));
    }
    
    if let Some(path_save) = save_to {
        if let Err(e) = fs::write(path_save, &output) {
            return DiagramResult::err(format!("Не удалось сохранить: {}", e));
        }
        output.push_str(&format!("\n\n Сохранено: {}", path_save));
    }
    
    DiagramResult::ok(output)
}

/// Граф зависимостей
pub fn dependency_graph(file_path: &str, save_to: Option<&str>) -> DiagramResult {
    let path = Path::new(file_path);
    if !path.exists() {
        return DiagramResult::err(format!("Файл не найден: {}", file_path));
    }
    
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return DiagramResult::err(format!("Не удалось прочитать файл: {}", e)),
    };
    
    let lang = detect_language(path);
    let imports = extract_imports(&content, lang);
    
    let mut output = format!("\n Граф зависимостей: {} (язык: {})\n\n", path.file_name().unwrap_or_default().to_string_lossy(), lang);
    output.push_str(&format!("   {}\n", path.file_stem().unwrap_or_default().to_string_lossy()));
    output.push_str("  │\n");
    
    if let Some(local) = imports.get("local") {
        if !local.is_empty() {
            output.push_str("  ├──  Локальные модули\n");
            for imp in local {
                output.push_str(&format!("  │   └── {}\n", imp));
            }
        }
    }
    
    if let Some(third) = imports.get("third_party") {
        if !third.is_empty() {
            output.push_str("  ├──  Сторонние пакеты\n");
            for imp in third {
                output.push_str(&format!("  │   └── {}\n", imp));
            }
        }
    }
    
    if let Some(stdlib) = imports.get("stdlib") {
        if !stdlib.is_empty() {
            output.push_str("  └──  Стандартная библиотека\n");
            for imp in stdlib {
                output.push_str(&format!("      └── {}\n", imp));
            }
        }
    }
    
    if let Some(path_save) = save_to {
        if let Err(e) = fs::write(path_save, &output) {
            return DiagramResult::err(format!("Не удалось сохранить: {}", e));
        }
        output.push_str(&format!("\n\n Сохранено: {}", path_save));
    }
    
    DiagramResult::ok(output)
}

/// Конвертация Mermaid в ASCII
pub fn mermaid_to_ascii(mermaid_code: &str, save_to: Option<&str>) -> DiagramResult {
    let lines: Vec<&str> = mermaid_code.trim().lines().collect();
    let mut output = String::from("\n Диаграмма\n");
    
    if mermaid_code.trim().starts_with("classDiagram") {
        output.push_str("Диаграмма классов:\n");
        for line in lines.iter().skip(1) {
            let line = line.trim();
            if line.is_empty() || line.starts_with("%%") {
                continue;
            }
            if line.contains("<|--") {
                let parts: Vec<&str> = line.split("<|--").collect();
                if parts.len() == 2 {
                    output.push_str(&format!("  {} ──► {} (наследование)\n", parts[1].trim(), parts[0].trim()));
                }
            } else if line.contains(':') && !line.starts_with("%%") {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    output.push_str(&format!("   {}\n      {}\n", parts[0].trim(), parts[1].trim()));
                }
            }
        }
    } else if mermaid_code.contains("graph ") || mermaid_code.contains("flowchart ") {
        output.push_str("Блок-схема:\n");
        for line in lines.iter().skip(1) {
            let line = line.trim();
            if line.is_empty() || line.starts_with("%%") {
                continue;
            }
            for arrow in ["-->", "->", "==>", "-.->"] {
                if line.contains(arrow) {
                    let parts: Vec<&str> = line.split(arrow).collect();
                    if parts.len() == 2 {
                        let src = parts[0].trim().trim_matches(|c| c == '[' || c == ']' || c == '(' || c == ')' || c == '{' || c == '}');
                        let dst = parts[1].trim().trim_matches(|c| c == '[' || c == ']' || c == '(' || c == ')' || c == '{' || c == '}');
                        output.push_str(&format!("  [{}] ──────► [{}]\n", src, dst));
                    }
                    break;
                }
            }
        }
    } else {
        output.push_str(mermaid_code);
    }
    
    if let Some(path_save) = save_to {
        if let Err(e) = fs::write(path_save, &output) {
            return DiagramResult::err(format!("Не удалось сохранить: {}", e));
        }
        output.push_str(&format!("\n\n Сохранено: {}", path_save));
    }
    
    DiagramResult::ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language(Path::new("test.py")), "python");
        assert_eq!(detect_language(Path::new("test.rs")), "rust");
        assert_eq!(detect_language(Path::new("test.go")), "go");
        assert_eq!(detect_language(Path::new("test.js")), "javascript");
        assert_eq!(detect_language(Path::new("test.ts")), "typescript");
        assert_eq!(detect_language(Path::new("test.java")), "java");
    }
}
