//! Skill Learner — автоматическое обучение из успешных Goal выполнений.
//! После каждого успешного /goal сохраняет план как переиспользуемый skill.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Структура сохранённого навыка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSkill {
    pub goal: String,
    pub steps: Vec<serde_json::Value>,
    pub success_rate: f64,
    pub created: String,
    pub uses: u32,
}

/// Skill Learner.
pub struct SkillLearner {
    skills: HashMap<String, LearnedSkill>,
    file_path: PathBuf,
}

impl SkillLearner {
    /// Создать новый SkillLearner.
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let file_path = PathBuf::from(home).join(".hephaestus/learned_skills.json");
        Self::with_file_path(file_path)
    }

    /// Конструктор с явным путём к хранилищу — для тестов, чтобы они не
    /// читали и не писали реальный ~/.hephaestus/learned_skills.json.
    fn with_file_path(file_path: PathBuf) -> Self {
        let skills = Self::load(&file_path);
        Self { skills, file_path }
    }

    fn load(path: &PathBuf) -> HashMap<String, LearnedSkill> {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(data) = serde_json::from_str(&content) {
                return data;
            }
        }
        HashMap::new()
    }

    fn save(&self) {
        if let Some(parent) = self.file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.skills) {
            let _ = std::fs::write(&self.file_path, json);
        }
    }

    /// Сохранить успешный план как skill.
    pub fn learn_from_goal(
        &mut self,
        goal: &str,
        steps: &[serde_json::Value],
        success_rate: f64,
    ) -> String {
        if success_rate < 0.7 {
            return format!(
                "Слишком много ошибок ({:.0}%), skill не сохранён",
                success_rate * 100.0
            );
        }

        let key = goal
            .to_lowercase()
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join("_")
            .replace(['/', '\\'], "");
        let now = chrono::Local::now().format("%m%d").to_string();
        let skill_id = format!("learned_{}_{}", key, now);

        let skill = LearnedSkill {
            goal: goal.to_string(),
            steps: steps.to_vec(),
            success_rate,
            created: chrono::Local::now().to_rfc3339(),
            uses: 0,
        };

        self.skills.insert(skill_id.clone(), skill);
        self.save();
        skill_id
    }

    /// Найти похожие learned skills для текущей задачи.
    pub fn find_similar(&self, query: &str, top_k: usize) -> Vec<LearnedSkill> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let query_set: std::collections::HashSet<&str> = query_words.iter().copied().collect();

        let mut scored: Vec<(f64, &LearnedSkill)> = Vec::new();
        for skill in self.skills.values() {
            let goal_lower = skill.goal.to_lowercase();
            let goal_words: Vec<&str> = goal_lower.split_whitespace().collect();
            let goal_set: std::collections::HashSet<&str> = goal_words.iter().copied().collect();

            let intersection = query_set.intersection(&goal_set).count();
            let union = query_set.union(&goal_set).count();
            let score = if union > 0 {
                intersection as f64 / union as f64
            } else {
                0.0
            };

            if score > 0.2 {
                scored.push((score, skill));
            }
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(top_k)
            .map(|(_, skill)| skill.clone())
            .collect()
    }

    /// Список навыков.
    pub fn list_skills(&self) -> String {
        if self.skills.is_empty() {
            return " Нет изученных навыков пока. Используй /goal для обучения!".to_string();
        }

        let mut lines = vec![format!(" Изученные навыки ({}):\n", self.skills.len())];
        for (sid, sk) in &self.skills {
            lines.push(format!("   {}", sid));
            let goal_trunc = if sk.goal.chars().count() > 60 {
                format!("{}…", crate::truncate_chars(&sk.goal, 60))
            } else {
                sk.goal.clone()
            };
            lines.push(format!("     Цель: {}", goal_trunc));
            lines.push(format!(
                "     Шагов: {} | Успех: {:.0}%",
                sk.steps.len(),
                sk.success_rate * 100.0
            ));
        }
        lines.join("\n")
    }
}

impl Default for SkillLearner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Изолированный learner во временной папке — тесты не должны трогать
    /// реальный ~/.hephaestus/learned_skills.json (раньше они и читали его,
    /// и писали в него, мешая друг другу при параллельном прогоне).
    fn test_learner() -> (tempfile::TempDir, SkillLearner) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("learned_skills.json");
        let learner = SkillLearner::with_file_path(path);
        (dir, learner)
    }

    #[test]
    fn test_learn_from_goal() {
        let (_dir, mut learner) = test_learner();
        let goal = "Создать файл test.txt";
        let steps = vec![serde_json::json!([{"tool": "file_write", "params": {"file_path": "test.txt"}}])];

        let id = learner.learn_from_goal(goal, &steps, 0.8);
        assert!(id.starts_with("learned_"));
        assert!(learner.skills.contains_key(&id));
    }

    #[test]
    fn test_learn_from_goal_low_rate() {
        let (_dir, mut learner) = test_learner();
        let goal = "Создать файл test.txt";
        let steps = vec![serde_json::json!([{"tool": "file_write", "params": {"file_path": "test.txt"}}])];

        let result = learner.learn_from_goal(goal, &steps, 0.5);
        assert!(result.contains("Слишком много ошибок"));
        assert!(learner.skills.is_empty());
    }

    #[test]
    fn test_find_similar() {
        let (_dir, mut learner) = test_learner();
        // goal2 подобрана так, чтобы НЕ пересекаться по словам с запросом
        // "создать файл" (Жаккар 0): раньше "Удалить файл old.txt" давала
        // 0.25 > порога 0.2 и тоже попадала в результат.
        let goal1 = "Создать файл report.txt";
        let goal2 = "Очистить кэш системы";
        let steps = vec![];

        learner.learn_from_goal(goal1, &steps, 0.9);
        learner.learn_from_goal(goal2, &steps, 0.9);

        let similar = learner.find_similar("создать файл", 2);
        assert_eq!(similar.len(), 1);
        assert!(similar[0].goal.contains("Создать"));
    }
}
