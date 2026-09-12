use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LLMProvider {
    Ollama,
    OpenAI,
    Anthropic,
    OpenRouter,
    KoboldCpp,
    LlamaServer,
    Custom,
}

impl LLMProvider {
    /// Строковое имя провайдера для меток метрик/логов (`monitoring.rs`,
    /// `logging_system.rs`) — раньше `Agent::new()` жёстко считал, что
    /// провайдер всегда Ollama (см. фикс в `Agent::new()`, main.rs), так
    /// что нигде и не было нужды печатать имя другого провайдера.
    pub fn as_str(&self) -> &'static str {
        match self {
            LLMProvider::Ollama => "ollama",
            LLMProvider::OpenAI => "openai",
            LLMProvider::Anthropic => "anthropic",
            LLMProvider::OpenRouter => "openrouter",
            LLMProvider::KoboldCpp => "koboldcpp",
            LLMProvider::LlamaServer => "llama_server",
            LLMProvider::Custom => "custom",
        }
    }

    /// Разбор имени провайдера из пользовательского ввода (REPL `/provider`,
    /// `config.rs`) — регистронезависимо, принимает пару обиходных
    /// синонимов ("local"/"локальная" → Ollama).
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "ollama" | "local" | "локальная" | "локальный" => Some(LLMProvider::Ollama),
            "openai" | "gpt" => Some(LLMProvider::OpenAI),
            "anthropic" | "claude" => Some(LLMProvider::Anthropic),
            "openrouter" => Some(LLMProvider::OpenRouter),
            "koboldcpp" | "kobold" => Some(LLMProvider::KoboldCpp),
            "llama_server" | "llamaserver" | "llama.cpp" | "llamacpp" => Some(LLMProvider::LlamaServer),
            "custom" => Some(LLMProvider::Custom),
            _ => None,
        }
    }

    pub fn all() -> &'static [LLMProvider] {
        &[
            LLMProvider::Anthropic,
            LLMProvider::OpenAI,
            LLMProvider::OpenRouter,
            LLMProvider::Ollama,
            LLMProvider::KoboldCpp,
            LLMProvider::LlamaServer,
            LLMProvider::Custom,
        ]
    }

    /// Человеко-читаемое имя для меню `/provider` в REPL.
    pub fn display_name(&self) -> &'static str {
        match self {
            LLMProvider::Ollama => "Ollama (локальная)",
            LLMProvider::OpenAI => "OpenAI",
            LLMProvider::Anthropic => "Anthropic",
            LLMProvider::OpenRouter => "OpenRouter",
            LLMProvider::KoboldCpp => "KoboldCPP (локальная)",
            LLMProvider::LlamaServer => "llama.cpp server (локальная)",
            LLMProvider::Custom => "Custom (свой OpenAI-совместимый эндпоинт)",
        }
    }

    /// Имя переменной окружения с API-ключом для этого провайдера.
    /// `None` — провайдер ключ не требует (локальные).
    pub fn env_key_var(&self) -> Option<&'static str> {
        match self {
            LLMProvider::Anthropic => Some("ANTHROPIC_API_KEY"),
            LLMProvider::OpenAI => Some("OPENAI_API_KEY"),
            LLMProvider::OpenRouter => Some("OPENROUTER_API_KEY"),
            LLMProvider::Ollama | LLMProvider::KoboldCpp | LLMProvider::LlamaServer => None,
            LLMProvider::Custom => Some("CUSTOM_API_KEY"),
        }
    }

    /// Дефолтный base_url для локальных провайдеров ("" — берётся
    /// API-провайдером по умолчанию, base_url не нужен).
    pub fn default_base_url(&self) -> Option<&'static str> {
        match self {
            LLMProvider::Ollama => Some("http://localhost:11434"),
            LLMProvider::KoboldCpp => Some("http://localhost:5001"),
            LLMProvider::LlamaServer => Some("http://localhost:8080"),
            // ИСПРАВЛЕНО: раньше None — и OpenRouter не имел рабочего
            // клиента вообще (заглушка "not implemented"). Теперь он
            // обслуживается стандартным OpenAI-совместимым клиентом.
            LLMProvider::OpenRouter => Some("https://openrouter.ai/api/v1"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolUseBlock>>,
    /// Для role=="tool": id вызова, на который это ответ (формат OpenAI
    /// требует tool_call_id в каждом tool-сообщении; Ollama — нет, там
    /// поле просто не сериализуется наружу).
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

impl LLMMessage {
    pub fn user(content: String) -> Self {
        Self { role: "user".to_string(), content, tool_calls: None, tool_call_id: None }
    }

    pub fn assistant(content: String) -> Self {
        Self { role: "assistant".to_string(), content, tool_calls: None, tool_call_id: None }
    }

    pub fn system(content: String) -> Self {
        Self { role: "system".to_string(), content, tool_calls: None, tool_call_id: None }
    }

    /// Ответ на вызов инструмента — role:"tool" с id исходного вызова.
    pub fn tool_result(call_id: String, content: String) -> Self {
        Self { role: "tool".to_string(), content, tool_calls: None, tool_call_id: Some(call_id) }
    }


    /// Assistant-сообщение с вызовами инструментов (с РЕАЛЬНЫМИ аргументами
    /// — раньше реконструировалось с input:{} и модель видела вызовы без
    /// параметров, сбиваясь при продолжении цепочки).
    pub fn assistant_with_tools(content: String, calls: Vec<ToolUseBlock>) -> Self {
        Self { role: "assistant".to_string(), content, tool_calls: Some(calls), tool_call_id: None }
    }


}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub content: String,
    pub stop_reason: String,
    pub usage: HashMap<String, u64>,
    pub model: String,
    #[serde(default)]
    pub tool_use_blocks: Vec<ToolUseBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone)]
