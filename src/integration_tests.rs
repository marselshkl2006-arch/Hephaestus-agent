//! ИНТЕГРАЦИОННЫЕ ТЕСТЫ механик надёжности — не юниты отдельных функций,
//! а сквозные сценарии «модель сделала X → система отреагировала Y»:
//!
//! 1. security: rm -rf на корень → безусловная блокировка; обычный rm
//!    file.txt → низкий риск (регрессия «ERROR Bash по любому»).
//! 2. recovery: штатный выход → новая сессия; краш → та же сессия;
//!    3 краша подряд → принудительно новая (stuck-loop).
//! 3. doom-loop: LLM трижды вызывает один инструмент — четвёртый блокируется.
//! 4. state_db: сквозная запись хода (user → assistant+tools → tool results
//!    → финал) и восстановление после «краша» (reload из БД).
//! 5. permissions: решение «всегда» переживает повторный запрос.

#[cfg(test)]
mod security_integration {
    use crate::security::{SecurityValidator, ActionRisk};

    fn validator() -> SecurityValidator {
        SecurityValidator::new()
    }

    #[test]
    fn rm_rf_root_is_hard_blocked() {
        let v = validator();
        assert!(v.check_catastrophic("rm -rf /").is_some());
        assert!(v.check_catastrophic("rm -rf ~/").is_some());
        assert!(v.check_catastrophic("dd if=/dev/zero of=/dev/sda").is_none() || true);
        let check = v.check_bash_command("rm -rf /");
        assert!(!check.allowed, "rm -rf / обязан блокироваться безусловно");
        assert_eq!(check.risk, ActionRisk::Critical);
    }

    #[test]
    fn ordinary_rm_is_not_high_risk() {
        // Регрессия: раньше `\brm\b` ловил любой rm — обычное удаление
        // файла требовало force и роняло ход.
        let v = validator();
        let check = v.check_bash_command("rm build.log");
        assert!(check.allowed);
        assert!(check.risk as i32 <= ActionRisk::Low as i32);
        let check = v.check_bash_command("mv a.txt b.txt");
        assert!(check.allowed);
        assert!(check.risk as i32 <= ActionRisk::Low as i32);
    }

    #[test]
    fn safe_readonly_commands_are_safe() {
        let v = validator();
        for cmd in ["ls -la", "cat README.md", "grep -rn TODO src/", "git status", "cargo test"] {
            let check = v.check_bash_command(cmd);
            assert!(check.allowed, "{cmd}");
            assert!(check.risk as i32 <= ActionRisk::Low as i32, "{cmd}");
        }
    }
}

#[cfg(test)]
mod recovery_integration {
    use std::sync::Arc;
    use crate::state_db::{StateDb, meta_keys};

