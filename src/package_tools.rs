//! Package Tools — установка/поиск пакетов через pip/npm/apt/cargo/go/yarn.
//! Слитый порт `package_tools.py` (install через pip/npm/apt + поиск) и
//! `advanced_tools.py::PackageInstallTool` (install через pip/npm/yarn/
//! cargo/go) — в оригинале это были два отдельных, местами дублирующих
//! друг друга класса `PackageInstallTool` в разных файлах с разным
//! набором менеджеров; здесь один инструмент со всеми менеджерами обоих.

use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use crate::tools::ToolResult;

fn detect_package_manager(workspace: &Path) -> &'static str {
    if workspace.join("requirements.txt").exists()
        || workspace.join("pyproject.toml").exists()
        || workspace.join("setup.py").exists()
    {
        "pip"
    } else if workspace.join("package.json").exists() {
        "npm"
    } else if workspace.join("Cargo.toml").exists() {
        "cargo"
    } else if workspace.join("go.mod").exists() {
        "go"
    } else {
        "pip"
    }
}

async fn run(cmd: &str, args: &[&str], cwd: &Path, timeout_secs: u64) -> ToolResult {
    let mut command = Command::new(cmd);
    command.args(args).current_dir(cwd);

    let output = match tokio::time::timeout(Duration::from_secs(timeout_secs), command.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return ToolResult::error(format!("`{}` не найден в PATH", cmd));
        }
        Ok(Err(e)) => return ToolResult::error(format!("Ошибка запуска `{}`: {}", cmd, e)),
        Err(_) => return ToolResult::error(format!("Таймаут выполнения `{}` ({}с)", cmd, timeout_secs)),
    };

    let mut out = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        out.push('\n');
        out.push_str(&stderr);
    }

    if output.status.success() {
        ToolResult::success(out)
    } else {
        ToolResult { success: false, output: out, error: Some(format!("Команда завершилась с ошибкой (код {:?})", output.status.code())), images: Vec::new() }
    }
}

/// Установить пакет. `manager` = "auto" | "pip" | "npm" | "yarn" | "apt" | "cargo" | "go".
pub async fn package_install(
    workspace: &Path,
    package: &str,
    manager: &str,
    dev: bool,
    global_install: bool,
) -> ToolResult {
    let manager = if manager == "auto" { detect_package_manager(workspace) } else { manager };

    match manager {
        "pip" => {
            let mut args = vec!["-m", "pip", "install"];
            if !global_install {
                args.push("--user");
            }
            args.push(package);
            run("python3", &args, workspace, 300).await
        }
        "npm" => {
            let mut args = vec!["install"];
            if global_install {
                args.push("-g");
            } else if dev {
                args.push("--save-dev");
            }
            args.push(package);
            run("npm", &args, workspace, 300).await
        }
        "yarn" => {
            let mut args = vec!["add"];
            if dev {
                args.push("--dev");
            }
            args.push(package);
            run("yarn", &args, workspace, 300).await
        }
        "apt" => run("sudo", &["apt-get", "install", "-y", package], workspace, 600).await,
        "cargo" => run("cargo", &["add", package], workspace, 300).await,
        "go" => run("go", &["get", package], workspace, 300).await,
        other => ToolResult::error(format!(
            "Неизвестный менеджер пакетов: {}. Поддерживаются: pip, npm, yarn, apt, cargo, go",
            other
        )),
    }
}

/// Поиск пакета. `manager` = "pip" (через PyPI JSON API) | "npm" (через `npm search --json`).
pub async fn package_search(query: &str, manager: &str) -> ToolResult {
    match manager {
        "pip" => {
            let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap_or_default();
            let url = format!("https://pypi.org/pypi/{}/json", query);
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => return ToolResult::error(format!("Ошибка запроса к PyPI: {}", e)),
            };
            if !resp.status().is_success() {
                return ToolResult::error(format!("Пакет '{}' не найден на PyPI", query));
            }
            let data: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Не удалось разобрать ответ PyPI: {}", e)),
            };
            let info = &data["info"];
            let output = format!(
                "Package: {}\nVersion: {}\nSummary: {}\nAuthor: {}\nLicense: {}\nURL: {}\n",
                info["name"].as_str().unwrap_or("?"),
                info["version"].as_str().unwrap_or("?"),
                info["summary"].as_str().unwrap_or("?"),
                info["author"].as_str().unwrap_or("?"),
                info["license"].as_str().unwrap_or("?"),
                info["package_url"].as_str().unwrap_or("?"),
            );
            ToolResult::success(output)
        }
        "npm" => {
            let result = run("npm", &["search", query, "--json"], Path::new("."), 30).await;
            if !result.success {
                return result;
            }
            let packages: Vec<serde_json::Value> = match serde_json::from_str(&result.output) {
                Ok(v) => v,
                Err(_) => return ToolResult::error("Не удалось разобрать вывод `npm search --json`"),
            };
            if packages.is_empty() {
                return ToolResult::error(format!("Пакеты по запросу '{}' не найдены", query));
            }
            let mut output = String::new();
            for pkg in packages.iter().take(5) {
                output.push_str(&format!(
                    "Package: {}\nVersion: {}\nDescription: {}\nAuthor: {}\n\n",
                    pkg["name"].as_str().unwrap_or("?"),
                    pkg["version"].as_str().unwrap_or("?"),
                    pkg["description"].as_str().unwrap_or("?"),
                    pkg["author"]["name"].as_str().unwrap_or("N/A"),
                ));
            }
            ToolResult::success(output)
        }
        other => ToolResult::error(format!("Поиск не поддерживается для менеджера: {}", other)),
    }
}
