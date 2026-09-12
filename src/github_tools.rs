use std::process::Command;
use std::path::Path;
use serde_json::Value;

/// Результат выполнения GitHub операций
#[derive(Clone)]
pub struct GithubResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

impl GithubResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self { success: true, output: output.into(), error: None }
    }
    pub fn err(error: impl Into<String>) -> Self {
        Self { success: false, output: String::new(), error: Some(error.into()) }
    }
}

/// GitHub Actions инструмент
pub struct GithubActionsTool {
    pub repo_path: String,
    pub token: Option<String>,
}

impl GithubActionsTool {
    pub fn new(repo_path: Option<&str>, token: Option<&str>) -> Self {
        Self {
            repo_path: repo_path.unwrap_or(".").to_string(),
            token: token.map(|s| s.to_string()),
        }
    }

    /// Запустить GitHub Actions workflow
    pub fn run_workflow(&self, workflow_file: &str, inputs: Option<Value>) -> GithubResult {
        let path = Path::new(&self.repo_path);
        if !path.exists() {
            return GithubResult::err(format!("Репозиторий не найден: {}", self.repo_path));
        }

        let mut cmd = Command::new("gh");
        cmd.current_dir(path)
            .arg("workflow")
            .arg("run")
            .arg(workflow_file);

        if let Some(token) = &self.token {
            cmd.env("GH_TOKEN", token);
        }

        if let Some(inputs) = inputs {
            if let Some(obj) = inputs.as_object() {
                for (key, value) in obj {
                    cmd.arg("-f");
                    cmd.arg(format!("{}={}", key, value.as_str().unwrap_or("")));
                }
            }
        }

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    GithubResult::ok(format!("✅ Workflow запущен: {}\n{}", workflow_file, stdout))
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    GithubResult::err(format!("Ошибка запуска workflow: {}", stderr))
                }
            }
            Err(e) => GithubResult::err(format!("Не удалось запустить gh: {}", e)),
        }
    }

    /// Получить статус workflow
    pub fn get_workflow_status(&self, workflow_file: &str) -> GithubResult {
        let path = Path::new(&self.repo_path);
        if !path.exists() {
            return GithubResult::err(format!("Репозиторий не найден: {}", self.repo_path));
        }

        let mut cmd = Command::new("gh");
        cmd.current_dir(path)
            .arg("workflow")
            .arg("list");

        if let Some(token) = &self.token {
            cmd.env("GH_TOKEN", token);
        }

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let filtered: Vec<&str> = stdout.lines()
                        .filter(|line| line.contains(workflow_file))
                        .collect();
                    if filtered.is_empty() {
                        GithubResult::ok(format!("Workflow '{}' не найден или нет запусков", workflow_file))
                    } else {
                        GithubResult::ok(format!("Статус workflow '{}':\n{}", workflow_file, filtered.join("\n")))
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    GithubResult::err(format!("Ошибка получения статуса: {}", stderr))
                }
            }
            Err(e) => GithubResult::err(format!("Не удалось запустить gh: {}", e)),
        }
    }

    /// Получить информацию о репозитории
    pub fn get_repo_info(&self) -> GithubResult {
        let path = Path::new(&self.repo_path);
        if !path.exists() {
            return GithubResult::err(format!("Репозиторий не найден: {}", self.repo_path));
        }

        // Проверяем, что это git репозиторий
        let git_check = Command::new("git")
            .current_dir(path)
            .arg("rev-parse")
            .arg("--git-dir")
            .output();

        match git_check {
            Ok(output) => {
                if !output.status.success() {
                    return GithubResult::err("Не git репозиторий");
                }
            }
            Err(e) => return GithubResult::err(format!("Ошибка проверки git: {}", e)),
        }

        let mut info = String::new();
        info.push_str(" Информация о репозитории\n");
        info.push_str(&format!("Путь: {}\n", self.repo_path));

        // Получаем remote URL
        let remote = Command::new("git")
            .current_dir(path)
            .arg("remote")
            .arg("get-url")
            .arg("origin")
            .output();

        if let Ok(out) = remote {
            if out.status.success() {
                let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
                info.push_str(&format!("Remote: {}\n", url));
            }
        }

        // Получаем текущую ветку
        let branch = Command::new("git")
            .current_dir(path)
            .arg("branch")
            .arg("--show-current")
            .output();

        if let Ok(out) = branch {
            if out.status.success() {
                let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
                info.push_str(&format!("Ветка: {}\n", b));
            }
        }

        // Получаем последние коммиты
        let log = Command::new("git")
            .current_dir(path)
            .arg("log")
            .arg("--oneline")
            .arg("-n")
            .arg("5")
            .output();

        if let Ok(out) = log {
            if out.status.success() {
                let commits = String::from_utf8_lossy(&out.stdout);
                info.push_str("\n Последние коммиты:\n");
                info.push_str(commits.trim());
                info.push('\n');
            }
        }

        GithubResult::ok(info)
    }

    /// Создать pull request
    pub fn create_pr(&self, title: &str, body: &str, base: Option<&str>, head: Option<&str>) -> GithubResult {
        let path = Path::new(&self.repo_path);
        if !path.exists() {
            return GithubResult::err(format!("Репозиторий не найден: {}", self.repo_path));
        }

        let mut cmd = Command::new("gh");
        cmd.current_dir(path)
            .arg("pr")
            .arg("create")
            .arg("--title")
            .arg(title)
            .arg("--body")
            .arg(body);

        if let Some(base_branch) = base {
            cmd.arg("--base").arg(base_branch);
        }
        if let Some(head_branch) = head {
            cmd.arg("--head").arg(head_branch);
        }

        if let Some(token) = &self.token {
            cmd.env("GH_TOKEN", token);
        }

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    GithubResult::ok(format!("✅ PR создан: {}\n{}", title, stdout))
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    GithubResult::err(format!("Ошибка создания PR: {}", stderr))
                }
            }
            Err(e) => GithubResult::err(format!("Не удалось запустить gh: {}", e)),
        }
    }

    /// Получить список PR
    pub fn list_prs(&self, state: Option<&str>) -> GithubResult {
        let path = Path::new(&self.repo_path);
        if !path.exists() {
            return GithubResult::err(format!("Репозиторий не найден: {}", self.repo_path));
        }

        let mut cmd = Command::new("gh");
        cmd.current_dir(path)
            .arg("pr")
            .arg("list");

        if let Some(state_val) = state {
            cmd.arg("--state").arg(state_val);
        }

        if let Some(token) = &self.token {
            cmd.env("GH_TOKEN", token);
        }

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    GithubResult::ok(format!(" Список PR:\n{}", stdout))
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    GithubResult::err(format!("Ошибка получения PR: {}", stderr))
                }
            }
            Err(e) => GithubResult::err(format!("Не удалось запустить gh: {}", e)),
        }
    }

    /// Получить статус CI/CD
    pub fn get_ci_status(&self) -> GithubResult {
        let path = Path::new(&self.repo_path);
        if !path.exists() {
            return GithubResult::err(format!("Репозиторий не найден: {}", self.repo_path));
        }

        let mut cmd = Command::new("gh");
        cmd.current_dir(path)
            .arg("run")
            .arg("list")
            .arg("--limit")
            .arg("10");

        if let Some(token) = &self.token {
            cmd.env("GH_TOKEN", token);
        }

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    GithubResult::ok(format!(" Статус CI/CD (последние 10 запусков):\n{}", stdout))
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    GithubResult::err(format!("Ошибка получения CI статуса: {}", stderr))
                }
            }
            Err(e) => GithubResult::err(format!("Не удалось запустить gh: {}", e)),
        }
    }
}

/// Создать GitHub инструменты
pub fn create_github_tools(repo_path: Option<&str>, token: Option<&str>) -> GithubActionsTool {
    GithubActionsTool::new(repo_path, token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_github_tool_new() {
        let tool = GithubActionsTool::new(Some("."), None);
        assert_eq!(tool.repo_path, ".");
    }

    #[test]
    fn test_github_tool_new_with_token() {
        let tool = GithubActionsTool::new(Some("."), Some("token123"));
        assert_eq!(tool.repo_path, ".");
        assert_eq!(tool.token, Some("token123".to_string()));
    }
}
