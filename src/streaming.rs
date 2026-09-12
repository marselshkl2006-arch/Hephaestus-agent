//! Streaming Response — потоковая передача ответа LLM по мере генерации.
//! Порт `streaming.py` (итерация 6). Как и в оригинале, честно
//! реализован НАСТОЯЩИЙ streaming только для Ollama (единственный
//! провайдер, для которого `streaming.py` тоже не был заглушкой —
//! `_stream_openai`/`_stream_anthropic` в оригинале сами были no-op
//! обёртками вокруг обычного `complete()` с последующей псевдо-разбивкой
//! на чанки). Для всех остальных провайдеров здесь — то же самое:
//! обычный `LLMClient::complete()`, результат режется на чанки и
//! отдаётся вызывающему коду с искусственной задержкой, чтобы UX
//! (постепенный вывод текста в REPL) выглядел одинаково независимо от
//! провайдера.
//!
//! НЕ подключено в основной цикл `Agent::chat()` в `main.rs` — тот
//! всегда использует `complete_with_tools()` (нужно для нативного tool
//! calling), а стриминг токен-за-токеном и параллельное выполнение
//! инструментов из недописанного потока — взаимоисключающие вещи без
//! существенного изменения основного цикла (см. также
//! `streaming_tool_calling.rs`, который решает именно эту задачу для
//! моделей без API tool-calling, но тоже не встроен в главный цикл —
//! по той же причине, что и `parser.rs`, см. STATUS_RU.md).

use futures_util::StreamExt;
use std::time::Duration;

use crate::llm::{LLMClient, LLMMessage};

/// Потоково стримит ответ Ollama по HTTP (NDJSON-чанки), вызывая
/// `on_token` для каждого куска текста. Возвращает полный склеенный
/// ответ. При любой ошибке сети — тихий fallback на `stream_emulated`.
pub async fn stream_ollama<F: FnMut(&str)>(
    base_url: &str,
    model: &str,
    messages: &[LLMMessage],
    system: Option<&str>,
    mut on_token: F,
) -> String {
    let client = reqwest::Client::new();
    let url = format!("{}/api/chat", base_url);

    let mut ollama_messages: Vec<serde_json::Value> = Vec::new();
    if let Some(s) = system {
        ollama_messages.push(serde_json::json!({"role": "system", "content": s}));
    }
    for m in messages {
        ollama_messages.push(serde_json::json!({"role": m.role, "content": m.content}));
    }

    let body = serde_json::json!({
        "model": model,
        "messages": ollama_messages,
        "stream": true,
    });

    let resp = match client.post(&url).json(&body).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return String::new(),
    };

    let mut full = String::new();
    let mut byte_stream = resp.bytes_stream();
    let mut leftover = String::new();

    while let Some(chunk) = byte_stream.next().await {
        let Ok(bytes) = chunk else { break };
        leftover.push_str(&String::from_utf8_lossy(&bytes));

        // Ollama шлёт NDJSON — один JSON-объект на строку.
        while let Some(pos) = leftover.find('\n') {
            let line = leftover[..pos].trim().to_string();
            leftover.drain(..=pos);
            if line.is_empty() {
                continue;
            }
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(token) = data["message"]["content"].as_str() {
                    if !token.is_empty() {
                        on_token(token);
                        full.push_str(token);
                    }
                }
                if data["done"].as_bool() == Some(true) {
                    return full;
                }
            }
        }
    }
    full
}

/// Разбить готовый текст на чанки по `chunk_size` слов и "проиграть" их
/// вызывающему коду с искусственной задержкой — эмуляция streaming для
/// провайдеров без нативной потоковой выдачи (см. заголовок файла).
pub async fn stream_emulated<F: FnMut(&str)>(text: &str, chunk_size: usize, mut on_chunk: F) {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return;
    }
    let mut i = 0;
    while i < words.len() {
        let end = (i + chunk_size).min(words.len());
        let mut chunk = words[i..end].join(" ");
        if end < words.len() {
            chunk.push(' ');
        }
        on_chunk(&chunk);
        tokio::time::sleep(Duration::from_millis(50)).await;
        i = end;
    }
}

/// Получить ответ LLM с потоковым выводом. Для Ollama — настоящий
/// streaming, для остальных провайдеров — обычный `complete()` +
/// пост-фактум эмуляция чанков (см. заголовок файла — так делал и
/// оригинал).
pub async fn complete_streaming<F: FnMut(&str)>(
    client: &dyn LLMClient,
    provider_name: &str,
    base_url: Option<&str>,
    model: &str,
    messages: &[LLMMessage],
    system: Option<&str>,
    mut on_token: F,
) -> Result<String, crate::llm::LLMError> {
    if provider_name == "ollama" {
        let url = base_url.unwrap_or("http://localhost:11434");
        let text = stream_ollama(url, model, messages, system, &mut on_token).await;
        if !text.is_empty() {
            return Ok(text);
        }
        // Streaming не удался (сеть/парсинг) — падаем на обычный запрос.
    }

    let response = client.complete(messages, system).await?;
    stream_emulated(&response.content, 5, &mut on_token).await;
    Ok(response.content)
}
