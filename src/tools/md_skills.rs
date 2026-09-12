//! MD-Скиллы — ручные процедуры в формате Claude Code Skills.
//!
//! Файл `~/.hephaestus/skills/<name>.md`:
//! ```text
//! ---
//! name: deploy-site
//! description: Задеплоить сайт через rsync на хостинг.
//! whenToUse: Когда пользователь просит задеплоить/обновить сайт.
//! ---
//! 1. Собрать проект...
//! 2. ...
//! ```
//!
//! Прогрессивная подгрузка (как в CC): в системном промпте живёт ТОЛЬКО
//! индекс «имя — описание» (~30 токенов на скилл); полное тело модель
//! получает по запросу через инструмент skill_load, когда оно реально
//! нужно. Автонакопленные навыки (skill_learner.rs) сосуществуют рядом.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MdSkill {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    /// Тело после frontmatter (инструкция).
    pub body: String,
}

pub fn skills_dir() -> PathBuf {
    let mut p = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push(".hephaestus");
    p.push("skills");
    p
}

/// Разобрать один файл скилла (frontmatter + тело).
fn parse_skill_file(path: &Path) -> Option<MdSkill> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return None; // без frontmatter — не скилл
    }
    let rest = &trimmed[3..];
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let mut name = None;
    let mut description = None;
    let mut when_to_use = None;
    for line in fm.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("whenToUse:") {
            when_to_use = Some(v.trim().to_string());
        }
    }
    Some(MdSkill {
        name: name.or_else(|| path.file_stem()?.to_str().map(String::from))?,
        description: description.unwrap_or_default(),
        when_to_use,
        body: rest[end + 4..].trim().to_string(),
    })
}

/// Все md-скиллы из папки (отсортированы по имени).
pub fn list_in(dir: &Path) -> Vec<MdSkill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<MdSkill> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .filter_map(|p| parse_skill_file(&p))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn list_all() -> Vec<MdSkill> {
    list_in(&skills_dir())
}

pub fn load_by_name(name: &str) -> Option<MdSkill> {
    list_all().into_iter().find(|s| s.name == name)
}

// ─────────────────────────────────────────────── инструменты ──

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use super::{Tool, ToolResult};

/// skill_list_md — индекс для модели (короткий).
pub struct SkillMdListTool;
#[async_trait]
impl Tool for SkillMdListTool {
    fn name(&self) -> &'static str {
        "skill_list_md"
    }

    fn description(&self) -> &'static str {
        "Список пользовательских md-навыков (процедур). Индекс также всегда виден тебе в системном промпте как AVAILABLE SKILLS."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: &Value) -> ToolResult {
        let skills = list_all();
        if skills.is_empty() {
            return ToolResult::success("(md-навыков нет; создать: skill_create)");
        }
        ToolResult::success(
            skills
                .iter()
                .map(|s| format!("- {} — {}", s.name, s.description))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// skill_load — полное тело одного навыка.
pub struct SkillLoadTool;

#[async_trait]
impl Tool for SkillLoadTool {
    fn name(&self) -> &'static str {
        "skill_load"
    }

    fn description(&self) -> &'static str {
        "Загрузить ПОЛНУЮ инструкцию md-навыка перед его выполнением. Вызывай сразу, как только задача совпала с описанием навыка из AVAILABLE SKILLS."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string", "description": "Имя навыка"}},
            "required": ["name"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let Some(name) = args.get("name").and_then(|n| n.as_str()) else {
            return ToolResult::error("нужен 'name'");
        };
        match load_by_name(name) {
            Some(s) => ToolResult::success(format!(
                "=== SKILL: {} ===\n{}\n(whenToUse: {})",
                s.name,
                s.body,
                s.when_to_use.as_deref().unwrap_or("-")
            )),
            None => ToolResult::error(format!("навык '{name}' не найден (skill_list_md)")),
        }
    }
}

/// skill_create — сохранить новую процедуру в md.
pub struct SkillCreateTool;

#[async_trait]
impl Tool for SkillCreateTool {
    fn name(&self) -> &'static str {
        "skill_create"
    }

    fn description(&self) -> &'static str {
        "Сохранить новую пользовательскую процедуру как md-навык (станет видна в AVAILABLE SKILLS и переживёт перезапуск). Используй, когда выработал с пользователем повторяемую последовательность действий."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Короткое kebab-case имя"},
                "description": {"type": "string", "description": "Одна строка — что делает и КОГДА применять"},
                "content": {"type": "string", "description": "Пошаговая инструкция (markdown)"}
            },
            "required": ["name", "description", "content"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let (Some(name), Some(desc), Some(content)) = (
            args.get("name").and_then(|v| v.as_str()),
            args.get("description").and_then(|v| v.as_str()),
            args.get("content").and_then(|v| v.as_str()),
        ) else {
            return ToolResult::error("нужны 'name', 'description', 'content'");
        };
        let safe: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect();
        if safe.is_empty() || safe.starts_with('-') {
            return ToolResult::error("некорректное имя навыка");
        }
        let dir = skills_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return ToolResult::error(format!("не удалось создать папку навыков: {e}"));
        }
        let path = dir.join(format!("{safe}.md"));
        if path.exists() && !args.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(false) {
            return ToolResult::error(format!(
                "навык '{safe}' уже существует. Передай overwrite:true для замены."
            ));
        }
        let file = format!(
            "---\nname: {safe}\ndescription: {}\n---\n{}\n",
            desc.replace('\n', " "),
            content.trim()
        );
        if let Err(e) = std::fs::write(&path, &file) {
            return ToolResult::error(format!("запись не удалась: {e}"));
        }
        ToolResult::success(format!(
            "✅ Навык сохранён: {} ({} байт). Теперь он виден в AVAILABLE SKILLS.",
            path.display(),
            file.len()
        ))
    }
}

pub fn create_md_skill_tools() -> Vec<(&'static str, Arc<dyn Tool>)> {
    use std::sync::Arc;
    vec![
        ("skill_list_md", Arc::new(SkillMdListTool)),
        ("skill_load", Arc::new(SkillLoadTool)),
        ("skill_create", Arc::new(SkillCreateTool)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sample(dir: &Path, name: &str, desc: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.md")),
            format!("---\nname: {name}\ndescription: {desc}\nwhenToUse: тест\n---\n{body}\n"),
        )
        .unwrap();
    }

    #[test]
    fn test_parse_and_list() {
        let d = tempfile::TempDir::new().unwrap();
        write_sample(d.path(), "deploy", "Задеплоить сайт.", "1. build\n2. rsync");
        // мусорный файл без frontmatter игнорируется
        std::fs::write(d.path().join("junk.md"), "без фронтматтера").unwrap();
        let skills = list_in(d.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "deploy");
        assert_eq!(skills[0].description, "Задеплоить сайт.");
        assert!(skills[0].body.contains("rsync"));
    }

    #[test]
    fn test_load_by_name() {
        let home = dirs::home_dir().unwrap();
        let real_dir = home.join(".hephaestus/skills");
        std::fs::create_dir_all(&real_dir).unwrap();
        write_sample(&real_dir, "zz_test_skill_probe", "probe", "body-probe");
        let s = load_by_name("zz_test_skill_probe").expect("найден");
        assert_eq!(s.body, "body-probe");
        // убираем за собой
        let _ = std::fs::remove_file(real_dir.join("zz_test_skill_probe.md"));
        assert!(load_by_name("zz_test_skill_probe").is_none());
    }
}
