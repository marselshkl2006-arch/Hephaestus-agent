//! Итерация 7 (продолжение доработки, по запросу пользователя —
//! "доделай"): обёртки над `caching.rs` и `task_manager.rs`, которые
//! ранее были признаны дубликатами и оставлены неподключёнными.
//! Пересмотрел решение при повторном чтении:
//!
//! - `caching.rs` — НЕ дубликат `tools/file_cache.rs`. Тот — узкоспециа-
//!   лизированный кэш содержимого файлов на `moka` для `file_tools.rs`.
//!   `caching.rs` — универсальный key-value scratch-кэш с TTL для любых
//!   данных агента (промежуточные результаты, дорогие вычисления между
//!   шагами одного диалога). Разные задачи → оба нужны, подключаю оба.
//! - `task_manager.rs` — НЕ дубликат `task_tools.rs`. `task_tools.rs`
//!   (itr.5, `task_add`/`task_info`/...) — простой to-do трекер с полями
//!   blocks/blocked_by (граф зависимостей). `task_manager.rs` — Kanban-
//!   доска с приоритетами (Low/Medium/High/Critical), дедлайнами,
//!   подзадачами, тегами, назначением на исполнителя. Разные модели
//!   данных для разных сценариев → регистрирую под отдельным префиксом
//!   `board_*`, чтобы не путать с `task_*`.
//!
//! `learning.rs` тоже подключаю здесь: инструменты для чтения/записи
//! статистики успехов-неудач инструментов и предпочтений пользователя.
//! Само накопление статистики (`record_success`/`record_failure` после
//! каждого вызова инструмента) — в `Agent::chat()` (main.rs), не здесь.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, Mutex};

use crate::caching::{Cache, CacheConfig};
use crate::task_manager::{Task, TaskManager, TaskPriority, TaskStatus};
use crate::tools::{Tool, ToolResult};

fn s(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

// ============================================================ caching =

pub struct CacheSetTool(Arc<Mutex<Cache>>);
#[async_trait]
impl Tool for CacheSetTool {
    fn name(&self) -> &'static str {
        "cache_set"
    }
    fn description(&self) -> &'static str {
        "Сохранить значение в кэше агента на время текущей сессии (опционально с TTL в секундах)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"},
                "value": {"description": "Любое JSON-значение"},
                "ttl_secs": {"type": "integer", "description": "Время жизни в секундах, опционально"}
            },
            "required": ["key", "value"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let key = match s(args, "key") {
            Some(k) => k,
            None => return ToolResult::error("нужен параметр 'key'"),
        };
        let value = args.get("value").cloned().unwrap_or(Value::Null);
        let ttl = args.get("ttl_secs").and_then(|v| v.as_u64());
        let mut cache = self.0.lock().unwrap();
        cache.set(&key, value, ttl);
        ToolResult::success(format!("Сохранено в кэше: {}", key))
    }
}

pub struct CacheGetTool(Arc<Mutex<Cache>>);
#[async_trait]
impl Tool for CacheGetTool {
    fn name(&self) -> &'static str {
        "cache_get"
    }
    fn description(&self) -> &'static str {
        "Прочитать значение из кэша агента по ключу."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let key = match s(args, "key") {
            Some(k) => k,
            None => return ToolResult::error("нужен параметр 'key'"),
        };
        let mut cache = self.0.lock().unwrap();
        match cache.get(&key) {
            Some(v) => ToolResult::success(v.to_string()),
            None => ToolResult::error(format!("Ключ не найден или истёк: {}", key)),
        }
    }
}

pub struct CacheDeleteTool(Arc<Mutex<Cache>>);
#[async_trait]
impl Tool for CacheDeleteTool {
    fn name(&self) -> &'static str {
        "cache_delete"
    }
    fn description(&self) -> &'static str {
        "Удалить значение из кэша агента по ключу."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let key = match s(args, "key") {
            Some(k) => k,
            None => return ToolResult::error("нужен параметр 'key'"),
        };
        let mut cache = self.0.lock().unwrap();
        match cache.remove(&key) {
            Some(_) => ToolResult::success(format!("Удалено: {}", key)),
            None => ToolResult::error(format!("Ключ не найден: {}", key)),
        }
    }
}

pub struct CacheStatsTool(Arc<Mutex<Cache>>);
#[async_trait]
impl Tool for CacheStatsTool {
    fn name(&self) -> &'static str {
        "cache_stats"
    }
    fn description(&self) -> &'static str {
        "Статистика кэша агента (hit rate, размер, кол-во попаданий/промахов)."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        let cache = self.0.lock().unwrap();
        ToolResult::success(cache.stats().format())
    }
}

