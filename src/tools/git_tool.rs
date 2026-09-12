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

    async fn run_git(&self, args: &[&str]) -> ToolResult {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.workdir.get())
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

    async fn status(&self) -> ToolResult {
        self.run_git(&["status"]).await
    }

    async fn diff(&self, staged: bool) -> ToolResult {
        if staged {
            self.run_git(&["diff", "--staged"]).await
        } else {
            self.run_git(&["diff"]).await
        }
    }

    async fn log(&self, max_count: usize) -> ToolResult {
        self.run_git(&["log", "--oneline", "-n", &max_count.to_string()]).await
    }

    async fn add(&self, files: &[String]) -> ToolResult {
        let mut args = vec!["add"];
        for f in files {
            args.push(f.as_str());
        }
        self.run_git(&args).await
    }

    async fn commit(&self, message: &str) -> ToolResult {
        self.run_git(&["commit", "-m", message]).await
    }

    async fn branch(&self) -> ToolResult {
        self.run_git(&["branch", "-a"]).await
    }

    async fn push(&self, remote: &str, branch: Option<&str>, force: bool) -> ToolResult {
        let mut args = vec!["push", remote];
        if let Some(b) = branch {
            args.push(b);
        }
        if force {
            args.push("--force");
        }
        self.run_git(&args).await
    }

    async fn pull(&self, remote: &str, branch: Option<&str>) -> ToolResult {
        let mut args = vec!["pull", remote];
        if let Some(b) = branch {
            args.push(b);
        }
        self.run_git(&args).await
    }

    async fn checkout(&self, branch: &str, create: bool) -> ToolResult {
        let mut args = vec!["checkout"];
        if create {
            args.push("-b");
        }
        args.push(branch);
        self.run_git(&args).await
    }

    async fn create_branch(&self, branch_name: &str) -> ToolResult {
        self.run_git(&["branch", branch_name]).await
    }

    async fn delete_branch(&self, branch_name: &str, force: bool) -> ToolResult {
        let flag = if force { "-D" } else { "-d" };
        self.run_git(&[flag, branch_name]).await
    }

    async fn stash(&self, message: Option<&str>) -> ToolResult {
        match message {
            Some(msg) => self.run_git(&["stash", "push", "-m", msg]).await,
            None => self.run_git(&["stash"]).await,
        }
    }

    async fn stash_pop(&self) -> ToolResult {
        self.run_git(&["stash", "pop"]).await
    }

    async fn stash_list(&self) -> ToolResult {
        self.run_git(&["stash", "list"]).await
    }

    async fn reset(&self, mode: &str, commit: &str) -> ToolResult {
        self.run_git(&["reset", &format!("--{}", mode), commit]).await
    }

    async fn fetch(&self, remote: &str) -> ToolResult {
        self.run_git(&["fetch", remote]).await
    }

    async fn merge(&self, branch: &str) -> ToolResult {
        self.run_git(&["merge", branch]).await
    }

    async fn rebase(&self, branch: &str) -> ToolResult {
        self.run_git(&["rebase", branch]).await
    }

    async fn tag(&self, tag_name: Option<&str>, message: Option<&str>) -> ToolResult {
        match (tag_name, message) {
            (Some(name), Some(msg)) => self.run_git(&["tag", "-a", name, "-m", msg]).await,
            (Some(name), None) => self.run_git(&["tag", name]).await,
            (None, _) => self.run_git(&["tag", "-l"]).await,
        }
    }

    async fn show(&self, commit: &str) -> ToolResult {
        self.run_git(&["show", commit]).await
    }

    async fn blame(&self, file_path: &str) -> ToolResult {
        self.run_git(&["blame", file_path]).await
    }
}

#[async_trait]
impl Tool for GitTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let cmd = args.get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("status");

        // CLI-привычки моделей: "git status --short" приходит как command
        // "status --short" — первый токен это action, хвост игнорируем
        // (специфичные опции передаются отдельными полями схемы).
        let action = cmd.split_whitespace().next().unwrap_or("status");

        match action {
            "status" => self.status().await,
            "diff" => {
                let staged = args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false);
                self.diff(staged).await
            }
            "log" => {
                let max_count = args.get("max_count").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                self.log(max_count).await
            }
            "add" => {
                let files = args.get("files")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
                    .unwrap_or_else(|| vec![".".to_string()]);
                self.add(&files).await
            }
            "commit" => {
                let message = args.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Update");
                self.commit(message).await
            }
            "branch" => self.branch().await,
            "push" => {
                let remote = args.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
                let branch = args.get("branch").and_then(|v| v.as_str());
                let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                self.push(remote, branch, force).await
            }
            "pull" => {
                let remote = args.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
                let branch = args.get("branch").and_then(|v| v.as_str());
                self.pull(remote, branch).await
            }
            "checkout" => {
                let branch = args.get("branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                let create = args.get("create").and_then(|v| v.as_bool()).unwrap_or(false);
                self.checkout(branch, create).await
            }
            "create_branch" => {
                let branch_name = args.get("branch_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("new-branch");
                self.create_branch(branch_name).await
            }
            "delete_branch" => {
                let branch_name = args.get("branch_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("branch");
                let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                self.delete_branch(branch_name, force).await
            }
            "stash" => {
                let message = args.get("message").and_then(|v| v.as_str());
                self.stash(message).await
            }
            "stash_pop" => self.stash_pop().await,
            "stash_list" => self.stash_list().await,
            "reset" => {
                let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("mixed");
                let commit = args.get("commit").and_then(|v| v.as_str()).unwrap_or("HEAD");
                self.reset(mode, commit).await
            }
            "fetch" => {
                let remote = args.get("remote").and_then(|v| v.as_str()).unwrap_or("origin");
                self.fetch(remote).await
            }
            "merge" => {
                let branch = args.get("branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                self.merge(branch).await
            }
            "rebase" => {
                let branch = args.get("branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                self.rebase(branch).await
            }
            "tag" => {
                let tag_name = args.get("tag_name").and_then(|v| v.as_str());
                let message = args.get("message").and_then(|v| v.as_str());
                self.tag(tag_name, message).await
            }
            "show" => {
                let commit = args.get("commit").and_then(|v| v.as_str()).unwrap_or("HEAD");
                self.show(commit).await
            }
            "blame" => {
                let file_path = args.get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.blame(file_path).await
            }
            _ => ToolResult::error(format!("Unknown git command: {}", cmd)),
        }
    }

    fn name(&self) -> &'static str {
        "git"
    }
}