pub struct LLMConfig {
    pub provider: LLMProvider,
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub stream: bool,
    /// Дополнительные заголовки профиля подключения (например, proxy/WAF
    /// auth). Применяются после Bearer, поэтому осознанно могут его заменить.
    pub extra_headers: HashMap<String, String>,
    /// Дополнительные поля OpenAI-compatible JSON-запроса. Нужны шлюзам с
    /// vendor-специфичными параметрами; базовые поля протокола защищены.
    pub extra_body: Option<Value>,
}

#[async_trait::async_trait]
pub trait LLMClient: Send + Sync {
    async fn complete(&self, messages: &[LLMMessage], system: Option<&str>) -> Result<LLMResponse, LLMError>;
    async fn complete_with_tools(
        &self,
        messages: &[LLMMessage],
        tools: &[Value],
        system: Option<&str>,
    ) -> Result<LLMResponse, LLMError>;

    /// Стриминговый вариант: текст отдаётся в on_text ПО МЕРЕ генерации
    /// (живой вывод в TUI), итоговый LLMResponse содержит полный текст и
    /// tool_calls. Дефолт — нестриминговый вызов с отдачей всего сразу:
    /// клиенты без поддержки стрима продолжают работать без изменений.
    async fn complete_with_tools_stream(
        &self,
        messages: &[LLMMessage],
        tools: &[Value],
        system: Option<&str>,
        on_text: std::sync::Arc<dyn Fn(String) + Send + Sync + 'static>,
    ) -> Result<LLMResponse, LLMError> {
        let r = self.complete_with_tools(messages, tools, system).await?;
        on_text(r.content.clone());
        Ok(r)
    }

    async fn chat(&self, messages: &[LLMMessage]) -> Result<LLMResponse, LLMError> {
        self.complete(messages, None).await
    }
}

#[derive(Error, Debug)]
pub enum LLMError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("API error: {0}")]
    Api(String),
    #[error("Configuration error: {0}")]
    Config(String),
}

pub fn auto_detect_provider() -> (LLMProvider, String) {
    if let Ok(_url) = env::var("LLAMA_SERVER_URL") {
        let model = env::var("LLAMA_MODEL").unwrap_or_else(|_| "qwen3.6-27b-uncensored".to_string());
        return (LLMProvider::LlamaServer, model);
    }
    if env::var("ANTHROPIC_API_KEY").is_ok() {
        return (LLMProvider::Anthropic, "claude-3-5-sonnet-20241022".to_string());
    }
    if env::var("OPENROUTER_API_KEY").is_ok() {
        let model = env::var("OPENROUTER_MODEL").unwrap_or_else(|_| "qwen/qwen-2.5-coder-32b-instruct".to_string());
        return (LLMProvider::OpenRouter, model);
    }
    if env::var("OPENAI_API_KEY").is_ok() {
        return (LLMProvider::OpenAI, "gpt-4o".to_string());
    }
    if env::var("OLLAMA_HOST").is_ok() {
        let model = env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2:3b".to_string());
        return (LLMProvider::Ollama, model);
    }
    (LLMProvider::Ollama, "llama3.2:3b".to_string())
}

pub fn create_llm_client(config: LLMConfig) -> Box<dyn LLMClient> {
    match config.provider {
        LLMProvider::Ollama => Box::new(OllamaClient::new(config)),
        // Раньше LlamaServer/Custom/KoboldCpp сюда не попадали вообще
        // (ловились веткой `_` ниже и тихо превращались в OllamaClient
        // — другой эндпоинт /api/chat, другой формат тела запроса).
        // Все четыре здесь говорят ОДНИМ диалектом — стандартным OpenAI
        // Chat Completions API (/v1/chat/completions) — включая боевой
        // сетап (--provider custom --base-url .../v1), который раньше
        // никогда реально не работал бы против такого сервера.
        LLMProvider::OpenAI | LLMProvider::LlamaServer | LLMProvider::Custom | LLMProvider::KoboldCpp => {
            Box::new(OpenAICompatibleClient::new(config))
        }
        LLMProvider::OpenRouter => {
            // ИСПРАВЛЕНО (заглушка "not implemented"): OpenRouter говорит
            // СТАНДАРТНЫМ OpenAI-диалектом — обслуживается тем же
            // совместимым клиентом, отличается только base_url и парой
            // рекомендованных заголовков.
            let mut c = config.clone();
            if c.base_url.as_deref().unwrap_or("").trim().is_empty() {
                c.base_url = Some("https://openrouter.ai/api/v1".to_string());
            }
            Box::new(OpenAICompatibleClient::new(c))
        }
        LLMProvider::Anthropic => Box::new(AnthropicClient::new(config)),
    }
}

// Прокси-клиенты для разных провайдеров
pub struct OllamaClient {
    config: LLMConfig,
    client: reqwest::Client,
}

impl OllamaClient {
    pub fn new(config: LLMConfig) -> Self {
        Self {
            config,
            // ИСПРАВЛЕНО: было `reqwest::Client::new()` — без таймаута
            // вообще. Если Ollama зависает (например, первая загрузка
            // крупной модели в память/VRAM может занимать минуты, либо
            // порт занят другим процессом и соединение просто висит).
            // Запрос ждал бы БЕСКОНЕЧНО — для пользователя это выглядит
            // ровно как "агент не отвечает": спиннер крутится, ошибки
            // нет, ответа нет. 10 минут — с запасом на медленные
            // локальные модели без GPU, но не бесконечность.
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_default(),
        }
    }
}

