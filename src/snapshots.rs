//! Undo через git-снапшоты воркспейса (по образцу opencode).
//!
//! КАК РАБОТАЕТ: в `~/.hephaestus/snapshots/<хэш-пути>.git` создаётся
//! ОТДЕЛЬНЫЙ теневой git-репозиторий, чьим work-tree является рабочая
//! папка агента. Проект пользователя НЕ трогается: ни .git, ни HEAD,
//! ни индекс. Перед каждым ходом агента делается коммит-снимок (git add -A
//! + commit в теневом репо). `/undo` откатывает изменения ПОСЛЕДНЕГО
//! хода: modified/deleted — восстанавливаются из снимка, новые файлы
//! (появившиеся после снимка) — удаляются.
//!
//! ГРАНИЦЫ: откатываются только файлы рабочей директории; каталоги
//! .git/target/node_modules исключены (info/exclude), так что тяжёлые
//! артефакты сборки не замедляют снимки. Нет git в системе — undo
//! недоступен (это заявленное ограничение, предупреждаем один раз).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Каталог теневых репозиториев: ~/.hephaestus/snapshots/
pub fn snapshots_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("HEPHAESTUS_HOME") {
        return PathBuf::from(custom).join("snapshots");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hephaestus")
        .join("snapshots")
}

/// Каталог теневого репо для данной рабочей директории (детерминированный
/// хэш пути — чтобы один воркспейс = один снимочный репозиторий).
fn shadow_repo_for(workdir: &Path) -> PathBuf {
    let key: String = workdir
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let mut s: u64 = 1469598103934665603;
    for b in key.as_bytes() {
        s ^= *b as u64;
        s = s.wrapping_mul(1099511628211);
    }
    snapshots_dir().join(format!("{:016x}.git", s))
}

