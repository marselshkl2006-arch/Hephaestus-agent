//! Background Agent — запуск инструментов в фоне без блокировки основного
//! диалога, с опросом статуса. Аналог `background_agent.py`.
//!
//! Важная оговорка по объёму: это фоновый запуск ОДНОГО вызова инструмента
//! (в т.ч. можно запустить в фоне `workflow_run` или `parallel_exec` —
//! получится составной фоновый план), а не отдельный полноценный цикл
//! `Agent::chat` с LLM. Полноценный фоновый под-агент с собственным
//! LLM-циклом упирается в то, что `Agent` сейчас не `Clone`/`Arc`-обёрнут
//! (см. STATUS_RU.md) — сделать это следующим шагом несложно, если понадобится.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{SharedToolRegistry, Tool, ToolResult};

#[derive(Clone, Debug, PartialEq)]
pub enum BgStatus {
    Running,
    Done,
    Failed,
}

#[derive(Clone)]
struct BgTask {
    tool: String,
    status: BgStatus,
    output: Option<String>,
    error: Option<String>,
    started_at: String,
}

#[derive(Clone, Default)]
pub struct BackgroundBoard {
    tasks: Arc<Mutex<HashMap<String, BgTask>>>,
}

static TASK_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_task_id() -> String {
    let n = TASK_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("bg_{}_{}", chrono::Local::now().format("%H%M%S"), n)
}

pub struct BackgroundRunTool {
    registry: SharedToolRegistry,
    board: BackgroundBoard,
}

impl BackgroundRunTool {
    pub fn new(registry: SharedToolRegistry, board: BackgroundBoard) -> Self {
        Self { registry, board }
    }
}

#[async_trait]
impl Tool for BackgroundRunTool {
    fn description(&self) -> &'static str {
        // ИСПРАВЛЕНО (см. TOOL_AUDIT_REPORT.md: "Ошибка параметров —
        // команда не передана"): у инструмента не было ни description(),
        // ни parameters_schema() — использовались пустые дефолты трейта
        // (`{"type":"object","properties":{}}`), так что LLM физически
        // не могла узнать, что нужно передавать 'tool' и 'args', а не
        // 'command'. Та же проблема была у Status/List ниже.
        "Запустить любой ЗАРЕГИСТРИРОВАННЫЙ инструмент (см. /tools) в фоне, \
         не дожидаясь его завершения — полезно для долгих операций \
         (сборка, тесты, docker pull). Не путать с bash в фоне через '&' — \
         это фоновый запуск инструмента АГЕНТА по имени, а не shell-команды."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool": {"type": "string", "description": "Имя зарегистрированного инструмента, напр. 'bash', 'db_query'"},
                "args": {"type": "object", "description": "Аргументы для этого инструмента — та же схема, что и при обычном вызове"}
            },
            "required": ["tool"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let tool_name = match args.get("tool").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return ToolResult::error("Требуется параметр 'tool' — имя инструмента для фонового запуска"),
        };
        let tool_args = args.get("args").cloned().unwrap_or_else(|| json!({}));

        let task_id = next_task_id();
        {
            let mut tasks = self.board.tasks.lock().await;
            tasks.insert(
                task_id.clone(),
                BgTask {
                    tool: tool_name.clone(),
                    status: BgStatus::Running,
                    output: None,
                    error: None,
                    started_at: chrono::Local::now().to_rfc3339(),
                },
            );
        }

        let registry = self.registry.clone();
        let board = self.board.clone();
        let task_id_clone = task_id.clone();
        let tool_name_display = tool_name.clone();

        tokio::spawn(async move {
            let outcome = registry.execute(&tool_name, &tool_args).await;
            let mut tasks = board.tasks.lock().await;
            if let Some(task) = tasks.get_mut(&task_id_clone) {
                match outcome {
                    Some(r) if r.success => {
                        task.status = BgStatus::Done;
                        task.output = Some(r.output);
                    }
                    Some(r) => {
                        task.status = BgStatus::Failed;
                        task.error = r.error;
                    }
                    None => {
                        task.status = BgStatus::Failed;
                        task.error = Some("Инструмент не найден".to_string());
                    }
                }
            }
        });

        ToolResult::success(format!(
            "Фоновая задача запущена: {} (инструмент: {}). Проверить: background_status(task_id=\"{}\")",
            task_id, tool_name_display, task_id
        ))
    }

    fn name(&self) -> &'static str {
        "background_run"
    }
}

pub struct BackgroundStatusTool {
    board: BackgroundBoard,
}

impl BackgroundStatusTool {
    pub fn new(board: BackgroundBoard) -> Self {
        Self { board }
    }
}

#[async_trait]
impl Tool for BackgroundStatusTool {
    fn description(&self) -> &'static str {
        "Проверить статус фоновой задачи, запущенной через background_run, по её task_id."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"task_id": {"type": "string", "description": "id из ответа background_run"}},
            "required": ["task_id"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let task_id = args.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
        if task_id.is_empty() {
            return ToolResult::error("Требуется параметр 'task_id'");
        }
        let tasks = self.board.tasks.lock().await;
        match tasks.get(task_id) {
            Some(t) => {
                let status_str = match t.status {
                    BgStatus::Running => "running",
                    BgStatus::Done => "done",
                    BgStatus::Failed => "failed",
                };
                let out = json!({
                    "task_id": task_id,
                    "tool": t.tool,
                    "status": status_str,
                    "started_at": t.started_at,
                    "output": t.output,
                    "error": t.error,
                });
                ToolResult::success(serde_json::to_string_pretty(&out).unwrap_or_default())
            }
            None => ToolResult::error(format!("Задача не найдена: {}", task_id)),
        }
    }

    fn name(&self) -> &'static str {
        "background_status"
    }
}

pub struct BackgroundListTool {
    board: BackgroundBoard,
}

impl BackgroundListTool {
    pub fn new(board: BackgroundBoard) -> Self {
        Self { board }
    }
}

#[async_trait]
impl Tool for BackgroundListTool {
    fn description(&self) -> &'static str {
        "Список всех фоновых задач (запущенных через background_run) и их статусов."
    }

    async fn execute(&self, _args: &Value) -> ToolResult {
        let tasks = self.board.tasks.lock().await;
        if tasks.is_empty() {
            return ToolResult::success("Фоновых задач нет.".to_string());
        }
        let mut lines = vec!["Фоновые задачи:".to_string()];
        for (id, t) in tasks.iter() {
            let status_str = match t.status {
                BgStatus::Running => "⏳ running",
                BgStatus::Done => "✅ done",
                BgStatus::Failed => "❌ failed",
            };
            lines.push(format!("  {} [{}] {} (запущена {})", id, status_str, t.tool, t.started_at));
        }
        ToolResult::success(lines.join("\n"))
    }

    fn name(&self) -> &'static str {
        "background_list"
    }
}
