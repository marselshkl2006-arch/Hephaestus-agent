//! Итерация 5 миграции: `Tool`-обёртки над модулями, которые существовали
//! в `src/*.rs` (написаны в предыдущих итерациях), но были объявлены
//! только как голые функции/структуры без реализации `crate::tools::Tool`
//! и без строчки `mod ...;` в `main.rs` — то есть были мёртвым кодом,
//! невидимым ни для сборки, ни тем более для LLM. См. STATUS_RU.md.
//!
//! Ничего не переписано с нуля: вся логика (сборка команд `docker`/`gh`,
//! SQL, HTTP-запросы, разбор .ipynb) остаётся в исходных модулях
//! (`crate::cron_tools`, `crate::database_tools`, ...) — здесь только
//! маппинг `serde_json::Value` аргументов -> вызов существующей функции,
//! чтобы LLM могла эти инструменты увидеть и вызвать через `ToolRegistry`.
//!
//! Как и весь остальной Rust-код в этом дереве — написано и вычитано
//! вручную, но НИ РАЗУ не прогнано через `cargo build` (в песочнице нет
//! тулчейна и сети). Возможны ошибки на стыках типов, которые обычный
//! `cargo build` покажет за секунды — см. итоговое сообщение с планом
//! первого прогона.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::tools::{Tool, ToolResult};

use crate::cron_tools::{self, Scheduler};
use crate::database_tools;
use crate::docker_tools;
use crate::github_tools::{GithubActionsTool, GithubResult};
use crate::http_tools::HttpTools;
use crate::notebook_tools;
use crate::ocr_tool;
use crate::task_tools::{self, TaskManager};

fn gh(result: GithubResult) -> ToolResult {
    if result.success {
        ToolResult::success(result.output)
    } else {
        ToolResult::error(result.error.unwrap_or(result.output))
    }
}

fn s(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn s_or(args: &Value, key: &str, default: &str) -> String {
    s(args, key).unwrap_or_else(|| default.to_string())
}

// ============================================================ cron ====

pub struct CronCreateTool(Arc<Scheduler>);
#[async_trait]
impl Tool for CronCreateTool {
    fn name(&self) -> &'static str {
        "cron_create"
    }
    fn description(&self) -> &'static str {
        "Запланировать периодическую задачу для агента по cron-выражению (5 или 6 полей)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cron": {"type": "string", "description": "Cron-выражение, напр. '0 9 * * *'"},
                "prompt": {"type": "string", "description": "Что должен сделать агент при срабатывании"},
                "description": {"type": "string", "description": "Человеко-читаемое описание задачи"}
            },
            "required": ["cron", "prompt"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let cron = match s(args, "cron") {
            Some(v) => v,
            None => return ToolResult::error("нужен параметр 'cron'"),
        };
        let prompt = match s(args, "prompt") {
            Some(v) => v,
            None => return ToolResult::error("нужен параметр 'prompt'"),
        };
        let description = s(args, "description");
        cron_tools::cron_create(&self.0, &cron, &prompt, description.as_deref()).await
    }
}

pub struct CronListTool(Arc<Scheduler>);
#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &'static str {
        "cron_list"
    }
    fn description(&self) -> &'static str {
        "Показать список запланированных cron-задач агента."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        cron_tools::cron_list(&self.0).await
    }
}

pub struct CronDeleteTool(Arc<Scheduler>);
#[async_trait]
impl Tool for CronDeleteTool {
    fn name(&self) -> &'static str {
        "cron_delete"
    }
    fn description(&self) -> &'static str {
        "Удалить запланированную cron-задачу по её id (см. cron_list)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"task_id": {"type": "string"}},
            "required": ["task_id"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "task_id") {
            Some(id) => cron_tools::cron_delete(&self.0, &id).await,
            None => ToolResult::error("нужен параметр 'task_id'"),
        }
    }
}

// ======================================================== database ====