pub struct CacheClearTool(Arc<Mutex<Cache>>);
#[async_trait]
impl Tool for CacheClearTool {
    fn name(&self) -> &'static str {
        "cache_clear"
    }
    fn description(&self) -> &'static str {
        "Полностью очистить кэш агента."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        let mut cache = self.0.lock().unwrap();
        let n = cache.size();
        cache.clear();
        ToolResult::success(format!("Кэш очищен ({} записей удалено)", n))
    }
}

// ======================================================= task_manager =

fn parse_priority(s: Option<&str>) -> TaskPriority {
    match s.map(|s| s.to_lowercase()) {
        Some(ref v) if v == "low" => TaskPriority::Low,
        Some(ref v) if v == "high" => TaskPriority::High,
        Some(ref v) if v == "critical" => TaskPriority::Critical,
        _ => TaskPriority::Medium,
    }
}

fn parse_status(s: &str) -> Option<TaskStatus> {
    match s.to_lowercase().as_str() {
        "pending" => Some(TaskStatus::Pending),
        "in_progress" | "inprogress" => Some(TaskStatus::InProgress),
        "completed" | "done" => Some(TaskStatus::Completed),
        "failed" => Some(TaskStatus::Failed),
        "cancelled" | "canceled" => Some(TaskStatus::Cancelled),
        _ => None,
    }
}

pub struct BoardCreateTool(Arc<Mutex<TaskManager>>);
#[async_trait]
impl Tool for BoardCreateTool {
    fn name(&self) -> &'static str {
        "board_create"
    }
    fn description(&self) -> &'static str {
        "Создать задачу на канбан-доске (приоритет/теги/исполнитель, отдельно от простого трекера task_add)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "description": {"type": "string"},
                "priority": {"type": "string", "description": "low|medium|high|critical, по умолчанию medium"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "assigned_to": {"type": "string"}
            },
            "required": ["title"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let title = match s(args, "title") {
            Some(t) => t,
            None => return ToolResult::error("нужен параметр 'title'"),
        };
        let mut task = Task::new(&title).with_priority(parse_priority(s(args, "priority").as_deref()));
        if let Some(desc) = s(args, "description") {
            task = task.with_description(&desc);
        }
        if let Some(tags) = args.get("tags").and_then(|v| v.as_array()) {
            task.tags = tags.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
        }
        if let Some(assignee) = s(args, "assigned_to") {
            task.assigned_to = Some(assignee);
        }
        let mut mgr = self.0.lock().unwrap();
        let id = mgr.create_task_full(task);
        ToolResult::success(format!("Задача создана на доске: {}", id))
    }
}

pub struct BoardUpdateStatusTool(Arc<Mutex<TaskManager>>);
#[async_trait]
impl Tool for BoardUpdateStatusTool {
    fn name(&self) -> &'static str {
        "board_update_status"
    }
    fn description(&self) -> &'static str {
        "Изменить статус задачи на канбан-доске (pending|in_progress|completed|failed|cancelled)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"id": {"type": "string"}, "status": {"type": "string"}},
            "required": ["id", "status"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let id = s(args, "id");
        let status = s(args, "status");
        match (id, status) {
            (Some(id), Some(status_str)) => {
                let Some(status) = parse_status(&status_str) else {
                    return ToolResult::error(format!("Неизвестный статус: {}", status_str));
                };
                let mut mgr = self.0.lock().unwrap();
                if mgr.update_status(&id, status) {
                    ToolResult::success(format!("Статус задачи {} обновлён", id))
                } else {
                    ToolResult::error(format!("Задача не найдена: {}", id))
                }
            }
            _ => ToolResult::error("нужны параметры 'id' и 'status'"),
        }
    }
}

pub struct BoardListTool(Arc<Mutex<TaskManager>>);
#[async_trait]
impl Tool for BoardListTool {
    fn name(&self) -> &'static str {
        "board_list"
    }
    fn description(&self) -> &'static str {
        "Показать канбан-доску целиком (все задачи, статусы, приоритеты)."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        let mgr = self.0.lock().unwrap();
        ToolResult::success(mgr.get_stats().format())
    }
}

pub struct BoardStatsTool(Arc<Mutex<TaskManager>>);
#[async_trait]
impl Tool for BoardStatsTool {
    fn name(&self) -> &'static str {
        "board_stats"
    }
    fn description(&self) -> &'static str {
        "Статистика канбан-доски (сколько задач в каждом статусе, прогресс в %)."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        let mgr = self.0.lock().unwrap();
        ToolResult::success(mgr.get_stats().format())
    }
}

