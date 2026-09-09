use std::fs;
use tokio::fs as tokio_fs;
use serde_json::Value;
use async_trait::async_trait;

use super::{ToolResult, Tool};
use super::file_cache::FileCache;
use crate::workdir::WorkDir;

// ─────────────────────────────────────────────────────────── дифф ──

/// Компактный unified-подобный дифф двух текстов построчным LCS.
/// Возвращает (текст диффа, добавлено строк, удалено строк) или None,
/// если файлы идентичны либо слишком велики для подсчёта (>2000 строк —
/// осознанный предел O(n·m) таблицы).
fn unified_diff(old: &str, new: &str, max_out_lines: usize) -> Option<(String, usize, usize)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() > 2000 || b.len() > 2000 {
        return None;
    }
    // LCS-таблица
    let n = a.len();
    let m = b.len();
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[idx(i, j)] = if a[i] == b[j] {
                dp[idx(i + 1, j + 1)] + 1
            } else {
                dp[idx(i + 1, j)].max(dp[idx(i, j + 1)])
            };
        }
    }
    // Проход по таблице -> список операций
    #[derive(PartialEq)]
    enum Op<'x> {
        Same(&'x str),
        Del(&'x str),
        Ins(&'x str),
    }
    let mut ops: Vec<Op> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Same(a[i]));
            i += 1;
            j += 1;
        } else if dp[idx(i + 1, j)] >= dp[idx(i, j + 1)] {
            ops.push(Op::Del(a[i]));
            i += 1;
        } else {
            ops.push(Op::Ins(b[j]));
            j += 1;
        }
    }
    while i < n { ops.push(Op::Del(a[i])); i += 1; }
    while j < m { ops.push(Op::Ins(b[j])); j += 1; }

    // Схлопываем в ханки: контекст 2 строки вокруг блоков изменений.
    let mut out = String::new();
    let (mut added, mut deleted) = (0usize, 0usize);
    let mut k = 0usize;
    let mut emitted = 0usize;
    while k < ops.len() {
        if matches!(ops[k], Op::Same(_)) {
            k += 1;
            continue;
        }
        // Начало ханка: отступаем 2 строки контекста.
        let hunk_start = k.saturating_sub(2);
        let mut end = k;
        while end < ops.len() {
            if !matches!(ops[end], Op::Same(_)) {
                end += 1;
            } else {
                // Две одинаковые строки подряд = конец ханка.
                if end + 1 < ops.len() && matches!(ops[end + 1], Op::Same(_)) {
                    break;
                }
                end += 1;
            }
        }
        let hunk_end = (end + 2).min(ops.len());
        if out.is_empty() {
            out.push_str("--- a\n+++ b\n");
        } else {
            out.push_str("...\n");
        }
        for op in &ops[hunk_start..hunk_end] {
            match op {
                Op::Same(l) => { if emitted < max_out_lines { out.push_str(&format!("  {l}\n")); emitted += 1; } }
                Op::Del(l) => { deleted += 1; if emitted < max_out_lines { out.push_str(&format!("- {l}\n")); emitted += 1; } }
                Op::Ins(l) => { added += 1; if emitted < max_out_lines { out.push_str(&format!("+ {l}\n")); emitted += 1; } }
            }
        }
        k = hunk_end;
    }
    if out.is_empty() {
        return None;
    }
    Some((out, added, deleted))
}

