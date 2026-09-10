use std::fs;
use tokio::fs as tokio_fs;
use serde_json::Value;
use async_trait::async_trait;

use super::{ToolResult, Tool};
use super::file_cache::FileCache;
use crate::workdir::WorkDir;

/// Дефолтный лимит строк file_read (по образцу opencode): большой файл не
/// тащится в контекст целиком.
pub const DEFAULT_READ_LIMIT: usize = 2000;
/// Жёсткий максимум limit — модель не может запросить «весь файл на 100k строк».
pub const MAX_READ_LIMIT: usize = 10_000;

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
    /// Правила разрешений: .env/ключи → ask по дефолту, deny/allow из конфига.
    engine: std::sync::Arc<crate::permissions::PermissionEngine>,
    /// Канал вопросов для ask-правил (None — ask = отказ с объяснением).
    permissions: Option<std::sync::Arc<crate::permissions::PermissionManager>>,
}

impl FileReadTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self {
            max_file_size: 10 * 1024 * 1024,
            cache: FileCache::default(),
            workdir,
            engine: std::sync::Arc::new(crate::permissions::PermissionEngine::new(vec![])),
            permissions: None,
        }
    }

    pub fn with_cache(
        cache: FileCache,
        workdir: WorkDir,
        engine: std::sync::Arc<crate::permissions::PermissionEngine>,
        permissions: std::sync::Arc<crate::permissions::PermissionManager>,
    ) -> Self {
        Self { max_file_size: 10 * 1024 * 1024, cache, workdir, engine, permissions: Some(permissions) }
    }
}

/// Общий хелпер: решить судьбу файловой операции по правилам движка.
/// Deny → Err(сообщение); Ask → запрос человеку через permissions;
/// Allow/None → Ok(()).
async fn guard_file_op(
    engine: &std::sync::Arc<crate::permissions::PermissionEngine>,
    permissions: Option<&std::sync::Arc<crate::permissions::PermissionManager>>,
    tool: &str,
    display_path: &str,
    key: &str,
) -> Result<(), String> {
    use crate::permissions::{Outcome, RuleAction};
    match engine.evaluate(tool, key) {
        Some(RuleAction::Deny) => {
            Err(format!(
                "🚫 Операция '{tool}' над '{display_path}' запрещена правилом разрешений (config.toml [[permissions.rules]])."
            ))
        }
        Some(RuleAction::Ask) => {
            let Some(pm) = permissions else {
                return Err(format!(
                    "🚫 '{tool}' над '{display_path}' требует подтверждения, но канал вопросов недоступен — отменено."
                ));
            };
            match pm.request(tool, &format!("{tool}: {display_path}"), "файл помечен правилом как чувствительный (ask)", key).await {
                Outcome::Granted => Ok(()),
                Outcome::Denied => Err(format!(
                    "🚫 Пользователь ОТКЛОНИЛ {tool} над {display_path}. Уточни, как действовать."
                )),
                Outcome::Expired => Err(format!(
                    "⏳ Нет ответа на запрос {tool} за 3 минуты — отменено ({display_path})."
                )),
            }
        }
        _ => Ok(()),
    }
}

/// Ключ для правил по файлу: относительный путь (если внутри workdir),
/// иначе полный — glob-паттерны вида **/.env матчятся на оба.
fn permission_key(workdir: &WorkDir, path: &std::path::Path) -> String {
    let wd = workdir.get();
    path.strip_prefix(&wd)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
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

        // offset/limit (по образцу opencode): большой файл НЕ тащится в
        // контекст целиком — по умолчанию первые 2000 строк. Модель
        // дочитывает дозапросами с offset; в конце всегда честный маркер
        // "обрезано", чтобы она знала, что ниже ещё есть.
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_READ_LIMIT);
        let limit = limit.min(MAX_READ_LIMIT);

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

        // PERMISSIONS-AS-DATA: .env/ключи → ask по дефолту, deny из конфига.
        let pkey = permission_key(&self.workdir, &path);
        if let Err(e) = guard_file_op(&self.engine, self.permissions.as_ref(), "read", &pkey, &pkey).await {
            return ToolResult::error(e);
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
                // Пагинация строк (кэш хранит весь файл — срез дешёвый).
                let total = text.lines().count();
                if offset == 0 && total <= limit {
                    return ToolResult::success(text);
                }
                let start = offset.min(total);
                let end = (offset.saturating_add(limit)).min(total);
                let selected: String = text.lines().skip(start).take(end - start).collect::<Vec<_>>().join("\n");
                let note = if end < total {
                    format!(
                        "\n\n…[показаны строки {}–{} из {} — файл ОБРЕЗАН; продолжайте offset={}]",
                        start + 1,
                        end,
                        total,
                        end
                    )
                } else if start > 0 {
                    format!("\n\n…[показаны строки {}–{} из {} — конец файла]", start + 1, end, total)
                } else {
                    String::new()
                };
                ToolResult::success(format!("{selected}{note}"))
            }
            Err(_) => ToolResult::error("Unable to decode file with UTF-8"),
        }
    }

    fn description(&self) -> &'static str {
        "Прочитать содержимое файла. Большие файлы по умолчанию обрезаются до первых 2000 строк — дочитывайте offset-ом"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::from_str(r#"{"type":"object","properties":{"file_path":{"type":"string","description":"Путь к файлу"},"offset":{"type":"integer","description":"Начальная строка (0-индексация), по умолчанию 0"},"limit":{"type":"integer","description":"Сколько строк прочитать (по умолчанию 2000, максимум 10000)"}},"required":["file_path"]}"#).unwrap()
    }

    fn name(&self) -> &'static str {
        "file_read"
    }
}

