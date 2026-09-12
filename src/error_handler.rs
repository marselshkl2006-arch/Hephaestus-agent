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
    /// LLM: аутентификация/ключ (401, invalid api key).
    Auth,
    /// LLM: квоты/биллинг (402, insufficient credits).
    Billing,
    /// LLM: лимит частоты запросов ИМЕННО от провайдера/шлюза (429).
    RateLimit,
    /// LLM: переполнение контекста ("context length exceeded") — лечится
    /// сжатием истории, а не повтором "как есть".
    ContextOverflow,
    /// Всё остальное — неизвестная/внутренняя ошибка.
    Internal,
}

/// Политика реакции НА УРОВНЕ ХОДА агента (не отдельного инструмента):
/// что делать с ошибкой LLM-запроса. По образцу error_classifier.py
/// Hermes: категория диктует ДЕЙСТВИЕ, а не просто yes/no на "ретрай".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Повторить с backoff (сеть/таймаут/перегрузка/429) — до 3 попыток.
    Retry,
    /// Сжать контекст и повторить — переполнение окна не лечится ожиданием.
    CompressAndRetry,
    /// Не повторять: ошибка воспроизводится идентично (401/402/404/400).
    FailFast,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ErrorCategory::Network => "сеть",
            ErrorCategory::Timeout => "таймаут/перегрузка",
            ErrorCategory::Permission => "нет прав",
            ErrorCategory::NotFound => "не найдено",
            ErrorCategory::Validation => "некорректный ввод",
            ErrorCategory::Auth => "аутентификация",
            ErrorCategory::Billing => "квоты/биллинг",
            ErrorCategory::RateLimit => "лимит запросов (429)",
            ErrorCategory::ContextOverflow => "переполнение контекста",
            ErrorCategory::Internal => "внутренняя ошибка",
        };
        write!(f, "{s}")
    }
}

impl ErrorCategory {
    /// Есть ли смысл повторить операцию (с задержкой), не меняя аргументы.
    /// Для инструментов (умеренный набор: сеть/таймаут).
    pub fn is_retryable(&self) -> bool {
        matches!(self, ErrorCategory::Network | ErrorCategory::Timeout)
    }

    /// Политика для LLM-запросов (шире инструментальной: сюда добавляются
    /// аут/биллинг/429/переполнение контекста).
    pub fn llm_policy(&self) -> Policy {
        match self {
            ErrorCategory::Network | ErrorCategory::Timeout | ErrorCategory::RateLimit => Policy::Retry,
            ErrorCategory::ContextOverflow => Policy::CompressAndRetry,
            _ => Policy::FailFast,
        }
    }
}

/// Классификация ОШИБОК LLM-ЗАПРОСА: те же слова, что classify(), плюс
/// LLM-специфичные паттерны (401/ключи, 402/квоты, 429, context window).
/// Порядок проверок важен: специфичное раньше общего ("context length
/// exceeded" содержит и "400", но категория — переполнение, не ввод).
pub fn classify_llm(message: &str) -> ErrorCategory {
    let m = message.to_lowercase();
    let has_any = |needles: &[&str]| needles.iter().any(|n| m.contains(n));

    if has_any(&[
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "prompt is too long",
        "too many tokens",
        "input too long",
        "input length exceeds",
        "reduce the length",
    ]) {
        ErrorCategory::ContextOverflow
    } else if has_any(&["401", "unauthorized", "invalid api key", "invalid_api_key", "authentication", "incorrect api key"]) {
        ErrorCategory::Auth
    } else if has_any(&["402", "insufficient", "quota exceeded", "billing", "credit balance", "exceeded your current quota"]) {
        ErrorCategory::Billing
    } else if has_any(&["429", "too many requests", "rate limit", "rate_limit"]) {
        ErrorCategory::RateLimit
    } else {
        classify(message)
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
        ErrorCategory::Auth => "🔑 Ошибка аутентификации",
        ErrorCategory::Billing => "💳 Квоты/биллинг",
        ErrorCategory::RateLimit => "🚦 Лимит запросов (429)",
        ErrorCategory::ContextOverflow => "📚 Переполнение контекста",
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

    #[test]
    fn classifies_llm_specific_errors() {
        assert_eq!(classify_llm("HTTP 401 Unauthorized: invalid api key"), ErrorCategory::Auth);
        assert_eq!(classify_llm("402 Payment Required: credit balance too low"), ErrorCategory::Billing);
        assert_eq!(classify_llm("HTTP 429: rate limit exceeded"), ErrorCategory::RateLimit);
        // Специфичное раньше общего: в строке есть и "400", но категория — окно.
        assert_eq!(
            classify_llm("400 This model's maximum context length is 8192 tokens"),
            ErrorCategory::ContextOverflow
        );
        assert_eq!(classify_llm("prompt is too long: 34000 tokens > 8192"), ErrorCategory::ContextOverflow);
        assert_eq!(classify_llm("Connection refused"), ErrorCategory::Network);
    }

    #[test]
    fn policies_match_categories() {
        assert_eq!(classify_llm("429").llm_policy(), Policy::Retry);
        assert_eq!(classify_llm("timed out").llm_policy(), Policy::Retry);
        assert_eq!(classify_llm("maximum context length").llm_policy(), Policy::CompressAndRetry);
        assert_eq!(classify_llm("401 unauthorized").llm_policy(), Policy::FailFast);
        assert_eq!(classify_llm("insufficient quota").llm_policy(), Policy::FailFast);
        assert_eq!(classify_llm("HTTP 404 model not found").llm_policy(), Policy::FailFast);
    }
}