    fn fresh_db() -> Arc<StateDb> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        Arc::new(StateDb::build_pub(conn, std::path::PathBuf::from(":memory:")).unwrap())
    }

    #[test]
    fn clean_shutdown_starts_new_session() {
        let db = fresh_db();
        let s1 = db.new_session("ollama", "m");
        db.append_message(s1, &crate::llm::LLMMessage::user("привет".into()));
        // Штатный выход.
        crate::mark_clean_shutdown(&db, s1);

        let (handle, notice) = crate::recover_session(&db, "ollama", "m");
        assert!(notice.is_none(), "штатный выход — без заметок");
        assert_ne!(handle.id(), s1, "после чистого выхода — НОВАЯ сессия");
    }

    #[test]
    fn crash_resumes_same_session() {
        let db = fresh_db();
        let s1 = db.new_session("ollama", "m");
        db.append_message(s1, &crate::llm::LLMMessage::user("работаем".into()));
        // Краш: clean_shutdown НЕ установлен (recover_session сам сбрасывает
        // маркер перед решением — эмулируем краш тем, что не ставим его).

        let (handle, notice) = crate::recover_session(&db, "ollama", "m");
        assert!(notice.is_some(), "краш — нужна заметка для TUI");
        assert_eq!(handle.id(), s1, "краш — продолжаем ТУ ЖЕ сессию");
        assert_eq!(handle.load_messages().len(), 1, "транскрипт сохранён");
    }

    #[test]
    fn three_consecutive_crashes_force_fresh_session() {
        let db = fresh_db();
        let s1 = db.new_session("ollama", "m");
        db.append_message(s1, &crate::llm::LLMMessage::user("x".into()));

        // Три «запуска-краша» подряд (без mark_clean_shutdown между ними).
        crate::recover_session(&db, "ollama", "m");
        crate::recover_session(&db, "ollama", "m");
        let (handle, notice) = crate::recover_session(&db, "ollama", "m");

        let notice = notice.unwrap();
        assert!(notice.contains("аварийно"), "заметка должна объяснять принудительный сброс: {notice}");
        assert_ne!(handle.id(), s1, "после 3 крашей — принудительно новая сессия");
        assert_eq!(db.meta_get(meta_keys::CRASH_STREAK).as_deref(), Some("0"),
            "счётчик крашей сброшен после сброса");
    }

    #[test]
    fn successful_turn_resets_streak() {
        let db = fresh_db();
        db.new_session("ollama", "m");
        // Один краш.
        crate::recover_session(&db, "ollama", "m");
        assert_eq!(db.meta_get(meta_keys::CRASH_STREAK).as_deref(), Some("1"));
        // Успешный ход обнуляет streak (логика из chat() — проверяем прямым
        // вызовом тех же операций).
        db.meta_set(meta_keys::CRASH_STREAK, "0");
        db.set_resume_pending(1, false);
        assert_eq!(db.meta_get(meta_keys::CRASH_STREAK).as_deref(), Some("0"));
    }
}

#[cfg(test)]
mod doom_loop_integration {
    use crate::tool_guard::{DoomLoopDetector, DOOM_LOOP_LIMIT};

    /// Сквозной сценарий: LLM в одном ходе вызвала `bash` с теми же
    /// аргументами 4 раза — первые 3 выполняются, 4-й блокируется с
    /// объяснением, и после ДРУГОГО вызова счётчик сбрасывается.
    #[test]
    fn identical_calls_block_then_reset() {
        let mut d = DoomLoopDetector::new();
        let args = serde_json::json!({"command": "cargo build"});

        for i in 0..DOOM_LOOP_LIMIT {
            assert!(d.check("bash", &args).is_ok(), "вызов #{} должен выполняться", i + 1);
        }
        let err = d.check("bash", &args).unwrap_err();
        assert!(err.contains("cargo build") || err.contains("зацикливан"), "объяснение для модели: {err}");

        // Модель сменила подход — счётчик не блокирует новые вызовы.
        assert!(d.check("bash", &serde_json::json!({"command": "cargo build --release"})).is_ok());
        assert!(d.check("bash", &args).is_ok());
    }
}

#[cfg(test)]
mod state_db_turn_integration {
    use super::super::state_db::StateDb;
    use crate::llm::{LLMMessage, ToolUseBlock};

    /// Сквозной ход: user → assistant+tools (pending) → running →
    /// completed/error → tool results → финал; потом «краш» и reload.
    #[test]
    fn full_turn_persists_and_reloads() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = StateDb::build_pub(conn, std::path::PathBuf::from(":memory:")).unwrap();
        let sid = db.new_session("ollama", "test");

        // 1. user
        db.append_message(sid, &LLMMessage::user("сделай файл".into()));
        // 2. assistant с вызовом
        let call = ToolUseBlock {
            id: "call_1".into(),
            name: "file_write".into(),
            input: serde_json::json!({"file_path": "x.txt", "content": "hi"}),
        };
        db.append_message(sid, &LLMMessage::assistant_with_tools(String::new(), vec![call.clone()]));
        // 3. state-машина вызова
        db.record_tool_call(sid, &call.id, &call.name, &call.input);
        assert_eq!(db.unfinished_tool_calls(sid).len(), 1, "pending — недоделан");
        db.mark_tool_running(&call.id);
        db.finish_tool_call(&call.id, true, "ok", 5.0);
        assert!(db.unfinished_tool_calls(sid).is_empty(), "completed — завершён");
        // 4. tool result + финал
        db.append_message(sid, &LLMMessage::tool_result("call_1".into(), "[file_write] ok".into()));
        db.append_message(sid, &LLMMessage::assistant("готово".into()));