pub struct DbQueryTool;
#[async_trait]
impl Tool for DbQueryTool {
    fn name(&self) -> &'static str {
        "db_query"
    }
    fn description(&self) -> &'static str {
        "Выполнить произвольный SQL-запрос к SQLite базе (SELECT возвращает таблицу текстом)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "db_path": {"type": "string", "description": "Путь к .db файлу (по умолчанию ~/.hephaestus/hephaestus.db)"},
                "sql": {"type": "string"}
            },
            "required": ["sql"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let db_path = s_or(args, "db_path", "");
        let sql = match s(args, "sql") {
            Some(sql) => sql,
            None => return ToolResult::error("нужен параметр 'sql'"),
        };
        
        tokio::task::spawn_blocking(move || {
            // Используем tokio runtime внутри блокирующего потока
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                database_tools::db_query(&db_path, &sql).await
            })
        })
        .await
        .unwrap_or_else(|_| ToolResult::error("Ошибка выполнения запроса в блокирующем потоке".to_string()))
    }
}

pub struct DbListTablesTool;
#[async_trait]
impl Tool for DbListTablesTool {
    fn name(&self) -> &'static str {
        "db_list_tables"
    }
    fn description(&self) -> &'static str {
        "Список таблиц в SQLite базе."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"db_path": {"type": "string"}}})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let db_path = s_or(args, "db_path", "");
        
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                database_tools::db_list_tables(&db_path).await
            })
        })
        .await
        .unwrap_or_else(|_| ToolResult::error("Ошибка выполнения запроса в блокирующем потоке".to_string()))
    }
}

pub struct DbTableInfoTool;
#[async_trait]
impl Tool for DbTableInfoTool {
    fn name(&self) -> &'static str {
        "db_table_info"
    }
    fn description(&self) -> &'static str {
        "Схема таблицы SQLite (PRAGMA table_info)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"db_path": {"type": "string"}, "table_name": {"type": "string"}},
            "required": ["table_name"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let db_path = s_or(args, "db_path", "");
        let table_name = match s(args, "table_name") {
            Some(t) => t,
            None => return ToolResult::error("нужен параметр 'table_name'"),
        };
        
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                database_tools::db_table_info(&db_path, &table_name).await
            })
        })
        .await
        .unwrap_or_else(|_| ToolResult::error("Ошибка выполнения запроса в блокирующем потоке".to_string()))
    }
}

pub struct DbCreateTableTool;
#[async_trait]
impl Tool for DbCreateTableTool {
    fn name(&self) -> &'static str {
        "db_create_table"
    }
    fn description(&self) -> &'static str {
        "Создать таблицу SQLite. `columns` — объект {имя_колонки: SQL-тип}."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "db_path": {"type": "string"},
                "table_name": {"type": "string"},
                "columns": {"type": "object", "description": "{\"id\": \"INTEGER PRIMARY KEY\", \"name\": \"TEXT\"}"}
            },
            "required": ["table_name", "columns"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let db_path = s_or(args, "db_path", "");
        let table_name = match s(args, "table_name") {
            Some(v) => v,
            None => return ToolResult::error("нужен параметр 'table_name'"),
        };
        let cols_obj = match args.get("columns").and_then(|v| v.as_object()) {
            Some(o) => o,
            None => return ToolResult::error("'columns' должен быть объектом {имя: тип}"),
        };
        let cols_owned: Vec<(String, String)> = cols_obj
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("TEXT").to_string()))
            .collect();
        
        tokio::task::spawn_blocking(move || {
            let cols_refs: Vec<(&str, &str)> =
                cols_owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                database_tools::db_create_table(&db_path, &table_name, &cols_refs).await
            })
        })
        .await
        .unwrap_or_else(|_| ToolResult::error("Ошибка выполнения запроса в блокирующем потоке".to_string()))
    }
}

pub struct DbInsertTool;
#[async_trait]
impl Tool for DbInsertTool {
    fn name(&self) -> &'static str {
        "db_insert"
    }
    fn description(&self) -> &'static str {
        "Вставить строку в таблицу SQLite. `columns` и `values` — параллельные массивы строк."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "db_path": {"type": "string"},
                "table_name": {"type": "string"},
                "columns": {"type": "array", "items": {"type": "string"}},
                "values": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["table_name", "columns", "values"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let db_path = s_or(args, "db_path", "");
        let table_name = match s(args, "table_name") {
            Some(v) => v,
            None => return ToolResult::error("нужен параметр 'table_name'"),
        };
        let columns: Vec<String> = match args.get("columns").and_then(|v| v.as_array()) {
            Some(a) => a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect(),
            None => return ToolResult::error("'columns' должен быть массивом строк"),
        };
        let values: Vec<String> = match args.get("values").and_then(|v| v.as_array()) {
            Some(a) => a
                .iter()
                .map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string()))
                .collect(),
            None => return ToolResult::error("'values' должен быть массивом"),
        };
        tokio::task::spawn_blocking(move || {
            let col_refs: Vec<&str> = columns.iter().map(|s| s.as_str()).collect();
            let val_refs: Vec<&str> = values.iter().map(|s| s.as_str()).collect();
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                database_tools::db_insert(&db_path, &table_name, &col_refs, &val_refs).await
            })
        })
        .await
        .unwrap_or_else(|_| ToolResult::error("Ошибка выполнения запроса в блокирующем потоке".to_string()))
    }
}

