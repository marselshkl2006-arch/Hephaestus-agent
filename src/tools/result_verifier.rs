//! Result Verifier — проверка результата выполненного действия по
//! декларативным условиям, без повторного обращения к LLM. Аналог
//! `result_verifier.py`. Полезно как финальный шаг после `workflow_run`
//! или серии `file_write`/`bash`: агент явно проверяет, что "действительно
//! получилось", а не просто доверяет отсутствию ошибки.

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolResult};

pub struct VerifyResultTool;

impl VerifyResultTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for VerifyResultTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let checks = match args.get("checks").and_then(|v| v.as_array()) {
            Some(c) if !c.is_empty() => c.clone(),
            _ => return ToolResult::error(
                "Требуется непустой массив 'checks'. Поддерживаемые типы: \
                 file_exists{path}, file_contains{path, text}, \
                 file_not_empty{path}, command_success{command}",
            ),
        };

        let mut results = Vec::new();
        let mut all_passed = true;

        for check in &checks {
            let check_type = check.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let (passed, detail) = match check_type {
                "file_exists" => {
                    let path = check.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let ok = !path.is_empty() && std::path::Path::new(path).exists();
                    (ok, format!("файл {}", if ok { "существует" } else { "не найден" }))
                }
                "file_not_empty" => {
                    let path = check.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let ok = std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false);
                    (ok, format!("файл {}", if ok { "не пуст" } else { "пуст или не найден" }))
                }
                "file_contains" => {
                    let path = check.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let text = check.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    let ok = std::fs::read_to_string(path)
                        .map(|content| content.contains(text))
                        .unwrap_or(false);
                    (ok, format!("подстрока '{}' {}", text, if ok { "найдена" } else { "не найдена" }))
                }
                "command_success" => {
                    let command = check.get("command").and_then(|v| v.as_str()).unwrap_or("");
                    // Кроссплатформенно (shell.rs): bash/PowerShell/cmd по ОС.
                    let shell = crate::shell::ShellKind::detect();
                    let (program, args) = match shell {
                        crate::shell::ShellKind::Bash => ("bash".to_string(), vec!["-c".to_string(), command.to_string()]),
                        crate::shell::ShellKind::PowerShell => ("powershell".to_string(), vec!["-NoProfile".to_string(), "-Command".to_string(), command.to_string()]),
                        crate::shell::ShellKind::Cmd => ("cmd".to_string(), vec!["/C".to_string(), command.to_string()]),
                    };
                    let ok = std::process::Command::new(&program)
                        .args(&args)
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);
                    (ok, format!("команда {}", if ok { "успешна" } else { "завершилась с ошибкой" }))
                }
                other => (false, format!("неизвестный тип проверки: '{}'", other)),
            };
            if !passed {
                all_passed = false;
            }
            results.push(serde_json::json!({
                "type": check_type,
                "passed": passed,
                "detail": detail,
            }));
        }

        let summary = serde_json::json!({
            "all_passed": all_passed,
            "checks": results,
        });

        let text = serde_json::to_string_pretty(&summary).unwrap_or_default();
        if all_passed {
            ToolResult::success(text)
        } else {
            ToolResult::error(text)
        }
    }


    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"checks":{"type":"array","items":{"type":"string"}}},"required":["checks"]})
    }

    fn name(&self) -> &'static str {
        "verify_result"
    }
}
