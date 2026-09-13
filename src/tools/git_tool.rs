use serde_json::Value;
use tokio::process::Command;
use async_trait::async_trait;

use crate::tools::{ToolResult, Tool};
use crate::workdir::WorkDir;

/// Git инструмент для работы с репозиториями
pub struct GitTool {
    workdir: WorkDir,
}

impl GitTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self { workdir }
    }

    /// Каталог репозитория: параметр `repo` из вызова ИЛИ workdir.
    /// ФИКС (живой прогон на Windows): модель просила git status в
    /// другой папке, но инструмент жёстко сидел в workdir — приходилось
    /// выкручиваться bash'ем. Теперь путь репо задаётся аргументом.
    fn repo_dir(&self, args: &Value) -> std::path::PathBuf {
        args.get("repo")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| self.workdir.get())
    }

    async fn run_git(&self, args: &[&str], repo: &std::path::Path) -> ToolResult {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .await;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let success = out.status.success();

                if success {
                    ToolResult::success(stdout)
                } else {
                    ToolResult::error(format!("Git error: {}", stderr))
                }
            }
            Err(e) => ToolResult::error(format!("Failed to run git: {}", e)),
        }
    }

    async fn status(&self, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["status"], repo).await
    }

    async fn diff(&self, staged: bool, repo: &std::path::Path) -> ToolResult {
        if staged {
            self.run_git(&["diff", "--staged"], repo).await
        } else {
            self.run_git(&["diff"], repo).await
        }
    }

    async fn log(&self, max_count: usize, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["log", "--oneline", "-n", &max_count.to_string()], repo).await
    }

    async fn add(&self, files: &[String], repo: &std::path::Path) -> ToolResult {
        let mut args = vec!["add"];
        for f in files {
            args.push(f.as_str());
        }
        self.run_git(&args, repo).await
    }

    async fn commit(&self, message: &str, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["commit", "-m", message], repo).await
    }

    async fn branch(&self, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["branch", "-a"], repo).await
    }

    async fn push(&self, remote: &str, branch: Option<&str>, force: bool, repo: &std::path::Path) -> ToolResult {
        let mut args = vec!["push", remote];
        if let Some(b) = branch {
            args.push(b);
        }
        if force {
            args.push("--force");
        }
        self.run_git(&args, repo).await
    }

    async fn pull(&self, remote: &str, branch: Option<&str>, repo: &std::path::Path) -> ToolResult {
        let mut args = vec!["pull", remote];
        if let Some(b) = branch {
            args.push(b);
        }
        self.run_git(&args, repo).await
    }

    async fn checkout(&self, branch: &str, create: bool, repo: &std::path::Path) -> ToolResult {
        let mut args = vec!["checkout"];
        if create {
            args.push("-b");
        }
        args.push(branch);
        self.run_git(&args, repo).await
    }

    async fn create_branch(&self, branch_name: &str, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["branch", branch_name], repo).await
    }

    async fn delete_branch(&self, branch_name: &str, force: bool, repo: &std::path::Path) -> ToolResult {
        let flag = if force { "-D" } else { "-d" };
        self.run_git(&[flag, branch_name], repo).await
    }

    async fn stash(&self, message: Option<&str>, repo: &std::path::Path) -> ToolResult {
        match message {
            Some(msg) => self.run_git(&["stash", "push", "-m", msg], repo).await,
            None => self.run_git(&["stash"], repo).await,
        }
    }

    async fn stash_pop(&self, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["stash", "pop"], repo).await
    }

    async fn stash_list(&self, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["stash", "list"], repo).await
    }

    async fn reset(&self, mode: &str, commit: &str, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["reset", &format!("--{}", mode), commit], repo).await
    }

    async fn fetch(&self, remote: &str, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["fetch", remote], repo).await
    }

    async fn merge(&self, branch: &str, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["merge", branch], repo).await
    }

    async fn rebase(&self, branch: &str, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["rebase", branch], repo).await
    }

    async fn tag(&self, tag_name: Option<&str>, message: Option<&str>, repo: &std::path::Path) -> ToolResult {
        match (tag_name, message) {
            (Some(name), Some(msg)) => self.run_git(&["tag", "-a", name, "-m", msg], repo).await,
            (Some(name), None) => self.run_git(&["tag", name], repo).await,
            (None, _) => self.run_git(&["tag", "-l"], repo).await,
        }
    }

    async fn show(&self, commit: &str, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["show", commit], repo).await
    }

    async fn blame(&self, file_path: &str, repo: &std::path::Path) -> ToolResult {
        self.run_git(&["blame", file_path], repo).await
    }
}