// ========================================================== docker ====

pub struct DockerListTool;
#[async_trait]
impl Tool for DockerListTool {
    fn name(&self) -> &'static str {
        "docker_list"
    }
    fn description(&self) -> &'static str {
        "Список Docker-контейнеров. all=true — включая остановленные."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"all": {"type": "boolean"}}})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        docker_tools::docker_list(args.get("all").and_then(|v| v.as_bool()).unwrap_or(false)).await
    }
}

pub struct DockerImagesTool;
#[async_trait]
impl Tool for DockerImagesTool {
    fn name(&self) -> &'static str {
        "docker_images"
    }
    fn description(&self) -> &'static str {
        "Список локальных Docker-образов."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        docker_tools::docker_images().await
    }
}

pub struct DockerRunTool;
#[async_trait]
impl Tool for DockerRunTool {
    fn name(&self) -> &'static str {
        "docker_run"
    }
    fn description(&self) -> &'static str {
        "Запустить Docker-контейнер. ports/env/volumes — строки через запятую \
         ('8080:80,9090:9090')."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "image": {"type": "string"},
                "name": {"type": "string"},
                "ports": {"type": "string"},
                "env": {"type": "string"},
                "volumes": {"type": "string"},
                "detach": {"type": "boolean", "description": "По умолчанию true"},
                "command": {"type": "string"}
            },
            "required": ["image"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let image = match s(args, "image") {
            Some(v) => v,
            None => return ToolResult::error("нужен параметр 'image'"),
        };
        docker_tools::docker_run(
            &image,
            s(args, "name").as_deref(),
            s(args, "ports").as_deref(),
            s(args, "env").as_deref(),
            s(args, "volumes").as_deref(),
            args.get("detach").and_then(|v| v.as_bool()).unwrap_or(true),
            s(args, "command").as_deref(),
        )
        .await
    }
}

pub struct DockerStopTool;
#[async_trait]
impl Tool for DockerStopTool {
    fn name(&self) -> &'static str {
        "docker_stop"
    }
    fn description(&self) -> &'static str {
        "Остановить Docker-контейнер по имени или id."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"container": {"type": "string"}}, "required": ["container"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "container") {
            Some(c) => docker_tools::docker_stop(&c).await,
            None => ToolResult::error("нужен параметр 'container'"),
        }
    }
}

pub struct DockerRmTool;
#[async_trait]
impl Tool for DockerRmTool {
    fn name(&self) -> &'static str {
        "docker_rm"
    }
    fn description(&self) -> &'static str {
        "Удалить Docker-контейнер. force=true — удалить даже запущенный."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"container": {"type": "string"}, "force": {"type": "boolean"}},
            "required": ["container"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "container") {
            Some(c) => docker_tools::docker_rm(&c, args.get("force").and_then(|v| v.as_bool()).unwrap_or(false)).await,
            None => ToolResult::error("нужен параметр 'container'"),
        }
    }
}

pub struct DockerLogsTool;
#[async_trait]
impl Tool for DockerLogsTool {
    fn name(&self) -> &'static str {
        "docker_logs"
    }
    fn description(&self) -> &'static str {
        "Логи Docker-контейнера (последние N строк, по умолчанию 100)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"container": {"type": "string"}, "tail": {"type": "integer"}},
            "required": ["container"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "container") {
            Some(c) => {
                let tail = args.get("tail").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
                docker_tools::docker_logs(&c, tail).await
            }
            None => ToolResult::error("нужен параметр 'container'"),
        }
    }
}

