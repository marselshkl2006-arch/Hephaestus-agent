use crate::tools::{Tool, ToolResult};
use crate::security::{ActionRisk, SecurityValidator};
use serde_json::Value;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use crate::workdir::WorkDir;
use crate::permissions::{PermissionManager, Outcome};

pub struct BashTool {
    /// ИСПРАВЛЕНО (Telegram-бот не мог работать с файлами): было
    /// `working_dir: String` — замороженное значение на момент старта
    /// процесса. Теперь общая динамическая WorkDir: `/work_dir` в REPL
    /// или `/workdir <путь>` в Telegram действует на bash немедленно.
    pub working_dir: WorkDir,
    validator: SecurityValidator,
    /// Система подтверждений как в opencode: вместо отклонения с текстом
    /// "повторите с force:true" опасная команда СПРАШИВАЕТ человека
    /// ("разрешить один раз / всегда / нет") — через TUI или Telegram,
    /// в зависимости от того, откуда работает агент.
    permissions: Arc<PermissionManager>,
}

impl BashTool {
    pub fn new(working_dir: WorkDir, permissions: Arc<PermissionManager>) -> Self {
        Self { working_dir, validator: SecurityValidator::new(), permissions }
    }
}

#[async_trait::async_trait]
impl Tool for BashTool {
    fn description(&self) -> &'static str {
        "Выполнить shell-команду в рабочей директории. Опасные команды (см. \
         security.rs) требуют force: true — иначе блокируются заранее. \
         Команды, которым нужен пароль (sudo/ssh/su/passwd), выполняются \
         с реальным доступом к терминалу — TUI на это время приостанавливается."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Команда для выполнения в шелле"},
                "force": {"type": "boolean", "description": "Подтвердить выполнение команды повышенного риска (по умолчанию false)"}
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        if command.is_empty() {
            return ToolResult::error("No command provided");
        }

        // Command Validator (аналог command_validator.py): катастрофическое
        // — блокируется БЕЗУСЛОВНО (не спрашиваем никогда). Остальной
        // высокий риск — через систему подтверждений (permissions.rs):
        // человек видит команду и выбирает "один раз / всегда / нет".
        let check = self.validator.check_bash_command(command);
        if !check.allowed {
            return ToolResult::error(format!(
                "🛑 Команда заблокирована: {}",
                check.reason.unwrap_or_else(|| "правило безопасности".to_string())
            ));
        }
        if check.risk == ActionRisk::Critical || check.risk == ActionRisk::High {
            // Явный `force: true` от модели ИЛИ глобальный `--yes`/`-y`
            // пропускают вопрос; иначе — запрос разрешения человеку.
            if !force && !crate::confirmation::is_auto_confirm() {
                let always_key = command
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_start_matches("./")
                    .to_lowercase();
                let summary = format!("$ {}", command);
                let reason = format!(
                    "{} {}",
                    check.warning.clone().unwrap_or_default(),
                    check.reason.clone().unwrap_or_default()
                )
                .trim()
                .to_string();
                match self.permissions.request("bash", &summary, &reason, &always_key).await {
                    Outcome::Granted => {}
                    Outcome::Denied => {
                        return ToolResult::error(format!(
                            "🚫 Пользователь ОТКЛОНИЛ команду. Не повторяй её без обсуждения — спроси, как действовать иначе.\nКоманда: {}",
                            command
                        ));
                    }
                    Outcome::Expired => {
                        return ToolResult::error(format!(
                            "⏳ Никто не ответил на запрос разрешения за 3 минуты — команда отменена.\nКоманда: {}\nПовтори вызов, когда пользователь будет рядом.",
                            command
                        ));
                    }
                }
            }
        }

        // ИСПРАВЛЕНО (жалоба "агент не отдаёт терминал для sudo, и для
        // ssh тоже нужно"): команды вроде `sudo`/`ssh`/`su` обычно читают
        // пароль НЕ из stdin, а напрямую с управляющего терминала
        // (/dev/tty) — а stdin у обычного `.output()` вообще null,
        // stdout/stderr — пайпы в никуда до завершения команды. Пока
        // работает TUI (raw-mode + EventStream), настоящий терминал
        // занят ratatui — sudo либо не видит его вообще, либо запрос
        // пароля и ввод пользователя расходятся мимо друг друга: именно
        // это выглядело как "терминал не отвечает, наглухо, пока не
        // закроешь". Для таких команд — как и для /voice и ask_user
        // (см. tty_guard.rs) — временно опускаем TUI и запускаем команду
        // с ПОЛНЫМ (не захваченным) доступом к реальному терминалу.
        if crate::tty_guard::looks_interactive(command) {
            let wd = self.working_dir.get();
            return run_interactive(command, &wd).await;
        }

        let cwd = self.working_dir.get();
        match Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if output.status.success() {
                    ToolResult::success(stdout)
                } else {
                    // ИСПРАВЛЕНО (кривые ошибки "[внутренняя ошибка] " без
                    // содержания): если команда упала, но stderr пуст —
                    // показываем код выхода и хвост stdout; раньше в чат
                    // уходила пустышка, которую не разобрать.
                    if !stderr.trim().is_empty() {
                        ToolResult::error(stderr)
                    } else if !stdout.trim().is_empty() {
                        let tail: String = stdout.trim().chars().rev().take(300).collect();
                        let tail: String = tail.chars().rev().collect();
                        ToolResult::error(format!(
                            "код выхода {:?}; вывод:\n{}",
                            output.status.code(),
                            tail
                        ))
                    } else {
                        ToolResult::error(format!(
                            "код выхода {:?} (ни stdout, ни stderr)",
                            output.status.code()
                        ))
                    }
                }
            }
            Err(e) => ToolResult::error(format!("Failed to execute command: {}", e)),
        }
    }

    fn name(&self) -> &'static str {
        "bash"
    }
}

/// Выполнить команду с полным доступом к реальному терминалу (не raw-mode,
/// не alternate screen, stdio унаследовано напрямую) — для sudo/ssh/su и
/// подобных. В отличие от обычного пути, вывод команды НЕ захватывается
/// (пользователь видит его напрямую на терминале, как обычно с sudo/ssh) —
/// возвращается только код завершения, а не текст stdout/stderr, потому что
/// эти каналы отданы напрямую пользователю, а не программе.
async fn run_interactive(command: &str, working_dir: &std::path::Path) -> ToolResult {
    let command = command.to_string();
    let working_dir = working_dir.to_path_buf();

    let status = tokio::task::spawn_blocking(move || {
        let label = format!("интерактивная команда — $ {}", command);
        crate::tty_guard::with_real_terminal(&label, || {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(&command)
                .current_dir(&working_dir)
                .status()
        })
    })
    .await;

    match status {
        Ok(Ok(exit_status)) => {
            if exit_status.success() {
                ToolResult::success("Команда выполнена (интерактивно, вывод показан напрямую в терминале).".to_string())
            } else {
                ToolResult::error(format!(
                    "Команда завершилась с ошибкой (код {:?}) — интерактивный вывод был показан напрямую в терминале выше.",
                    exit_status.code()
                ))
            }
        }
        Ok(Err(e)) => ToolResult::error(format!("Не удалось запустить интерактивную команду: {}", e)),
        Err(e) => ToolResult::error(format!("Ошибка фоновой задачи: {}", e)),
    }
}