impl OllamaClient {
    /// LLMMessage → формат истории нативного /api/chat. Отличия от
    /// OpenAI-диалекта: arguments — ОБЪЕКТ (не строка), tool-ответы без
    /// tool_call_id.
    fn build_ollama_messages(messages: &[LLMMessage], system: Option<&str>) -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        if let Some(system_text) = system {
            out.push(serde_json::json!({"role": "system", "content": system_text}));
        }
        for msg in messages {
            if msg.role == "tool" {
                out.push(serde_json::json!({"role": "tool", "content": msg.content}));
                continue;
            }
            match &msg.tool_calls {
                Some(calls) if !calls.is_empty() => {
                    let tc: Vec<Value> = calls
                        .iter()
                        .map(|c| serde_json::json!({
                            "function": {"name": c.name, "arguments": c.input},
                        }))
                        .collect();
                    out.push(serde_json::json!({
                        "role": "assistant",
                        "content": msg.content,
                        "tool_calls": tc,
                    }));
                }
                _ => {
                    if msg.role != "system" {
                        out.push(serde_json::json!({"role": msg.role, "content": msg.content}));
                    }
                }
            }
        }
        out
    }
}

#[async_trait::async_trait]
impl LLMClient for OllamaClient {
    async fn complete(&self, messages: &[LLMMessage], system: Option<&str>) -> Result<LLMResponse, LLMError> {
        let url = self.config.base_url.clone().unwrap_or_else(|| "http://localhost:11434".to_string());
        let url = format!("{}/api/chat", url);

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": Self::build_ollama_messages(messages, system),
            "stream": false,
            // ИСПРАВЛЕНО: было `"temperature": ..., "max_tokens": ...`
            // как ТОП-УРОВНЕВЫЕ поля тела запроса — Ollama `/api/chat`
            // ожидает их внутри `"options"` (и `max_tokens` там
            // называется `num_predict`, не `max_tokens`). Верхнеуровневые
            // поля, которых Ollama не знает, тихо игнорируются сервером —
            // значит temperature/лимит токенов реально никогда не
            // применялись, при этом никакой ошибки не было (оттого и
            // незаметно). Не является причиной "агент не отвечает" саму
            // по себе, но это реальная логическая ошибка рядом — раз уж
            // разбирался в этом файле, поправил и её.
            "options": {
                "temperature": self.config.temperature,
                "num_predict": self.config.max_tokens,
            },
        });

        let response = self.client.post(&url).json(&body).send().await?;
        let status = response.status();
        let raw_body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(LLMError::Api(format!(
                "Ollama HTTP {}: {}",
                status,
                raw_body.chars().take(500).collect::<String>()
            )));
        }

        let data: Value = serde_json::from_str(&raw_body)
            .map_err(|e| LLMError::Api(format!("Не удалось разобрать ответ Ollama как JSON: {} (тело: {})", e, raw_body.chars().take(300).collect::<String>())))?;

        let content = data["message"]["content"].as_str().unwrap_or("").to_string();
        let model = data["model"].as_str().unwrap_or("unknown").to_string();

        // ИСПРАВЛЕНО: раньше пустой content молча возвращался как
        // Ok(LLMResponse{content: "", ...}) — а REPL, в свою очередь,
        // для пустого текста не рисовал вообще НИ ОДНОЙ строки записи
        // (`entry.text.lines()` на пустой строке даёт 0 строк — см. фикс
        // в repl.rs), из-за чего ответ агента был НЕВИДИМ: спиннер
        // останавливался, счётчик LLM рос, а в чате не появлялось
        // ничего. Теперь пустой content — это явная ошибка с сырым
        // телом ответа (но ТОЛЬКО если нет tool_calls — при вызове
        // инструмента пустой текст норма).
        if content.is_empty() && data["message"]["tool_calls"].is_null() {
            return Err(LLMError::Api(format!(
                "Ollama вернул пустой ответ (модель '{}'). Сырое тело: {}",
                self.config.model,
                raw_body.chars().take(500).collect::<String>()
            )));
        }

        Ok(LLMResponse {
            content,
            stop_reason: "stop".to_string(),
            usage: HashMap::new(),
            model,
            tool_use_blocks: Vec::new(),
        })
    }

    /// ИСПРАВЛЕНО ("команды из telegram не доходят до машины"): раньше
    /// это был passthrough на complete() БЕЗ поля tools — модель на
    /// дефолтном провайдере физически не могла вызвать ни один
    /// инструмент и просто ОТВЕЧАЛА ТЕКСТОМ ("создаю файл..."), ничего
    /// не выполняя. Теперь — нативный tool-calling /api/chat:
    ///   • запрос: "tools" в том же формате схем, что и OpenAI
    ///     (Ollama его принимает как есть);
    ///   • ответ: message.tool_calls[].function.{name,arguments},
    ///     где arguments — ОБЪЕКТ (в отличие от строкового OpenAI);
    ///     id вызова Ollama не присылает — генерируем свой.
    async fn complete_with_tools(
        &self,
        messages: &[LLMMessage],
        tools: &[Value],
        system: Option<&str>,
    ) -> Result<LLMResponse, LLMError> {
        let url = self.config.base_url.clone().unwrap_or_else(|| "http://localhost:11434".to_string());
        let url = format!("{}/api/chat", url);

        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": Self::build_ollama_messages(messages, system),
            "stream": false,
            "options": {
                "temperature": self.config.temperature,
                "num_predict": self.config.max_tokens,
            },
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
        }

        let response = self.client.post(&url).json(&body).send().await?;
        let status = response.status();
        let raw_body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(LLMError::Api(format!(
                "Ollama HTTP {}: {}",
                status,
                raw_body.chars().take(500).collect::<String>()
            )));
        }

        let data: Value = serde_json::from_str(&raw_body)
            .map_err(|e| LLMError::Api(format!("Не удалось разобрать ответ Ollama как JSON: {} (тело: {})", e, raw_body.chars().take(300).collect::<String>())))?;

        let content = data["message"]["content"].as_str().unwrap_or("").to_string();
        let model = data["model"].as_str().unwrap_or("unknown").to_string();

        // tool_calls: arguments может прийти объектом ИЛИ строкой с JSON
        // (разные версии) — принимаем оба варианта; id генерируем сами.
        let mut call_idx = 0usize;
        let tool_use_blocks: Vec<ToolUseBlock> = data["message"]["tool_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|call| {
                        let name = call["function"]["name"].as_str()?.to_string();
                        let input = match call["function"]["arguments"] {
                            Value::Object(ref o) => Value::Object(o.clone()),
                            Value::String(ref s) => {
                                serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({}))
                            }
                            _ => serde_json::json!({}),
                        };
                        call_idx += 1;
                        Some(ToolUseBlock {
                            id: format!("ollama_call_{}", call_idx),
                            name,
                            input,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        if content.is_empty() && tool_use_blocks.is_empty() {
            return Err(LLMError::Api(format!(
                "Ollama вернул пустой ответ (модель '{}'). Сырое тело: {}",
                self.config.model,
                raw_body.chars().take(500).collect::<String>()
            )));
        }

        Ok(LLMResponse {
            content,
            stop_reason: if tool_use_blocks.is_empty() { "stop" } else { "tool_use" }.to_string(),
            usage: HashMap::new(),
            model,
            tool_use_blocks,
        })
    }

    /// Стриминг нативного /api/chat: NDJSON-строки
    /// {"message":{"content":"кусочек"},"done":false} ... финал с done:true.
    /// Текст отдаём в on_text по мере прихода; tool_calls (приходят
    /// обычно одним куском) собираем в итоговые блоки.
    async fn complete_with_tools_stream(
        &self,
        messages: &[LLMMessage],
        tools: &[Value],
        system: Option<&str>,
        on_text: std::sync::Arc<dyn Fn(String) + Send + Sync + 'static>,
    ) -> Result<LLMResponse, LLMError> {
        use futures_util::StreamExt;

        let url = self.config.base_url.clone().unwrap_or_else(|| "http://localhost:11434".to_string());
        let url = format!("{}/api/chat", url);

        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": Self::build_ollama_messages(messages, system),
            "stream": true,
            "options": {
                "temperature": self.config.temperature,
                "num_predict": self.config.max_tokens,
            },
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
        }

        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let t = resp.text().await.unwrap_or_default();
            return Err(LLMError::Api(format!("Ollama HTTP {status}: {}", crate::truncate_chars(&t, 300))));
        }

        let mut text_all = String::new();
        let mut model = self.config.model.clone();
        let mut calls: Vec<ToolUseBlock> = Vec::new();
        let mut call_idx = 0usize;

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(300), stream.next()).await {
                Ok(Some(Ok(chunk))) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(Some(Err(e))) => return Err(LLMError::Api(format!("поток Ollama: {e}"))),
                Ok(None) | Err(_) => break,
            }
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].to_string();
                buf.drain(..pos + 1);
                let trimmed = line.trim();
                if trimmed.is_empty() { continue; }
                let Ok(ev) = serde_json::from_str::<Value>(trimmed) else { continue };
                if let Some(m) = ev.get("model").and_then(|m| m.as_str()) { model = m.to_string(); }
                if let Some(c) = ev.pointer("/message/content").and_then(|c| c.as_str()) {
                    if !c.is_empty() {
                        text_all.push_str(c);
                        on_text(c.to_string());
                    }
                }
                if let Some(tcs) = ev.pointer("/message/tool_calls").and_then(|t| t.as_array()) {
                    for c in tcs {
                        let name = c.pointer("/function/name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        if name.is_empty() { continue; }
                        let input = match c.pointer("/function/arguments") {
                            Some(Value::Object(o)) => Value::Object(o.clone()),
                            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({})),
                            _ => serde_json::json!({}),
                        };
                        call_idx += 1;
                        calls.push(ToolUseBlock { id: format!("ollama_call_{call_idx}"), name, input });
                    }
                }
                if ev.get("done").and_then(|d| d.as_bool()).unwrap_or(false) { return Ok(LLMResponse {
                    content: text_all,
                    stop_reason: if calls.is_empty() { "stop" } else { "tool_use" }.to_string(),
                    usage: HashMap::new(),
                    model,
                    tool_use_blocks: calls,
                }); }
            }
        }

        Ok(LLMResponse {
            content: text_all,
            stop_reason: if calls.is_empty() { "stop" } else { "tool_use" }.to_string(),
            usage: HashMap::new(),
            model,
            tool_use_blocks: calls,
        })
    }
}