pub struct DockerExecTool;
#[async_trait]
impl Tool for DockerExecTool {
    fn name(&self) -> &'static str {
        "docker_exec"
    }
    fn description(&self) -> &'static str {
        "Выполнить команду внутри работающего Docker-контейнера."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"container": {"type": "string"}, "command": {"type": "string"}},
            "required": ["container", "command"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let container = s(args, "container");
        let command = s(args, "command");
        match (container, command) {
            (Some(c), Some(cmd)) => docker_tools::docker_exec(&c, &cmd).await,
            _ => ToolResult::error("нужны параметры 'container' и 'command'"),
        }
    }
}

pub struct DockerInspectTool;
#[async_trait]
impl Tool for DockerInspectTool {
    fn name(&self) -> &'static str {
        "docker_inspect"
    }
    fn description(&self) -> &'static str {
        "Подробная информация о Docker-контейнере (docker inspect)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"container": {"type": "string"}}, "required": ["container"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "container") {
            Some(c) => docker_tools::docker_inspect(&c).await,
            None => ToolResult::error("нужен параметр 'container'"),
        }
    }
}

// ========================================================== github ====

fn gh_tool(args: &Value) -> GithubActionsTool {
    let repo_path = s(args, "repo_path");
    let token = s(args, "token").or_else(|| std::env::var("GITHUB_TOKEN").ok());
    GithubActionsTool::new(repo_path.as_deref(), token.as_deref())
}

pub struct GithubRepoInfoTool;
#[async_trait]
impl Tool for GithubRepoInfoTool {
    fn name(&self) -> &'static str {
        "github_repo_info"
    }
    fn description(&self) -> &'static str {
        "Информация о git-репозитории: remote, текущая ветка, последние коммиты."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"repo_path": {"type": "string", "description": "По умолчанию текущая директория"}}})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        gh(gh_tool(args).get_repo_info())
    }
}

pub struct GithubWorkflowRunTool;
#[async_trait]
impl Tool for GithubWorkflowRunTool {
    fn name(&self) -> &'static str {
        "github_workflow_run"
    }
    fn description(&self) -> &'static str {
        "Запустить GitHub Actions workflow через `gh workflow run` (нужен установленный `gh`)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "workflow_file": {"type": "string"},
                "repo_path": {"type": "string"},
                "token": {"type": "string"},
                "inputs": {"type": "object"}
            },
            "required": ["workflow_file"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "workflow_file") {
            Some(wf) => gh(gh_tool(args).run_workflow(&wf, args.get("inputs").cloned())),
            None => ToolResult::error("нужен параметр 'workflow_file'"),
        }
    }
}

pub struct GithubWorkflowStatusTool;
#[async_trait]
impl Tool for GithubWorkflowStatusTool {
    fn name(&self) -> &'static str {
        "github_workflow_status"
    }
    fn description(&self) -> &'static str {
        "Статус GitHub Actions workflow (`gh workflow list`, отфильтровано по имени файла)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"workflow_file": {"type": "string"}, "repo_path": {"type": "string"}, "token": {"type": "string"}},
            "required": ["workflow_file"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "workflow_file") {
            Some(wf) => gh(gh_tool(args).get_workflow_status(&wf)),
            None => ToolResult::error("нужен параметр 'workflow_file'"),
        }
    }
}

pub struct GithubPrCreateTool;
#[async_trait]
impl Tool for GithubPrCreateTool {
    fn name(&self) -> &'static str {
        "github_pr_create"
    }
    fn description(&self) -> &'static str {
        "Создать pull request через `gh pr create`."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"}, "body": {"type": "string"},
                "base": {"type": "string"}, "head": {"type": "string"},
                "repo_path": {"type": "string"}, "token": {"type": "string"}
            },
            "required": ["title", "body"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let title = s(args, "title");
        let body = s(args, "body");
        match (title, body) {
            (Some(t), Some(b)) => gh(gh_tool(args).create_pr(&t, &b, s(args, "base").as_deref(), s(args, "head").as_deref())),
            _ => ToolResult::error("нужны параметры 'title' и 'body'"),
        }
    }
}

pub struct GithubPrListTool;
#[async_trait]
impl Tool for GithubPrListTool {
    fn name(&self) -> &'static str {
        "github_pr_list"
    }
    fn description(&self) -> &'static str {
        "Список pull request-ов репозитория (`gh pr list`)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"state": {"type": "string", "description": "open|closed|merged|all"}, "repo_path": {"type": "string"}, "token": {"type": "string"}}
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        gh(gh_tool(args).list_prs(s(args, "state").as_deref()))
    }
}

