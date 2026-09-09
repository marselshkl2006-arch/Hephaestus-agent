//! Todo Tools — список задач для больших планов, по образцу agentic
//! todo-list'ов (opencode/Claude Code): на сложную многошаговую задачу
//! модель сначала СОСТАВЛЯЕТ план (todo_write), затем идёт по нему,
//! отмечая выполненные шаги и держа ровно одну in_progress.
//!
//! Зачем это модели: без внешнего списка LLM "держит план в голове" —
//! при длинных ходах, сжатии контекста (Smart Compression) или ошибках
//! посреди пути план теряется и агент начинает импровизировать. Список:
//!   • живёт вне истории сообщений (не сжимается),
//!   • виден модели каждый ход через подсказку в system prompt
//!     (см. Agent::chat → TodoBoard::context_hint()),
//!   • виден человеку — вызовы todo_write отображаются в чате как 🔧.
//!
//! Это НЕ замена task_manager.rs (Kanban-доска пользователя): доска —
//! про задачи ПОЛЬЗОВАТЕЛЯ, этот список — рабочий план самой модели на
//! текущий запрос.

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{Tool, ToolResult};

pub const STATUSES: &[&str] = &["pending", "in_progress", "completed", "cancelled"];
pub const PRIORITIES: &[&str] = &["high", "medium", "low"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_priority")]
    pub priority: String,
}

fn default_status() -> String {
    "pending".to_string()
}
fn default_priority() -> String {
    "medium".to_string()
}

/// Общий список задач агента. Arc-разделяется между Agent и инструментами.
#[derive(Default)]
pub struct TodoBoard {
    items: StdMutex<Vec<TodoItem>>,
}

impl TodoBoard {
    pub fn new() -> Self {
        Self::default()
    }

    fn replace(&self, items: Vec<TodoItem>) {
        if let Ok(mut g) = self.items.lock() {
            *g = items;
        }
    }

    /// Снимок списка.
    pub fn snapshot(&self) -> Vec<TodoItem> {
        self.items.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Текстовый рендер — в результат инструмента (модель видит своё же
    /// состояние) и в /todos (человек).
    pub fn render(&self) -> String {
        let items = self.snapshot();
        if items.is_empty() {
            return "(список задач пуст)".to_string();
        }
        let mark = |s: &str| match s {
            "completed" => "[x]",
            "in_progress" => "[~]",
            "cancelled" => "[-]",
            _ => "[ ]",
        };
        let done = items.iter().filter(|i| i.status == "completed").count();
        let mut out = vec![format!("План: {} из {} шагов выполнено", done, items.len())];
        for (i, item) in items.iter().enumerate() {
            out.push(format!("{}. {} {} ({})", i + 1, mark(&item.status), item.content, item.priority));
        }
        out.join("\n")
    }

    /// Подсказка для system prompt: есть активный план — напомнить о нём.
    /// Пусто, когда списка нет или он весь закрыт.
    pub fn context_hint(&self) -> Option<String> {
        let items = self.snapshot();
        let active: Vec<&TodoItem> = items
            .iter()
            .filter(|i| i.status == "pending" || i.status == "in_progress")
            .collect();
        if active.is_empty() {
            return None;
        }
        let current = active.first().map(|i| i.content.as_str()).unwrap_or("");
        Some(format!(
            "Активный план задачи: {} шаг(ов) осталось. Текущий шаг: \"{}\". \
             Выполняй шаги по одному; после каждого шага обновляй список через todo_write \
             (отмечай completed, ставь следующий in_progress). Не бросай план без причины.",
            active.len(),
            current
        ))
    }

    /// Валидация + сохранение. Возвращает Err с понятным текстом для модели.
    fn validate(items: &[TodoItem]) -> Result<(), String> {
        if items.is_empty() {
            return Err("Список пустой: передай хотя бы один шаг или используй все статусы completed/cancelled, чтобы закрыть план.".to_string());
        }
        for (i, item) in items.iter().enumerate() {
            if item.content.trim().is_empty() {
                return Err(format!("Шаг {}: пустой content.", i + 1));
            }
            if !STATUSES.contains(&item.status.as_str()) {
                return Err(format!(
                    "Шаг '{}': неизвестный статус '{}' (допустимо: {})",
                    item.content, item.status, STATUSES.join("/")
                ));
            }
            if !PRIORITIES.contains(&item.priority.as_str()) {
                return Err(format!(
                    "Шаг '{}': неизвестный приоритет '{}' (допустимо: {})",
                    item.content, item.priority, PRIORITIES.join("/")
                ));
            }
        }
        let in_progress = items.iter().filter(|i| i.status == "in_progress").count();
        if in_progress > 1 {
            return Err(format!(
                "in_progress может быть только ОДИН шаг (передано {}). Порядок: заверши предыдущий (completed), потом начинай следующий.",
                in_progress
            ));
        }
        Ok(())
    }
}

/// todo_write — заменить список целиком (как в opencode/claude code:
/// модель всегда шлёт ПОЛНЫЙ актуальный список, а не диффы).
pub struct TodoWriteTool {
    board: Arc<TodoBoard>,
}

impl TodoWriteTool {
    pub fn new(board: Arc<TodoBoard>) -> Self {
        Self { board }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        "todo_write"
    }

    fn description(&self) -> &'static str {
        "Вести план большой задачи: составь список шагов В НАЧАЛЕ сложной задачи и обновляй статусы по ходу работы. Правила: передаётся ПОЛНЫЙ список целиком; статус 'in_progress' максимум у ОДНОГО шага; сразу после выполнения шага отмечай его 'completed' и ставь следующий в 'in_progress'; новые обнаруженные подзадачи добавляй в конец со статусом pending."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "Полный список шагов плана",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string", "description": "Что сделать (конкретное действие)"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"], "description": "Статус, по умолчанию pending"},
                            "priority": {"type": "string", "enum": ["high", "medium", "low"], "description": "Приоритет, по умолчанию medium"}
                        },
                        "required": ["content"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let Some(raw) = args.get("todos").and_then(|v| v.as_array()) else {
            return ToolResult::error("нужен массив 'todos'");
        };
        let items: Vec<TodoItem> = raw
            .iter()
            .filter_map(|v| serde_json::from_value::<TodoItem>(v.clone()).ok())
            .collect();
        if items.len() != raw.len() {
            return ToolResult::error("часть элементов todos не разобралась — каждый должен быть объектом с полем 'content'");
        }
        if let Err(e) = TodoBoard::validate(&items) {
            return ToolResult::error(e);
        }
        self.board.replace(items);
        // Отдаём отрендеренный список — модель подтверждает себе состояние.
        ToolResult::success(self.board.render())
    }
}