// ─────────────────────────────────────────────────────────────────
// OpenAICompatibleClient — РЕАЛЬНАЯ реализация для всех провайдеров,
// говорящих стандартным диалектом OpenAI Chat Completions API:
// OpenAI, LlamaServer, KoboldCpp (его OpenAI-совместимый режим) и
// Custom (произвольный self-hosted сервер с этим же форматом, тот же
// DeepSeek через --base-url http://localhost:9655/v1 из реального
// использования). Раньше LlamaServer/Custom/KoboldCpp вообще не имели
// своей ветки в create_llm_client() и тихо подменялись на OllamaClient
// (см. match ниже, было `_ => OllamaClient`) — у Ollama другой путь
// (/api/chat) и другой формат тела запроса, так что боевой сетап
// (--provider custom) фактически никогда не работал бы против
// OpenAI-совместимого сервера.
pub struct OpenAICompatibleClient {
    config: LLMConfig,
    client: reqwest::Client,
}

impl OpenAICompatibleClient {
    pub fn new(config: LLMConfig) -> Self {
        Self { config, client: reqwest::Client::new() }
    }

    /// Тестовый доступ к сборке истории (регресс-тест sanitizer-а
    /// осиротевших tool-результатов — см. integration_tests).
    #[cfg(test)]
    pub fn build_messages_for_test(&self, messages: &[LLMMessage], system: Option<&str>) -> Vec<Value> {
        self.build_messages(messages, system)
    }