pub struct GithubCiStatusTool;
#[async_trait]
impl Tool for GithubCiStatusTool {
    fn name(&self) -> &'static str {
        "github_ci_status"
    }
    fn description(&self) -> &'static str {
        "Статус последних 10 запусков CI/CD (`gh run list`)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"repo_path": {"type": "string"}, "token": {"type": "string"}}})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        gh(gh_tool(args).get_ci_status())
    }
}

// ============================================================ http ====

pub struct HttpRequestTool(HttpTools);
impl HttpRequestTool {
    pub fn new() -> Self {
        Self(HttpTools::new())
    }
}
#[async_trait]
impl Tool for HttpRequestTool {
    fn name(&self) -> &'static str {
        "http_request"
    }
    fn description(&self) -> &'static str {
        "HTTP-запрос (GET/POST/PUT/DELETE/HEAD). 'json' — тело как JSON (для POST), \
         'body' — сырое тело (для PUT), 'headers' — объект строка->строка."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "method": {"type": "string", "description": "GET (по умолчанию) | POST | PUT | DELETE | HEAD"},
                "url": {"type": "string"},
                "headers": {"type": "object"},
                "json": {"description": "Тело запроса как JSON (для POST)"},
                "body": {"type": "string", "description": "Сырое тело запроса (для PUT)"}
            },
            "required": ["url"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let url = match s(args, "url") {
            Some(v) => v,
            None => return ToolResult::error("нужен параметр 'url'"),
        };
        let method = s_or(args, "method", "GET").to_uppercase();
        let headers = args.get("headers").and_then(|v| v.as_object()).map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<std::collections::HashMap<_, _>>()
        });

        let result = match method.as_str() {
            "GET" => self.0.get(&url, headers).await,
            "POST" => {
                if let Some(json) = args.get("json") {
                    self.0.post_json(&url, json, headers).await
                } else if let Some(form) = args.get("form").and_then(|v| v.as_object()) {
                    let form_map: std::collections::HashMap<String, String> = form
                        .iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect();
                    self.0.post_form(&url, &form_map, headers).await
                } else {
                    self.0.post_json(&url, &serde_json::json!({}), headers).await
                }
            }
            "PUT" => self.0.put(&url, s(args, "body").as_deref(), headers).await,
            "DELETE" => self.0.delete(&url, headers).await,
            "HEAD" => self.0.head(&url, headers).await,
            other => return ToolResult::error(format!("Неподдерживаемый метод: {}", other)),
        };

        if result.success {
            ToolResult::success(format!("HTTP {}\n\n{}", result.status_code.unwrap_or(0), result.body))
        } else {
            ToolResult::error(result.error.unwrap_or_else(|| "неизвестная ошибка запроса".to_string()))
        }
    }
}

pub struct HttpDownloadTool(HttpTools);
impl HttpDownloadTool {
    pub fn new() -> Self {
        Self(HttpTools::new())
    }
}
#[async_trait]
impl Tool for HttpDownloadTool {
    fn name(&self) -> &'static str {
        "http_download"
    }
    fn description(&self) -> &'static str {
        "Скачать файл по URL и сохранить на диск."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"url": {"type": "string"}, "save_path": {"type": "string"}},
            "required": ["url", "save_path"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let url = s(args, "url");
        let path = s(args, "save_path");
        match (url, path) {
            (Some(u), Some(p)) => {
                let r = self.0.download(&u, &p).await;
                if r.success {
                    ToolResult::success(r.body)
                } else {
                    ToolResult::error(r.error.unwrap_or_default())
                }
            }
            _ => ToolResult::error("нужны параметры 'url' и 'save_path'"),
        }
    }
}

// ======================================================== notebook ====

pub struct NotebookReadTool;
#[async_trait]
impl Tool for NotebookReadTool {
    fn name(&self) -> &'static str {
        "notebook_read"
    }
    fn description(&self) -> &'static str {
        "Прочитать Jupyter notebook (.ipynb) — список ячеек с превью содержимого."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "file_path") {
            Some(p) => notebook_tools::notebook_read(&p).await,
            None => ToolResult::error("нужен параметр 'file_path'"),
        }
    }
}

