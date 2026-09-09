//! System Prompt — компактное «поведенческое ядро» агента.
//!
//! Переработано из утечьших промптов Claude Code (Anthropic): честность
//! о результатах, дисциплина инструментов, точный скоуп, верификация.
//!
//! Текст ядра живёт в docs/system_prompt_core.md (include_str!) и МОЖЕТ
//! быть переопределён файлом ~/.hephaestus/prompt_core.md без
//! перекомпиляции. Строки "%% ..." в файле вырезаются (это комментарии
//! для человека). Живые данные (git-ветка, грязные файлы, глоссарий
//! инструментов из learning, avoid-подсказки) добавляются секциями
//! ниже ядра. Итог жёстко ограничен по размеру.

use std::path::Path;

/// Бюджет системного промпта в символах (~2600 токенов). Достаточно для
/// ядра + проекта + скиллов/памяти, но далеко от «съесть контекст».
pub const MAX_SYSTEM_CHARS: usize = 8000;
/// Мягкие лимиты отдельных секций (подрезаются первыми при переполнении).
const CAP_PROJECT: usize = 2000;
const CAP_SKILLS: usize = 1200;
const CAP_MEMORY: usize = 1000;
const CAP_SESSION_HINTS: usize = 1600;

const CORE_DEFAULT: &str = include_str!("../docs/system_prompt_core.md");

/// Дополнительные живые данные для ENV-секции.
#[derive(Default)]
pub struct EnvExtras {
    pub git_branch: Option<String>,
    pub dirty_files: Vec<String>,
    /// Глоссарий инструментов: "bash 12✓/0✗, file_edit 3✓/2✗, ..."
    pub glossary: String,
    /// "Избегать": инструменты с повторяющимися ошибками + причина.
    pub avoid: Vec<String>,
    /// Индекс md-навыков: "- name — description" построчно.
    pub skills_index: String,
    /// Индекс долговременной памяти: "- [name](file.md) — desc".
    pub memory_index: String,
}

fn strip_comments_and_trim(md: &str) -> String {
    let kept: Vec<&str> = md.lines().filter(|l| !l.trim_start().starts_with("%%")).collect();
    kept.join("\n").trim().to_string()
}

fn core_text() -> String {
    let override_path = dirs::home_dir()
        .map(|h| h.join(".hephaestus/prompt_core.md"))
        .unwrap_or_default();
    let raw = std::fs::read_to_string(&override_path).unwrap_or_else(|_| CORE_DEFAULT.to_string());
    strip_comments_and_trim(&raw)
}