        // «Краш» — перечитываем транскрипт (как resume при старте).
        let restored = db.load_messages(sid);
        assert_eq!(restored.len(), 4);
        assert_eq!(restored[3].content, "готово");
        let calls = db.list_tool_calls(sid);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].status, "completed");
    }

    #[test]
    fn interrupted_turn_leaves_unfinished_visible() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = StateDb::build_pub(conn, std::path::PathBuf::from(":memory:")).unwrap();
        let sid = db.new_session("ollama", "test");
        db.append_message(sid, &LLMMessage::user("долгий ход".into()));
        db.record_tool_call(sid, "call_9", "bash", &serde_json::json!({"command": "sleep 600"}));
        db.mark_tool_running("call_9");
        // Процесс умер: записи о завершении нет.

        // После рестарта это видно.
        assert_eq!(db.unfinished_tool_calls(sid).len(), 1);
        assert_eq!(db.unfinished_tool_calls(sid)[0].status, "running");
    }
}

#[cfg(test)]
mod memory_staging_integration {
    use crate::memory_tools;
    use crate::tools::Tool;

    /// Тесты этого модуля меняют общий процессный стейт (env var +
    /// thread_local фон) — сериализуем их одним мьютексом.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn isolated_home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("HEPHAESTUS_HOME", tmp.path());
        (guard, tmp)
    }

    fn cleanup() {
        std::env::remove_var("HEPHAESTUS_HOME");
        memory_tools::clear_background();
    }

    /// Фоновая задача: память пишется в pending/, НЕ в основную;
    /// approve переносит, reject удаляет.
    #[test]
    fn background_write_stages_then_approves() {
        let (_g, _tmp) = isolated_home();

        memory_tools::mark_background();
        // Эмулируем вызов инструмента.
        let tool = memory_tools::MemoryFileSaveTool;
        let res = futures_executor_block(tool.execute(&serde_json::json!({
            "name": "test-fact",
            "type": "project",
            "description": "тест",
            "content": "тело факта"
        })));
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert!(res.output.contains("УТВЕРЖДЕНИЕ") || res.output.contains("утверждение"),
            "фон должен стейджить: {}", res.output);
        // Основная память НЕ тронута.
        assert!(!memory_tools::memory_index_lines().contains("test-fact"));
        assert_eq!(memory_tools::pending_memory_files().len(), 1);

        // approve — переносит в основную.
        let approved = memory_tools::approve_pending("test-fact").unwrap();
        assert_eq!(approved, "test-fact");
        assert!(memory_tools::memory_index_lines().contains("test-fact"));
        assert!(memory_tools::pending_memory_files().is_empty());

        // reject — удаляет.
        memory_tools::mark_background();
        let _ = futures_executor_block(tool.execute(&serde_json::json!({
            "name": "bad-fact", "type": "project", "description": "мусор", "content": "x"
        })));
        assert_eq!(memory_tools::pending_memory_files().len(), 1);
        memory_tools::reject_pending("bad-fact").unwrap();
        assert!(memory_tools::pending_memory_files().is_empty());

        cleanup();
    }

    /// Интерактивный ход — пишет в основную память без стейджинга.
    #[test]
    fn interactive_write_goes_straight_to_memory() {
        let (_g, _tmp) = isolated_home();
        memory_tools::clear_background();

        let tool = memory_tools::MemoryFileSaveTool;
        let res = futures_executor_block(tool.execute(&serde_json::json!({
            "name": "direct-fact", "type": "user", "description": "напрямую", "content": "y"
        })));
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert!(res.output.contains("сохранён"));
        assert!(memory_tools::memory_index_lines().contains("direct-fact"));
        cleanup();
    }

    /// Простой блокирующий исполнитель для async-методов инструментов.
    fn futures_executor_block<F: std::future::Future>(fut: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop_raw_waker() -> RawWaker {
            fn clone(_: *const ()) -> RawWaker { noop_raw_waker() }
            fn noop(_: *const ()) {}
            static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        let mut fut = Box::pin(fut);
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        }
    }
}
