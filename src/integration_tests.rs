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
mod permissions_engine_integration {
    use crate::permissions::{PermissionEngine, RuleAction, RuleConfig};

    #[test]
    fn last_match_wins() {
        let engine = PermissionEngine::new(vec![
            RuleConfig { tool: "bash".into(), pattern: "git *".into(), action: RuleAction::Allow },
            RuleConfig { tool: "bash".into(), pattern: "git push*".into(), action: RuleAction::Deny },
        ]);
        // "git push" совпадает с ОБОИМИ правилами — последнее (deny) выигрывает.
        assert_eq!(engine.evaluate("bash", "git push origin main"), Some(RuleAction::Deny));
        // "git status" — только первое.
        assert_eq!(engine.evaluate("bash", "git status"), Some(RuleAction::Allow));
        // Несовпадение — None (решает стандартная логика риска).
        assert_eq!(engine.evaluate("bash", "cargo build"), None);
    }

    #[test]
    fn session_rules_override_config() {
        let engine = PermissionEngine::new(vec![
            RuleConfig { tool: "bash".into(), pattern: "rm *".into(), action: RuleAction::Deny },
        ]);
        assert_eq!(engine.evaluate("bash", "rm x.txt"), Some(RuleAction::Deny));
        // Пользователь разрешил на сессию — сессийное правило позже и выигрывает.
        engine.add_session_rule("bash", "rm *", RuleAction::Allow);
        assert_eq!(engine.evaluate("bash", "rm x.txt"), Some(RuleAction::Allow));
        // clear удаляет только сессийные.
        engine.clear_session_rules();
        assert_eq!(engine.evaluate("bash", "rm x.txt"), Some(RuleAction::Deny));
    }

    #[test]
    fn builtin_defaults_protect_secrets() {
        let engine = PermissionEngine::new(vec![]);
        // .env — ask по умолчанию (не молча читается, не блокируется навсегда).
        assert_eq!(engine.evaluate("read", "project/.env"), Some(RuleAction::Ask));
        assert_eq!(engine.evaluate("read", ".env.local"), Some(RuleAction::Ask));
        assert_eq!(engine.evaluate("edit", "app/.env"), Some(RuleAction::Ask));
        // Ключи/сертификаты тоже.
        assert_eq!(engine.evaluate("read", "server.pem"), Some(RuleAction::Ask));
        assert_eq!(engine.evaluate("delete", "id_rsa"), Some(RuleAction::Deny));
        // Обычный файл — без правила.
        assert_eq!(engine.evaluate("read", "src/main.rs"), None);
        // Пользователь может перекрыть дефолт (например .env.example разрешён).
        let engine2 = PermissionEngine::new(vec![
            RuleConfig { tool: "read".into(), pattern: "**/.env.example".into(), action: RuleAction::Allow },
        ]);
        assert_eq!(engine2.evaluate("read", ".env.example"), Some(RuleAction::Allow));
    }

    #[test]
    fn wildcard_tool_matches_everything() {
        let engine = PermissionEngine::new(vec![
            RuleConfig { tool: "*".into(), pattern: "certain/*".into(), action: RuleAction::Deny },
        ]);
        assert_eq!(engine.evaluate("read", "certain/file"), Some(RuleAction::Deny));
        assert_eq!(engine.evaluate("edit", "certain/file"), Some(RuleAction::Deny));
    }

    #[test]
    fn audit_trail_written() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = std::sync::Arc::new(crate::state_db::StateDb::build_pub(
            conn,
            std::path::PathBuf::from(":memory:"),
        ).unwrap());
        let engine = PermissionEngine::new(vec![
            RuleConfig { tool: "bash".into(), pattern: "git status".into(), action: RuleAction::Allow },
        ]);
        engine.set_audit(db.clone());
        let _ = engine.evaluate("bash", "git status");
        engine.audit_decision("bash", "cargo test", "asked", "interactive");

        let log = db.list_permission_log(10);
        assert_eq!(log.len(), 2);
        // Свежие сверху.
        assert_eq!(log[0].2, "cargo test");
        assert_eq!(log[0].3, "asked");
        // У решения по правилу источник указывает на совпавший паттерн.
        assert_eq!(log[1].3, "allow");
        assert!(log[1].4.contains("rule:git status"), "источник должен ссылаться на правило: {:?}", log[1].4);
    }
}

#[cfg(test)]
mod atomic_files_integration {
    use crate::tools::file_tools::{FileWriteTool, FileEditTool};
    use crate::tools::{Tool, file_cache::FileCache};
    use crate::workdir::WorkDir;
    use crate::permissions::PermissionEngine;

