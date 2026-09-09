//! Async Executor — параллельное выполнение нескольких вызовов инструментов
//! за один ход. Аналог `async_tool_executor.py`.
//!
//! Регистрируется как обычный инструмент `parallel_exec`, принимающий список
//! вызовов и выполняющий их конкурентно через `futures::future::join_all`.
//! (Параллельность вызовов *внутри одного ответа LLM* с несколькими
//! tool_use-блоками реализована отдельно, в `Agent::chat` — см. `main.rs`.)

use async_trait::async_trait;
use futures_util::future::join_all;
use serde_json::{json, Value};

use super::{SharedToolRegistry, Tool, ToolResult};

pub struct ParallelExecTool {
    registry: SharedToolRegistry,
}

impl ParallelExecTool {
    pub fn new(registry: SharedToolRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for ParallelExecTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let calls = match args.get("calls").and_then(|v| v.as_array()) {
            Some(c) if !c.is_empty() => c.clone(),
            _ => return ToolResult::error("Требуется непустой массив 'calls': [{\"tool\": \"...\", \"args\": {...}}, ...]"),
        };

        let futures = calls.iter().map(|call| {
            let tool_name = call.get("tool").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let tool_args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            let registry = self.registry.clone();
            async move {
                if tool_name.is_empty() {
                    return json!({"tool": "", "success": false, "error": "Не указано поле 'tool'"});
                }
                match registry.execute(&tool_name, &tool_args).await {
                    Some(r) if r.success => json!({"tool": tool_name, "success": true, "output": r.output}),
                    Some(r) => json!({"tool": tool_name, "success": false, "error": r.error.unwrap_or_default()}),
                    None => json!({"tool": tool_name, "success": false, "error": "Инструмент не найден"}),
                }
            }
        });

        let results: Vec<Value> = join_all(futures).await;
        match serde_json::to_string_pretty(&results) {
            Ok(s) => ToolResult::success(s),
            Err(e) => ToolResult::error(format!("Ошибка сериализации результатов: {}", e)),
        }
    }

    fn name(&self) -> &'static str {
        "parallel_exec"
    }
}