/// todo_read — посмотреть текущий план (после сжатия контекста, при
/// восстановлении сессии и т.п.).
pub struct TodoReadTool {
    board: Arc<TodoBoard>,
}

impl TodoReadTool {
    pub fn new(board: Arc<TodoBoard>) -> Self {
        Self { board }
    }
}

#[async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &'static str {
        "todo_read"
    }

    fn description(&self) -> &'static str {
        "Показать текущий план задачи (шаги и статусы). Используй, чтобы вспомнить, на каком шаге работа."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: &Value) -> ToolResult {
        ToolResult::success(self.board.render())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board_with(items: Vec<TodoItem>) -> Arc<TodoBoard> {
        let b = Arc::new(TodoBoard::new());
        b.replace(items);
        b
    }

    fn item(content: &str, status: &str) -> TodoItem {
        TodoItem {
            content: content.to_string(),
            status: status.to_string(),
            priority: "medium".to_string(),
        }
    }

    #[test]
    fn test_validate_two_in_progress_rejected() {
        let items = vec![item("a", "in_progress"), item("b", "in_progress")];
        assert!(TodoBoard::validate(&items).is_err());
    }

    #[test]
    fn test_validate_bad_status_rejected() {
        let items = vec![item("a", "done")];
        assert!(TodoBoard::validate(&items).is_err());
    }

    #[test]
    fn test_validate_empty_rejected() {
        assert!(TodoBoard::validate(&[]).is_err());
    }

    #[test]
    fn test_render_and_hint() {
        let b = board_with(vec![
            item("шаг 1", "completed"),
            item("шаг 2", "in_progress"),
            item("шаг 3", "pending"),
        ]);
        let rendered = b.render();
        assert!(rendered.contains("1 из 3"));
        assert!(rendered.contains("[x] шаг 1"));
        assert!(rendered.contains("[~] шаг 2"));

        let hint = b.context_hint().unwrap();
        assert!(hint.contains("2 шаг"));
        assert!(hint.contains("шаг 2"));

        // Всё завершено — подсказки нет.
        let b2 = board_with(vec![item("готово", "completed")]);
        assert!(b2.context_hint().is_none());
    }

    #[tokio::test]
    async fn test_todo_write_tool_roundtrip() {
        let board = Arc::new(TodoBoard::new());
        let tool = TodoWriteTool::new(board.clone());

        let res = tool
            .execute(&serde_json::json!({
                "todos": [
                    {"content": "разобраться", "status": "completed"},
                    {"content": "починить", "status": "in_progress", "priority": "high"}
                ]
            }))
            .await;
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert_eq!(board.snapshot().len(), 2);
        assert!(board.render().contains("[~] починить"));
    }
}