#[async_trait]
impl Tool for GitTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let cmd = args.get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("status");
        let repo = self.repo_dir(args);
        let repo = repo.as_path();

        // CLI-привычки моделей: "git status --short" приходит как command
        // "status --short" — первый токен это action, хвост игнорируем
        // (специфичные опции передаются отдельными полями схемы).
        let action = cmd.split_whitespace().next().unwrap_or("status");

        match action {
            "status" => self.status(repo).await,
            "diff" => {
                let staged = args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
                self.diff(staged, repo).await
            }
            "log" => {
                let max_count = args.get("max_count").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                self.log(max_count, repo).await
            }
            "add" => {
                let files = args.get("files")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
                    .unwrap_or_else(|| vec![".".to_string()]);
                self.add(&files, repo).await
            }
            "commit" => {
                let message = args.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Update");
                self.commit(message, repo).await
            }
            "branch" => self.branch(repo).await,
            "push" => {
                let remote = args.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
                let branch = args.get("branch").and_then(|v| v.as_str());
                let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                self.push(remote, branch, force, repo).await
            }
            "pull" => {
                let remote = args.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
                let branch = args.get("branch").and_then(|v| v.as_str());
                self.pull(remote, branch, repo).await
            }
            "checkout" => {
                let branch = args.get("branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                let create = args.get("create").and_then(|v| v.as_bool()).unwrap_or(false);
                self.checkout(branch, create, repo).await
            }
            "create_branch" => {
                let branch_name = args.get("branch_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("new-branch");
                self.create_branch(branch_name, repo).await
            }
            "delete_branch" => {
                let branch_name = args.get("branch_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("branch");
                let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                self.delete_branch(branch_name, force, repo).await
            }
            "stash" => {
                let message = args.get("message").and_then(|v| v.as_str());
                self.stash(message, repo).await
            }
            "stash_pop" => self.stash_pop(repo).await,
            "stash_list" => self.stash_list(repo).await,
            "reset" => {
                let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("mixed");
                let commit = args.get("commit").and_then(|v| v.as_str()).unwrap_or("HEAD");
                self.reset(mode, commit, repo).await
            }
            "fetch" => {
                let remote = args.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
                self.fetch(remote, repo).await
            }
            "merge" => {
                let branch = args.get("branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                self.merge(branch, repo).await
            }
            "rebase" => {
                let branch = args.get("branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                self.rebase(branch, repo).await
            }
            "tag" => {
                let tag_name = args.get("tag_name").and_then(|v| v.as_str());
                let message = args.get("message").and_then(|v| v.as_str());
                self.tag(tag_name, message, repo).await
            }
            "show" => {
                let commit = args.get("commit").and_then(|v| v.as_str()).unwrap_or("HEAD");
                self.show(commit, repo).await
            }
            "blame" => {
                let file_path = args.get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.blame(file_path, repo).await
            }
            _ => ToolResult::error(format!("Unknown git command: {}", cmd)),
        }
    }

    fn name(&self) -> &'static str {
        "git"
    }


    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "git-подкоманда: status | diff | log | add | commit | branch | push | pull | checkout | create_branch | delete_branch | stash | stash_pop | stash_list | reset | fetch | merge | rebase | show | blame"},
                "repo": {"type": "string", "description": "Путь к репозиторию. По умолчанию — рабочая папка агента."},
                "files": {"type": "array", "items": {"type": "string"}},
                "message": {"type": "string"},
                "staged": {"type": "boolean"},
                "remote": {"type": "string"},
                "branch": {"type": "string"},
                "force": {"type": "boolean"},
                "create": {"type": "boolean"},
                "branch_name": {"type": "string"},
                "max_count": {"type": "integer"},
                "commit": {"type": "string"},
                "file_path": {"type": "string"},
                "mode": {"type": "string"}
            }
        })
    }

}