/// Фаззи-поиск фрагмента old_text в content: точное совпадение -> байтовый
/// диапазон; иначе скользящее окно ПО СТРОКАМ со сравнением нормализованных
/// (обрезанные края + схлопнутые пробелы) представлений. Возвращает
/// диапазон для замены.
fn find_fuzzy_span(content: &str, old_text: &str) -> Option<std::ops::Range<usize>> {
    if let Some(p) = content.find(old_text) {
        return Some(p..p + old_text.len());
    }
    let norm = |s: &str| -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let target: Vec<String> = old_text.lines().map(norm).collect();
    if target.iter().all(|l| l.is_empty()) {
        return None;
    }
    let lines: Vec<(usize, usize)> = {
        // (байт-старт строки, байт-конец включая \n)
        let mut v = Vec::new();
        let mut off = 0usize;
        for l in content.split_inclusive('\n') {
            v.push((off, off + l.len()));
            off += l.len();
        }
        v
    };
    let normed: Vec<String> = content.lines().map(|l| norm(l.trim_end())).collect();
    let w = target.len();
    if w == 0 || normed.len() < w {
        return None;
    }
    for start in 0..=(normed.len() - w) {
        if normed[start..start + w] == target[..] {
            let byte_from = lines.get(start).map(|(s, _)| *s).unwrap_or(0);
            let byte_to = lines.get(start + w - 1).map(|(_, e)| *e).unwrap_or(content.len());
            // Не захватываем завершающий \n последней строки окна в замену?
            // Захватываем целиком — new_text отвечает за свои переносы.
            return Some(byte_from..byte_to);
        }
    }
    None
}

pub struct FileReadTool {
    max_file_size: u64,
    cache: FileCache,
    workdir: WorkDir,
}

impl FileReadTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self { max_file_size: 10 * 1024 * 1024, cache: FileCache::default(), workdir }
    }

    pub fn with_cache(cache: FileCache, workdir: WorkDir) -> Self {
        Self { max_file_size: 10 * 1024 * 1024, cache, workdir }
    }
}

#[async_trait]
impl Tool for FileReadTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let file_path = args.get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // ИСПРАВЛЕНО (жалоба "агент не может писать/читать файлы из
        // telegram-бота"): раньше здесь было std::env::current_dir(),
        // захваченное на момент старта ПРОЦЕССА — при запуске не из той
        // папки, где надо работать (типично для --telegram), относительные
        // пути резолвились мимо. Теперь — общая динамическая WorkDir
        // (меняется /work_dir в REPL и /workdir <путь> в Telegram).
        let path = self.workdir.resolve(file_path);

        if !path.exists() {
            // Подсказка с резолвнутым путём и рабочей директорией:
            // модель часто зовёт относительным путём не из той папки —
            // сразу видно, ГДЕ искали, и она исправляется за один шаг.
            return ToolResult::error(format!(
                "Файл не найден.\nИскали: {}\nРезолвнуто как: {}\nРабочая директория: {}\nИспользуйте абсолютный путь либо уточните имя.",
                file_path,
                path.display(),
                self.workdir.get().display()
            ));
        }

        if !path.is_file() {
            return ToolResult::error(format!("Not a file: {}", file_path));
        }

        if let Some(cached) = self.cache.get_if_fresh(&path) {
            return ToolResult::success(cached);
        }

        let metadata = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => return ToolResult::error(format!("Failed to read metadata: {}", e)),
        };

        if metadata.len() > self.max_file_size {
            return ToolResult::error(format!("File too large: {} bytes (max: {})", metadata.len(), self.max_file_size));
        }

        let content = match tokio_fs::read(&path).await {
            Ok(data) => data,
            Err(e) => return ToolResult::error(format!("Failed to read file: {}", e)),
        };

        if content.iter().any(|&b| b == 0) {
            return ToolResult::error("Binary file detected");
        }

        match String::from_utf8(content) {
            Ok(text) => {
                self.cache.put(&path, text.clone());
                ToolResult::success(text)
            }
            Err(_) => ToolResult::error("Unable to decode file with UTF-8"),
        }
    }

    fn description(&self) -> &'static str {
        "Прочитать содержимое файла"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"file_path":{"type":"string","description":"Путь к файлу"}},"required":["file_path"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "file_read"
    }
}

pub struct FileWriteTool {
    workdir: WorkDir,
}

