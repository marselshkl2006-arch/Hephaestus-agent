//! Каноническое хранилище состояния Гефеста — SQLite (WAL).
//!
//! Роль аналогична `opencode.db` и `SessionDB` из Hermes: ЕДИНСТВЕННЫЙ
//! источник истины о сессиях. Старый `session.json` становится
//! legacy-экспортом/миграцией, не стором.
//!
//! Принципы (из исследования opencode/Hermes):
//! 1. **Инкрементальная запись по мере хода** — каждый push сообщения и
//!    каждая смена статуса tool-call пишутся СРАЗУ. Краш посреди ответа
//!    не теряет уже записанное (раньше история жила только в памяти
//!    `Arc<Mutex<Vec<LLMMessage>>>` и сохранялась JSON-ом после ответа).
//! 2. **Канонические данные ≠ производные индексы**: тут каноничны
//!    `sessions/messages/tool_calls`; при желании поиск можно достроить
//!    сверху, не трогая ядро.
//! 3. **Tool-call — персистентная state-машина** `pending → running →
//!    completed|error`: после рестарта видно, что было недоделано.
//! 4. **Recovery-мета** (`clean_shutdown`, `crash_streak`) — в таблице
//!    `meta`, по образцу clean-shutdown-маркера Hermes.
//! 5. **Деградация вместо смерти**: сбой БД НЕ рвёт диалог — память
//!    продолжает работать, ошибка логируется один раз (аналог
//!    «ledger-сбой никогда не блокирует отправку»).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension};

use crate::llm::LLMMessage;

/// Ключи таблицы `meta`.
pub mod meta_keys {
    pub const SCHEMA_VERSION: &str = "schema_version";
    pub const CLEAN_SHUTDOWN: &str = "clean_shutdown";
    pub const CRASH_STREAK: &str = "crash_streak";
    pub const JSON_MIGRATED: &str = "session_json_migrated";
    pub const LAST_PROVIDER: &str = "last_provider";
    pub const LAST_MODEL: &str = "last_model";
}

const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA busy_timeout=5000;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    provider       TEXT,
    model          TEXT,
    resume_pending INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS messages (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   INTEGER NOT NULL,
    role         TEXT NOT NULL,
    content      TEXT NOT NULL,
    tool_calls   TEXT,
    tool_call_id TEXT,
    created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, seq);