/// Собрать системный промпт. `hints` — динамические секции агента
/// (learning/план), идут последними; итог обрезается до лимита.
pub fn build(
    workdir: &Path,
    provider: &str,
    model: &str,
    extras: &EnvExtras,
    hints: Option<&str>,
) -> String {
    let git = workdir.join(".git").exists();

    let mut env = format!(
        "\n\nENVIRONMENT\n- Working directory: {wd}\n- Platform: linux (shell: bash)\n- Provider/model: {prov}/{model}\n- Git repository here: {git}",
        wd = workdir.display(),
        prov = provider,
        model = model,
        git = if git { "yes" } else { "no" },
    );
    if let Some(b) = &extras.git_branch {
        env.push_str(&format!("\n- Git branch: {b}"));
    }
    if !extras.dirty_files.is_empty() {
        env.push_str("\n- Changed files (uncommitted): ");
        env.push_str(&extras.dirty_files.join(", "));
    }
    if !extras.glossary.is_empty() {
        env.push_str("\n\nTOOL NOTES (your recent stats)\n- ");
        env.push_str(&extras.glossary);
    }
    if !extras.skills_index.is_empty() {
        env.push_str("\n\nAVAILABLE SKILLS (load full instructions with skill_load before following)\n");
        env.push_str(&extras.skills_index);
    }
    if !extras.memory_index.is_empty() {
        env.push_str("\n\nMEMORY (persistent facts; details via memory_file_load)\n");
        env.push_str(&extras.memory_index);
    }
    for a in &extras.avoid {
        env.push_str(&format!("\n⚠ AVOID: {a}"));
    }
    env.push_str("\n- The conversation may be Russian: reply in it when the user writes in Russian.");

    // УМНЫЙ БЮДЖЕТ вместо жёсткой обрезки: ядро+ENV всегда целые,
    // необязательные секции подрезаются по приоритету, пока влезает.
    let mut out = String::with_capacity(MAX_SYSTEM_CHARS + 512);
    out.push_str(&core_text());
    out.push_str(&env);

    let fits = |out: &str, extra: &str| out.chars().count() + extra.chars().count() <= MAX_SYSTEM_CHARS;

    // Приоритет 1: инструкции проекта (GEFEST.md) уже внутри env? Нет —
    // они приходят в hints первыми строками; поэтому hints обрабатываем
    // с повышенным лимитом ниже.

    for (title, text, cap) in [
        ("AVAILABLE SKILLS", extras.skills_index.as_str(), CAP_SKILLS),
        ("MEMORY", extras.memory_index.as_str(), CAP_MEMORY),
    ] {
        if text.trim().is_empty() { continue; }
        let mut t: String = text.trim().chars().take(cap).collect();
        t.push('\n');
        let block = format!("\n\n{title} (load details via skill_load / memory_file_load)\n{t}");
        if fits(&out, &block) {
            out.push_str(&block);
        }
    }

    if !extras.avoid.is_empty() {
        let avoid_text = extras.avoid.join("\n⚠ ");
        let block = format!("\n\n⚠ AVOID (repeated failures)\n⚠ {avoid_text}");
        if fits(&out, &block) {
            out.push_str(&block);
        }
    }

    if let Some(h) = hints.map(str::trim).filter(|h| !h.is_empty()) {
        // SESSION HINTS (включая PROJECT INSTRUCTIONS) — до CAP_SESSION_HINTS,
        // но не в ущерб уже добавленному.
        let remaining = MAX_SYSTEM_CHARS.saturating_sub(out.chars().count()) + CAP_SESSION_HINTS;
        let cap = CAP_SESSION_HINTS.min(remaining);
        let trimmed: String = h.chars().take(cap).collect();
        let block = format!("\n\nSESSION HINTS\n{trimmed}");
        if fits(&out, &block) {
            out.push_str(&block);
        } else {
            // Гарантированный минимум: хотя бы первые 400 символов hints.
            let minimal: String = h.chars().take(400).collect();
            let block = format!("\n\nSESSION HINTS\n{minimal}");
            if fits(&out, &block) { out.push_str(&block); }
        }
    }

    // Предупреждение только если даже ЯДРО не влезло (реальная деградация).
    let core_env_len = core_text().chars().count() + env.chars().count();
    if core_env_len > MAX_SYSTEM_CHARS {
        crate::logging_system::warning(&format!(
            "[system-prompt] ядро+ENV ({core_env_len}) превышает бюджет {MAX_SYSTEM_CHARS}"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_size_content_and_no_comments() {
        let ex = EnvExtras {
            git_branch: Some("main".into()),
            ..Default::default()
        };
        let p = build(Path::new("/tmp"), "custom", "deepseek-chat", &ex, None);
        assert!(p.chars().count() <= MAX_SYSTEM_CHARS);
        assert!(p.contains("Working directory: /tmp"));
        assert!(p.contains("deepseek-chat"));
        assert!(p.contains("Git branch: main"));
        assert!(p.contains("EXAMPLES")); // few-shot на месте
        assert!(!p.contains("%% Этот файл")); // комментарии вырезаны
        assert!(p.contains("Never invent"));
    }

    #[test]
    fn test_glossary_avoid_and_cap() {
        let ex = EnvExtras {
            glossary: "bash 12✓/0✗".into(),
            avoid: vec!["file_edit — 3 failures".into()],
            ..Default::default()
        };
        let big_hint = "H".repeat(MAX_SYSTEM_CHARS * 2);
        let p = build(Path::new("/tmp"), "ollama", "m", &ex, Some(&big_hint));
        assert!(p.chars().count() <= MAX_SYSTEM_CHARS);
        assert!(p.contains("bash 12✓"));
        // avoid до обрезки — важнее подсказок
        assert!(p.contains("AVOID"));
    }
}
