use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<Message>,
    pub summary: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

pub struct SessionManager {
    dir: PathBuf,
}

impl SessionManager {
    pub fn new() -> Self {
        // ФИКС: раньше только $HOME/.hephaestus — кастомный HEPHAESTUS_HOME
        // игнорировался (сессии уезжали в реальный home даже при изолированном).
        let dir = crate::bootstrap::hephaestus_home().join("sessions");
        let _ = fs::create_dir_all(&dir);
        Self { dir }
    }

    pub fn save(&self, messages: &[Message], summary: Option<&str>) -> Result<String, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?;
        let timestamp = now.as_secs();
        let session_id = format!("ses_{}", timestamp);

        let created = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let updated = created.clone();

        let summary = summary.unwrap_or("");
        let summary = if summary.is_empty() {
            self.auto_summary(messages)
        } else {
            summary.to_string()
        };

        let session = Session {
            session_id: session_id.clone(),
            created_at: created,
            updated_at: updated,
            messages: messages.to_vec(),
            summary,
            tags: vec![],
        };

        let path = self.dir.join(format!("{}.json", session_id));
        let json = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())?;

        Ok(session_id)
    }

    pub fn load(&self, session_id: &str) -> Result<Vec<Message>, String> {
        let path = self.dir.join(format!("{}.json", session_id));
        if !path.exists() {
            let pattern = format!("*{}*.json", session_id);
            let matches: Vec<_> = glob::glob(&self.dir.join(pattern).to_str().unwrap_or(""))
                .map_err(|e| e.to_string())?
                .filter_map(|p| p.ok())
                .collect();
            if matches.is_empty() {
                return Err("Session not found".to_string());
            }
            let path = &matches[0];
            let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
            let session: Session = serde_json::from_str(&data).map_err(|e| e.to_string())?;
            return Ok(session.messages);
        }

        let data = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let session: Session = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        Ok(session.messages)
    }

    pub fn list_sessions(&self, limit: usize) -> Vec<HashMap<String, String>> {
        let mut sessions = Vec::new();
        let mut paths: Vec<_> = glob::glob(&self.dir.join("*.json").to_str().unwrap_or(""))
            .unwrap_or_else(|_| glob::glob("").unwrap())
            .filter_map(|p| p.ok())
            .collect();
        paths.sort_by(|a, b| b.cmp(a));

        for path in paths.iter().take(limit) {
            if let Ok(data) = fs::read_to_string(path) {
                if let Ok(session) = serde_json::from_str::<Session>(&data) {
                    let mut map = HashMap::new();
                    map.insert("id".to_string(), session.session_id);
                    map.insert("created".to_string(), session.created_at);
                    map.insert("messages".to_string(), session.messages.len().to_string());
                    let summary = if session.summary.chars().count() > 60 {
                        format!("{}...", crate::truncate_chars(&session.summary, 60))
                    } else {
                        session.summary
                    };
                    map.insert("summary".to_string(), summary);
                    sessions.push(map);
                }
            }
        }
        sessions
    }

    pub fn delete(&self, session_id: &str) -> bool {
        let path = self.dir.join(format!("{}.json", session_id));
        if path.exists() {
            let _ = fs::remove_file(&path);
            true
        } else {
            false
        }
    }

    fn auto_summary(&self, messages: &[Message]) -> String {
        for msg in messages {
            if msg.role == "user" {
                let content = &msg.content;
                if content.chars().count() > 80 {
                    return format!("{}...", crate::truncate_chars(content, 80));
                }
                return content.clone();
            }
        }
        "Без описания".to_string()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}