pub struct NotebookEditTool;
#[async_trait]
impl Tool for NotebookEditTool {
    fn name(&self) -> &'static str {
        "notebook_edit"
    }
    fn description(&self) -> &'static str {
        "Заменить содержимое одной ячейки Jupyter notebook по индексу (с нуля)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "cell_index": {"type": "integer"},
                "new_content": {"type": "string"}
            },
            "required": ["file_path", "cell_index", "new_content"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let file_path = s(args, "file_path");
        let cell_index = args.get("cell_index").and_then(|v| v.as_u64());
        let new_content = s(args, "new_content");
        match (file_path, cell_index, new_content) {
            (Some(p), Some(i), Some(c)) => notebook_tools::notebook_edit(&p, i as usize, &c).await,
            _ => ToolResult::error("нужны параметры 'file_path', 'cell_index', 'new_content'"),
        }
    }
}

pub struct NotebookCreateTool;
#[async_trait]
impl Tool for NotebookCreateTool {
    fn name(&self) -> &'static str {
        "notebook_create"
    }
    fn description(&self) -> &'static str {
        "Создать новый пустой Jupyter notebook (.ipynb) с одной code-ячейкой."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "file_path") {
            Some(p) => notebook_tools::notebook_create(&p).await,
            None => ToolResult::error("нужен параметр 'file_path'"),
        }
    }
}

// ============================================================= ocr ====

pub struct OcrExtractTool;
#[async_trait]
impl Tool for OcrExtractTool {
    fn name(&self) -> &'static str {
        "ocr_extract"
    }
    fn description(&self) -> &'static str {
        "Распознать текст на изображении через Tesseract OCR (нужен установленный `tesseract`)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "image_path": {"type": "string"},
                "lang": {"type": "string", "description": "Код языка tesseract, по умолчанию 'eng'"},
                "psm": {"type": "integer", "description": "Page segmentation mode, по умолчанию 3"}
            },
            "required": ["image_path"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "image_path") {
            Some(p) => {
                let lang = s_or(args, "lang", "eng");
                let psm = args.get("psm").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
                ocr_tool::ocr_extract(&p, &lang, psm).await
            }
            None => ToolResult::error("нужен параметр 'image_path'"),
        }
    }
}

pub struct OcrLanguagesTool;
#[async_trait]
impl Tool for OcrLanguagesTool {
    fn name(&self) -> &'static str {
        "ocr_languages"
    }
    fn description(&self) -> &'static str {
        "Список языков, установленных для Tesseract OCR."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        ocr_tool::ocr_languages().await
    }
}

pub struct OcrStatusTool;
#[async_trait]
impl Tool for OcrStatusTool {
    fn name(&self) -> &'static str {
        "ocr_status"
    }
    fn description(&self) -> &'static str {
        "Проверить, установлен ли Tesseract и какой версии."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        ocr_tool::ocr_status().await
    }
}

// ======================================================= task_tools ====
// Названия инструментов ("task_add"/"task_info"/...) намеренно не
// пересекаются с memory_tools/goal_mode, чтобы не конфликтовать в реестре.

pub struct TaskAddTool(Arc<TaskManager>);
#[async_trait]
impl Tool for TaskAddTool {
    fn name(&self) -> &'static str {
        "task_add"
    }
    fn description(&self) -> &'static str {
        "Создать новую задачу в трекере задач текущей сессии агента."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subject": {"type": "string"},
                "description": {"type": "string"},
                "active_form": {"type": "string", "description": "Форма 'в процессе', напр. 'Пишу тесты'"}
            },
            "required": ["subject", "description"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        let subject = s(args, "subject");
        let description = s(args, "description");
        match (subject, description) {
            (Some(subj), Some(desc)) => {
                task_tools::TaskCreateTool
                    .execute(&self.0, subj, desc, s(args, "active_form"), None)
                    .await
            }
            _ => ToolResult::error("нужны параметры 'subject' и 'description'"),
        }
    }
}

pub struct TaskInfoTool(Arc<TaskManager>);
#[async_trait]
impl Tool for TaskInfoTool {
    fn name(&self) -> &'static str {
        "task_info"
    }
    fn description(&self) -> &'static str {
        "Показать детали задачи по id (см. task_add/task_list_all)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {"task_id": {"type": "string"}}, "required": ["task_id"]})
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "task_id") {
            Some(id) => task_tools::TaskGetTool.execute(&self.0, id).await,
            None => ToolResult::error("нужен параметр 'task_id'"),
        }
    }
}

