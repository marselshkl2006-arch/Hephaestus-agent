//! WorkDir — ДИНАМИЧЕСКАЯ рабочая директория инструментов.
//!
//! Проблема, которую решает: раньше `workspace_root`/`working_dir`
//! инструментов (file_write, bash, git, glob, ...) замораживались ОДИН
//! раз при старте процесса (`std::env::current_dir()` в момент
//! `create_tools()`). Если агент запущен из одной папки, а работать
//! надо с другой (типичный случай — Telegram-бот: процесс поднят где
//! попало), относительные пути резолвились мимо — файлы писались НЕ
//! туда, куда думал пользователь ("мы с ним искали файл час"), а
//! file_read не находил то, что bash только что создал.
//!
//! Решение: один `Arc<RwLock<PathBuf>>`, который видят ВСЕ инструменты
//! и Agent. Смена директории (`/work_dir` в REPL, `/workdir <путь>` +
//! подтверждение доверия в Telegram) мгновенно действует на все
//! инструменты без пересоздания реестра и перезапуска процесса.
//!
//! Подтверждение доверия ("Do you trust the files in this folder?") —
//! как в Claude Code: смена рабочей директории даёт агенту доступ на
//! запись в новое место, поэтому это осознанное действие пользователя,
//! а не то, что LLM может сделать сама тихо.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Разделяемая рабочая директория. Clone — дёшево (Arc).
#[derive(Clone)]
pub struct WorkDir(Arc<RwLock<PathBuf>>);

impl WorkDir {
    pub fn new(initial: PathBuf) -> Self {
        Self(Arc::new(RwLock::new(initial)))
    }

    /// Из текущей директории процесса (поведение по умолчанию).
    pub fn from_cwd() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    pub fn get(&self) -> PathBuf {
        self.0.read().map(|g| g.clone()).unwrap_or_else(|_| PathBuf::from("."))
    }

    pub fn set(&self, path: PathBuf) {
        if let Ok(mut guard) = self.0.write() {
            *guard = path;
        }
    }

    /// Резолв пути относительно ТЕКУЩЕЙ рабочей директории:
    /// абсолютный путь — как есть; `~/...` — домашняя папка;
    /// относительный — join с workdir. Именно этот метод должны
    /// использовать инструменты вместо `std::env::current_dir()`.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let expanded = expand_home(path);
        if expanded.is_absolute() {
            expanded
        } else {
            self.get().join(expanded)
        }
    }
}

/// Расширение `~` и `~/...` до домашней папки. Пути из чата Telegram
/// часто приходят именно в таком виде; `std::env::current_dir()` их не
/// понимает, а bash понимал бы — теперь поведение единое.
pub fn expand_home(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    if let Some(rest) = trimmed.strip_prefix("~/").or_else(|| trimmed.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

/// Валидация кандидата в рабочие директории для /work_dir//workdir:
/// путь должен существовать и быть папкой. Возвращает канонический
/// путь (без симлинков и "..") или текст ошибки для показа пользователю.
pub fn validate_candidate(path: &str) -> Result<PathBuf, String> {
    let expanded = expand_home(path);
    if expanded.as_os_str().is_empty() {
        return Err("Путь пустой.".to_string());
    }
    let canonical = expanded
        .canonicalize()
        .map_err(|e| format!("{}: {}", expanded.display(), e))?;
    if !canonical.is_dir() {
        return Err(format!("{} — не директория.", canonical.display()));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_home() {
        if dirs::home_dir().is_none() {
            return;
        }
        assert_eq!(expand_home("~"), dirs::home_dir().unwrap());
        assert_eq!(expand_home(" ~/foo "), dirs::home_dir().unwrap().join("foo"));
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_home("rel/path"), PathBuf::from("rel/path"));
    }

    #[test]
    fn test_workdir_dynamic_resolve() {
        let tmp = tempfile::TempDir::new().unwrap();
        let other = tempfile::TempDir::new().unwrap();
        let wd = WorkDir::new(tmp.path().to_path_buf());

        // Относительный путь резолвится от текущего значения...
        assert_eq!(
            wd.resolve("file.txt"),
            tmp.path().join("file.txt")
        );
        // ...и ПОСЛЕ смены — уже от нового (это и есть фикс "файлы писались
        // не туда": инструменты видят смену директории без пересоздания).
        wd.set(other.path().to_path_buf());
        assert_eq!(
            wd.resolve("file.txt"),
            other.path().join("file.txt")
        );
        // Абсолютный — всегда как есть.
        assert_eq!(wd.resolve("/etc/passwd"), PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn test_validate_candidate() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(validate_candidate(tmp.path().to_str().unwrap()).is_ok());
        assert!(validate_candidate("/definitely/not/exist/hephaestus").is_err());
        let file_path = tmp.path().join("f.txt");
        std::fs::write(&file_path, "x").unwrap();
        assert!(validate_candidate(file_path.to_str().unwrap()).is_err());
    }

    #[test]
    fn test_get_set_threaded() {
        let wd = WorkDir::from_cwd();
        let wd2 = wd.clone();
        let h = std::thread::spawn(move || wd2.set(PathBuf::from("/tmp")));
        h.join().unwrap();
        assert_eq!(wd.get(), PathBuf::from("/tmp"));
    }
}
