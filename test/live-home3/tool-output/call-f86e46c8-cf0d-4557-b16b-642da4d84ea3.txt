//! Универсальные БД-инструменты (напарник должен видеть ВСЕ данные
//! пользователя, а не только SQLite).
//!
//! ЕДИНЫЙ API: sqlx покрывает SQLite + PostgreSQL + MySQL одним кодом
//! (чистый Rust, TLS — rustls, системных libpq/libmysql не нужно).
//! Redis — отдельный крейт (командная модель). MongoDB — намеренно НЕ
//! встроен: документная модель — другой API и очень тяжёлый крейт;
//! подключается MCP-сервером (~/.hephaestus/mcp.toml) или скиллом
//! (см. skills: database-mongodb).
//!
//! СОЕДИНЕНИЯ НЕ светятся модели: описываются в config.toml секцией
//! [databases.NAME] с url (поддерживает {env:VAR} — секреты в переменных
//! окружения) и read_only. Модель вызывает db_query { connection: "name" }.
//!
//! БЕЗОПАСНОСТЬ: read_only-подключение отклоняет DML на уровне
//! инструмента; запись (INSERT/UPDATE/DELETE/DDL) на обычном подключении
//! требует подтверждения человека (permissions) — permissions-as-data
//! работает и здесь (tool="db_write").

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Tool, ToolResult};
use crate::permissions::{Outcome, PermissionEngine};

/// Описание одного подключения из config.toml.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DatabaseConfig {
    /// URL: sqlite://path/file.db?mode=rw | postgres://user:pass@host/db | mysql://...
    /// Поддерживается {env:VAR} — значение подставляется из окружения.
    pub url: String,
    /// true — DML/DDL запрещён на уровне инструмента (аналитика/чтение).
    #[serde(default = "default_true")]
    pub read_only: bool,
    /// Что это за база — подсказка модели.
    #[serde(default)]
    pub description: String,
}

fn default_true() -> bool {
    true
}

/// Реестр подключений: имя → конфиг. Общий для db_query/db_redis.
pub struct DbConnections {
    map: HashMap<String, DatabaseConfig>,
}

impl DbConnections {
    pub fn new(map: HashMap<String, DatabaseConfig>) -> Self {
        Self { map }
    }

    pub fn empty() -> Self {
        Self { map: HashMap::new() }
    }