pub struct TaskListAllTool(Arc<TaskManager>);
#[async_trait]
impl Tool for TaskListAllTool {
    fn name(&self) -> &'static str {
        "task_list_all"
    }
    fn description(&self) -> &'static str {
        "Список всех задач трекера текущей сессии (до 50)."
    }
    async fn execute(&self, _args: &Value) -> ToolResult {
        task_tools::TaskListTool.execute(&self.0).await
    }
}

pub struct TaskUpdateStatusTool(Arc<TaskManager>);
#[async_trait]
impl Tool for TaskUpdateStatusTool {
    fn name(&self) -> &'static str {
        "task_update_status"
    }
    fn description(&self) -> &'static str {
        "Обновить статус/поля существующей задачи (status: pending|in_progress|completed|deleted)."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "status": {"type": "string"},
                "subject": {"type": "string"},
                "description": {"type": "string"},
                "owner": {"type": "string"}
            },
            "required": ["task_id"]
        })
    }
    async fn execute(&self, args: &Value) -> ToolResult {
        match s(args, "task_id") {
            Some(id) => {
                task_tools::TaskUpdateTool
                    .execute(
                        &self.0,
                        id,
                        s(args, "status"),
                        s(args, "subject"),
                        s(args, "description"),
                        s(args, "active_form"),
                        s(args, "owner"),
                        None,
                        None,
                        None,
                    )
                    .await
            }
            None => ToolResult::error("нужен параметр 'task_id'"),
        }
    }
}

// ==================================================== регистрация =====

/// Собрать все инструменты этого файла в виде `(имя, Arc<dyn Tool>)`.
/// Общие ресурсы (планировщик cron, менеджер задач) создаются один раз
/// здесь и разделяются между соответствующими инструментами через `Arc`.
pub fn create_extra_tools() -> Vec<(&'static str, Arc<dyn Tool>)> {
    let scheduler = Arc::new(cron_tools::create_scheduler(None));
    let task_manager = Arc::new(TaskManager::new());

    vec![
        ("cron_create", Arc::new(CronCreateTool(scheduler.clone()))),
        ("cron_list", Arc::new(CronListTool(scheduler.clone()))),
        ("cron_delete", Arc::new(CronDeleteTool(scheduler))),
        ("docker_list", Arc::new(DockerListTool)),
        ("docker_images", Arc::new(DockerImagesTool)),
        ("docker_run", Arc::new(DockerRunTool)),
        ("docker_stop", Arc::new(DockerStopTool)),
        ("docker_rm", Arc::new(DockerRmTool)),
        ("docker_logs", Arc::new(DockerLogsTool)),
        ("docker_exec", Arc::new(DockerExecTool)),
        ("docker_inspect", Arc::new(DockerInspectTool)),
        ("github_repo_info", Arc::new(GithubRepoInfoTool)),
        ("github_workflow_run", Arc::new(GithubWorkflowRunTool)),
        ("github_workflow_status", Arc::new(GithubWorkflowStatusTool)),
        ("github_pr_create", Arc::new(GithubPrCreateTool)),
        ("github_pr_list", Arc::new(GithubPrListTool)),
        ("github_ci_status", Arc::new(GithubCiStatusTool)),
        ("http_request", Arc::new(HttpRequestTool::new())),
        ("http_download", Arc::new(HttpDownloadTool::new())),
        ("notebook_read", Arc::new(NotebookReadTool)),
        ("notebook_edit", Arc::new(NotebookEditTool)),
        ("notebook_create", Arc::new(NotebookCreateTool)),
        ("ocr_extract", Arc::new(OcrExtractTool)),
        ("ocr_languages", Arc::new(OcrLanguagesTool)),
        ("ocr_status", Arc::new(OcrStatusTool)),
        ("task_add", Arc::new(TaskAddTool(task_manager.clone()))),
        ("task_info", Arc::new(TaskInfoTool(task_manager.clone()))),
        ("task_list_all", Arc::new(TaskListAllTool(task_manager.clone()))),
        ("task_update_status", Arc::new(TaskUpdateStatusTool(task_manager))),
    ]
}
