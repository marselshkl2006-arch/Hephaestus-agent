//! Bootstrap первого запуска: примеры конфигов ВШИТЫ в бинарник и
//! распаковываются в ~/.hephaestus/ у каждого пользователя свои.
//!
//! ЗАЧЕМ: бинарник не носит с собой папку конфигов (их бы пришлось
//! тащить рядом и следить за путями) — вместо этого при первом старте
//! создаётся ~/.hephaestus с config.example.toml, mcp.example.toml и
//! README с инструкцией по токену Telegram. Реальные конфиги пользователь
//! правит сам; агент НИКОГДА не перезаписывает существующие файлы.
//!
//! ТОКЕН TELEGRAM: получают у @BotFather, сохраняют командой
//! `/telegram save <токен>` или кладут в ~/.hephaestus/telegram_token.txt.
//! Он персональный — в npm-пакет и в бинарь не входит никогда.

use std::path::PathBuf;

pub const CONFIG_EXAMPLE: &str = include_str!("../assets/config.example.toml");
pub const MCP_EXAMPLE: &str = include_str!("../assets/mcp.example.toml");
pub const README: &str = include_str!("../assets/README.first-run.md");

/// Каталог данных (учитывает HEPHAESTUS_HOME).
pub fn hephaestus_home() -> PathBuf {
    if let Ok(custom) = std::env::var("HEPHAESTUS_HOME") {
        return PathBuf::from(custom);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hephaestus")
}

/// Распаковать примеры, если их ещё нет. Возвращает список созданных
/// файлов (для подсказки при первом запуске). Существующие НЕ трогаем.
pub fn ensure_examples() -> Vec<PathBuf> {
    let home = hephaestus_home();
    let _ = std::fs::create_dir_all(&home);
    let mut created = Vec::new();

    let files: Vec<(&str, &str)> = vec![
        ("config.example.toml", CONFIG_EXAMPLE),
        ("mcp.example.toml", MCP_EXAMPLE),
        ("README.first-run.md", README),
    ];
    for (name, content) in files {
        let path = home.join(name);
        if !path.exists() {
            if std::fs::write(&path, content).is_ok() {
                created.push(path);
            }
        }
    }
    created
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_examples_into_isolated_home() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("HEPHAESTUS_HOME", tmp.path());
        std::fs::remove_dir_all(tmp.path()).ok();

        let created = ensure_examples();
        assert!(created.iter().any(|p| p.file_name().unwrap() == "config.example.toml"));
        assert!(created.iter().any(|p| p.file_name().unwrap() == "mcp.example.toml"));

        // Повторный запуск НЕ создаёт дубли/не перезаписывает.
        let again = ensure_examples();
        assert!(again.is_empty(), "повторный bootstrap ничего не создаёт");

        // Примеры — валидный TOML (иначе бы мы раздали мусор).
        let cfg = std::fs::read_to_string(tmp.path().join("config.example.toml")).unwrap();
        assert!(cfg.parse::<toml::Value>().is_ok(), "config.example.toml битый: {cfg}");
        let mcp = std::fs::read_to_string(tmp.path().join("mcp.example.toml")).unwrap();
        assert!(mcp.parse::<toml::Value>().is_ok(), "mcp.example.toml битый");

        std::env::remove_var("HEPHAESTUS_HOME");
    }
}