impl FileWriteTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let file_path = args.get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let content = args.get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Динамическая рабочая директория вместо замороженной на старте:
        // раньше файл писался в папку запуска процесса, а не туда, где
        // пользователь ждёт результат (особенно из Telegram).
        let path = self.workdir.resolve(file_path);

        if let Some(parent) = path.parent() {
            if let Err(e) = tokio_fs::create_dir_all(parent).await {
                return ToolResult::error(format!("Failed to create directories: {}", e));
            }
        }

        if content.len() < 100 {
            let lower = content.to_lowercase();
            let suspicious = [
                "вставьте здесь", "insert here", "updated code",
                "обновленный код", "placeholder", "todo:",
                "// add code", "# add code"
            ];
            for phrase in suspicious {
                if lower.contains(phrase) {
                    return ToolResult::error(format!(
                        "CRITICAL ERROR: You wrote a placeholder/description instead of actual code! Content: '{}'. You MUST write real, compilable code, not comments or descriptions!",
                        &crate::truncate_chars(&content, 100)
                    ));
                }
            }
        }

        // ДИФФ вместо немого перезаписи: если файл существовал — показываем
        // что именно поменялось (+/− по строкам). Модель и человек сразу
        // видят результат правки, а не догадываются.
        let prev = tokio_fs::read_to_string(&path).await.ok();
        match tokio_fs::write(&path, content.as_bytes()).await {
            Ok(_) => {
                let diff_note = match prev {
                    Some(prev) => match unified_diff(&prev, content, 40) {
                        Some((d, a, r)) => format!("\nИзменения (+{a}/−{r}):\n{d}"),
                        None => "\n(содержимое не изменилось)".to_string(),
                    },
                    None => format!("\nНовый файл: {} строк", content.lines().count()),
                };
                ToolResult::success(format!("File written: {} ({} bytes){}", file_path, content.len(), diff_note))
            }
            Err(e) => ToolResult::error(format!("Error writing file: {}", e)),
        }
    }

    fn description(&self) -> &'static str {
        "Записать содержимое в файл (перезаписывает целиком)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"file_path":{"type":"string","description":"Путь к файлу"},"content":{"type":"string","description":"Новое содержимое файла"}},"required":["file_path","content"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "file_write"
    }
}

pub struct FileEditTool {
    workdir: WorkDir,
}

impl FileEditTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for FileEditTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let file_path = args.get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let old_text = args.get("old_text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_text = args.get("new_text")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if file_path.is_empty() || old_text.is_empty() {
            return ToolResult::error("Missing required parameters: file_path and old_text");
        }

        // Read the file
        let read_tool = FileReadTool::new(self.workdir.clone());
        let read_result = read_tool.execute(&serde_json::json!({ "file_path": file_path })).await;
        if !read_result.success {
            return read_result;
        }

        let content = read_result.output;
        // ФАЗЗИ-МАТЧИНГ: точное совпадение -> замена; иначе поиск окна
        // строк с нормализованными пробелами (модель часто промахивается
        // на пару пробелов/переносов — раньше это был жёсткий отказ).
        let new_content = match find_fuzzy_span(&content, old_text) {
            Some(span) => {
                let mut s = String::with_capacity(content.len());
                s.push_str(&content[..span.start]);
                s.push_str(new_text);
                s.push_str(&content[span.end..]);
                s
            }
            None => {
                return ToolResult::error(format!(
                    "old_text не найден даже фаззи-поиском (первые 120 симв.: {}…)",
                    old_text.chars().take(120).collect::<String>()
                ))
            }
        };

        // Write the file
        let diff_note = unified_diff(&content, &new_content, 40)
            .map(|(d, a, r)| format!("\nИзменения (+{a}/−{r}):\n{d}"))
            .unwrap_or_default();
        let write_tool = FileWriteTool::new(self.workdir.clone());
        let write_result = write_tool.execute(&serde_json::json!({
            "file_path": file_path,
            "content": new_content
        })).await;

        if !write_result.success {
            return write_result;
        }

        // Убираем дублирующий дифф от write, оставляя свой компактный.
        ToolResult::success(format!("File edited: {}{}", file_path, diff_note))
    }

    fn description(&self) -> &'static str {
        "Заменить точное вхождение old_text на new_text в файле"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"file_path":{"type":"string","description":"Путь к файлу"},"old_text":{"type":"string","description":"Текст, который нужно найти (должен встречаться ровно один раз)"},"new_text":{"type":"string","description":"Текст на замену"}},"required":["file_path","old_text","new_text"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "file_edit"
    }
}

