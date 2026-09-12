//! Простое сохранение/восстановление сессии на диск.
//!
//! Заменяет часть функциональности старого (неподключённого) `session.rs`:
//! только то, что реально нужно REPL и graceful shutdown — сохранить историю
//! сообщений и восстановить её при следующем запуске.

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::llm::LLMMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub messages: Vec<LLMMessage>,
    pub saved_at: String,
}

/// Путь к файлу сессии: `$HEPHAESTUS_HOME/session.json`, либо
/// `~/.hephaestus/session.json`, если `HEPHAESTUS_HOME` не задан.
pub fn session_path() -> PathBuf {
    if let Ok(custom) = std::env::var("HEPHAESTUS_HOME") {
        return PathBuf::from(custom).join("session.json");
    }
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".hephaestus").join("session.json")
}

pub fn save(messages: &[LLMMessage]) -> std::io::Result<PathBuf> {
    let path = session_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = SessionData {
        messages: messages.to_vec(),
        saved_at: chrono::Local::now().to_rfc3339(),
    };
    let json = serde_json::to_string_pretty(&data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, json)?;
    Ok(path)
}

pub fn load() -> Option<SessionData> {
    let path = session_path();
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}