    fn endpoint(&self) -> String {
        let base = self
            .config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let base = base.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        }
    }

    fn merge_profile_extra_body(&self, body: &mut Value) {
        let Some(extra) = self.config.extra_body.as_ref().and_then(Value::as_object) else {
            return;
        };
        let Some(target) = body.as_object_mut() else { return; };
        // Профиль не должен незаметно поменять модель, историю, набор
        // инструментов или режим streaming, сформированные агентом.
        for (key, value) in extra {
            if !matches!(key.as_str(), "model" | "messages" | "tools" | "tool_choice" | "stream") {
                target.insert(key.clone(), value.clone());
            }
        }
    }

    fn apply_profile_headers(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.config.extra_headers {
            request = request.header(name, value);
        }
        request
    }

    /// LLMMessage -> тело запроса в формате OpenAI. Отдельная функция —
    /// используется и в complete(), и в complete_with_tools(), чтобы не
    /// дублировать логику сборки сообщений (единственное отличие между
    /// ними — наличие поля "tools" и связанный с этим формат
    /// assistant-сообщений с tool_calls).
    fn build_messages(&self, messages: &[LLMMessage], system: Option<&str>) -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();

        // Сначала добавляем системное сообщение из параметра (если есть)
        if let Some(system_text) = system {
            out.push(serde_json::json!({"role": "system", "content": system_text}));
        }

        // Затем проходим по всем сообщениям из истории.
        // SANITIZER (баг из живого прогона, HTTP 400 "tool message must
        // follow an assistant message"): после Esc-прерывания/ретрая/
        // recovery в истории мог остаться tool-результат БЕЗ родительского
        // assistant с tool_calls — строгие валидаторы (TokenRouter, Nvidia)
        // такое отклоняют. Осиротевший tool превращаем в user-сообщение.
        let mut prev_assistant_had_tool_calls = false;
        for msg in messages {
            if msg.role == "system" {
                continue;
            }

            if msg.role == "tool" {
                if !prev_assistant_had_tool_calls {
                    // Сирота: валидируем как user — модель всё равно видит
                    // содержимое результата, а история становится корректной.
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": format!("(результат инструмента) {}", msg.content),
                    }));
                    continue;
                }
                // Результат вызова инструмента: стандарт требует
                // tool_call_id, привязывающий ответ к конкретному вызову.
                out.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": msg.tool_call_id.clone().unwrap_or_default(),
                    "content": msg.content,
                }));
                // Несколько tool подряд легальны только после ОДНОГО
                // assistant с несколькими tool_calls — флаг остаётся true
                // до следующего non-tool сообщения.
                continue;
            }

            prev_assistant_had_tool_calls = msg
                .tool_calls
                .as_ref()
                .map(|c| !c.is_empty())
                .unwrap_or(false) && msg.role == "assistant";

            match &msg.tool_calls {
                Some(calls) if !calls.is_empty() => {
                    // Assistant-сообщение, породившее вызовы инструментов —
                    // OpenAI ждёт tool_calls в формате function.arguments
                    // как JSON-СТРОКУ (не вложенный объект).
                    let tool_calls_json: Vec<Value> = calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.input.to_string(),
                                }
                            })
                        })
                        .collect();
                    out.push(serde_json::json!({
                        "role": "assistant",
                        "content": if msg.content.is_empty() { Value::Null } else { Value::String(msg.content.clone()) },
                        "tool_calls": tool_calls_json,
                    }));
                }
                _ => {
                    out.push(serde_json::json!({"role": msg.role, "content": msg.content}));
                }
            }
        }
        out
    }

    async fn send(&self, body: Value) -> Result<LLMResponse, LLMError> {
        let mut request = self.client.post(self.endpoint()).json(&body);
        if let Some(key) = &self.config.api_key {
            if !key.is_empty() {
                request = request.bearer_auth(key);
            }
        }
        request = self.apply_profile_headers(request);

        let response = request.send().await?;
        let status = response.status();
        // Совместимые API иногда возвращают HTML или plain text (например,
        // от CDN/proxy) вместо JSON. Раньше `response.json()` скрывал такой
        // ответ за бесполезным «error decoding response body», из-за чего
        // было невозможно отличить неверный endpoint от ошибки модели.
        let response_text = response.text().await?;
        let data: Value = serde_json::from_str(&response_text).map_err(|e| {
            let preview: String = response_text.chars().take(300).collect();
            LLMError::Api(format!(
                "HTTP {status}: сервер вернул не-JSON ответ: {:?} ({})",
                preview,
                e
            ))
        })?;

        if !status.is_success() {
            let message = data["error"]["message"].as_str().unwrap_or_else(|| data.as_str().unwrap_or("неизвестная ошибка"));
            return Err(LLMError::Api(format!("HTTP {status}: {message}")));
        }

        let choice = &data["choices"][0];
        let message = &choice["message"];
        let content = message["content"].as_str().unwrap_or("").to_string();
        let stop_reason = choice["finish_reason"].as_str().unwrap_or("stop").to_string();
        let model = data["model"].as_str().unwrap_or(&self.config.model).to_string();

        let mut usage = HashMap::new();
        if let Some(u) = data.get("usage").and_then(|u| u.as_object()) {
            for (key, value) in u {
                if let Some(n) = value.as_u64() {
                    usage.insert(key.clone(), n);
                }
            }
        }

        // tool_calls: OpenAI отдаёт function.arguments СТРОКОЙ — если это
        // не валидный JSON (маленькие модели такое иногда присылают),
        // не роняем весь ответ, подставляем пустой объект аргументов и
        // даём модели/инструменту разобраться на следующем шаге, а не
        // теряем остальной успешно распарсенный ответ целиком.
        let tool_use_blocks = message["tool_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|call| {
                        let id = call["id"].as_str()?.to_string();
                        let name = call["function"]["name"].as_str()?.to_string();
                        let args_str = call["function"]["arguments"].as_str().unwrap_or("{}");
                        let input = serde_json::from_str(args_str).unwrap_or_else(|_| serde_json::json!({}));
                        Some(ToolUseBlock { id, name, input })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(LLMResponse { content, stop_reason, usage, model, tool_use_blocks })
    }
}