pub struct FileDeleteTool {
    workdir: WorkDir,
    /// Удаление файла — необратимое действие: как в opencode, спрашиваем
    /// разрешение ("один раз / всегда для file_delete / нет"). Ответ
    /// "всегда" снимает вопросы на остаток сессии.
    permissions: std::sync::Arc<crate::permissions::PermissionManager>,
}

impl FileDeleteTool {
    pub fn new(
        workdir: WorkDir,
        permissions: std::sync::Arc<crate::permissions::PermissionManager>,
    ) -> Self {
        Self { workdir, permissions }
    }
}

#[async_trait]
impl Tool for FileDeleteTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let file_path = args.get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let path = self.workdir.resolve(file_path);

        if !path.exists() {
            return ToolResult::error(format!("File not found: {}", file_path));
        }

        if !path.is_file() {
            return ToolResult::error(format!("Not a file: {}", file_path));
        }

        // Подтверждение удаления (пропускается при "всегда разрешено",
        // --yes или force:true).
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        if !force && !crate::confirmation::is_auto_confirm() {
            use crate::permissions::Outcome;
            let outcome = self
                .permissions
                .request("file_delete", &format!("удалить файл {}", path.display()), "", "file_delete")
                .await;
            match outcome {
                Outcome::Granted => {}
                Outcome::Denied => {
                    return ToolResult::error(format!(
                        "🚫 Пользователь ОТКЛОНИЛ удаление {}. Уточни, как действовать.",
                        path.display()
                    ));
                }
                Outcome::Expired => {
                    return ToolResult::error(format!(
                        "⏳ Нет ответа на запрос удаления за 3 минуты — отменено ({}).",
                        path.display()
                    ));
                }
            }
        }

        match tokio_fs::remove_file(&path).await {
            Ok(_) => ToolResult::success(format!("File deleted: {}", file_path)),
            Err(e) => ToolResult::error(format!("Error deleting file: {}", e)),
        }
    }

    fn description(&self) -> &'static str {
        "Удалить файл"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"file_path":{"type":"string","description":"Путь к файлу"}},"required":["file_path"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "file_delete"
    }
}

pub struct FileMoveTool {
    workdir: WorkDir,
}

impl FileMoveTool {
    pub fn new(workdir: WorkDir) -> Self { Self { workdir } }
}

#[async_trait]
impl Tool for FileMoveTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let source = args.get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let destination = args.get("destination")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let src = self.workdir.resolve(source);
        let dst = self.workdir.resolve(destination);

        if !src.exists() {
            return ToolResult::error(format!("Source not found: {}", source));
        }

        if let Some(parent) = dst.parent() {
            if let Err(e) = tokio_fs::create_dir_all(parent).await {
                return ToolResult::error(format!("Failed to create directories: {}", e));
            }
        }

        match tokio_fs::rename(&src, &dst).await {
            Ok(_) => ToolResult::success(format!("Moved: {} -> {}", source, destination)),
            Err(e) => ToolResult::error(format!("Error moving file: {}", e)),
        }
    }

    fn description(&self) -> &'static str {
        "Переместить/переименовать файл"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"source":{"type":"string","description":"Текущий путь"},"destination":{"type":"string","description":"Новый путь"}},"required":["source","destination"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "file_move"
    }
}

pub struct FileCopyTool {
    workdir: WorkDir,
}

impl FileCopyTool {
    pub fn new(workdir: WorkDir) -> Self { Self { workdir } }
}