CREATE TABLE IF NOT EXISTS tool_calls (
    call_id     TEXT PRIMARY KEY,
    session_id  INTEGER NOT NULL,
    name        TEXT NOT NULL,
    args_json   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',
    output      TEXT,
    error       TEXT,
    duration_ms REAL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tool_calls_session ON tool_calls(session_id, created_at);

CREATE TABLE IF NOT EXISTS messages_archive (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   INTEGER NOT NULL,
    role         TEXT NOT NULL,
    content      TEXT NOT NULL,
    tool_calls   TEXT,
    tool_call_id TEXT,
    created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_archive_session ON messages_archive(session_id, seq);
"#;

/// Строка tool-call для просмотра после рестарта.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolCallRow {
    pub call_id: String,
    pub name: String,
    pub status: String,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
}

pub struct StateDb {
    conn: Mutex<Connection>,
    path: PathBuf,
    /// Флаг деградации: после первой ошибки записи БД переводится в
    /// режим no-op (с логом ОДИН раз) — не рвём диалог, не спамим лог.
    degraded: AtomicBool,
    pub started: Instant,
}

fn now_str() -> String {
    chrono::Local::now().to_rfc3339()
}

impl StateDb {
    /// Дефолтный путь: `$HEPHAESTUS_HOME/state.db` либо `~/.hephaestus/state.db`.
    pub fn default_path() -> PathBuf {
        if let Ok(custom) = std::env::var("HEPHAESTUS_HOME") {
            return PathBuf::from(custom).join("state.db");
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".hephaestus")
            .join("state.db")
    }

    pub fn open_default() -> std::sync::Arc<Self> {
        let path = Self::default_path();
        match Self::open(&path) {
            Ok(db) => std::sync::Arc::new(db),
            Err(e) => {
                crate::logging_system::error(&format!(
                    "[state_db] НЕ УДАЛОСЬ ОТКРЫТЬ {}: {e} — работаю без персистентности",
                    path.display()
                ));
                Self::open_in_memory()
            }
        }
    }

    /// Эфемерное хранилище для суб-агентов: их диалоги не должны
    /// засорять каноническую БД основной сессии.
    pub fn open_in_memory() -> std::sync::Arc<Self> {
        let conn = Connection::open_in_memory().expect("in-memory sqlite всегда открывается");
        let db = Self::build(conn, PathBuf::from(":memory:"))
            .expect("in-memory схема всегда применяется");
        std::sync::Arc::new(db)
    }

    pub fn open(path: &PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        Self::build(conn, path.clone())
    }

    fn build(conn: Connection, path: PathBuf) -> Result<Self, String> {
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("schema: {e}"))?;
        let db = Self {
            conn: Mutex::new(conn),
            path,
            degraded: AtomicBool::new(false),
            started: Instant::now(),
        };
        let v = db.meta_get(meta_keys::SCHEMA_VERSION).unwrap_or_default();
        if v.is_empty() {
            db.meta_set(meta_keys::SCHEMA_VERSION, "1");
        }
        Ok(db)
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Внутренний хелпер: выполнить f над соединением. При деградации —
    /// None; при первой ошибке — лог + перевод в деградацию.
    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Option<T> {
        if self.degraded.load(Ordering::Relaxed) {
            return None;
        }
        let guard = self.conn.lock().ok()?;
        match f(&guard) {
            Ok(v) => Some(v),
            Err(e) => {
                self.degraded.store(true, Ordering::Relaxed);
                crate::logging_system::error(&format!(
                    "[state_db] БД деградировала ({e}) — дальше работаю в памяти, БД отключена до рестарта"
                ));
                None
            }
        }
    }

    // ── meta ────────────────────────────────────────────────────────────

    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .flatten()
    }

    pub fn meta_set(&self, key: &str, value: &str) {
        let _ = self.with_conn(|c| {
            c.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![key, value],
            )
        });
    }

    // ── sessions ────────────────────────────────────────────────────────

    /// Идентификатор последней сессии (если БД уже использовалась).
    pub fn last_session_id(&self) -> Option<i64> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT id FROM sessions ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get::<_, i64>(0),
            )
            .optional()
        })
        .flatten()
    }

    /// Создать новую сессию и вернуть её id.
    pub fn new_session(&self, provider: &str, model: &str) -> i64 {
        let ts = now_str();
        let id = self.with_conn(|c| {
            c.execute(
                "INSERT INTO sessions(created_at, updated_at, provider, model) VALUES (?1, ?1, ?2, ?3)",
                rusqlite::params![ts, provider, model],
            )?;
            Ok(c.last_insert_rowid())
        });
        match id {
            Some(id) => id,
            None => -1, // деградация: идентификатор-заглушка, память работает
        }
    }

    pub fn touch_session(&self, session_id: i64) {
        let _ = self.with_conn(|c| {
            c.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                rusqlite::params![session_id, now_str()],
            )
        });
    }

    pub fn set_session_provider_model(&self, session_id: i64, provider: &str, model: &str) {
        let _ = self.with_conn(|c| {
            c.execute(
                "UPDATE sessions SET provider = ?2, model = ?3, updated_at = ?4 WHERE id = ?1",
                rusqlite::params![session_id, provider, model, now_str()],
            )
        });
    }

    pub fn set_resume_pending(&self, session_id: i64, pending: bool) {
        let _ = self.with_conn(|c| {
            c.execute(
                "UPDATE sessions SET resume_pending = ?2 WHERE id = ?1",
                rusqlite::params![session_id, pending as i64],
            )
        });
    }

    /// Время последнего обновления сессии (для «восстановлена сессия от …»).
    pub fn session_updated_at(&self, session_id: i64) -> Option<String> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT updated_at FROM sessions WHERE id = ?1",
                rusqlite::params![session_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .flatten()
    }

    // ── messages ────────────────────────────────────────────────────────

    /// Инкрементальная запись одного сообщения (вызывается из chat() на
    /// каждом push — краш-безопасность вместо end-of-turn сохранения).
    pub fn append_message(&self, session_id: i64, msg: &LLMMessage) {
        let tool_calls = msg
            .tool_calls
            .as_ref()
            .map(|tc| serde_json::to_string(tc).unwrap_or_default());
        let _ = self.with_conn(|c| {
            c.execute(
                "INSERT INTO messages(session_id, role, content, tool_calls, tool_call_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    session_id,
                    msg.role,
                    msg.content,
                    tool_calls,
                    msg.tool_call_id,
                    now_str()
                ],
            )
        });
        self.touch_session(session_id);
    }

    /// Заменить всю историю сессии (используется /load, миграцией JSON и
    /// синхронизацией после сжатия контекста).
    pub fn replace_messages(&self, session_id: i64, msgs: &[LLMMessage]) {
        let _ = self.with_conn(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("DELETE FROM messages WHERE session_id = ?1", rusqlite::params![session_id])?;
            for m in msgs {
                let tool_calls = m
                    .tool_calls
                    .as_ref()
                    .map(|tc| serde_json::to_string(tc).unwrap_or_default());
                tx.execute(
                    "INSERT INTO messages(session_id, role, content, tool_calls, tool_call_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![session_id, m.role, m.content, tool_calls, m.tool_call_id, now_str()],
                )?;
            }
            tx.commit()
        });
        self.touch_session(session_id);
    }

    pub fn load_messages(&self, session_id: i64) -> Vec<LLMMessage> {
        let rows: Option<Vec<LLMMessage>> = self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT role, content, tool_calls, tool_call_id FROM messages
                 WHERE session_id = ?1 ORDER BY seq",
            )?;
            let it = stmt.query_map(rusqlite::params![session_id], |r| {
                let role: String = r.get(0)?;
                let content: String = r.get(1)?;
                let tool_calls_raw: Option<String> = r.get(2)?;
                let tool_call_id: Option<String> = r.get(3)?;
                let tool_calls: Option<Vec<crate::llm::ToolUseBlock>> = tool_calls_raw
                    .and_then(|s| serde_json::from_str(&s).ok());
                Ok(LLMMessage { role, content, tool_calls, tool_call_id })
            })?;
            let mut out = Vec::new();
            for row in it {
                out.push(row.map_err(rusqlite::Error::from)?);
            }
            Ok(out)
        });
        rows.unwrap_or_default()
    }

    /// АРХИВ сжатия: спрятать первые `count` сообщений сессии в
    /// messages_archive (НИЧЕГО не удаляется — канонический транскрипт
    /// сохраняется полностью, активная таблица messages остаётся источником
    /// «живого» контекста). Возвращает число спрятанных строк.
    pub fn archive_head_messages(&self, session_id: i64, count: usize) -> usize {
        self.with_conn(|c| {
            let tx = c.unchecked_transaction()?;
            let moved = tx.execute(
                "INSERT INTO messages_archive(session_id, role, content, tool_calls, tool_call_id, created_at)
                 SELECT session_id, role, content, tool_calls, tool_call_id, created_at
                 FROM messages WHERE session_id = ?1 ORDER BY seq LIMIT ?2",
                rusqlite::params![session_id, count as i64],
            )?;
            tx.execute(
                "DELETE FROM messages WHERE seq IN (
                    SELECT seq FROM messages WHERE session_id = ?1 ORDER BY seq LIMIT ?2
                )",
                rusqlite::params![session_id, count as i64],
            )?;
            tx.commit()?;
            Ok(moved)
        })
        .unwrap_or(0)
    }

    pub fn count_messages(&self, session_id: i64) -> usize {
        self.with_conn(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                rusqlite::params![session_id],
                |r| r.get::<_, i64>(0),
            )
        })
        .unwrap_or(0) as usize
    }

    // ── tool_calls (персистентная state-машина) ─────────────────────────

    /// LLM запросил вызов → pending. Пишется ДО выполнения.
    pub fn record_tool_call(&self, session_id: i64, call_id: &str, name: &str, args: &serde_json::Value) {
        let ts = now_str();
        let _ = self.with_conn(|c| {
            c.execute(
                "INSERT OR REPLACE INTO tool_calls(call_id, session_id, name, args_json, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5)",
                rusqlite::params![call_id, session_id, name, args.to_string(), ts],
            )
        });
    }

    pub fn mark_tool_running(&self, call_id: &str) {
        let _ = self.with_conn(|c| {
            c.execute(
                "UPDATE tool_calls SET status = 'running', updated_at = ?2 WHERE call_id = ?1",
                rusqlite::params![call_id, now_str()],
            )
        });
    }

    /// Финал вызова: completed | error (+ вывод/ошибка/длительность).
    pub fn finish_tool_call(&self, call_id: &str, ok: bool, output_or_error: &str, duration_ms: f64) {
        let status = if ok { "completed" } else { "error" };
        let _ = self.with_conn(|c| {
            c.execute(
                "UPDATE tool_calls SET status = ?2, output = ?3, error = ?4, duration_ms = ?5, updated_at = ?6
                 WHERE call_id = ?1",
                rusqlite::params![
                    call_id,
                    status,
                    if ok { Some(output_or_error) } else { None },
                    if ok { None } else { Some(output_or_error) },
                    duration_ms,
                    now_str()
                ],
            )
        });
    }

    pub fn list_tool_calls(&self, session_id: i64) -> Vec<ToolCallRow> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT call_id, name, status, duration_ms, error FROM tool_calls
                 WHERE session_id = ?1 ORDER BY created_at",
            )?;
            let it = stmt.query_map(rusqlite::params![session_id], |r| {
                Ok(ToolCallRow {
                    call_id: r.get(0)?,
                    name: r.get(1)?,
                    status: r.get(2)?,
                    duration_ms: r.get(3)?,
                    error: r.get(4)?,
                })
            })?;
            let mut out = Vec::new();
            for row in it {
                out.push(row.map_err(rusqlite::Error::from)?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    /// Недоделанные вызовы (pending/running) — их видно после краша.
    pub fn unfinished_tool_calls(&self, session_id: i64) -> Vec<ToolCallRow> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT call_id, name, status, duration_ms, error FROM tool_calls
                 WHERE session_id = ?1 AND status IN ('pending','running') ORDER BY created_at",
            )?;
            let it = stmt.query_map(rusqlite::params![session_id], |r| {
                Ok(ToolCallRow {
                    call_id: r.get(0)?,
                    name: r.get(1)?,
                    status: r.get(2)?,
                    duration_ms: r.get(3)?,
                    error: r.get(4)?,
                })
            })?;
            let mut out = Vec::new();
            for row in it {
                out.push(row.map_err(rusqlite::Error::from)?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }
}

/// Дескриптор текущей сессии: атомарный id (меняется /reset-ом) поверх
/// общего Arc<StateDb>.
#[derive(Clone)]
pub struct SessionHandle {
    pub db: std::sync::Arc<StateDb>,
    id: std::sync::Arc<AtomicI64>,
}

impl SessionHandle {
    pub fn new(db: std::sync::Arc<StateDb>, id: i64) -> Self {
        Self { db, id: std::sync::Arc::new(AtomicI64::new(id)) }
    }
    pub fn id(&self) -> i64 {
        self.id.load(Ordering::Relaxed)
    }
    pub fn set_id(&self, id: i64) {
        self.id.store(id, Ordering::Relaxed);
    }
    pub fn append_message(&self, msg: &LLMMessage) {
        let sid = self.id();
        if sid >= 0 {
            self.db.append_message(sid, msg);
        }
    }
    pub fn replace_messages(&self, msgs: &[LLMMessage]) {
        let sid = self.id();
        if sid >= 0 {
            self.db.replace_messages(sid, msgs);
        }
    }
    pub fn load_messages(&self) -> Vec<LLMMessage> {
        let sid = self.id();
        if sid >= 0 {
            self.db.load_messages(sid)
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolUseBlock;

    fn db_fresh() -> StateDb {
        let conn = Connection::open_in_memory().unwrap();
        StateDb::build(conn, PathBuf::from(":memory:")).unwrap()
    }

    #[test]
    fn messages_roundtrip() {
        let d = db_fresh();
        let sid = d.new_session("ollama", "test");
        assert!(sid > 0);

        d.append_message(sid, &LLMMessage::user("привет мир".into()));
        let mut msg = LLMMessage::assistant_with_tools(
            "вызываю инструмент".into(),
            vec![ToolUseBlock {
                id: "call_1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            }],
        );
        let _ = &mut msg;
        d.append_message(sid, &msg);
        d.append_message(sid, &LLMMessage::tool_result("call_1".into(), "[bash] итог".into()));

        let loaded = d.load_messages(sid);
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].content, "привет мир");
        let tc = loaded[1].tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].name, "bash");
        assert_eq!(loaded[2].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(d.count_messages(sid), 3);
    }

    #[test]
    fn replace_messages_clears_old() {
        let d = db_fresh();
        let sid = d.new_session("ollama", "test");
        d.append_message(sid, &LLMMessage::user("старое".into()));
        d.replace_messages(sid, &[LLMMessage::user("новое".into())]);
        let loaded = d.load_messages(sid);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content, "новое");
    }

    #[test]
    fn tool_call_state_machine() {
        let d = db_fresh();
        let sid = d.new_session("ollama", "test");
        d.record_tool_call(sid, "c1", "bash", &serde_json::json!({"command":"ls"}));
        assert_eq!(d.unfinished_tool_calls(sid).len(), 1);
        d.mark_tool_running("c1");
        assert_eq!(d.unfinished_tool_calls(sid)[0].status, "running");
        d.finish_tool_call("c1", true, "ok", 12.5);
        assert!(d.unfinished_tool_calls(sid).is_empty());
        let all = d.list_tool_calls(sid);
        assert_eq!(all[0].status, "completed");
        assert_eq!(all[0].duration_ms, Some(12.5));
    }

    #[test]
    fn error_tool_call_stays_unfinished_visible() {
        // error — завершённое состояние, но в unfinished не попадает.
        let d = db_fresh();
        let sid = d.new_session("ollama", "test");
        d.record_tool_call(sid, "c1", "bash", &serde_json::json!({}));
        d.finish_tool_call("c1", false, "boom", 1.0);
        assert!(d.unfinished_tool_calls(sid).is_empty());
        assert_eq!(d.list_tool_calls(sid)[0].status, "error");
        assert_eq!(d.list_tool_calls(sid)[0].error.as_deref(), Some("boom"));
    }

    #[test]
    fn meta_roundtrip() {
        let d = db_fresh();
        assert_eq!(d.meta_get("nope"), None);
        d.meta_set("k", "v1");
        d.meta_set("k", "v2");
        assert_eq!(d.meta_get("k").as_deref(), Some("v2"));
    }

    #[test]
    fn resume_pending_flag() {
        let d = db_fresh();
        let sid = d.new_session("ollama", "test");
        d.set_resume_pending(sid, true);
        assert!(d.session_updated_at(sid).is_some());
        d.set_resume_pending(sid, false);
    }
}
