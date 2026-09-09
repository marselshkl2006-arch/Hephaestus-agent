use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// Глобальный флаг автоматического подтверждения
static AUTO_CONFIRM: AtomicBool = AtomicBool::new(false);

/// Установить режим автоматического подтверждения
pub fn set_auto_confirm(enabled: bool) {
    AUTO_CONFIRM.store(enabled, Ordering::SeqCst);
}

/// Проверить, включён ли режим автоматического подтверждения
pub fn is_auto_confirm() -> bool {
    AUTO_CONFIRM.load(Ordering::SeqCst)
}

/// Запросить подтверждение у пользователя
/// 
/// # Arguments
/// * `prompt` - Текст запроса
/// * `default` - Значение по умолчанию (true = да, false = нет)
/// 
/// # Returns
/// `true` если пользователь подтвердил, `false` если отказался
pub fn ask_confirmation(prompt: &str, default: bool) -> bool {
    if is_auto_confirm() {
        return true;
    }

    let default_str = if default { "Y/n" } else { "y/N" };
    print!("{} [{}]: ", prompt, default_str);
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    let input = input.trim().to_lowercase();

    if input.is_empty() {
        return default;
    }

    matches!(input.as_str(), "y" | "yes" | "да" | "д" | "+")
}

/// Запросить подтверждение для опасной операции
pub fn ask_dangerous(prompt: &str) -> bool {
    if is_auto_confirm() {
        return true;
    }

    print!("⚠️  {} (y/N): ", prompt);
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    let input = input.trim().to_lowercase();

    matches!(input.as_str(), "y" | "yes" | "да" | "д")
}

/// Запросить текстовый ввод от пользователя
pub fn ask_input(prompt: &str) -> String {
    print!("{}: ", prompt);
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    input.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_confirm() {
        set_auto_confirm(true);
        assert!(is_auto_confirm());
        assert!(ask_confirmation("Test", false));
        set_auto_confirm(false);
        assert!(!is_auto_confirm());
    }
}