    pub fn list(&self) -> Vec<(String, DatabaseConfig)> {
        let mut v: Vec<_> = self.map.iter().map(|(k, c)| (k.clone(), c.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    pub fn get(&self, name: &str) -> Option<&DatabaseConfig> {
        self.map.get(name)
    }
}

/// Подстановка {env:VAR} в URL (секреты не лежат в config.toml).
fn resolve_url(raw: &str) -> Result<String, String> {
    let mut out = raw.to_string();
    while let Some(start) = out.find("{env:") {
        let rest = &out[start + 5..];
        let Some(end) = rest.find('}') else {
            return Err("незакрытый {env:...} в url".into());
        };
        let var = &rest[..end];
        let value = std::env::var(var)
            .map_err(|_| format!("переменная окружения {var} не задана (url ссылается на неё)"))?;
        out.replace_range(start..start + 5 + end + 1, &value);
    }
    Ok(out)
}

/// Определяет, читающий ли SQL (fetch) или пишущий (execute + permission).
fn is_read_query(sql: &str) -> bool {
    let head = sql.trim_start().split_whitespace().next().unwrap_or("").to_lowercase();
    matches!(head.as_str(), "select" | "pragma" | "show" | "explain" | "with" | "desc" | "describe")
}

/// Привести Any-ячейку к JSON: перебор поддерживаемых типов.
fn cell_to_json(row: &sqlx::any::AnyRow, idx: usize) -> Value {
    use sqlx::Row;
    if let Ok(v) = row.try_get::<Option<i64>, usize>(idx) {
        return match v {
            Some(n) => json!(n),
            None => Value::Null,
        };
    }
    if let Ok(v) = row.try_get::<Option<f64>, usize>(idx) {
        return match v {
            Some(f) => json!(f),
            None => Value::Null,
        };
    }
    if let Ok(v) = row.try_get::<Option<bool>, usize>(idx) {
        return match v {
            Some(b) => json!(b),
            None => Value::Null,
        };
    }
    if let Ok(v) = row.try_get::<Option<String>, usize>(idx) {
        return match v {
            Some(s) => json!(s),
            None => Value::Null,
        };
    }
    Value::Null
}

// ══════════════════════════════════════════════════════════ db_query ──

pub struct DbQueryTool {
    connections: Arc<DbConnections>,
    permissions: Arc<crate::permissions::PermissionManager>,
    engine: Arc<PermissionEngine>,
}

impl DbQueryTool {
    pub fn new(
        connections: Arc<DbConnections>,
        permissions: Arc<crate::permissions::PermissionManager>,
        engine: Arc<PermissionEngine>,
    ) -> Self {
        Self { connections, permissions, engine }
    }
}

#[async_trait]
impl Tool for DbQueryTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let Some(name) = args.get("connection").and_then(|v| v.as_str()) else {
            return ToolResult::error("нужен параметр 'connection' — имя из [databases.NAME] в config.toml (см. /databases)");
        };
        let Some(sql) = args.get("sql").and_then(|v| v.as_str()) else {
            return ToolResult::error("нужен параметр 'sql'");
        };
        if sql.trim().is_empty() {
            return ToolResult::error("sql пустой");
        }
        let max_rows = args.get("max_rows").and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize;

        let Some(cfg) = self.connections.get(name) else {
            let available: Vec<String> = self.connections.list().iter().map(|(n, _)| n.clone()).collect();
            return ToolResult::error(format!(
                "подключение '{name}' не найдено. Доступные: {} (настройки: [databases.NAME] в ~/.hephaestus/config.toml)",
                if available.is_empty() { "нет ни одного".into() } else { available.join(", ") }
            ));
        };

        let url = match resolve_url(&cfg.url) {
            Ok(u) => u,
            Err(e) => return ToolResult::error(format!("url подключения '{name}': {e}")),
        };

        let read_query = is_read_query(sql);

        // БЕЗОПАСНОСТЬ: запись — только на не-read_only и с подтверждением.
        if !read_query {
            if cfg.read_only {
                return ToolResult::error(format!(
                    "🚫 Подключение '{name}' помечено read_only=true в config.toml — запись запрещена на уровне инструмента."
                ));
            }
            // permissions-as-data: db_write-правила решают до вопроса.
            match self.engine.evaluate("db_write", name) {
                Some(crate::permissions::RuleAction::Deny) => {
                    return ToolResult::error(format!("🚫 Запись в '{name}' запрещена правилом разрешений."));
                }
                Some(crate::permissions::RuleAction::Allow) => {}
                _ => {
                    match self
                        .permissions
                        .request("db_write", &format!("SQL-запись в базу '{name}': {}", crate::truncate_chars(sql, 120)), "", &format!("db:{name}"))
                        .await
                    {
                        Outcome::Granted => {}
                        Outcome::Denied => {
                            return ToolResult::error("🚫 Пользователь ОТКЛОНИЛ запись в базу.");
                        }
                        Outcome::Expired => {
                            return ToolResult::error("⏳ Нет ответа на запрос записи за 3 минуты — отменено.");
                        }
                    }
                }
            }
        }

        // Единый пул для всех трёх СУБД. ВАЖНО (sqlx 0.8): Any-драйверы
        // устанавливаются явно; install_default_drivers идемпотентен
        // (Once внутри) и берёт все включённые в сборке драйверы.
        sqlx::any::install_default_drivers();
        // AnyPool реэкспортируется в КОРЕНЬ крейта (sqlx::AnyPool),
        // а не в sqlx::any — см. lib.rs: pub use crate::any::reexports::*.
        let pool = match sqlx::AnyPool::connect(&url).await {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("подключение к '{name}' не удалось: {e}")),
        };

        if read_query {
            use sqlx::Row as _;
            let rows = match sqlx::query(sql).fetch_all(&pool).await {
                Ok(r) => r,
                Err(e) => return ToolResult::error(format!("sql: {e}")),
            };
            let total = rows.len();
            let mut out_rows = Vec::new();
            for row in rows.iter().take(max_rows) {
                let mut obj = serde_json::Map::new();
                for (i, col) in row.columns.iter().enumerate() {
                    obj.insert(col.name.to_string(), cell_to_json(row, i));
                }
                out_rows.push(Value::Object(obj));
            }
            let shown = out_rows.len();
            let mut report = json!({
                "rows_returned": shown,
                "rows_total": total,
                "truncated": total > shown,
                "rows": out_rows,
            });
            if total > shown {
                report["note"] = json!(format!(
                    "показаны первые {} строк из {} — сузь LIMIT или подними max_rows (до 1000)",
                    shown, total
                ));
            }
            drop(pool);
            ToolResult::success(serde_json::to_string_pretty(&report).unwrap_or_default())
        } else {
            let result = match sqlx::query(sql).execute(&pool).await {
                Ok(r) => r,
                Err(e) => return ToolResult::error(format!("sql: {e}")),
            };
            drop(pool);
            ToolResult::success(format!(
                "✅ Выполнено. Затронуто строк: {}, последний id: {:?}",
                result.rows_affected(),
                result.last_insert_id()
            ))
        }
    }

