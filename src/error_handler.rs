//! Error Handler — аналог `error_handler.py`.
//!
//! Проблема, которую он решает: почти каждый инструмент (`bash`, `http`,
//! `docker`, `github`, ...) сейчас просто прокидывает наружу сырой
//! `stderr`/текст исключения в `ToolResult::error(...)`. Модели (и
//! человеку в логе) это не говорит, стоит ли повторить попытку, подождать,
//! или это вообще бессмысленно повторять (например 404 vs таймаут сети).
//! `classify()` даёт грубую, но полезную категорию по тексту ошибки —
//! без парсинга кодов возврата каждой отдельной программы — и
//! `is_retryable()`/`with_retry()` дают готовый метод "попробовать ещё раз
//! с задержкой", который инструменты могут подключить по желанию, не
//! переписывая свою логику с нуля.

use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Сеть недоступна / DNS / connection refused / timeout на уровне TCP.
    Network,
    /// HTTP/операция специально сказала "подожди" (429, 503) или процесс
    /// превысил заданный таймаут выполнения.
    Timeout,
    /// Нет прав (403, EACCES, "permission denied", "sudo").
    Permission,
    /// Искомого не существует (404, "no such file", "not found").
    NotFound,
    /// Вход был некорректным (400, "invalid", "parse error") — повтор без
    /// изменения аргументов не поможет.
    Validation,
    /// Всё остальное — неизвестная/внутренняя ошибка.
    Internal,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ErrorCategory::Network => "сеть",
            ErrorCategory::Timeout => "таймаут/перегрузка",
            ErrorCategory::Permission => "нет прав",
            ErrorCategory::NotFound => "не найдено",
            ErrorCategory::Validation => "некорректный ввод",
            ErrorCategory::Internal => "внутренняя ошибка",
        };
        write!(f, "{s}")
    }
}

impl ErrorCategory {
    /// Есть ли смысл повторить операцию (с задержкой), не меняя аргументы.
    pub fn is_retryable(&self) -> bool {
        matches!(self, ErrorCategory::Network | ErrorCategory::Timeout)
    }
}

/// Классифицирует ошибку по тексту — грубая эвристика на ключевых словах,
/// намеренно не привязанная к конкретному инструменту, чтобы работать
/// одинаково для stderr от bash, текста reqwest::Error и т.п.
pub fn classify(message: &str) -> ErrorCategory {
    let m = message.to_lowercase();

    let has_any = |needles: &[&str]| needles.iter().any(|n| m.contains(n));

    if has_any(&["econnrefused", "connection refused", "dns", "network is unreachable", "could not resolve", "no route to host"]) {
        ErrorCategory::Network
    } else if has_any(&["timed out", "timeout", "429", "too many requests", "503", "service unavailable"]) {
        ErrorCategory::Timeout
    } else if has_any(&["permission denied", "eacces", "forbidden", "403", "access denied", "not permitted"]) {
        ErrorCategory::Permission
    } else if has_any(&["no such file", "not found", "404", "does not exist", "enoent"]) {
        ErrorCategory::NotFound
    } else if has_any(&["invalid", "parse error", "400", "bad request", "malformed", "unexpected token"]) {
        ErrorCategory::Validation
    } else {
        ErrorCategory::Internal
    }
}

/// Готовое к показу пользователю/модели сообщение — ЧЕЛОВЕЧЕСКИЙ вид:
/// заголовок с сутью, детали строкой, подсказка при временной проблеме.
/// Раньше было "[внутренняя ошибка] " c пустым хвостом — в чат летел
/// нечитаемый мусор (жалоба "предупреждение не разобрать").
pub fn friendly_message(raw_error: &str) -> String {
    let trimmed = raw_error.trim();
    if trimmed.is_empty() {
        return "❌ Команда завершилась с ошибкой и не вывела подробностей \
                (ненулевой код выхода). Загляните в логи или попробуйте иначе."
            .to_string();
    }

    let category = classify(trimmed);
    let title = match category {
        ErrorCategory::Network => "🌐 Сетевая проблема",
        ErrorCategory::Timeout => "⏳ Таймаут или перегрузка",
        ErrorCategory::Permission => "🔒 Нет прав доступа",
        ErrorCategory::NotFound => "🔍 Не найдено",
        ErrorCategory::Validation => "📝 Некорректный ввод",
        ErrorCategory::Internal => "⚙️ Ошибка выполнения",
    };

    // Сырой текст одной строкой; длинные дампы обрезаем по СИМВОЛАМ,
    // чтобы не разрезать UTF-8 посередине (кириллица!).
    let one_line: String = {
        let flat = trimmed.replace('\n', " · ");
        let mut s: String = flat.chars().take(400).collect();
        if flat.chars().count() > 400 {
            s.push('…');
        }
        s
    };

    let hint = if category.is_retryable() {
        "\n💡 Похоже на временную проблему — можно повторить попытку."
    } else {
        ""
    };

    format!("{title}\n{one_line}{hint}")
}

/// Повторяет асинхронную операцию до `max_attempts` раз с экспоненциальной
/// задержкой (200ms, 400ms, 800ms, ...), но только если классификатор
/// считает ошибку retryable — иначе возвращает первую же ошибку без
/// лишних попыток (нет смысла повторять "404" три раза).
///
/// ```ignore
/// let body = with_retry(3, || http_tool.get(url)).await?;
/// ```
pub async fn with_retry<F, Fut, T>(max_attempts: u32, mut op: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let category = classify(&e);
                if attempt >= max_attempts || !category.is_retryable() {
                    return Err(friendly_message(&e));
                }
                let delay = Duration::from_millis(200 * 2u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_cases() {
        assert_eq!(classify("Connection refused (os error 111)"), ErrorCategory::Network);
        assert_eq!(classify("HTTP 429 Too Many Requests"), ErrorCategory::Timeout);
        assert_eq!(classify("Permission denied"), ErrorCategory::Permission);
        assert_eq!(classify("No such file or directory"), ErrorCategory::NotFound);
        assert_eq!(classify("Invalid argument: expected string"), ErrorCategory::Validation);
        assert_eq!(classify("something exploded"), ErrorCategory::Internal);
    }
}