#[async_trait::async_trait]
impl LLMClient for OpenAICompatibleClient {
    async fn complete(&self, messages: &[LLMMessage], system: Option<&str>) -> Result<LLMResponse, LLMError> {
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": self.build_messages(messages, system),
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
            "stream": false,
        });
        self.merge_profile_extra_body(&mut body);
        self.send(body).await
    }

    async fn complete_with_tools(
        &self,
        messages: &[LLMMessage],
        tools: &[Value],
        system: Option<&str>,
    ) -> Result<LLMResponse, LLMError> {
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": self.build_messages(messages, system),
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
            "stream": false,
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
            // Некоторые шлюзы (включая node-прокси перед llama.cpp)
            // активируют режим инструментов ТОЛЬКО при явном tool_choice;
            // без него поле tools молча игнорируется и модель отвечает
            // текстом. "auto" — стандартное значение OpenAI.
            body["tool_choice"] = serde_json::json!("auto");
        }
        self.merge_profile_extra_body(&mut body);
        self.send(body).await
    }

    /// Стриминг SSE Chat Completions: delta.content -> on_text живьём;
    /// кусочки delta.tool_calls (index/id/name/arguments-фрагменты)
    /// собираются в целые ToolUseBlock к концу ответа.
    async fn complete_with_tools_stream(
        &self,
        messages: &[LLMMessage],
        tools: &[Value],
        system: Option<&str>,
        on_text: std::sync::Arc<dyn Fn(String) + Send + Sync + 'static>,
    ) -> Result<LLMResponse, LLMError> {
        use futures_util::StreamExt;

        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": self.build_messages(messages, system),
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
            "stream": true,
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
            body["tool_choice"] = serde_json::json!("auto");
        }
        self.merge_profile_extra_body(&mut body);

        let mut req = self.client.post(self.endpoint()).json(&body).header("Accept", "text/event-stream");
        if let Some(k) = &self.config.api_key {
            if !k.is_empty() {
                req = req.bearer_auth(k);
            }
        }
        req = self.apply_profile_headers(req);
        if self.config.provider == LLMProvider::OpenRouter {
            // Рекомендации OpenRouter для атрибуции приложений.
            req = req.header("HTTP-Referer", "https://github.com/hephaestus-agent")
                     .header("X-Title", "Hephaestus");
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let t = resp.text().await.unwrap_or_default();
            return Err(LLMError::Api(format!("HTTP {status}: {}", crate::truncate_chars(&t, 300))));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut text_all = String::new();
        let mut stop_reason = String::from("stop");
        let mut model = self.config.model.clone();
        let mut usage: HashMap<String, u64> = HashMap::new();
        let mut saw_sse_data = false; // шлюз вообще отдал SSE?
        // Сборка tool_calls из фрагментов: индекс -> (id, name, args).
        let mut tc: std::collections::HashMap<i64, (String, String, String)> = Default::default();

        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(300), stream.next()).await {
                Ok(Some(Ok(chunk))) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(Some(Err(e))) => return Err(LLMError::Http(e)),
                Ok(None) | Err(_) => break,
            }
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim().to_string();
                buf.drain(..pos + 1);
                let Some(payload) = line.strip_prefix("data:") else { continue };
                saw_sse_data = true;
                let payload = payload.trim();
                if payload.is_empty() { continue; }
                if payload == "[DONE]" { break; }
                let Ok(ev) = serde_json::from_str::<Value>(payload) else { continue };

                if let Some(m) = ev.get("model").and_then(|m| m.as_str()) { model = m.to_string(); }
                if let Some(fr) = ev.pointer("/choices/0/finish_reason").and_then(|f| f.as_str()) {
                    if fr != "null" && !fr.is_empty() { stop_reason = fr.to_string(); }
                }
                if let Some(u) = ev.get("usage").and_then(|u| u.as_object()) {
                    for (k, v) in u {
                        if let Some(n) = v.as_u64() { usage.insert(k.clone(), n); }
                    }
                }
                let Some(delta) = ev.pointer("/choices/0/delta") else { continue };
                if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                    if !t.is_empty() {
                        text_all.push_str(t);
                        on_text(t.to_string());
                    }
                }
                if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                    for c in calls {
                        let idx = c.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                        let e = tc.entry(idx).or_insert_with(|| (String::new(), String::new(), String::new()));
                        if let Some(id) = c.get("id").and_then(|i| i.as_str()) { e.0 = id.to_string(); }
                        if let Some(n) = c.pointer("/function/name").and_then(|n| n.as_str()) { e.1.push_str(n); }
                        if let Some(a) = c.pointer("/function/arguments").and_then(|a| a.as_str()) { e.2.push_str(a); }
                    }
                }
            }
        }

        // ФОЛБЭК: шлюз не отдал ни одного SSE-события (не поддерживает
        // stream:true и вернул обычный JSON мимо парсера) — повторяем
        // запрос нестримингово, иначе пользователь получил бы пустоту.
        if !saw_sse_data && text_all.is_empty() && tc.is_empty() {
            crate::logging_system::info(
                "[llm] стриминг не поддержан шлюзом (нет SSE-событий) — повторяю запрос без stream",
            );
            return self.complete_with_tools(messages, tools, system).await;
        }

        let tool_use_blocks: Vec<ToolUseBlock> = {
            let mut v: Vec<(i64, ToolUseBlock)> = tc.into_iter()
                .map(|(i, (id, name, args))| (i, ToolUseBlock {
                    id,
                    name,
                    input: serde_json::from_str(&args).unwrap_or_else(|_| serde_json::json!({})),
                }))
                .collect();
            v.sort_by_key(|(i, _)| *i);
            v.into_iter().map(|(_, b)| b).collect()
        };

        Ok(LLMResponse { content: text_all, stop_reason, usage, model, tool_use_blocks })
    }
}

