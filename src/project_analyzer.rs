//! Project Analyzer — определяет язык/фреймворк/тест-раннер проекта по
//! файлам-маркерам. Порт `advanced_tools.py::ProjectAnalyzerTool`
//! (итерация 6). Не путать с `system_tools.rs`/`project_tools.rs`,
//! которые уже подключены и делают другие вещи (git-статус, структура
//! файлов и т.п. — см. STATUS_RU.md, там же объяснение почему их не
//! трогал).

use std::path::Path;
use walkdir::WalkDir;

use crate::tools::ToolResult;

pub fn analyze(path: &str) -> ToolResult {
    let project_path = match Path::new(path).canonicalize() {
        Ok(p) => p,
        Err(_) => return ToolResult::error(format!("Path not found: {}", path)),
    };

    let entries: Vec<_> = WalkDir::new(&project_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path().to_string_lossy();
            !p.contains("/node_modules/") && !p.contains("/.git/") && !p.contains("/target/") && !p.contains("/venv/")
        })
        .collect();

    let file_names: Vec<String> = entries
        .iter()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    let has = |name: &str| file_names.iter().any(|f| f == name);

    let mut language: Option<&str> = None;
    let mut framework: Option<String> = None;
    let mut package_manager: Option<&str> = None;
    let mut test_framework: Option<String> = None;

    if has("requirements.txt") || has("setup.py") || has("pyproject.toml") {
        language = Some("Python");
        package_manager = Some("pip");
        if has("manage.py") {
            framework = Some("Django".to_string());
        } else if file_names.iter().any(|f| f.to_lowercase().contains("fastapi")) {
            framework = Some("FastAPI".to_string());
        } else if file_names.iter().any(|f| f.to_lowercase().contains("flask")) {
            framework = Some("Flask".to_string());
        }
        if has("pytest.ini") || file_names.iter().any(|f| f.contains("pytest")) {
            test_framework = Some("pytest".to_string());
        } else if file_names.iter().any(|f| f.contains("unittest")) {
            test_framework = Some("unittest".to_string());
        }
    } else if has("package.json") {
        language = Some("JavaScript/TypeScript");
        package_manager = Some("npm");
        if let Ok(content) = std::fs::read_to_string(project_path.join("package.json")) {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                let mut deps = std::collections::HashSet::new();
                for key in ["dependencies", "devDependencies"] {
                    if let Some(obj) = data.get(key).and_then(|v| v.as_object()) {
                        for k in obj.keys() {
                            deps.insert(k.clone());
                        }
                    }
                }
                framework = if deps.contains("next") {
                    Some("Next.js".to_string())
                } else if deps.contains("react") {
                    Some("React".to_string())
                } else if deps.contains("vue") {
                    Some("Vue".to_string())
                } else if deps.contains("express") {
                    Some("Express".to_string())
                } else {
                    None
                };
                test_framework = if deps.contains("jest") {
                    Some("Jest".to_string())
                } else if deps.contains("mocha") {
                    Some("Mocha".to_string())
                } else {
                    None
                };
            }
        }
    } else if has("go.mod") {
        language = Some("Go");
        package_manager = Some("go");
        test_framework = Some("go test".to_string());
    } else if has("Cargo.toml") {
        language = Some("Rust");
        package_manager = Some("cargo");
        test_framework = Some("cargo test".to_string());
    } else if has("Gemfile") {
        language = Some("Ruby");
        package_manager = Some("bundler");
        if has("config.ru") {
            framework = Some("Rails/Rack".to_string());
        }
    } else if has("pom.xml") {
        language = Some("Java");
        package_manager = Some("maven");
    } else if has("build.gradle") {
        language = Some("Java/Kotlin");
        package_manager = Some("gradle");
    }

    let mut report = format!(
        "Project Analysis:\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\nRoot: {}\nLanguage: {}\nFramework: {}\nPackage Manager: {}\nTest Framework: {}\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\nFiles found: {}\n",
        project_path.display(),
        language.unwrap_or("Unknown"),
        framework.as_deref().unwrap_or("None detected"),
        package_manager.unwrap_or("Unknown"),
        test_framework.as_deref().unwrap_or("None detected"),
        entries.len(),
    );

    let mut dirs: Vec<_> = entries
        .iter()
        .filter(|e| e.file_type().is_dir())
        .filter(|e| {
            e.path()
                .strip_prefix(&project_path)
                .map(|rel| !rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')))
                .unwrap_or(true)
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    dirs.sort();

    if !dirs.is_empty() {
        report.push_str("\nKey directories:\n");
        for d in dirs.iter().take(10) {
            let rel = d.strip_prefix(&project_path).unwrap_or(d);
            report.push_str(&format!("  - {}/\n", rel.display()));
        }
    }

    ToolResult::success(report)
}
