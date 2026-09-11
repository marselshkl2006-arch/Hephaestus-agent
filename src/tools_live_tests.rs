//! ЖИВЫЕ тесты инструментов, которые e2e-прогон через LLM не покрыл
//! (модель выполнила не все задачи): кэш, канбан-доски, workflow,
//! parallel_exec, background-задачи, verify_result, secret_store, cron.
//!
//! Отличие от юнит-тестов в самих модулях: здесь инструменты вызываются
//! как их вызывает МОДЕЛЬ — через execute(json), с настоящим реестром
//! и в той комбинации, что используется в бою.

use crate::tools::{Tool, ToolRegistry};

fn registry_with_all() -> (crate::tools::SharedToolRegistry, std::path::PathBuf) {
    // Полный реестр как в create_tools, но с изолированными путями.
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    std::env::set_var("HEPHAESTUS_HOME", &home);
    let workdir = crate::workdir::WorkDir::new(tmp.path().to_path_buf());
    let monitoring = std::sync::Arc::new(crate::monitoring::MonitoringSystem::new());
    let permissions = std::sync::Arc::new(crate::permissions::PermissionManager::new());
    let engine = std::sync::Arc::new(crate::permissions::PermissionEngine::new(vec![]));
    let databases = std::sync::Arc::new(crate::tools::db_universal::DbConnections::empty());
    let todos = std::sync::Arc::new(crate::todo_tools::TodoBoard::new());
    let llm_config = crate::llm::LLMConfig {
        provider: crate::llm::LLMProvider::Ollama,
        model: "test".into(),
        api_key: None,
        base_url: None,
        temperature: 0.1,
        max_tokens: 128,
        stream: false,
        extra_headers: Default::default(),
        extra_body: None,
    };
    let subagents = crate::tools::subagent::SubAgentRunner::empty();
    let env = crate::tools::ToolsEnv {
        workdir,
        monitoring,
        permissions,
        engine,
        databases,
        todos,
        llm_config,
        subagents,
    };
    let registry = crate::tools::create_tools(env);
    std::mem::forget(tmp); // живёт до конца процесса — env указывает туда
    (registry, home)
}

async fn call(reg: &crate::tools::SharedToolRegistry, name: &str, args: serde_json::Value) -> crate::tools::ToolResult {
    reg.execute(name, &args).await
        .unwrap_or_else(|| crate::tools::ToolResult::error("tool not found"))
}

#[tokio::test]
async fn cache_set_get_stats_roundtrip() {
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, _home) = registry_with_all();
    let r = call(&reg, "cache_set", serde_json::json!({"key": "e2e", "value": "hello", "ttl": 60})).await;
    assert!(r.success, "{}", r.error.unwrap_or_default());

    let r = call(&reg, "cache_get", serde_json::json!({"key": "e2e"})).await;
    assert!(r.success && r.output.contains("hello"), "{}", r.output);

    let r = call(&reg, "cache_stats", serde_json::json!({})).await;
    assert!(r.success, "{}", r.error.unwrap_or_default());
}

#[tokio::test]
async fn board_create_list_update() {
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, _home) = registry_with_all();
    // board_create создаёт ЗАДАЧУ на доске (title), не «доску».
    let r = call(&reg, "board_create", serde_json::json!({"title": "e2e-задача"})).await;
    assert!(r.success, "board_create: {} | out: {}", r.error.unwrap_or_default(), r.output);
    let id = r.output.split("доске: ").nth(1).map(|s| s.trim().to_string()).unwrap_or_default();
    let r = call(&reg, "board_list", serde_json::json!({})).await;
    // board_list показывает СТАТИСТИКУ доски, не названия задач.
    assert!(r.success && r.output.contains("1"), "board_list: {}", r.output);
    let r = call(&reg, "board_update_status", serde_json::json!({"id": id, "status": "done"})).await;
    assert!(r.success, "{}", r.error.unwrap_or_default());
}

#[tokio::test]
async fn workflow_run_sequential_two_bash() {
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, _home) = registry_with_all();
    let r = call(&reg, "workflow_run", serde_json::json!({
        "steps": [
            {"tool": "bash", "args": {"command": "echo WF-1"}},
            {"tool": "bash", "args": {"command": "echo WF-2"}}
        ]
    })).await;
    assert!(r.success, "{}", r.error.unwrap_or_default());
    assert!(r.output.contains("WF-1") && r.output.contains("WF-2"), "{}", r.output);
}

#[tokio::test]
async fn parallel_exec_two_commands() {
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, _home) = registry_with_all();
    let r = call(&reg, "parallel_exec", serde_json::json!({
        "calls": [
            {"tool": "bash", "args": {"command": "echo A"}},
            {"tool": "bash", "args": {"command": "echo B"}}
        ]
    })).await;
    assert!(r.success, "{}", r.error.unwrap_or_default());
    assert!(r.output.contains("A") && r.output.contains("B"), "{}", r.output);
}

