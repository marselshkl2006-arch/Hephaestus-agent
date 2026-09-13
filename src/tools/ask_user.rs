//! Ask User Question — интерактивный вопрос пользователю прямо посреди
//! выполнения инструментов. Аналог `ask_user_question.py`.
//!
//! ИСПРАВЛЕНО (жалоба "ask_user ломает терминал наглухо, пока не
//! закроешь"): раньше читал ответ через блокирующий `stdin`, пока TUI
//! (`repl.rs`) держал терминал в raw-mode — обычное построчное чтение
//! (`read_line`) в raw-mode не работает: нет echo, нет буферизации по
//! Enter, ввод пользователя никуда не попадал, а вопрос из-за этого же
//! не был виден в самом TUI. Теперь вызывает `tty_guard::with_real_terminal`
//! (тот же приём, что уже применён для `/voice` в repl.rs) — временно
//! опускает raw-mode/alternate screen, печатает вопрос и читает ответ
//! ОБЫЧНЫМ образом, затем поднимает TUI обратно.

use async_trait::async_trait;
use serde_json::Value;
use std::io::Write;

use super::{Tool, ToolResult};

pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AskUserTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let question = args.get("question").and_then(|v| v.as_str()).unwrap_or("");
        if question.is_empty() {
            return ToolResult::error("Требуется параметр 'question'");
        }
        let options: Vec<String> = args
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let question_owned = question.to_string();

        // `with_real_terminal` — блокирующий вызов (см. tty_guard.rs),
        // поэтому через spawn_blocking, чтобы не держать executor tokio
        // застрявшим на синхронном I/O (как и раньше — сам блокирующий
        // read_line был внутри spawn_blocking, только теперь ЕЩЁ и в
        // окружении из настоящего, не raw-mode, терминала).
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(180),
            tokio::task::spawn_blocking(move || {
                crate::tty_guard::with_real_terminal("вопрос пользователю (ask_user)", || {
                    println!("\n❓ {}", question_owned);
                    if !options.is_empty() {
                        for (i, opt) in options.iter().enumerate() {
                            println!("   {}) {}", i + 1, opt);
                        }
                    }
                    print!("> ");
                    let _ = std::io::stdout().flush();

                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).map(|_| line)
                })
            }),
        )
        .await;

        match result {
            Ok(Ok(Ok(line))) => {
                let answer = line.trim().to_string();
                if answer.is_empty() {
                    ToolResult::error("Пустой ответ пользователя")
                } else {
                    ToolResult::success(answer)
                }
            }
            Ok(Ok(Err(e))) => ToolResult::error(format!("Ошибка чтения ввода: {}", e)),
            Ok(Err(e)) => ToolResult::error(format!("Ошибка задачи ввода: {}", e)),
            Err(_) => ToolResult::error("Таймаут ожидания ответа пользователя (180с)."),
        }
    }


    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"]})
    }

    fn name(&self) -> &'static str {
        "ask_user"
    }
}
