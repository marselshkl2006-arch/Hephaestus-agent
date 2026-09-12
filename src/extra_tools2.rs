//! Итерация 6: `Tool`-обёртки над модулями, написанными в этой же
//! итерации (`vector_search`, `repomap`, `model_detector`, `monitoring`,
//! `package_tools`, `project_analyzer`). Тот же принцип, что
//! `extra_tools.rs` в итерации 5 — маппинг `Value` аргументов на вызов
//! функции в исходном модуле, без переноса логики сюда.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::tools::{Tool, ToolResult};
use crate::vector_search::VectorStore;

fn s(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}
fn s_or(args: &Value, key: &str, default: &str) -> String {
    s(args, key).unwrap_or_else(|| default.to_string())
}

// ===================================================== vector_search ==

pub struct VectorAddTool(Arc<VectorStore>);
#[async_trait]
impl Tool for VectorAddTool {
    fn name(&self) -> &'static str {
        "vector_add"
    }
    fn description(&self) -> &'static str {
        "Добавить документ в векторное хранилище для семантического поиска (TF-IDF, без внешних моделей)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "doc_id": {"type": "string"},
                "content": {"type": "string"},
                "metadata": {"type": "object"}
            },
            "required": ["doc_id", "content"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let doc_id = s(args, "doc_id");
        let content = s(args, "content");
        match (doc_id, content) {
            (Some(id), Some(c)) => {
                let metadata = args
                    .get("metadata")
                    .and_then(|v| v.as_object())
                    .map(|o| o.clone().into_iter().collect());
                self.0.add_document(id, c, metadata)
            }
            _ => ToolResult::error("нужны параметры 'doc_id' и 'content'"),
        }
    }
}

pub struct VectorSearchTool(Arc<VectorStore>);
#[async_trait]
impl Tool for VectorSearchTool {
    fn name(&self) -> &'static str {
        "vector_search"
    }
    fn description(&self) -> &'static str {
        "Семантический поиск похожих документов в векторном хранилище (см. vector_add)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "top_k": {"type": "integer", "description": "По умолчанию 5"},
                "threshold": {"type": "number", "description": "Минимальное сходство 0..1, по умолчанию 0"}
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "query") {
            Some(q) => {
                let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                let threshold = args.get("threshold").and_then(|v| v.as_f64()).unwrap_or(0.0);
                self.0.search(&q, top_k, threshold)
            }
            None => ToolResult::error("нужен параметр 'query'"),
        }
    }
}

pub struct VectorDeleteTool(Arc<VectorStore>);
#[async_trait]
impl Tool for VectorDeleteTool {
    fn name(&self) -> &'static str {
        "vector_delete"
    }
    fn description(&self) -> &'static str {
        "Удалить документ из векторного хранилища по id."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"doc_id": {"type": "string"}}, "required": ["doc_id"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "doc_id") {
            Some(id) => self.0.delete_document(&id),
            None => ToolResult::error("нужен параметр 'doc_id'"),
        }
    }
}

pub struct VectorListTool(Arc<VectorStore>);
#[async_trait]
impl Tool for VectorListTool {
    fn name(&self) -> &'static str {
        "vector_list"
    }
    fn description(&self) -> &'static str {
        "Список всех документов в векторном хранилище."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        self.0.list_documents()
    }
}

pub struct VectorGetTool(Arc<VectorStore>);
#[async_trait]
impl Tool for VectorGetTool {
    fn name(&self) -> &'static str {
        "vector_get"
    }
    fn description(&self) -> &'static str {
        "Получить документ из векторного хранилища по id."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"doc_id": {"type": "string"}}, "required": ["doc_id"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "doc_id") {
            Some(id) => self.0.get_document(&id),
            None => ToolResult::error("нужен параметр 'doc_id'"),
        }
    }
}

pub struct VectorUpdateTool(Arc<VectorStore>);
#[async_trait]
impl Tool for VectorUpdateTool {
    fn name(&self) -> &'static str {
        "vector_update"
    }
    fn description(&self) -> &'static str {
        "Обновить содержимое и/или метаданные документа в векторном хранилище."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"doc_id": {"type": "string"}, "content": {"type": "string"}, "metadata": {"type": "object"}},
            "required": ["doc_id"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "doc_id") {
            Some(id) => {
                let content = s(args, "content");
                let metadata = args
                    .get("metadata")
                    .and_then(|v| v.as_object())
                    .map(|o| o.clone().into_iter().collect());
                self.0.update_document(&id, content, metadata)
            }
            None => ToolResult::error("нужен параметр 'doc_id'"),
        }
    }
}

pub struct VectorClearTool(Arc<VectorStore>);
#[async_trait]
impl Tool for VectorClearTool {
    fn name(&self) -> &'static str {
        "vector_clear"
    }
    fn description(&self) -> &'static str {
        "Очистить всё векторное хранилище (необратимо)."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        self.0.clear()
    }
}

// ============================================================ repomap =

pub struct RepomapScanTool(crate::workdir::WorkDir);
#[async_trait]
impl Tool for RepomapScanTool {
    fn name(&self) -> &'static str {
        "repomap_scan"
    }
    fn description(&self) -> &'static str {
        "Построить карту репозитория: классы и функции по всем исходникам проекта (как в Aider)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "max_files": {"type": "integer", "description": "По умолчанию 50"},
                "save_to": {"type": "string"}
            }
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let max_files = args.get("max_files").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
        crate::repomap::scan(&self.0.get(), max_files, s(args, "save_to").as_deref())
    }
}

