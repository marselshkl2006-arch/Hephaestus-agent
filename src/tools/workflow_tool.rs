//! Workflow System — многошаговые workflow: последовательное выполнение
//! списка вызовов инструментов, с остановкой на первой ошибке (если не
//! указано `continue_on_error: true`). Аналог `workflow_system.py`.
//!
//! В отличие от `parallel_exec` (async_executor.rs), шаги здесь выполняются
//! строго по порядку — так поддерживаются шаги, зависящие от результата
//! предыдущих (например: создать файл → затем его прочитать).

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{SharedToolRegistry, Tool, ToolResult};

pub struct WorkflowRunTool {
    registry: SharedToolRegistry,
}

impl WorkflowRunTool {
    pub fn new(registry: SharedToolRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for WorkflowRunTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let steps = match args.get("steps").and_then(|v| v.as_array()) {
            Some(s) if !s.is_empty() => s.clone(),
            _ => return ToolResult::error(
                "Требуется непустой массив 'steps': [{\"tool\": \"...\", \"args\": {...}}, ...]",
            ),
        };
        let continue_on_error = args
            .get("continue_on_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut results = Vec::new();
        for (i, step) in steps.iter().enumerate() {
            let tool_name = step.get("tool").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let tool_args = step.get("args").cloned().unwrap_or_else(|| json!({}));

            if tool_name.is_empty() {
                results.push(json!({
                    "step": i + 1, "tool": "", "success": false,
                    "error": "Не указано поле 'tool'",
                }));
                if !continue_on_error {
                    break;
                }
                continue;
            }

            let outcome = self.registry.execute(&tool_name, &tool_args).await;
            let (success, output_or_error) = match outcome {
                Some(r) if r.success => (true, r.output),
                Some(r) => (false, r.error.unwrap_or_default()),
                None => (false, "Инструмент не найден".to_string()),
            };

            results.push(json!({
                "step": i + 1,
                "tool": tool_name,
                "success": success,
                "output": output_or_error,
            }));

            if !success && !continue_on_error {
                break;
            }
        }

        let completed = results.iter().filter(|r| r["success"].as_bool().unwrap_or(false)).count();
        let summary = json!({
            "total_steps": steps.len(),
            "completed": completed,
            "results": results,
        });

        match serde_json::to_string_pretty(&summary) {
            Ok(s) => ToolResult::success(s),
            Err(e) => ToolResult::error(format!("Ошибка сериализации результатов: {}", e)),
        }
    }


    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
  "type": "object",
  "properties": {
    "steps": {"type": "array", "items": {"type": "object", "properties": {"tool": {"type": "string"}, "args": {"type": "object"}}, "required": ["tool"]}}
  },
  "required": ["steps"]
})
    }

    fn name(&self) -> &'static str {
        "workflow_run"
    }
}
