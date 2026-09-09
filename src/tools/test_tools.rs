//! Test Tools — запуск тестов проекта с автоопределением тестраннера по
//! файлам-маркерам (`Cargo.toml` → `cargo test`, `package.json` →
//! `npm test`, `pytest.ini`/`pyproject.toml`/`setup.py` → `pytest`).
//! Аналог `test_tools.py`.

use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use super::{Tool, ToolResult};
use crate::workdir::WorkDir;

fn detect_runner(dir: &Path) -> Option<(&'static str, Vec<&'static str>)> {
    if dir.join("Cargo.toml").exists() {
        Some(("cargo", vec!["test"]))
    } else if dir.join("package.json").exists() {
        Some(("npm", vec!["test"]))
    } else if dir.join("pytest.ini").exists()
        || dir.join("pyproject.toml").exists()
        || dir.join("setup.py").exists()
    {
        Some(("pytest", vec![]))
    } else if dir.join("go.mod").exists() {
        Some(("go", vec!["test", "./..."]))
    } else {
        None
    }
}

pub struct RunTestsTool {
    workdir: WorkDir,
}
impl RunTestsTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for RunTestsTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let dir = args.get("directory").and_then(|v| v.as_str()).unwrap_or(".");
        // Раньше "." означало CWD процесса — при запуске не из папки
        // проекта тесты искались не там. Теперь относительный путь
        // (включая дефолт ".") резолвится от динамической WorkDir.
        let dir_path: PathBuf = if Path::new(dir).is_absolute() {
            PathBuf::from(dir)
        } else if dir == "." {
            self.workdir.get()
        } else {
            self.workdir.resolve(dir)
        };
        let dir_path = dir_path.canonicalize().unwrap_or(dir_path);

        let (runner, base_args) = match args.get("runner").and_then(|v| v.as_str()) {
            Some(explicit) => (explicit, vec![]),
            None => match detect_runner(&dir_path) {
                Some((r, a)) => (r, a),
                None => {
                    return ToolResult::error(
                        "Не удалось определить тестраннер (нет Cargo.toml/package.json/pytest.ini/go.mod). \
                         Укажите 'runner' явно.",
                    )
                }
            },
        };

        let extra: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let mut cmd = Command::new(runner);
        cmd.args(&base_args).args(&extra).current_dir(dir_path);

        match cmd.output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let combined = format!("$ {} {} {}\n\n{}\n{}", runner, base_args.join(" "), extra.join(" "), stdout, stderr);
                if output.status.success() {
                    ToolResult::success(combined)
                } else {
                    ToolResult::error(combined)
                }
            }
            Err(e) => ToolResult::error(format!(
                "Не удалось запустить '{}': {} (установлен ли он и есть ли в PATH?)",
                runner, e
            )),
        }
    }

    fn name(&self) -> &'static str {
        "run_tests"
    }
}