pub struct BoardDeleteTool(Arc<Mutex<TaskManager>>);
#[async_trait]
impl Tool for BoardDeleteTool {
    fn name(&self) -> &'static str {
        "board_delete"
    }
    fn description(&self) -> &'static str {
        "Удалить задачу с канбан-доски по id."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "id") {
            Some(id) => {
                let mut mgr = self.0.lock().unwrap();
                if mgr.delete_task(&id) {
                    ToolResult::success(format!("Задача удалена: {}", id))
                } else {
                    ToolResult::error(format!("Задача не найдена: {}", id))
                }
            }
            None => ToolResult::error("нужен параметр 'id'"),
        }
    }
}

pub struct BoardSearchTool(Arc<Mutex<TaskManager>>);
#[async_trait]
impl Tool for BoardSearchTool {
    fn name(&self) -> &'static str {
        "board_search"
    }
    fn description(&self) -> &'static str {
        "Найти задачи на доске по подстроке в названии/описании."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "query") {
            Some(q) => {
                let mgr = self.0.lock().unwrap();
                let results = mgr.search_tasks(&q);
                if results.is_empty() {
                    ToolResult::success("Ничего не найдено".to_string())
                } else {
                    let mut out = format!("Найдено {} задач:\n", results.len());
                    for t in results {
                        out.push_str(&format!("  [{}] {} — {}\n", t.id, t.title, t.status));
                    }
                    ToolResult::success(out)
                }
            }
            None => ToolResult::error("нужен параметр 'query'"),
        }
    }
}

// =========================================================== learning =

pub struct LearningStatsTool;
#[async_trait]
impl Tool for LearningStatsTool {
    fn name(&self) -> &'static str {
        "learning_stats"
    }
    fn description(&self) -> &'static str {
        "Статистика накопленного опыта агента: сколько успешных/неудачных вызовов инструментов, предпочтений."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        let learning = crate::learning::ContextLearning::new();
        ToolResult::success(learning.get_stats())
    }
}

pub struct LearningHintTool;
#[async_trait]
impl Tool for LearningHintTool {
    fn name(&self) -> &'static str {
        "learning_hint"
    }
    fn description(&self) -> &'static str {
        "Подсказки на основе накопленного опыта: какие инструменты часто ошибаются, известные предпочтения пользователя."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        let learning = crate::learning::ContextLearning::new();
        let hint = learning.get_context_hint();
        if hint.is_empty() {
            ToolResult::success("Пока недостаточно данных для подсказок.".to_string())
        } else {
            ToolResult::success(hint)
        }
    }
}

pub struct LearningSetPreferenceTool;
#[async_trait]
impl Tool for LearningSetPreferenceTool {
    fn name(&self) -> &'static str {
        "learning_set_preference"
    }
    fn description(&self) -> &'static str {
        "Запомнить предпочтение пользователя (напр. 'code_style: без комментариев') — агент будет учитывать его дальше."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"key": {"type": "string"}, "value": {"type": "string"}},
            "required": ["key", "value"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let key = s(args, "key");
        let value = s(args, "value");
        match (key, value) {
            (Some(k), Some(v)) => {
                let mut learning = crate::learning::ContextLearning::new();
                learning.set_preference(&k, &v);
                ToolResult::success(format!("Предпочтение сохранено: {} = {}", k, v))
            }
            _ => ToolResult::error("нужны параметры 'key' и 'value'"),
        }
    }
}

// ==================================================== регистрация =====

pub fn create_extra_tools3() -> Vec<(&'static str, Arc<dyn Tool>)> {
    let cache = Arc::new(Mutex::new(Cache::new(CacheConfig::default())));
    let board = Arc::new(Mutex::new(TaskManager::new()));

    vec![
        ("cache_set", Arc::new(CacheSetTool(cache.clone()))),
        ("cache_get", Arc::new(CacheGetTool(cache.clone()))),
        ("cache_delete", Arc::new(CacheDeleteTool(cache.clone()))),
        ("cache_stats", Arc::new(CacheStatsTool(cache.clone()))),
        ("cache_clear", Arc::new(CacheClearTool(cache))),
        ("board_create", Arc::new(BoardCreateTool(board.clone()))),
        ("board_update_status", Arc::new(BoardUpdateStatusTool(board.clone()))),
        ("board_list", Arc::new(BoardListTool(board.clone()))),
        ("board_stats", Arc::new(BoardStatsTool(board.clone()))),
        ("board_delete", Arc::new(BoardDeleteTool(board.clone()))),
        ("board_search", Arc::new(BoardSearchTool(board))),
        ("learning_stats", Arc::new(LearningStatsTool)),
        ("learning_hint", Arc::new(LearningHintTool)),
        ("learning_set_preference", Arc::new(LearningSetPreferenceTool)),
    ]
}