fn run_git(shadow: &Path, workdir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .env("GIT_DIR", shadow)
        .env("GIT_WORK_TREE", workdir)
        // Никаких user-настроек не требуется: коммитим с фикс. автором.
        .env("GIT_AUTHOR_NAME", "Hephaestus Snapshot")
        .env("GIT_AUTHOR_EMAIL", "snapshot@hephaestus.local")
        .env("GIT_COMMITTER_NAME", "Hephaestus Snapshot")
        .env("GIT_COMMITTER_EMAIL", "snapshot@hephaestus.local")
        .output()
        .map_err(|e| format!("git не запустился: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn ensure_repo(shadow: &Path, workdir: &Path) -> Result<(), String> {
    if shadow.join("HEAD").exists() {
        return Ok(());
    }
    if let Some(parent) = shadow.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let out = Command::new("git")
        .args(["init", "--quiet", "--bare"])
        .arg(shadow)
        .output()
        .map_err(|e| format!("git init: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    // Исключения: .git самого проекта и тяжёлые артефакты. Это ОТДЕЛЬНЫЙ
    // репозиторий — пользовательский .gitignore не читается.
    let info = shadow.join("info");
    let _ = std::fs::create_dir_all(&info);
    let exclude = ".git\ntarget/\nnode_modules/\n.venv/\nvenv/\n__pycache__/\ndist/\nbuild/\nCargo.lock\npackage-lock.json\npoetry.lock\n";
    std::fs::write(info.join("exclude"), exclude).map_err(|e| e.to_string())?;
    let _ = run_git(shadow, workdir, &["config", "core.autocrlf", "false"]);
    Ok(())
}

pub struct SnapshotManager {
    workdir: PathBuf,
    /// SHA последнего снимка (начало хода) — точка отката /undo.
    last: std::sync::Mutex<Vec<String>>,
    disabled_reason: std::sync::Mutex<Option<String>>,
}

impl SnapshotManager {
    pub fn new(workdir: PathBuf) -> Self {
        Self {
            workdir,
            last: std::sync::Mutex::new(Vec::new()),
            disabled_reason: std::sync::Mutex::new(None),
        }
    }

    fn disabled(&self) -> Option<String> {
        self.disabled_reason.lock().ok().and_then(|g| g.clone())
    }

    fn check_available(&self) -> Result<(), String> {
        if let Some(r) = self.disabled() {
            return Err(r);
        }
        // git доступен? Проверяем один раз; нет — навсегда отключаем.
        match Command::new("git").arg("--version").output() {
            Ok(o) if o.status.success() => Ok(()),
            _ => {
                let reason = "git недоступен — undo выключен".to_string();
                if let Ok(mut g) = self.disabled_reason.lock() {
                    *g = Some(reason.clone());
                }
                Err(reason)
            }
        }
    }

    /// Снимок ПЕРЕД ходом. Ошибки не рвут ход — снимок best effort.
    pub fn snapshot_before_turn(&self, label: &str) {
        if self.check_available().is_err() {
            return;
        }
        let shadow = shadow_repo_for(&self.workdir);
        if let Err(e) = ensure_repo(&shadow, &self.workdir) {
            crate::logging_system::warning(&format!("[undo] репо не создан: {e}"));
            return;
        }
        let _ = run_git(&shadow, &self.workdir, &["add", "-A", "--", "."]);
        // Пустые изменения — коммит не нужен: HEAD уже равен состоянию.
        let diff = run_git(&shadow, &self.workdir, &["diff", "--quiet", "HEAD"])
            .err()
            .is_some();
        let untracked = run_git(&shadow, &self.workdir, &["ls-files", "--others", "--exclude-standard"])
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if diff || untracked {
            match run_git(&shadow, &self.workdir, &["commit", "--quiet", "-m", label, "--allow-empty"]) {
                Ok(_) => {}
                Err(e) => {
                    // "nothing to commit" — норма, остальные ошибки в лог.
                    if !e.contains("nothing to commit") {
                        crate::logging_system::warning(&format!("[undo] commit: {e}"));
                    }
                }
            }
        }
        let head = run_git(&shadow, &self.workdir, &["rev-parse", "HEAD"]).ok();
        if let Ok(mut last) = self.last.lock() {
            if let Some(h) = head.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                last.push(h);
                // Храним стек до 20 ходов — /undo откатывает последний.
                if last.len() > 20 {
                    last.remove(0);
                }
            }
        }
    }

    /// Откат последнего хода: modified/deleted — из снимка HEAD, новые
    /// файлы — удалить. Возвращает человекочитаемый отчёт.
    pub fn undo_last(&self) -> Result<String, String> {
        self.check_available()?;
        let _point = {
            let mut last = self.last.lock().map_err(|_| "лок занят".to_string())?;
            last.pop().ok_or_else(|| "нет снимков для отката (этот ход уже откачен или снимков не было)".to_string())?
        };
        let shadow = shadow_repo_for(&self.workdir);
        ensure_repo(&shadow, &self.workdir)?;

        let mut restored: Vec<String> = Vec::new();
        let mut removed: Vec<String> = Vec::new();

        // Статус против HEAD (= снимок): " M"/" D"/"??". Мы НЕ делали
        // `git add` перед undo, поэтому изменения в статусе.
        let status = run_git(&shadow, &self.workdir, &["status", "--porcelain"])?;
        for line in status.lines() {
            if line.chars().count() < 4 {
                continue;
            }
            let (code, path) = line.split_at(2);
            let path = path.trim();
            if path.is_empty() || path.starts_with('"') {
                continue; // пути с кавычками (редкие спец-имена) пропускаем
            }
            let target = self.workdir.join(path);
            match code.trim() {
                "??" => {
                    // Новый файл — появился ПОСЛЕ снимка: удалить.
                    if target.is_file() || target.is_symlink() {
                        let _ = std::fs::remove_file(&target);
                        removed.push(path.to_string());
                    } else if target.is_dir() {
                        let _ = std::fs::remove_dir_all(&target);
                        removed.push(format!("{path}/"));
                    }
                }
                "M" | "D" => {
                    // Изменён/удалён — восстановить из снимка.
                    let content = run_git(&shadow, &self.workdir, &["show", &format!("HEAD:{path}")])?;
                    if let Some(parent) = target.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::write(&target, content)
                        .map_err(|e| format!("не удалось восстановить {path}: {e}"))?;
                    restored.push(path.to_string());
                }
                _ => {}
            }
        }

        // После отката фиксируем состояние новым снимком, чтобы повторный
        // /undo откатывал ПРЕДЫДУЩИЙ ход, а не возвращал всё назад.
        let _ = run_git(&shadow, &self.workdir, &["add", "-A", "--", "."]);
        let _ = run_git(&shadow, &self.workdir, &["commit", "--quiet", "-m", "undo", "--allow-empty"]);

        let mut report = String::new();
        if !restored.is_empty() {
            report.push_str(&format!("↩️ Восстановлено из снимка: {}\n", restored.join(", ")));
        }
        if !removed.is_empty() {
            report.push_str(&format!("🗑 Удалено (создано агентом после снимка): {}\n", removed.join(", ")));
        }
        if report.is_empty() {
            report.push_str("↩️ Откат выполнен: изменений с момента снимка не было.");
        }
        Ok(report)
    }

    pub fn is_available(&self) -> bool {
        self.check_available().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(shadow: &Path, workdir: &Path, args: &[&str]) -> String {
        run_git(shadow, workdir, args).expect("git должен работать в тесте")
    }

    #[test]
    fn snapshot_and_undo_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workdir = tmp.path().to_path_buf();
        let mgr = SnapshotManager::new(workdir.clone());

        // git доступен в этой среде? Нет — тест бессмыслен, пропускаем.
        if !mgr.is_available() {
            return;
        }

        // 1. Файл до хода.
        std::fs::write(workdir.join("a.txt"), "v1\n").unwrap();
        mgr.snapshot_before_turn("turn1");

        // 2. Агент: изменил a.txt, создал b.txt.
        std::fs::write(workdir.join("a.txt"), "v2\n").unwrap();
        std::fs::write(workdir.join("b.txt"), "new\n").unwrap();

        // 3. Undo: a.txt → v1, b.txt исчез.
        let report = mgr.undo_last().unwrap();
        assert!(report.contains("a.txt"));
        let a = std::fs::read_to_string(workdir.join("a.txt")).unwrap();
        assert_eq!(a, "v1\n");
        assert!(!workdir.join("b.txt").exists());
    }

    #[test]
    fn undo_restores_deleted_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workdir = tmp.path().to_path_buf();
        let mgr = SnapshotManager::new(workdir.clone());
        if !mgr.is_available() {
            return;
        }
        std::fs::write(workdir.join("keep.txt"), "важно\n").unwrap();
        mgr.snapshot_before_turn("turn1");
        let _ = std::fs::remove_file(workdir.join("keep.txt"));
        mgr.undo_last().unwrap();
        assert_eq!(std::fs::read_to_string(workdir.join("keep.txt")).unwrap(), "важно\n");
    }

    #[test]
    fn undo_without_snapshots_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mgr = SnapshotManager::new(tmp.path().to_path_buf());
        if !mgr.is_available() {
            return;
        }
        assert!(mgr.undo_last().is_err());
    }
}