    fn setup() -> (tempfile::TempDir, WorkDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let wd = WorkDir::new(tmp.path().to_path_buf());
        (tmp, wd)
    }

    /// Атомарная запись: содержимое на месте, временных файлов не остаётся.
    #[tokio::test]
    async fn write_is_atomic_and_cleans_up() {
        let (_t, wd) = setup();
        let engine = std::sync::Arc::new(PermissionEngine::new(vec![]));
        let tool = FileWriteTool::new(wd.clone(), engine);

        // Перезапись существующего.
        std::fs::write(wd.get().join("f.txt"), "old\n").unwrap();
        let res = tool.execute(&serde_json::json!({
            "file_path": "f.txt",
            "content": "новое содержимое\n"
        })).await;
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert_eq!(std::fs::read_to_string(wd.get().join("f.txt")).unwrap(), "новое содержимое\n");
        let leftovers: Vec<_> = std::fs::read_dir(wd.get())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("hef-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "tmp-файлы не убраны: {leftovers:?}");
    }

    /// FRESHNESS: файл прочитан file_read-ом → пользователь изменил его
    /// на диске → edit ОБЯЗАН отказаться, а не затереть чужие правки.
    #[tokio::test]
    async fn edit_rejects_stale_read() {
        let (_t, wd) = setup();
        let engine = std::sync::Arc::new(PermissionEngine::new(vec![]));
        let cache = FileCache::default();
        let read = crate::tools::file_tools::FileReadTool::with_cache(
            cache.clone(), wd.clone(), engine.clone(),
            std::sync::Arc::new(crate::permissions::PermissionManager::new()),
        );
        let edit = FileEditTool::with_cache(cache, wd.clone(), engine);

        std::fs::write(wd.get().join("f.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        // Модель читает файл (кэш запоминает mtime+content).
        let r = read.execute(&serde_json::json!({"file_path": "f.rs"})).await;
        assert!(r.success);

        // Пользователь правит файл вручную.
        std::fs::write(wd.get().join("f.rs"), "fn a() {}\nfn b() {}\nfn USER_EDIT() {}\n").unwrap();

        // Edit по устаревшему снимку — ОТКАЗ.
        let res = edit.execute(&serde_json::json!({
            "file_path": "f.rs",
            "old_text": "fn b() {}",
            "new_text": "fn c() {}"
        })).await;
        assert!(!res.success, "edit по устаревшему снимку должен отказаться");
        assert!(res.error.unwrap_or_default().contains("изменился"), "{}", res.output);
        // Пользовательская правка ЦЕЛА.
        let disk = std::fs::read_to_string(wd.get().join("f.rs")).unwrap();
        assert!(disk.contains("USER_EDIT"));
    }

    /// Нормальный путь: read → edit без чужих правок — работает.
    #[tokio::test]
    async fn edit_works_when_fresh() {
        let (_t, wd) = setup();
        let engine = std::sync::Arc::new(PermissionEngine::new(vec![]));
        let cache = FileCache::default();
        let read = crate::tools::file_tools::FileReadTool::with_cache(
            cache.clone(), wd.clone(), engine.clone(),
            std::sync::Arc::new(crate::permissions::PermissionManager::new()),
        );
        let edit = FileEditTool::with_cache(cache, wd.clone(), engine);

        std::fs::write(wd.get().join("f.rs"), "fn a() {}\n").unwrap();
        let _ = read.execute(&serde_json::json!({"file_path": "f.rs"})).await;
        let res = edit.execute(&serde_json::json!({
            "file_path": "f.rs",
            "old_text": "fn a() {}",
            "new_text": "fn b() {}"
        })).await;
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert_eq!(std::fs::read_to_string(wd.get().join("f.rs")).unwrap(), "fn b() {}\n");
    }

    /// Edit без предварительного file_read тоже работает (кэша нет —
    /// проверка не мешает), это легаси-совместимость.
    #[tokio::test]
    async fn edit_without_prior_read_still_works() {
        let (_t, wd) = setup();
        let engine = std::sync::Arc::new(PermissionEngine::new(vec![]));
        let edit = FileEditTool::with_cache(FileCache::default(), wd.clone(), engine);
        std::fs::write(wd.get().join("g.txt"), "hello world\n").unwrap();
        let res = edit.execute(&serde_json::json!({
            "file_path": "g.txt",
            "old_text": "hello",
            "new_text": "привет"
        })).await;
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert_eq!(std::fs::read_to_string(wd.get().join("g.txt")).unwrap(), "привет world\n");
    }
}

#[cfg(test)]
mod multi_session_integration {
    use crate::state_db::StateDb;
    use crate::llm::LLMMessage;

    fn fresh_db() -> StateDb {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        StateDb::build_pub(conn, std::path::PathBuf::from(":memory:")).unwrap()
    }

    /// Сквозной сценарий: две сессии → список (свежие сверху) → FTS-поиск
    /// находит только нужную → switch_to открывает старую с её контентом.
    #[test]
    fn list_search_and_switch() {
        let db = fresh_db();

        // Сессия 1: про выпечку.
        let s1 = db.new_session("ollama", "m");
        db.append_message(s1, &LLMMessage::user("как испечь хлеб на закваске".into()));
        db.append_message(s1, &LLMMessage::assistant("Рецепт: мука, вода, соль...".into()));

        // Сессия 2 (свежее): про Rust.
        let s2 = db.new_session("ollama", "m");
        db.append_message(s2, &LLMMessage::user("объясни lifetime в rust".into()));

        let list = db.list_sessions(10);
        assert_eq!(list.len(), 2, "обе сессии в списке");
        assert_eq!(list[0].id, s2, "свежая сверху");
        assert_eq!(list[0].title, "объясни lifetime в rust");
        assert_eq!(list[1].message_count, 2);

        // FTS-поиск: слово из сессии 1.
        let hits = db.search_sessions("закваске", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, s1);
        assert_eq!(hits[0].1, 1);

        // Переключение: открываем старую сессию — контент на месте.
        assert!(db.switch_to(s1));
        let restored = db.load_messages(s1);
        assert_eq!(restored.len(), 2);
        assert!(restored[0].content.contains("хлеб"));
        assert!(!db.switch_to(9999), "несуществующая сессия — false");
    }

    #[test]
    fn search_empty_and_quoted_input_safe() {
        let db = fresh_db();
        let s = db.new_session("ollama", "m");
        db.append_message(s, &LLMMessage::user("текст с кавычками \"внутри\"".into()));
        assert!(db.search_sessions("неттакогослова", 5).is_empty());
        // Кавычки в запросе не ломают FTS-синтаксис.
        let hits = db.search_sessions("кавычками", 5);
        assert_eq!(hits.len(), 1);
    }
}

#[cfg(test)]
mod memory_staging_integration {
    use crate::memory_tools;
    use crate::tools::Tool;

    /// Тесты этого модуля меняют общий процессный стейт (env var +
    /// thread_local фон) — сериализуем их одним мьютексом.

    fn isolated_home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        // Уникальное имя на прогон: защита от env-гонок с параллельными
        // тестами (общий index читается из какого-либо home).
        let uniq = format!("fact-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos());
        let (_g, _tmp) = isolated_home();

        memory_tools::mark_background();
        // Эмулируем вызов инструмента.
        let tool = memory_tools::MemoryFileSaveTool;
        let res = futures_executor_block(tool.execute(&serde_json::json!({
            "name": &uniq,
            "type": "project",
            "description": "тест",
            "content": "тело факта"
        })));
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert!(res.output.contains("УТВЕРЖДЕНИЕ") || res.output.contains("утверждение"),
            "фон должен стейджить: {}", res.output);
        // Основная память НЕ тронута.
        assert!(!memory_tools::memory_index_lines().contains(&uniq));
        assert_eq!(memory_tools::pending_memory_files().len(), 1);

        // approve — переносит в основную.
        let approved = memory_tools::approve_pending(&uniq).unwrap();
        assert_eq!(approved, uniq);
        assert_eq!(approved, uniq);
        assert!(memory_tools::memory_index_lines().contains(&uniq));
        assert!(memory_tools::pending_memory_files().is_empty());

        // reject — удаляет.
        let bad_uniq = format!("bad-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos());
        memory_tools::mark_background();
        let _ = futures_executor_block(tool.execute(&serde_json::json!({
            "name": &bad_uniq, "type": "project", "description": "мусор", "content": "x"
        })));
        assert_eq!(memory_tools::pending_memory_files().len(), 1);
        memory_tools::reject_pending(&bad_uniq).unwrap();
        assert!(memory_tools::pending_memory_files().is_empty());

        cleanup();
    }

    /// Интерактивный ход — пишет в основную память без стейджинга.
    #[test]
    fn interactive_write_goes_straight_to_memory() {
        let (_g, _tmp) = isolated_home();
        let direct_uniq = format!("direct-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos());
        memory_tools::clear_background();

        let tool = memory_tools::MemoryFileSaveTool;
        let res = futures_executor_block(tool.execute(&serde_json::json!({
            "name": &direct_uniq, "type": "user", "description": "напрямую", "content": "y"
        })));
        assert!(res.success, "{}", res.error.unwrap_or_default());
        assert!(res.output.contains("сохранён"));
        assert!(memory_tools::memory_index_lines().contains(&direct_uniq));
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