#[tokio::test]
async fn background_run_then_status() {
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, _home) = registry_with_all();
    let r = call(&reg, "background_run", serde_json::json!({
        "tool": "bash", "args": {"command": "echo BG-OK"}
    })).await;
    assert!(r.success, "{}", r.error.unwrap_or_default());
    // id из вывода → статус.
    let id: String = r
        .output
        .lines()
        .find(|l| l.contains("bg_"))
        .and_then(|l| l.split_whitespace().find(|w| w.starts_with("bg_")))
        .unwrap_or("bg_missing")
        .to_string();
    // Даём фоновой задаче время завершиться.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let r = call(&reg, "background_status", serde_json::json!({"task_id": id})).await;
    assert!(r.success, "{}", r.error.unwrap_or_default());
    assert!(r.output.contains("BG-OK") || r.output.contains("running") || r.output.contains("done"), "{}", r.output);
}

#[tokio::test]
async fn verify_result_file_checks() {
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, home_dir) = registry_with_all();
    let dir = home_dir.to_string_lossy().to_string();
    let probe = std::path::PathBuf::from(&dir).join("probe.txt");
    std::fs::write(&probe, "x").unwrap();
    // Проверка НЕ проходит целиком (all_passed=false) → ToolResult::error.
    let r = call(&reg, "verify_result", serde_json::json!({
        "checks": [
            {"type": "file_exists", "path": probe.to_string_lossy()},
            {"type": "file_exists", "path": "/definitely/missing/xyz"}
        ]
    })).await;
    assert!(!r.success, "проваленная проверка обязана быть ошибкой");
    let err = r.error.unwrap_or_default();
    assert!(err.contains("passed"), "JSON-сводка в error: {err}");
    // Только успешные проверки → success.
    let r = call(&reg, "verify_result", serde_json::json!({
        "checks": [{"type": "file_exists", "path": probe.to_string_lossy()}]
    })).await;
    assert!(r.success, "verify: err={:?} out={}", r.error, r.output);
}

#[tokio::test]
async fn secret_store_roundtrip() {
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, _home) = registry_with_all();
    assert!(call(&reg, "secret_set", serde_json::json!({"key": "t1", "value": "v1"})).await.success);
    let r = call(&reg, "secret_get", serde_json::json!({"key": "t1"})).await;
    assert!(r.success && r.output.contains("v1"), "{}", r.output);
    let r = call(&reg, "secret_list", serde_json::json!({})).await;
    assert!(r.output.contains("t1"), "{}", r.output);
    assert!(call(&reg, "secret_delete", serde_json::json!({"key": "t1"})).await.success);
}

#[tokio::test]
async fn cron_create_list_delete() {
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, _home) = registry_with_all();
    assert!(call(&reg, "cron_create", serde_json::json!({
        "cron": "0 9 * * *", "prompt": "напоминание e2e"
    })).await.success);
    let r = call(&reg, "cron_list", serde_json::json!({})).await;
    assert!(r.success && r.output.contains("напоминание e2e"), "{}", r.output);
    // id берём из cron_list: строка формата "✅ task_<ts>_<n>".
    let list = call(&reg, "cron_list", serde_json::json!({})).await;
    let task_id = list
        .output
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            let first = it.next()?;
            let id = it.next()?;
            (id.starts_with("task_")).then(|| id.to_string())
        })
        .unwrap_or_default();
    assert!(!task_id.is_empty(), "cron_list не дал id: {}", list.output);
    let ok = call(&reg, "cron_delete", serde_json::json!({"task_id": task_id})).await.success;
    assert!(ok, "cron_delete {task_id} не сработал");
}

#[tokio::test]
async fn git_accepts_command_with_args() {
    // Регрессия: "status --short" раньше не распознавался (Unknown git command).
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, _home) = registry_with_all();
    // git требует репозиторий в workdir — создаём через bash.
    call(&reg, "bash", serde_json::json!({"command": "git init -q ."})).await;
    let r = call(&reg, "git", serde_json::json!({"command": "status --short"})).await;
    assert!(r.success, "git status --short должен работать: {}", r.error.unwrap_or_default());
}

#[tokio::test]
async fn diagram_finds_pub_structs() {
    // Регрессия: `pub struct` не матчился — 0 классов на типичном Rust-файле.
    let _env = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (reg, home_dir2) = registry_with_all();
    let src = std::path::PathBuf::from(std::env::var("HEPHAESTUS_HOME").unwrap()).join("diag.rs");
    std::fs::write(&src, "pub struct AlphaCfg { x: u8 }\nstruct Priv { }\npub enum Kind { A }\n").unwrap();
    let r = call(&reg, "diagram", serde_json::json!({
        "kind": "class_diagram",
        "file_path": home_dir2.join("diag.rs").to_string_lossy()
    })).await;
    assert!(r.success, "{}", r.error.unwrap_or_default());
    assert!(r.output.contains("AlphaCfg") && r.output.contains("Kind"), "{}", r.output);
}