    fn name(&self) -> &'static str {
        "db_query"
    }

    fn description(&self) -> &'static str {
        "Универсальный SQL: SQLite + PostgreSQL + MySQL одним инструментом (sqlx). Параметры: connection (имя из [databases.NAME] в config.toml), sql, max_rows. SELECT возвращает JSON-строки; INSERT/UPDATE/DELETE — счётчик затронутых (требует подтверждения на не-read_only подключении). Redis — инструмент db_redis; MongoDB — через MCP (mongo-mcp) или скилл database-mongodb."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "connection": {"type": "string", "description": "Имя подключения из config.toml [databases.NAME]"},
                "sql": {"type": "string", "description": "SQL-запрос (SELECT — чтение, DML — запись с подтверждением)"},
                "max_rows": {"type": "integer", "description": "Лимит строк вывода (по умолчанию 100, максимум 1000)"}
            },
            "required": ["connection", "sql"]
        })
    }
}

// ═════════════════════════════════════════════════════════ db_redis ──

pub struct DbRedisTool {
    connections: Arc<DbConnections>,
}

impl DbRedisTool {
    pub fn new(connections: Arc<DbConnections>) -> Self {
        Self { connections }
    }
}

#[async_trait]
impl Tool for DbRedisTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let Some(name) = args.get("connection").and_then(|v| v.as_str()) else {
            return ToolResult::error("нужен 'connection' — имя redis-подключения из [databases.NAME]");
        };
        let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
            return ToolResult::error("нужен 'command' — команда Redis (GET/SET/KEYS/HGETALL/...)");
        };
        let args_list: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }).collect())
            .unwrap_or_default();

        let Some(cfg) = self.connections.get(name) else {
            return ToolResult::error(format!("подключение '{name}' не найдено (см. /databases)"));
        };
        let url = match resolve_url(&cfg.url) {
            Ok(u) => u,
            Err(e) => return ToolResult::error(e),
        };

        // WRITE-команды через permission (SET/DEL/...), чтение — свободно.
        const WRITE_CMDS: &[&str] = &["SET", "DEL", "INCR", "DECR", "HSET", "HDEL", "LPUSH", "RPUSH", "LPOP", "RPOP", "EXPIRE", "FLUSHDB", "FLUSHALL", "SADD", "SREM", "ZADD", "ZREM", "RENAME", "SETEX", "MSET"];
        let upper = command.to_uppercase();
        if WRITE_CMDS.contains(&upper.as_str()) && cfg.read_only {
            return ToolResult::error(format!("🚫 '{name}' read_only=true — команда {upper} запрещена."));
        }

        use redis::AsyncCommands as _;
        let client = match redis::Client::open(url.as_str()) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("redis url: {e}")),
        };
        let mut conn = match client.get_multiplexed_tokio_connection().await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("подключение к redis '{name}': {e}")),
        };

        let mut cmd = redis::cmd(&upper);
        for a in &args_list {
            cmd.arg(a);
        }
        match cmd.query_async::<redis::Value>(&mut conn).await {
            Ok(v) => {
                ToolResult::success(serde_json::to_string_pretty(&json!({
                    "command": upper,
                    "result": redis_value_to_json(v)
                })).unwrap_or_default())
            }
            Err(e) => ToolResult::error(format!("redis {upper}: {e}")),
        }
    }

    fn name(&self) -> &'static str {
        "db_redis"
    }

    fn description(&self) -> &'static str {
        "Redis: любая команда (GET/SET/KEYS/HGETALL/...) через параметр command + args. Подключения — [databases.NAME] в config.toml с url redis://... и read_only=true/false. Write-команды на read_only запрещены."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "connection": {"type": "string", "description": "Имя redis-подключения"},
                "command": {"type": "string", "description": "Команда Redis: GET, SET, KEYS, HGETALL, TTL, ..."},
                "args": {"type": "array", "description": "Аргументы команды"}
            },
            "required": ["connection", "command"]
        })
    }
}

