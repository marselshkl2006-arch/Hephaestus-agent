//! Автоопределение доступных LLM провайдеров/моделей. Порт
//! `model_detector.py` (итерация 6). У оригинала было два конфликтующих
//! определения `get_recommended_model()` (Python просто использует
//! последнее — первое было мёртвым кодом с недостижимым `return`
//! посередине); здесь только финальная, реально используемая логика.
//! `koboldcpp_discovery.py` (умный автопоиск по подсети) не портирован —
//! используется прямой fallback-список адресов, как и в самом
//! `model_detector.py` при `ImportError`.

use serde::Serialize;
use std::env;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct AvailableModel {
    pub provider: String,
    pub model: String,
    pub name: String,
    pub size: Option<String>,
    pub speed: String, // fast | medium | slow
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default()
}

async fn detect_ollama_models() -> Vec<AvailableModel> {
    let c = client();
    let resp = match c.get("http://localhost:11434/api/tags").send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return vec![],
    };
    let data: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let mut models = vec![];
    if let Some(arr) = data.get("models").and_then(|v| v.as_array()) {
        for m in arr {
            let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let size = m
                .get("details")
                .and_then(|d| d.get("parameter_size"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let lower = name.to_lowercase();
            let speed = if lower.contains("1.5b") || lower.contains("1b") {
                "fast"
            } else if lower.contains("13b") || lower.contains("70b") {
                "slow"
            } else {
                "medium"
            };
            models.push(AvailableModel {
                provider: "ollama".to_string(),
                model: name.clone(),
                name: format!("Ollama: {}", name),
                size,
                speed: speed.to_string(),
            });
        }
    }
    models
}

fn detect_anthropic_available() -> Vec<AvailableModel> {
    if env::var("ANTHROPIC_API_KEY").is_ok() {
        vec![
            AvailableModel {
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet-20241022".to_string(),
                name: "Anthropic: Claude 3.5 Sonnet".to_string(),
                size: None,
                speed: "fast".to_string(),
            },
            AvailableModel {
                provider: "anthropic".to_string(),
                model: "claude-3-opus-20240229".to_string(),
                name: "Anthropic: Claude 3 Opus".to_string(),
                size: None,
                speed: "medium".to_string(),
            },
        ]
    } else {
        vec![]
    }
}

fn detect_openai_available() -> Vec<AvailableModel> {
    if env::var("OPENAI_API_KEY").is_ok() {
        vec![
            AvailableModel {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                name: "OpenAI: GPT-4o".to_string(),
                size: None,
                speed: "fast".to_string(),
            },
            AvailableModel {
                provider: "openai".to_string(),
                model: "gpt-4o-mini".to_string(),
                name: "OpenAI: GPT-4o Mini".to_string(),
                size: None,
                speed: "fast".to_string(),
            },
            AvailableModel {
                provider: "openai".to_string(),
                model: "gpt-4-turbo".to_string(),
                name: "OpenAI: GPT-4 Turbo".to_string(),
                size: None,
                speed: "medium".to_string(),
            },
        ]
    } else {
        vec![]
    }
}

fn detect_openrouter_available() -> Vec<AvailableModel> {
    match env::var("OPENROUTER_API_KEY") {
        Ok(_) => {
            let model = env::var("OPENROUTER_MODEL")
                .unwrap_or_else(|_| "qwen/qwen-2.5-coder-32b-instruct".to_string());
            let lower = model.to_lowercase();
            let speed = if lower.contains("405b") {
                "slow"
            } else if lower.contains("32b") || lower.contains("70b") {
                "medium"
            } else {
                "fast"
            };
            vec![AvailableModel {
                provider: "openrouter".to_string(),
                model: model.clone(),
                name: format!("OpenRouter: {}", model),
                size: None,
                speed: speed.to_string(),
            }]
        }
        Err(_) => vec![],
    }
}

async fn detect_koboldcpp_available() -> Vec<AvailableModel> {
    let c = client();
    let mut candidates = vec![env::var("KOBOLDCPP_URL").unwrap_or_else(|_| "http://localhost:5001".to_string())];
    for u in ["http://localhost:5001", "http://192.168.1.136:5001", "http://127.0.0.1:5001"] {
        if !candidates.contains(&u.to_string()) {
            candidates.push(u.to_string());
        }
    }

    for url in candidates {
        let resp = match c.get(format!("{}/api/v1/model", url)).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };
        if let Ok(data) = resp.json::<serde_json::Value>().await {
            let model_name = data.get("result").and_then(|v| v.as_str()).unwrap_or("local-model").to_string();
            return vec![AvailableModel {
                provider: "koboldcpp".to_string(),
                model: model_name.clone(),
                name: format!("KoboldCPP: {}", model_name),
                size: None,
                speed: "fast".to_string(),
            }];
        }
    }
    vec![]
}

