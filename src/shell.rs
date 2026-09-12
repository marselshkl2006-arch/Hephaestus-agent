//! Shell abstraction — кроссплатформенная оболочка для BashTool.
//!
//! ПОРТ НА WINDOWS: на Linux/macOS — bash; на Windows — детект по цепочке
//! `pwsh (PowerShell 7) → powershell (5.1) → cmd`. Модель сообщается
//! актуальная оболочка через system_prompt (system_prompt::EnvExtras.shell),
//! поэтому она генерирует команды под правильный синтаксис.
//!
//! Экранирование: каждая оболочка имеет свои правила — они собраны здесь,
//! чтобы инструменты не дублировали.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    PowerShell,   // pwsh 7+ или powershell 5.1 — синтаксис один, бинарник разный
    Cmd,
}

impl ShellKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::PowerShell => "powershell",
            ShellKind::Cmd => "cmd",
        }
    }

    /// Детект оболочки ОДИН РАЗ за процесс (дорогое существование-поиск).
    pub fn detect() -> ShellKind {
        use std::sync::OnceLock;
        static DETECTED: OnceLock<ShellKind> = OnceLock::new();
        *DETECTED.get_or_init(|| {
            if cfg!(windows) {
                // pwsh 7 → powershell 5.1 → cmd.
                if which_exists("pwsh") {
                    ShellKind::PowerShell
                } else if which_exists("powershell") {
                    ShellKind::PowerShell
                } else {
                    ShellKind::Cmd
                }
            } else {
                ShellKind::Bash
            }
        })
    }

    /// Человекочитаемое имя для system prompt / /doctor.
    pub fn display_name(&self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::PowerShell => "PowerShell",
            ShellKind::Cmd => "cmd.exe",
        }
    }
}

/// Есть ли бинарник в PATH (лёгкий аналог which, без внешних вызовов).
fn which_exists(name: &str) -> bool {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let ext = if cfg!(windows) { ".exe" } else { "" };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(format!("{name}{ext}"));
        if candidate.is_file() {
            return true;
        }
    }
    false
}

/// Аргументы запуска КОМАНДНОЙ СТРОКИ через данную оболочку:
/// (программа, префикс-аргументы).
fn spawn_args(kind: ShellKind, command: &str) -> (String, Vec<String>) {
    match kind {
        ShellKind::Bash => ("bash".to_string(), vec!["-c".to_string(), command.to_string()]),
        ShellKind::PowerShell => (
            "powershell".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
            // pwsh 7 предпочтительнее, но powershell.exe есть всегда —
            // синтаксис команд общий, скорость не критична.
        ),
        ShellKind::Cmd => ("cmd".to_string(), vec!["/C".to_string(), command.to_string()]),
    }
}

/// Выполнить командную строку в заданной оболочке, захватывая stdout/stderr.
/// Интерактивные команды (sudo/ssh) — отдельный путь tty_guard, НЕ сюда.
pub async fn run_captured(
    kind: ShellKind,
    command: &str,
    cwd: &Path,
) -> std::io::Result<(bool, String, String)> {
    let (program, args) = spawn_args(kind, command);
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stdout, stderr))
}

/// Экранирование ОДНОГО аргумента для командной строки данной оболочки
/// (используется инструментами, собирающими команды из кусочков).
pub fn quote_arg(kind: ShellKind, arg: &str) -> String {
    match kind {
        // bash: одинарные кавычки, ' внутри → '\''.
        ShellKind::Bash => format!("'{}'", arg.replace('\'', r"'\''")),
        // PowerShell: одинарные кавычки, ' внутри → ''.
        ShellKind::PowerShell => format!("'{}'", arg.replace('\'', "''")),
        // cmd: двойные кавычки, "^ внутри не экранируется надёжно —
        // минимум: кавычки удвоить нечем, поэтому запрещаем вложенные.
        ShellKind::Cmd => format!("\"{}\"", arg.replace('"', "")),
    }
}

/// Команды, которым нужен РЕАЛЬНЫЙ терминал (пароль), по оболочкам.
/// bash: sudo/su/ssh; PowerShell: ssh, а sudo нет (UAC — отдельная история).
pub fn looks_interactive(kind: ShellKind, command: &str) -> bool {
    let first = command.trim().split_whitespace().next().unwrap_or("").to_lowercase();
    let base = first.rsplit(['/', '\\']).next().unwrap_or(&first);
    match kind {
        ShellKind::Bash => matches!(base, "sudo" | "su" | "ssh" | "passwd" | "visudo"),
        ShellKind::PowerShell => matches!(base, "ssh" | "pscp" | "sftp"),
        ShellKind::Cmd => matches!(base, "ssh"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_valid_shell() {
        let k = ShellKind::detect();
        // На Linux — bash; на Windows — один из двух. Тест проверяет инвариант.
        if cfg!(windows) {
            assert!(matches!(k, ShellKind::PowerShell | ShellKind::Cmd));
        } else {
            assert_eq!(k, ShellKind::Bash);
        }
    }

    #[test]
    fn quoting_per_shell() {
        assert_eq!(quote_arg(ShellKind::Bash, "it's"), r"'it'\''s'");
        assert_eq!(quote_arg(ShellKind::PowerShell, "it's"), "'it''s'");
        assert_eq!(quote_arg(ShellKind::Cmd, "a\"b"), "\"ab\"");
    }

    #[test]
    fn interactive_detection() {
        assert!(looks_interactive(ShellKind::Bash, "sudo apt install x"));
        assert!(looks_interactive(ShellKind::Bash, "/usr/bin/ssh host"));
        assert!(!looks_interactive(ShellKind::Bash, "ls -la"));
        assert!(looks_interactive(ShellKind::PowerShell, "ssh admin@host"));
        assert!(!looks_interactive(ShellKind::PowerShell, "sudo x"), "sudo нет на Windows");
    }

    #[tokio::test]
    async fn run_captured_works_on_current_platform() {
        let k = ShellKind::detect();
        let (ok, out, _err) = match k {
            ShellKind::Bash => run_captured(k, "echo hello", std::path::Path::new(".")).await.unwrap(),
            ShellKind::PowerShell => run_captured(k, "Write-Output hello", std::path::Path::new(".")).await.unwrap(),
            ShellKind::Cmd => run_captured(k, "echo hello", std::path::Path::new(".")).await.unwrap(),
        };
        assert!(ok);
        assert!(out.trim().contains("hello"), "out: {out:?}");
    }
}