pub struct FileWriteTool {
    workdir: WorkDir,
    engine: std::sync::Arc<crate::permissions::PermissionEngine>,
}

impl FileWriteTool {
    pub fn new(workdir: WorkDir, engine: std::sync::Arc<crate::permissions::PermissionEngine>) -> Self {
        Self { workdir, engine }
    }
}

/// АТОМАРНАЯ ЗАПИСЬ: пишем во временный файл РЯДОМ (тот же каталог —
/// rename внутри одной ФС атомарен) и переименовываем. Краш процесса
/// посреди записи больше НЕ портит файл пользователя: либо старая версия,
/// либо новая целиком, никогда — обрезанная половина.
async fn atomic_write(path: &std::path::Path, content: &[u8]) -> Result<(), String> {
    let stem = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let tmp = path.with_file_name(format!(".{stem}.hef-tmp-{}", std::process::id()));
    let write_res = tokio_fs::write(&tmp, content).await;
    if let Err(e) = write_res {
        let _ = tokio_fs::remove_file(&tmp).await;
        return Err(format!("tmp write: {e}"));
    }
    if let Err(e) = tokio_fs::rename(&tmp, path).await {
        let _ = tokio_fs::remove_file(&tmp).await;
        return Err(format!("rename (атомарная подмена): {e}"));
    }
    Ok(())
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

        // PERMISSIONS-AS-DATA: .env/ключи → ask, deny из конфига.
        let pkey = permission_key(&self.workdir, &path);
        if let Err(e) = guard_file_op(&self.engine, None, "edit", &pkey, &pkey).await {
            return ToolResult::error(e);
        }

        // ДИФФ вместо немого перезаписи: если файл существовал — показываем
        // что именно поменялось (+/− по строкам). Модель и человек сразу
        // видят результат правки, а не догадываются.
        let prev = tokio_fs::read_to_string(&path).await.ok();
        match atomic_write(&path, content.as_bytes()).await {
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
    cache: FileCache,
    engine: std::sync::Arc<crate::permissions::PermissionEngine>,
}

impl FileEditTool {
    pub fn new(workdir: WorkDir) -> Self {
        Self {
            workdir,
            cache: FileCache::default(),
            engine: std::sync::Arc::new(crate::permissions::PermissionEngine::new(vec![])),
        }
    }

    pub fn with_cache(
        cache: FileCache,
        workdir: WorkDir,
        engine: std::sync::Arc<crate::permissions::PermissionEngine>,
    ) -> Self {
        Self { cache, workdir, engine }
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

        let path = self.workdir.resolve(file_path);

        // PERMISSIONS-AS-DATA: .env/ключи → ask, deny из конфига.
        let pkey = permission_key(&self.workdir, &path);
        if let Err(e) = guard_file_op(&self.engine, None, "edit", &pkey, &pkey).await {
            return ToolResult::error(e);
        }

        // ЧИТАЕМ НАПРЯМУЮ (не через file_read): edit должен работать с
        // ПОЛНЫМ содержимым — пагинация file_read обрезала бы большие
        // файлы, и edit построенный по обрезанной версии уничтожил бы
        // весь хвост.
        let content = match tokio_fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Не удалось прочитать файл (нужен file_read первым?): {e}")),
        };

        // FRESHNESS CHECK (по образцу opencode/CC): если файл был прочитан
        // file_read-ом и С ТЕХ ПОР изменился на диске (пользователь правил
        // вручную, git checkout и т.п.) — edit затер бы чужие правки.
        // Кэш хранит (mtime, content) на момент чтения; mtime разошёлся —
        // честный отказ с требованием перечитать.
        if let Some((cached_mtime, cached_content)) = self.cache.peek_cached(&path) {
            let current_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if current_mtime != Some(cached_mtime) || cached_content != content {
                return ToolResult::error(format!(
                    "⚠️ Файл {} изменился ПОСЛЕ твоего file_read (кто-то правил вручную или другой процесс). \
                     Затирать чужие правки нельзя — вызови file_read заново и повтори edit с учётом свежего содержимого.",
                    file_path
                ));
            }
        }

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

        // Write the file — АТОМАРНО (tmp+rename): краш не оставит обрезанный файл.
        let diff_note = unified_diff(&content, &new_content, 40)
            .map(|(d, a, r)| format!("\nИзменения (+{a}/−{r}):\n{d}"))
            .unwrap_or_default();
        if let Err(e) = atomic_write(&path, new_content.as_bytes()).await {
            return ToolResult::error(format!("Error writing file: {}", e));
        }
        // Обновляем кэш свежим содержимым (mtime возьмётся текущий).
        self.cache.put(&path, new_content.clone());

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
    /// Правила: deny блокирует удаление без вопросов (например секреты).
    engine: std::sync::Arc<crate::permissions::PermissionEngine>,
}

impl FileDeleteTool {
    pub fn new(
        workdir: WorkDir,
        permissions: std::sync::Arc<crate::permissions::PermissionManager>,
        engine: std::sync::Arc<crate::permissions::PermissionEngine>,
    ) -> Self {
        Self { workdir, permissions, engine }
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

        // PERMISSIONS-AS-DATA: deny (секреты по дефолту) блокирует до
        // вопросов; allow — пропускает; ask/None — старый интерактивный путь.
        let pkey = permission_key(&self.workdir, &path);
        match self.engine.evaluate("delete", &pkey) {
            Some(crate::permissions::RuleAction::Deny) => {
                return ToolResult::error(format!(
                    "🚫 Удаление '{pkey}' запрещено правилом разрешений (это защищённый файл)."
                ));
            }
            Some(crate::permissions::RuleAction::Allow) => {
                // Правило доверия — удаляем без вопроса.
            }
            _ => {
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