// ─────────────────────────────────────────────────────────────────
// AnthropicClient — ПОЛНАЯ реализация Messages API (раньше была
// заглушка "not implemented", хотя автодетект выбирал Anthropic по
// наличию ANTHROPIC_API_KEY — то есть дефолтный путь вёл в тупик).
//
// Особенности диалекта Anthropic:
//   • system — ОТДЕЛЬНОЕ поле запроса, не сообщение;
//   • max_tokens ОБЯЗАТЕЛЕН;
//   • инструменты: [{name, description, input_schema}], tool_choice;
//   • ответы: content[] блоки ({type:"text"} / {type:"tool_use"});
//   • результаты инструментов — ВНУТРИ user-сообщения блоками
//     {type:"tool_result", tool_use_id, content};
//   • стриминг: SSE-события content_block_delta (delta.text и
//     input_json_delta.partial_json для аргументов инструментов).
pub struct AnthropicClient {
    config: LLMConfig,
    client: reqwest::Client,
}

impl AnthropicClient {
    pub fn new(config: LLMConfig) -> Self {
        Self { config, client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build().unwrap_or_default() }
    }

    fn endpoint(&self) -> String {
        let base = self.config.base_url.clone()
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        format!("{}/v1/messages", base.trim_end_matches('/'))
    }

    fn auth(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(k) = &self.config.api_key {
            req = req.header("x-api-key", k);
        }
        req.header("anthropic-version", "2023-06-01")
    }

    /// Наши LLMMessage -> формат Messages API.
    fn build_messages(messages: &[LLMMessage]) -> Vec<Value> {
        use serde_json::json;
        let mut out: Vec<Value> = Vec::new();
        // tool_result идут внутри СЛЕДУЮЩЕГО user-сообщения блоками,
        // поэтому копим подряд идущие результаты и сливаем в один user.
        let flush_results = |out: &mut Vec<Value>, acc: &mut Vec<Value>| {
            if !acc.is_empty() {
                out.push(json!({"role": "user", "content": acc.drain(..).collect::<Vec<_>>()}));
            }
        };
        let mut acc: Vec<Value> = Vec::new();
        for msg in messages {
            match msg.role.as_str() {
                "tool" => {
                    acc.push(json!({
                        "type": "tool_result",
                        "tool_use_id": msg.tool_call_id.clone().unwrap_or_default(),
                        "content": msg.content,
                    }));
                }
                "assistant" => {
                    flush_results(&mut out, &mut acc);
                    let mut blocks: Vec<Value> = Vec::new();
                    if !msg.content.is_empty() {
                        blocks.push(json!({"type": "text", "text": msg.content}));
                    }
                    if let Some(calls) = &msg.tool_calls {
                        for c in calls {
                            blocks.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.input}));
                        }
                    }
                    if blocks.is_empty() {
                        blocks.push(json!({"type": "text", "text": "(пусто)"}));
                    }
                    out.push(json!({"role": "assistant", "content": blocks}));
                }
                "user" => {
                    flush_results(&mut out, &mut acc);
                    out.push(json!({"role": "user", "content": msg.content}));
                }
                _ => {} // system уходит отдельным полем
            }
        }
        flush_results(&mut out, &mut acc);
        out
    }

    fn base_body(&self, messages: &[Value], system: Option<&str>, tools: &[Value], stream: bool) -> Value {
        use serde_json::json;
        let mut body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": messages,
            "stream": stream,
        });
        if let Some(s) = system {
            // PROMPT CACHING (#4): системный промпт стабилен между ходами —
            // помечаем ephemeral-кэш Anthropic, чтобы не платить полную
            // цену обработки ядра на каждом запросе.
            body["system"] = json!([{
                "type": "text",
                "text": s,
                "cache_control": {"type": "ephemeral"},
            }]);
        }
        if !tools.is_empty() {
            let conv: Vec<Value> = tools.iter().map(|t| {
                let f = &t["function"];
                json!({
                    "name": f["name"],
                    "description": f["description"],
                    "input_schema": f["parameters"],
                })
            }).collect();
            body["tools"] = json!(conv);
            body["tool_choice"] = json!({"type": "auto"});
        }
        body
    }

    /// Разбор content[] ответа/аккумулированных блоков.
    fn parse_blocks(content: &[Value]) -> (String, Vec<ToolUseBlock>) {
        let mut text = String::new();
        let mut calls = Vec::new();
        for b in content {
            match b.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
                Some("tool_use") => {
                    calls.push(ToolUseBlock {
                        id: b.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                        name: b.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                        input: b.get("input").cloned().unwrap_or_else(|| serde_json::json!({})),
                    });
                }
                _ => {}
            }
        }
        (text, calls)
    }

    async fn send_once(&self, body: Value) -> Result<LLMResponse, LLMError> {
        let resp = self.auth(self.client.post(self.endpoint())).json(&body).send().await?;
        let status = resp.status();
        let data: Value = resp.json().await?;
        if !status.is_success() {
            let m = data.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or("unknown");
            return Err(LLMError::Api(format!("Anthropic HTTP {status}: {m}")));
        }
        let arr = data.get("content").and_then(|c| c.as_array()).cloned().unwrap_or_default();
        let (content, tool_use_blocks) = Self::parse_blocks(&arr);
        let mut usage = HashMap::new();
        if let Some(u) = data.get("usage").and_then(|u| u.as_object()) {
            for (k, v) in u {
                if let Some(n) = v.as_u64() { usage.insert(k.clone(), n); }
            }
        }
        Ok(LLMResponse {
            content,
            stop_reason: data.get("stop_reason").and_then(|s| s.as_str()).unwrap_or("stop").to_string(),
            usage,
            model: data.get("model").and_then(|m| m.as_str()).unwrap_or(&self.config.model).to_string(),
            tool_use_blocks,
        })
    }
}

