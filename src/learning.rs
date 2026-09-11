use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessEntry {
    pub tool: String,
    pub params_keys: Vec<String>,
    pub context: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEntry {
    pub tool: String,
    pub error: String,
    pub params_keys: Vec<String>,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningData {
    pub successes: Vec<SuccessEntry>,
    pub failures: Vec<FailureEntry>,
    pub preferences: HashMap<String, String>,
}

impl Default for LearningData {
    fn default() -> Self {
        Self {
            successes: Vec::new(),
            failures: Vec::new(),
            preferences: HashMap::new(),
        }
    }
}

pub struct ContextLearning {
    file_path: PathBuf,
    data: LearningData,
}

impl ContextLearning {
    pub fn new() -> Self {
        // HEPHAESTUS_HOME — как во всех модулях.
        let home = std::env::var("HEPHAESTUS_HOME")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        let file_path = PathBuf::from(home).join(".hephaestus").join("learning.json");
        let _ = fs::create_dir_all(file_path.parent().unwrap());
        let data = Self::load_data(&file_path);
        Self { file_path, data }
    }

    fn load_data(file_path: &PathBuf) -> LearningData {
        if let Ok(content) = fs::read_to_string(file_path) {
            if let Ok(data) = serde_json::from_str(&content) {
                return data;
            }
        }
        LearningData::default()
    }

    fn save_data(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.data) {
            let _ = fs::write(&self.file_path, json);
        }
    }

    pub fn record_success(&mut self, tool: &str, params: &HashMap<String, serde_json::Value>, context: &str) {
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        let entry = SuccessEntry {
            tool: tool.to_string(),
            params_keys: params.keys().cloned().collect(),
            context: crate::truncate_chars(context, 100),
            ts,
        };

        self.data.successes.push(entry);
        if self.data.successes.len() > 100 {
            self.data.successes = self.data.successes.split_off(self.data.successes.len() - 100);
        }
        self.save_data();
    }

    pub fn record_failure(&mut self, tool: &str, error: &str, params: &HashMap<String, serde_json::Value>) {
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        let entry = FailureEntry {
            tool: tool.to_string(),
            error: crate::truncate_chars(error, 200),
            params_keys: params.keys().cloned().collect(),
            ts,
        };

        self.data.failures.push(entry);
        if self.data.failures.len() > 50 {
            self.data.failures = self.data.failures.split_off(self.data.failures.len() - 50);
        }
        self.save_data();
    }

    pub fn set_preference(&mut self, key: &str, value: &str) {
        self.data.preferences.insert(key.to_string(), value.to_string());
        self.save_data();
    }

    /// Глоссарий инструментов по опыту сессии: "bash 12✓/0✗, ..."
    pub fn tool_glossary(&self, top: usize) -> String {
        use std::collections::HashMap;
        let mut ok: HashMap<&str, u64> = HashMap::new();
        for s in &self.data.successes {
            *ok.entry(s.tool.as_str()).or_insert(0) += 1;
        }
        let mut err: HashMap<&str, u64> = HashMap::new();
        for f in &self.data.failures {
            *err.entry(f.tool.as_str()).or_insert(0) += 1;
        }
        let mut names: Vec<&str> = ok.keys().chain(err.keys()).copied().collect();
        names.sort_by_key(|n| std::cmp::Reverse(
            ok.get(n).copied().unwrap_or(0) + err.get(n).copied().unwrap_or(0)));
        names.truncate(top);
        names.iter()
            .map(|n| format!("{n} {}✓/{}✗",
                ok.get(*n).copied().unwrap_or(0),
                err.get(*n).copied().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Инструменты с >=2 падениями — для ENV-секции AVOID.
    pub fn avoid_hints(&self) -> Vec<String> {
        use std::collections::HashMap;
        let mut err: HashMap<&str, (u64, Option<&String>)> = HashMap::new();
        for f in &self.data.failures {
            let e = err.entry(f.tool.as_str()).or_insert((0, None));
            e.0 += 1;
            if e.1.is_none() { e.1 = Some(&f.error); }
        }
        err.into_iter()
            .filter(|(_, (c, _))| *c >= 2)
            .map(|(tool, (c, last))| {
                let snip = last.and_then(|e| e.lines().next())
                    .map(|l| l.chars().take(80).collect::<String>())
                    .unwrap_or_default();
                format!("{tool} ({c} ошибок) — {snip}")
            })
            .collect()
    }

    pub fn get_context_hint(&self) -> String {
        let mut hints = Vec::new();

        let mut error_tools: HashMap<String, usize> = HashMap::new();
        for f in &self.data.failures {
            *error_tools.entry(f.tool.clone()).or_insert(0) += 1;
        }

        if let Some((worst, count)) = error_tools.iter().max_by_key(|(_, v)| *v) {
            if *count >= 2 {
                hints.push(format!("ВНИМАНИЕ: инструмент '{}' часто вызывает ошибки — будь особенно аккуратен.", worst));
            }
        }

        let prefs = &self.data.preferences;
        if !prefs.is_empty() {
            let pref_str: Vec<String> = prefs.iter()
                .take(5)
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect();
            hints.push(format!("Предпочтения пользователя: {}", pref_str.join(", ")));
        }

        hints.join("\n")
    }

    pub fn get_stats(&self) -> String {
        format!(
            "Успехов: {} | Ошибок: {} | Предпочтений: {}",
            self.data.successes.len(),
            self.data.failures.len(),
            self.data.preferences.len()
        )
    }
}

impl Default for ContextLearning {
    fn default() -> Self {
        Self::new()
    }
}
