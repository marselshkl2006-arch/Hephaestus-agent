//! Web Surfer — минимальный набор веб-инструментов на чистом Rust:
//! `web_fetch` (скачать страницу и вытащить текст) и `web_search`
//! (поиск через HTML-версию DuckDuckGo, без API-ключа).
//!
//! Никаких Python-обёрток и headless-браузеров — только `reqwest` + `regex`,
//! которые уже есть в зависимостях проекта. Это "достаточно хороший" surfer
//! для агента (получить текст страницы / найти ссылки по запросу), а не
//! полноценный браузерный движок.

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

use super::{Tool, ToolResult};

const MAX_OUTPUT_CHARS: usize = 8000;

fn strip_html(html: &str) -> String {
    static SCRIPT_RE: OnceLock<Regex> = OnceLock::new();
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    static WS_RE: OnceLock<Regex> = OnceLock::new();

    let script_re = SCRIPT_RE
        .get_or_init(|| Regex::new(r"(?is)<(script|style)[^>]*>.*?</(script|style)>").unwrap());
    let tag_re = TAG_RE.get_or_init(|| Regex::new(r"(?s)<[^>]+>").unwrap());
    let ws_re = WS_RE.get_or_init(|| Regex::new(r"[ \t]+").unwrap());

    let no_scripts = script_re.replace_all(html, " ");
    let no_tags = tag_re.replace_all(&no_scripts, " ");
    let decoded = no_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let collapsed = ws_re.replace_all(&decoded, " ");
    collapsed
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}\n… [обрезано, показаны первые {} символов]", truncated, max_chars)
    }
}

/// Скачать страницу и вернуть очищенный от HTML-тегов текст.
pub struct WebFetchTool {
    client: reqwest::Client,
}

impl WebFetchTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; HephaestusAgent/1.0; +https://example.local)")
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
        if url.is_empty() {
            return ToolResult::error("Требуется параметр 'url'");
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return ToolResult::error("URL должен начинаться с http:// или https://");
        }

        let resp = match self.client.get(url).send().await {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Ошибка запроса: {}", e)),
        };

        let status = resp.status();
        if !status.is_success() {
            return ToolResult::error(format!("HTTP {}", status.as_u16()));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => return ToolResult::error(format!("Ошибка чтения тела ответа: {}", e)),
        };

        let text = if content_type.contains("html") {
            strip_html(&body)
        } else {
            body
        };

        ToolResult::success(truncate(&text, MAX_OUTPUT_CHARS))
    }


    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"url":{"type":"string"},"query":{"type":"string"},"max_results":{"type":"integer"}}})
    }

    fn name(&self) -> &'static str {
        "web_fetch"
    }
}

/// Поиск через HTML-версию DuckDuckGo (html.duckduckgo.com) — не требует API-ключа.
/// Возвращает список найденных заголовков + ссылок (текстом, без JS-рендеринга).
pub struct WebSearchTool {
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; HephaestusAgent/1.0; +https://example.local)")
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.is_empty() {
            return ToolResult::error("Требуется параметр 'query'");
        }
        let max_results = args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let resp = match self
            .client
            .get("https://html.duckduckgo.com/html/")
            .query(&[("q", query)])
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Ошибка запроса к поисковику: {}", e)),
        };

        if !resp.status().is_success() {
            return ToolResult::error(format!("HTTP {}", resp.status().as_u16()));
        }

        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => return ToolResult::error(format!("Ошибка чтения ответа: {}", e)),
        };

        static RESULT_RE: OnceLock<Regex> = OnceLock::new();
        let result_re = RESULT_RE.get_or_init(|| {
            Regex::new(r#"(?is)<a[^>]+class="result__a"[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#)
                .unwrap()
        });

        let mut lines = Vec::new();
        for cap in result_re.captures_iter(&body).take(max_results) {
            let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let title_html = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            let title = strip_html(title_html);
            if !href.is_empty() && !title.is_empty() {
                lines.push(format!("- {}\n  {}", title, href));
            }
        }

        if lines.is_empty() {
            ToolResult::success("Ничего не найдено (или DuckDuckGo изменил разметку страницы)".to_string())
        } else {
            ToolResult::success(lines.join("\n"))
        }
    }

    fn name(&self) -> &'static str {
        "web_search"
    }
}
