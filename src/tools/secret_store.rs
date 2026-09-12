//! Secret Store — локальное хранилище секретов для агента (API-ключи,
//! токены), аналог `secret_store.py`.
//!
//! ЧЕСТНАЯ ОГОВОРКА: это НЕ шифрованное хранилище. Значения хранятся в
//! `~/.hephaestus/secrets.json` открытым текстом, защита — только права
//! доступа файловой системы (`0600`, только владелец). Добавление
//! настоящего шифрования (например через `age` или `ring`) — отдельная
//! задача, требующая новой crypto-зависимости, которую я не стал добавлять
//! не проверив сборку. Если нужно реальное шифрование — скажите отдельно.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolResult};

fn store_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".hephaestus").join("secrets.json")
}

fn load() -> HashMap<String, String> {
    let path = store_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(data: &HashMap<String, String>) -> std::io::Result<()> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, json)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

pub struct SecretSetTool;
impl SecretSetTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SecretSetTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("");
        if key.is_empty() {
            return ToolResult::error("Требуется параметр 'key'");
        }
        let mut data = load();
        data.insert(key.to_string(), value.to_string());
        match save(&data) {
            Ok(_) => ToolResult::success(format!("Секрет '{}' сохранён (не зашифрован, доступ ограничен правами файла)", key)),
            Err(e) => ToolResult::error(format!("Ошибка сохранения: {}", e)),
        }
    }
    fn name(&self) -> &'static str {
        "secret_set"
    }
}

pub struct SecretGetTool;
impl SecretGetTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SecretGetTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
        if key.is_empty() {
            return ToolResult::error("Требуется параметр 'key'");
        }
        let data = load();
        match data.get(key) {
            Some(v) => ToolResult::success(v.clone()),
            None => ToolResult::error(format!("Секрет не найден: {}", key)),
        }
    }
    fn name(&self) -> &'static str {
        "secret_get"
    }
}

pub struct SecretListTool;
impl SecretListTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SecretListTool {
    async fn execute(&self, _args: &Value) -> ToolResult {
        let data = load();
        if data.is_empty() {
            return ToolResult::success("Секретов нет.".to_string());
        }
        let mut keys: Vec<&String> = data.keys().collect();
        keys.sort();
        ToolResult::success(format!("Ключи (значения скрыты): {}", keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")))
    }
    fn name(&self) -> &'static str {
        "secret_list"
    }
}

pub struct SecretDeleteTool;
impl SecretDeleteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SecretDeleteTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
        if key.is_empty() {
            return ToolResult::error("Требуется параметр 'key'");
        }
        let mut data = load();
        if data.remove(key).is_some() {
            match save(&data) {
                Ok(_) => ToolResult::success(format!("Секрет '{}' удалён", key)),
                Err(e) => ToolResult::error(format!("Ошибка сохранения: {}", e)),
            }
        } else {
            ToolResult::error(format!("Секрет не найден: {}", key))
        }
    }
    fn name(&self) -> &'static str {
        "secret_delete"
    }
}