#[async_trait::async_trait]
impl LLMClient for AnthropicClient {
    async fn complete(&self, messages: &[LLMMessage], system: Option<&str>) -> Result<LLMResponse, LLMError> {
        let msgs = Self::build_messages(messages);
        self.send_once(self.base_body(&msgs, system, &[], false)).await
    }

    async fn complete_with_tools(
        &self,
        messages: &[LLMMessage],
        tools: &[Value],
        system: Option<&str>,
    ) -> Result<LLMResponse, LLMError> {
        let msgs = Self::build_messages(messages);
        self.send_once(self.base_body(&msgs, system, tools, false)).await
    }

    /// Стриминг: читаем SSE; текст отдаём в on_text по мере прихода,
    /// аргументы tool_use собираем из partial_json кусочков.
    async fn complete_with_tools_stream(
        &self,
        messages: &[LLMMessage],
        tools: &[Value],
        system: Option<&str>,
        on_text: std::sync::Arc<dyn Fn(String) + Send + Sync + 'static>,
    ) -> Result<LLMResponse, LLMError> {
        use futures_util::StreamExt;
        let msgs = Self::build_messages(messages);
        let resp = self.auth(self.client.post(self.endpoint()))
            .json(&self.base_body(&msgs, system, tools, true))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let t = resp.text().await.unwrap_or_default();
            return Err(LLMError::Api(format!("Anthropic HTTP {status}: {}", crate::truncate_chars(&t, 300))));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut text_all = String::new();
        // Аккумуляторы tool_use по индексам блоков.
        let mut call_ids: std::collections::HashMap<i64, String> = Default::default();
        let mut call_names: std::collections::HashMap<i64, String> = Default::default();
        let mut call_args: std::collections::HashMap<i64, String> = Default::default();

        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(300), stream.next()).await {
                Ok(Some(Ok(chunk))) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(Some(Err(e))) => return Err(LLMError::Http(e)),
                Ok(None) | Err(_) => break,
            }
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim().to_string();
                buf.drain(..pos + 1);
                let Some(payload) = line.strip_prefix("data:") else { continue };
                let payload = payload.trim();
                if payload.is_empty() || payload == "[DONE]" { continue; }
                let Ok(ev) = serde_json::from_str::<Value>(payload) else { continue };
                match ev.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "content_block_start" => {
                        if let Some(b) = ev.pointer("/content_block") {
                            let idx = ev.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                            if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                call_ids.insert(idx, b.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string());
                                call_names.insert(idx, b.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string());
                                call_args.insert(idx, String::new());
                            }
                        }
                    }
                    "content_block_delta" => {
                        let idx = ev.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                        if let Some(d) = ev.get("delta") {
                            match d.get("type").and_then(|t| t.as_str()) {
                                Some("text_delta") => {
                                    if let Some(t) = d.get("text").and_then(|t| t.as_str()) {
                                        text_all.push_str(t);
                                        on_text(t.to_string());
                                    }
                                }
                                Some("input_json_delta") => {
                                    if let Some(p) = d.get("partial_json").and_then(|p| p.as_str()) {
                                        call_args.entry(idx).or_default().push_str(p);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut tool_use_blocks = Vec::new();
        for (idx, id) in &call_ids {
            tool_use_blocks.push(ToolUseBlock {
                id: id.clone(),
                name: call_names.get(idx).cloned().unwrap_or_default(),
                input: serde_json::from_str(call_args.get(idx).map(|s| s.as_str()).unwrap_or("{}"))
                    .unwrap_or_else(|_| serde_json::json!({})),
            });
        }
        Ok(LLMResponse {
            content: text_all,
            stop_reason: "stop".to_string(),
            usage: HashMap::new(),
            model: self.config.model.clone(),
            tool_use_blocks,
        })
    }
}