/// Определить все доступные модели с приоритетом на локальные
/// (KoboldCPP/Ollama > облачные API, если локальных не найдено).
pub async fn detect_all_models() -> Vec<AvailableModel> {
    let mut models = vec![];

    let kobold = detect_koboldcpp_available().await;
    if !kobold.is_empty() {
        models.extend(kobold);
    }

    let ollama = detect_ollama_models().await;
    if !ollama.is_empty() {
        models.extend(ollama);
    }

    if models.is_empty() {
        let openrouter = detect_openrouter_available();
        if !openrouter.is_empty() {
            models.extend(openrouter);
        }
    }

    if models.is_empty() {
        models.extend(detect_anthropic_available());
        models.extend(detect_openai_available());
    }

    models
}

/// Самая быстрая доступная модель (fast > medium > slow).
pub fn fastest(models: &[AvailableModel]) -> Option<&AvailableModel> {
    fn rank(speed: &str) -> u8 {
        match speed {
            "fast" => 0,
            "medium" => 1,
            _ => 2,
        }
    }
    models.iter().min_by_key(|m| rank(&m.speed))
}

/// Рекомендуемая модель — баланс скорости и качества. Приоритет (как в
/// финальной, реально используемой версии `model_detector.py`):
/// OpenRouter > Anthropic > OpenAI > Ollama(medium) > первая из списка.
pub fn recommended(models: &[AvailableModel]) -> Option<&AvailableModel> {
    if models.is_empty() {
        return None;
    }
    for provider in ["openrouter", "anthropic", "openai"] {
        if let Some(m) = models.iter().find(|m| m.provider == provider) {
            return Some(m);
        }
    }
    if let Some(m) = models.iter().find(|m| m.provider == "ollama" && m.speed == "medium") {
        return Some(m);
    }
    models.first()
}

/// Отформатированный вывод (аналог `print_available_models()`), но
/// возвращает строку вместо печати — вызывающий код сам решает, куда её
/// вывести (REPL, лог, инструмент).
pub async fn describe_available_models() -> String {
    let models = detect_all_models().await;
    if models.is_empty() {
        return "❌ Нет доступных моделей\n\nУстановите:\n  - Ollama: https://ollama.com\n  \
                - OpenRouter API: export OPENROUTER_API_KEY='sk-or-v1-...'\n  \
                - Anthropic API: export ANTHROPIC_API_KEY='sk-ant-...'\n  \
                - OpenAI API: export OPENAI_API_KEY='sk-...'"
            .to_string();
    }

    let mut out = format!("✅ Найдено моделей: {}\n\n", models.len());
    let mut by_provider: std::collections::BTreeMap<&str, Vec<&AvailableModel>> = std::collections::BTreeMap::new();
    for m in &models {
        by_provider.entry(m.provider.as_str()).or_default().push(m);
    }
    for (provider, ms) in by_provider {
        out.push_str(&format!("📦 {}:\n", provider.to_uppercase()));
        for m in ms {
            let icon = match m.speed.as_str() {
                "fast" => "⚡",
                "medium" => "🔄",
                "slow" => "🐌",
                _ => "❓",
            };
            let size_info = m.size.as_ref().map(|s| format!(" ({})", s)).unwrap_or_default();
            out.push_str(&format!("   {} {}{}\n", icon, m.model, size_info));
        }
        out.push('\n');
    }

    if let Some(f) = fastest(&models) {
        out.push_str(&format!("⚡ Самая быстрая: {}\n", f.name));
    }
    if let Some(r) = recommended(&models) {
        if fastest(&models).map(|f| &f.name) != Some(&r.name) {
            out.push_str(&format!("⭐ Рекомендуемая: {}\n", r.name));
        }
    }
    out
}