// ════════════════════════════════════════════════════ db_connections ──

/// Список подключений — модель видит, какие базы существуют.
pub struct DbListTool {
    connections: Arc<DbConnections>,
}

impl DbListTool {
    pub fn new(connections: Arc<DbConnections>) -> Self {
        Self { connections }
    }
}

#[async_trait]
impl Tool for DbListTool {
    async fn execute(&self, _args: &Value) -> ToolResult {
        let list = self.connections.list();
        if list.is_empty() {
            return ToolResult::success(
                "БД-подключений нет. Добавь в ~/.hephaestus/config.toml:\n\n[databases.mydb]\nurl = \"postgres://user:{env:PG_PASSWORD}@host/db\"\nread_only = true\ndescription = \"прод-база, только чтение\"\n\nПоддержка: sqlite://, postgres://, mysql:// (db_query), redis:// (db_redis). MongoDB — через MCP (mongo-mcp) или скилл database-mongodb.",
            );
        }
        let mut lines = vec![format!("Подключения БД ({}):", list.len())];
        for (name, cfg) in &list {
            // URL в выводе маскируем: креды не светим модели.
            let masked = if cfg.url.contains('@') {
                match cfg.url.split_once("://") {
                    Some((scheme, rest)) => match rest.split_once('@') {
                        Some((_, after)) => format!("{scheme}://***@{after}"),
                        None => cfg.url.clone(),
                    },
                    None => cfg.url.clone(),
                }
            } else {
                cfg.url.clone()
            };
            lines.push(format!(
                "  {} — {} [{}] {}",
                name,
                masked,
                if cfg.read_only { "read_only" } else { "read-write" },
                cfg.description
            ));
        }
        ToolResult::success(lines.join("\n"))
    }

    fn name(&self) -> &'static str {
        "db_connections"
    }

    fn description(&self) -> &'static str {
        "Список доступных БД-подключений (имена, типы, read_only). Не раскрывает креды."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }
}

pub fn create_db_tools(
    connections: Arc<DbConnections>,
    permissions: Arc<crate::permissions::PermissionManager>,
    engine: Arc<PermissionEngine>,
) -> Vec<(&'static str, Arc<dyn Tool>)> {
    vec![
        ("db_query", Arc::new(DbQueryTool::new(connections.clone(), permissions, engine))),
        ("db_redis", Arc::new(DbRedisTool::new(connections.clone()))),
        ("db_connections", Arc::new(DbListTool::new(connections))),
    ]
}

