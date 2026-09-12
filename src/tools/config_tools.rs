//! Config Tools — чтение/запись конфига агента (`~/.hephaestus/config.json`).
//! Аналог `config_tools.py`. Простой key-value JSON, без валидации схемы —
//! агент сам решает, какие ключи ему нужны (например, "default_model",
//! "workspace_root", "auto_approve").

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolResult};

fn config_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".hephaestus").join("config.json")
}

fn load() -> HashMap<String, Value> {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(data: &HashMap<String, Value>) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, json)
}

pub struct ConfigGetTool;
impl ConfigGetTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ConfigGetTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let data = load();
        let key = args.get("key").and_then(|v| v.as_str());
        match key {
            Some(k) => match data.get(k) {
                Some(v) => ToolResult::success(v.to_string()),
                None => ToolResult::error(format!("Ключ не найден: {}", k)),
            },
            None => ToolResult::success(serde_json::to_string_pretty(&data).unwrap_or_default()),
        }
    }
    fn name(&self) -> &'static str {
        "config_get"
    }
}

pub struct ConfigSetTool;
impl ConfigSetTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ConfigSetTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
        if key.is_empty() {
            return ToolResult::error("Требуется параметр 'key'");
        }
        let value = args.get("value").cloned().unwrap_or(Value::Null);
        let mut data = load();
        data.insert(key.to_string(), value);
        match save(&data) {
            Ok(_) => ToolResult::success(format!("Конфиг обновлён: {}", key)),
            Err(e) => ToolResult::error(format!("Ошибка сохранения: {}", e)),
        }
    }
    fn name(&self) -> &'static str {
        "config_set"
    }
}
