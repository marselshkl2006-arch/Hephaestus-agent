//! File Cache — кэш содержимого файлов, инвалидируемый по mtime.
//!
//! Экономит повторное чтение+UTF-8-декодирование одного и того же файла,
//! если агент читает его несколько раз за сессию (частый паттерн: читает
//! файл, редактирует, снова читает для проверки).

use std::path::Path;
use std::time::SystemTime;
use moka::sync::Cache;

#[derive(Clone)]
struct Entry {
    mtime: SystemTime,
    content: String,
}

#[derive(Clone)]
pub struct FileCache {
    inner: Cache<String, Entry>,
}

impl FileCache {
    pub fn new(max_entries: u64) -> Self {
        Self {
            inner: Cache::new(max_entries),
        }
    }

    /// Вернуть кэшированное содержимое, если файл не менялся с момента кэширования.
    pub fn get_if_fresh(&self, path: &Path) -> Option<String> {
        let key = path.to_string_lossy().to_string();
        let entry = self.inner.get(&key)?;
        let current_mtime = std::fs::metadata(path).ok()?.modified().ok()?;
        if current_mtime == entry.mtime {
            Some(entry.content.clone())
        } else {
            self.inner.invalidate(&key);
            None
        }
    }

    /// Сырой peek БЕЗ проверки свежести: (mtime на момент кэширования,
    /// content). Нужен edit-инструменту для детекта устаревания — САМО
    /// расхождение mtime и есть сигнал «файл изменился после file_read».
    pub fn peek_cached(&self, path: &Path) -> Option<(SystemTime, String)> {
        let key = path.to_string_lossy().to_string();
        let entry = self.inner.get(&key)?;
        Some((entry.mtime, entry.content))
    }

    pub fn put(&self, path: &Path, content: String) {
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(mtime) = meta.modified() {
                let key = path.to_string_lossy().to_string();
                self.inner.insert(key, Entry { mtime, content });
            }
        }
    }

    pub fn invalidate(&self, path: &Path) {
        let key = path.to_string_lossy().to_string();
        self.inner.invalidate(&key);
    }

    pub fn stats(&self) -> String {
        format!("entries≈{}", self.inner.entry_count())
    }
}

impl Default for FileCache {
    fn default() -> Self {
        Self::new(500)
    }
}
