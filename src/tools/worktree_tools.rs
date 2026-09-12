//! Worktree Tools — обёртка над `git worktree`, чтобы агент мог работать
//! в отдельном каталоге поверх той же истории git (например, чтобы
//! попробовать рискованное изменение, не трогая текущую рабочую копию).
//! Аналог `worktree_tools.py`.

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use super::{Tool, ToolResult};

async fn run_git(dir: &str, args: &[&str]) -> ToolResult {
    match Command::new("git").args(args).current_dir(dir).output().await {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                ToolResult::success(if stdout.trim().is_empty() { stderr } else { stdout })
            } else {
                ToolResult::error(if stderr.trim().is_empty() { stdout } else { stderr })
            }
        }
        Err(e) => ToolResult::error(format!("Не удалось выполнить git: {}", e)),
    }
}

pub struct WorktreeTool;
impl WorktreeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WorktreeTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let repo_dir = args.get("repo_dir").and_then(|v| v.as_str()).unwrap_or(".");
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");

        match action {
            "list" => run_git(repo_dir, &["worktree", "list"]).await,
            "add" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                if path.is_empty() {
                    return ToolResult::error("Требуется параметр 'path' для action=add");
                }
                match args.get("branch").and_then(|v| v.as_str()) {
                    Some(branch) => run_git(repo_dir, &["worktree", "add", path, branch]).await,
                    None => run_git(repo_dir, &["worktree", "add", path]).await,
                }
            }
            "remove" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                if path.is_empty() {
                    return ToolResult::error("Требуется параметр 'path' для action=remove");
                }
                let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                if force {
                    run_git(repo_dir, &["worktree", "remove", "--force", path]).await
                } else {
                    run_git(repo_dir, &["worktree", "remove", path]).await
                }
            }
            "prune" => run_git(repo_dir, &["worktree", "prune"]).await,
            other => ToolResult::error(format!(
                "Неизвестный action: '{}'. Доступно: list, add, remove, prune",
                other
            )),
        }
    }

    fn name(&self) -> &'static str {
        "worktree"
    }
}