#[async_trait]
impl Tool for FileCopyTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let source = args.get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let destination = args.get("destination")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let src = self.workdir.resolve(source);
        let dst = self.workdir.resolve(destination);

        if !src.exists() {
            return ToolResult::error(format!("Source not found: {}", source));
        }

        if !src.is_file() {
            return ToolResult::error(format!("Not a file: {}", source));
        }

        if let Some(parent) = dst.parent() {
            if let Err(e) = tokio_fs::create_dir_all(parent).await {
                return ToolResult::error(format!("Failed to create directories: {}", e));
            }
        }

        match tokio_fs::copy(&src, &dst).await {
            Ok(_) => ToolResult::success(format!("Copied: {} -> {}", source, destination)),
            Err(e) => ToolResult::error(format!("Error copying file: {}", e)),
        }
    }

    fn description(&self) -> &'static str {
        "Скопировать файл"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"source":{"type":"string","description":"Путь-источник"},"destination":{"type":"string","description":"Путь назначения"}},"required":["source","destination"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "file_copy"
    }
}

pub struct FileExistsTool {
    workdir: WorkDir,
}

impl FileExistsTool {
    pub fn new(workdir: WorkDir) -> Self { Self { workdir } }
}

#[async_trait]
impl Tool for FileExistsTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let path = args.get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let p = self.workdir.resolve(path);

        if p.exists() {
            let file_type = if p.is_file() { "file" } else if p.is_dir() { "directory" } else { "other" };
            ToolResult::success(format!("exists ({})", file_type))
        } else {
            ToolResult::success("not_found".to_string())
        }
    }

    fn description(&self) -> &'static str {
        "Проверить, существует ли файл/директория по пути"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"path":{"type":"string","description":"Путь для проверки"}},"required":["path"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "file_exists"
    }
}

pub struct GlobTool {
    workdir: WorkDir,
}

impl GlobTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self { workdir }
    }
}

#[async_trait]
impl Tool for GlobTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let pattern = args.get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let pattern = if !pattern.contains('/') && !pattern.contains('*') && !pattern.contains('?') {
            format!("**/{}", pattern)
        } else {
            pattern.to_string()
        };

        let workspace_root = self.workdir.get();
        let full_pattern = workspace_root.join(&pattern);
        let pattern_str = full_pattern.to_str().unwrap_or("");

        let mut matches = Vec::new();
        for entry in glob::glob(pattern_str).unwrap_or_else(|_| glob::glob("").unwrap()) {
            if let Ok(path) = entry {
                if let Ok(rel) = path.strip_prefix(&workspace_root) {
                    if let Some(rel_str) = rel.to_str() {
                        matches.push(rel_str.to_string());
                    }
                }
            }
        }
        matches.sort();

        let output = if matches.is_empty() {
            "No matches found".to_string()
        } else {
            matches.join("\n")
        };

        ToolResult::success(output)
    }

    fn description(&self) -> &'static str {
        "Найти файлы по glob-шаблону (например **/*.rs)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"pattern":{"type":"string","description":"Glob-шаблон поиска"}},"required":["pattern"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "glob"
    }
}

#[cfg(test)]
mod diff_tests {
    use super::*;

    #[test]
    fn test_unified_diff_counts() {
        let old = "a\nb\nc\n";
        let new = "a\nX\nc\nd\n";
        let (d, add, del) = unified_diff(old, new, 40).unwrap();
        assert_eq!((add, del), (2, 1));
        assert!(d.contains("- b"));
        assert!(d.contains("+ X"));
        assert!(d.contains("+ d"));
    }

    #[test]
    fn test_identical_no_diff() {
        assert!(unified_diff("same\n", "same\n", 40).is_none());
    }

    #[test]
    fn test_fuzzy_whitespace_match() {
        let content = "fn main() {\n    let  x = 1;\n}\n";
        // Модель прислала old_text с другими пробелами.
        let old = "let x = 1;";
        let span = find_fuzzy_span(content, old).expect("фаззи должен найти");
        let mut fixed = String::new();
        fixed.push_str(&content[..span.start]);
        fixed.push_str("let y = 2;");
        fixed.push_str(&content[span.end..]);
        assert!(fixed.contains("let y = 2;"));
        assert!(!fixed.contains("let  x"));
    }

    #[test]
    fn test_fuzzy_exact_still_works() {
        let c = "hello world";
        let s = find_fuzzy_span(c, "world").unwrap();
        assert_eq!(&c[s], "world");
        assert!(find_fuzzy_span(c, "absent").is_none());
    }
}