pub struct RepomapFileSummaryTool;
#[async_trait]
impl Tool for RepomapFileSummaryTool {
    fn name(&self) -> &'static str {
        "repomap_file_summary"
    }
    fn description(&self) -> &'static str {
        "Краткое содержание одного файла: список классов и функций."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "file_path") {
            Some(p) => crate::repomap::file_summary(&p),
            None => ToolResult::error("нужен параметр 'file_path'"),
        }
    }
}

// ===================================================== model_detector =

pub struct ListModelsTool;
#[async_trait]
impl Tool for ListModelsTool {
    fn name(&self) -> &'static str {
        "list_available_models"
    }
    fn description(&self) -> &'static str {
        "Показать все обнаруженные локальные и облачные LLM (Ollama, KoboldCPP, Anthropic, OpenAI, OpenRouter) с рекомендацией."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        ToolResult::success(crate::model_detector::describe_available_models().await)
    }
}

// ======================================================== monitoring ==

pub struct MonitoringDashboardTool(Arc<crate::monitoring::MonitoringSystem>);
#[async_trait]
impl Tool for MonitoringDashboardTool {
    fn name(&self) -> &'static str {
        "monitoring_dashboard"
    }
    fn description(&self) -> &'static str {
        "Показать дашборд метрик агента (LLM токены, время инструментов, активные алерты)."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        self.0.get_dashboard()
    }
}

pub struct MonitoringHealthTool(Arc<crate::monitoring::MonitoringSystem>);
#[async_trait]
impl Tool for MonitoringHealthTool {
    fn name(&self) -> &'static str {
        "monitoring_health"
    }
    fn description(&self) -> &'static str {
        "Проверить общее состояние здоровья агента по активным алертам."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        self.0.check_health()
    }
}

// ====================================================== package_tools =

pub struct PackageInstallTool2(crate::workdir::WorkDir);
#[async_trait]
impl Tool for PackageInstallTool2 {
    fn name(&self) -> &'static str {
        "package_install"
    }
    fn description(&self) -> &'static str {
        "Установить пакет через pip/npm/yarn/apt/cargo/go (manager='auto' — определить по файлам проекта)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "package": {"type": "string"},
                "manager": {"type": "string", "description": "auto|pip|npm|yarn|apt|cargo|go, по умолчанию auto"},
                "dev": {"type": "boolean"},
                "global_install": {"type": "boolean"}
            },
            "required": ["package"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "package") {
            Some(pkg) => {
                let manager = s_or(args, "manager", "auto");
                let dev = args.get("dev").and_then(|v| v.as_bool()).unwrap_or(false);
                let global_install = args.get("global_install").and_then(|v| v.as_bool()).unwrap_or(false);
                crate::package_tools::package_install(&self.0.get(), &pkg, &manager, dev, global_install).await
            }
            None => ToolResult::error("нужен параметр 'package'"),
        }
    }
}

pub struct PackageSearchTool2;
#[async_trait]
impl Tool for PackageSearchTool2 {
    fn name(&self) -> &'static str {
        "package_search"
    }
    fn description(&self) -> &'static str {
        "Найти пакет в PyPI (manager='pip') или npm (manager='npm')."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}, "manager": {"type": "string", "description": "pip|npm, по умолчанию pip"}},
            "required": ["query"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "query") {
            Some(q) => crate::package_tools::package_search(&q, &s_or(args, "manager", "pip")).await,
            None => ToolResult::error("нужен параметр 'query'"),
        }
    }
}

// ==================================================== project_analyzer =

pub struct ProjectAnalyzeTool;
#[async_trait]
impl Tool for ProjectAnalyzeTool {
    fn name(&self) -> &'static str {
        "project_analyze"
    }
    fn description(&self) -> &'static str {
        "Определить язык, фреймворк, менеджер пакетов и тест-раннер проекта по файлам-маркерам."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"path": {"type": "string", "description": "По умолчанию '.'"}}})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        crate::project_analyzer::analyze(&s_or(args, "path", "."))
    }
}

// ==================================================== регистрация =====

pub fn create_extra_tools2(
    workdir: crate::workdir::WorkDir,
    monitoring: Arc<crate::monitoring::MonitoringSystem>,
) -> Vec<(&'static str, Arc<dyn Tool>)> {
    let vs = Arc::new(VectorStore::new());

    vec![
        ("vector_add", Arc::new(VectorAddTool(vs.clone()))),
        ("vector_search", Arc::new(VectorSearchTool(vs.clone()))),
        ("vector_delete", Arc::new(VectorDeleteTool(vs.clone()))),
        ("vector_list", Arc::new(VectorListTool(vs.clone()))),
        ("vector_get", Arc::new(VectorGetTool(vs.clone()))),
        ("vector_update", Arc::new(VectorUpdateTool(vs.clone()))),
        ("vector_clear", Arc::new(VectorClearTool(vs))),
        ("repomap_scan", Arc::new(RepomapScanTool(workdir.clone()))),
        ("repomap_file_summary", Arc::new(RepomapFileSummaryTool)),
        ("list_available_models", Arc::new(ListModelsTool)),
        ("monitoring_dashboard", Arc::new(MonitoringDashboardTool(monitoring.clone()))),
        ("monitoring_health", Arc::new(MonitoringHealthTool(monitoring))),
        ("package_install", Arc::new(PackageInstallTool2(workdir))),
        ("package_search", Arc::new(PackageSearchTool2)),
        ("project_analyze", Arc::new(ProjectAnalyzeTool)),
    ]
}