/// redis::Value → serde_json::Value (redis не даёт прямого FromRedisValue
/// для serde_json — конвертируем сами, рекурсивно для массивов).
fn redis_value_to_json(v: redis::Value) -> Value {
    match v {
        redis::Value::Nil => Value::Null,
        redis::Value::Int(i) => json!(i),
        redis::Value::BulkString(b) => {
            match String::from_utf8(b) {
                Ok(s) => {
                    // Числа/числа с плавающей точкой как строки чисел —
                    // попытка распарсить (GET счётчика → 42, а не "42").
                    if let Ok(n) = s.parse::<i64>() {
                        json!(n)
                    } else if let Ok(f) = s.parse::<f64>() {
                        if s.contains('.') {
                            json!(f)
                        } else {
                            json!(s)
                        }
                    } else {
                        json!(s)
                    }
                }
                Err(_) => Value::Null,
            }
        }
        redis::Value::Array(arr) => Value::Array(arr.into_iter().map(redis_value_to_json).collect()),
        redis::Value::SimpleString(s) => json!(s),
        redis::Value::Okay => json!(true),
        redis::Value::Map(pairs) => {
            // HGETALL: пары ключ-значение → объект.
            let mut obj = serde_json::Map::new();
            for (k, v) in pairs {
                let key = match redis_value_to_json(k) {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                obj.insert(key, redis_value_to_json(v));
            }
            Value::Object(obj)
        }
        other => json!(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_interpolation() {
        std::env::set_var("TEST_DB_PASS_XYZ", "s3cret");
        assert_eq!(resolve_url("postgres://u:{env:TEST_DB_PASS_XYZ}@h/db").unwrap(), "postgres://u:s3cret@h/db");
        assert!(resolve_url("postgres://u:{env:NO_SUCH_VAR_XYZ}@h/db").is_err());
        std::env::remove_var("TEST_DB_PASS_XYZ");
    }

    #[test]
    fn read_query_detection() {
        assert!(is_read_query("SELECT 1"));
        assert!(is_read_query("  explain analyze select 1"));
        assert!(is_read_query("with t as (select 1) select * from t"));
        assert!(!is_read_query("INSERT INTO t VALUES (1)"));
        assert!(!is_read_query("update t set x=1"));
        assert!(!is_read_query("DROP TABLE t"));
        assert!(!is_read_query("create table t (id int)"));
    }

    #[tokio::test]
    async fn sqlite_end_to_end() {
        // Чистый end-to-end: файловая SQLite (Any-пул не принимает :memory:),
        // SELECT — без сети и без interactive-разрешений.
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("t.db").to_string_lossy().to_string();
        let url = format!("sqlite://{db_path}?mode=rwc");

        // Создаём таблицу и данные напрямую через конкретный SqlitePool.
        let setup = sqlx::SqlitePool::connect(&url).await.unwrap();
        sqlx::query("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)")
            .execute(&setup).await.unwrap();
        sqlx::query("INSERT INTO items (name) VALUES ('альфа'), ('бета')")
            .execute(&setup).await.unwrap();
        setup.close().await;

        let mut map = HashMap::new();
        map.insert("test".to_string(), DatabaseConfig {
            url,
            read_only: true,
            description: "тест".into(),
        });
        let conns = Arc::new(DbConnections::new(map));
        let tool = DbQueryTool::new(
            conns.clone(),
            Arc::new(crate::permissions::PermissionManager::new()),
            Arc::new(PermissionEngine::new(vec![])),
        );

        // SELECT через универсальный инструмент — кириллица и типы на месте.
        let res = tool.execute(&json!({
            "connection": "test",
            "sql": "SELECT id, name FROM items ORDER BY id"
        })).await;
        assert!(res.success, "{}", res.error.unwrap_or_default());
        let parsed: Value = serde_json::from_str(&res.output).unwrap();
        assert_eq!(parsed["rows_total"], 2);
        assert_eq!(parsed["rows"][0]["name"], "альфа");
        assert_eq!(parsed["rows"][1]["id"], 2);

        // DML на read_only — отказ на уровне инструмента (до permissions).
        let res = tool.execute(&json!({
            "connection": "test",
            "sql": "DELETE FROM items"
        })).await;
        assert!(!res.success);
        assert!(res.error.unwrap_or_default().contains("read_only"));

        // SELECT на несуществующем подключении — внятная ошибка.
        let res = tool.execute(&json!({"connection": "нет", "sql": "SELECT 1"})).await;
        assert!(res.error.unwrap_or_default().contains("не найдено"));
    }

    #[tokio::test]
    async fn redis_read_write_guard() {
        let mut map = HashMap::new();
        map.insert("r".to_string(), DatabaseConfig {
            url: "redis://127.0.0.1:6399/".into(), // заведомо пустой порт
            read_only: true,
            description: String::new(),
        });
        let tool = DbRedisTool::new(Arc::new(DbConnections::new(map)));
        // read_only блокирует write-команду ДО сети.
        let res = tool.execute(&json!({"connection": "r", "command": "SET", "args": ["k", "v"]})).await;
        assert!(!res.success);
        let err = res.error.unwrap_or_default();
        assert!(err.contains("read_only"), "{err}");
    }
}
