use crate::tools::{Tool, ToolResult};
use serde_json::Value;
use std::path::PathBuf;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub type_: String,
    pub tags: Vec<String>,
    pub importance: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct MemorySystem {
    storage_path: PathBuf,
    entries: Arc<Mutex<Vec<MemoryEntry>>>,
}

impl MemorySystem {
    pub fn new(storage_path: Option<PathBuf>) -> Self {
        let path = storage_path.unwrap_or_else(|| {
            let mut home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            home.push(".hephaestus");
            home.push("memory.json");
            home
        });

        let entries = if path.exists() {
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(entries) = serde_json::from_str(&data) {
                    entries
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Self {
            storage_path: path,
            entries: Arc::new(Mutex::new(entries)),
        }
    }

    pub async fn add(&self, content: String, type_: String, tags: Vec<String>, importance: i32) -> MemoryEntry {
        let mut entries = self.entries.lock().await;
        let id = format!("mem_{}", chrono::Utc::now().timestamp());
        let now = chrono::Utc::now();

        let entry = MemoryEntry {
            id: id.clone(),
            content,
            type_,
            tags,
            importance: importance.clamp(1, 10),
            created_at: now,
            updated_at: now,
        };

        entries.push(entry.clone());
        self.save(&entries).await;
        entry
    }

    pub async fn search(&self, query: Option<String>, type_: Option<String>, tags: Option<Vec<String>>, min_importance: i32) -> Vec<MemoryEntry> {
        let entries = self.entries.lock().await;
        let mut results: Vec<MemoryEntry> = entries
            .iter()
            .filter(|e| e.importance >= min_importance)
            .filter(|e| {
                if let Some(ref q) = query {
                    e.content.to_lowercase().contains(&q.to_lowercase())
                } else {
                    true
                }
            })
            .filter(|e| {
                if let Some(ref t) = type_ {
                    e.type_.eq_ignore_ascii_case(t)
                } else {
                    true
                }
            })
            .filter(|e| {
                if let Some(ref tags_filter) = tags {
                    if tags_filter.is_empty() {
                        true
                    } else {
                        tags_filter.iter().any(|t| e.tags.iter().any(|et| et.eq_ignore_ascii_case(t)))
                    }
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        results.sort_by(|a, b| b.importance.cmp(&a.importance));
        results
    }

    pub async fn update(&self, mem_id: &str, content: Option<String>, tags: Option<Vec<String>>, importance: Option<i32>) -> Option<MemoryEntry> {
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.iter_mut().find(|e| e.id == mem_id) {
            if let Some(c) = content {
                entry.content = c;
            }
            if let Some(t) = tags {
                entry.tags = t;
            }
            if let Some(i) = importance {
                entry.importance = i.clamp(1, 10);
            }
            entry.updated_at = chrono::Utc::now();
            let updated = entry.clone();
            self.save(&entries).await;
            Some(updated)
        } else {
            None
        }
    }

    pub async fn delete(&self, mem_id: &str) -> bool {
        let mut entries = self.entries.lock().await;
        let len_before = entries.len();
        entries.retain(|e| e.id != mem_id);
        if entries.len() < len_before {
            self.save(&entries).await;
            true
        } else {
            false
        }
    }

    pub async fn list_all(&self, limit: usize) -> Vec<MemoryEntry> {
        let entries = self.entries.lock().await;
        entries.iter().take(limit).cloned().collect()
    }

    async fn save(&self, entries: &[MemoryEntry]) {
        if let Some(parent) = self.storage_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(entries) {
            let _ = fs::write(&self.storage_path, json);
        }
    }
}

/// Создаёт все memory_* инструменты с общим (Arc-расшаренным) хранилищем.
/// Возвращает пары (имя, Arc<dyn Tool>) — то, что напрямую принимает
/// ToolRegistry::register() в tools/mod.rs::create_tools().
pub fn create_memory_tools(storage_path: Option<PathBuf>) -> Vec<(&'static str, Arc<dyn crate::tools::Tool>)> {
    let system = Arc::new(MemorySystem::new(storage_path));
    vec![
        ("memory_add", Arc::new(MemoryAddTool::new(system.clone())) as Arc<dyn crate::tools::Tool>),
        ("memory_search", Arc::new(MemorySearchTool::new(system.clone()))),
        ("memory_update", Arc::new(MemoryUpdateTool::new(system.clone()))),
        ("memory_delete", Arc::new(MemoryDeleteTool::new(system.clone()))),
        ("memory_list", Arc::new(MemoryListTool::new(system))),
    ]
}

pub struct MemoryAddTool {
    system: Arc<MemorySystem>,
}

impl MemoryAddTool {
    pub fn new(system: Arc<MemorySystem>) -> Self {
        Self { system }
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for MemoryAddTool {
    fn name(&self) -> &'static str {
        "memory_add"
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let content = args.get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let type_ = args.get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("fact")
            .to_string();
        let tags_str = args.get("tags")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tags: Vec<String> = tags_str.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let importance = args.get("importance")
            .and_then(|v| v.as_i64())
            .unwrap_or(5) as i32;

        if content.is_empty() {
            return ToolResult::error("content is required".to_string());
        }

        let entry = self.system.add(content, type_, tags, importance).await;
        ToolResult::success(format!("✅ Запись добавлена в память: {}\nТип: {}\nВажность: {}/10", entry.id, entry.type_, entry.importance))
    }
}

pub struct MemorySearchTool {
    system: Arc<MemorySystem>,
}

impl MemorySearchTool {
    pub fn new(system: Arc<MemorySystem>) -> Self {
        Self { system }
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for MemorySearchTool {
    fn name(&self) -> &'static str {
        "memory_search"
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).map(|s| s.to_string());
        let type_ = args.get("type").and_then(|v| v.as_str()).map(|s| s.to_string());
        let tags_str = args.get("tags").and_then(|v| v.as_str()).unwrap_or("");
        let tags: Option<Vec<String>> = if tags_str.is_empty() {
            None
        } else {
            Some(tags_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        };
        let min_importance = args.get("min_importance")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;

        let results = self.system.search(query, type_, tags, min_importance).await;

        if results.is_empty() {
            return ToolResult::success("❌ Записи не найдены".to_string());
        }

        let mut lines = vec![format!("✅ Найдено записей: {}\n", results.len())];
        for mem in results.iter().take(20) {
            lines.push(format!(" {}", mem.id));
            lines.push(format!("   Тип: {} | Важность: {}/10", mem.type_, mem.importance));
            lines.push(format!("   {}", mem.content));
            if !mem.tags.is_empty() {
                lines.push(format!("   Теги: {}", mem.tags.join(", ")));
            }
            lines.push(String::new());
        }

        ToolResult::success(lines.join("\n"))
    }
}

pub struct MemoryUpdateTool {
    system: Arc<MemorySystem>,
}

impl MemoryUpdateTool {
    pub fn new(system: Arc<MemorySystem>) -> Self {
        Self { system }
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for MemoryUpdateTool {
    fn name(&self) -> &'static str {
        "memory_update"
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let mem_id = args.get("mem_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let content = args.get("content").and_then(|v| v.as_str()).map(|s| s.to_string());
        let tags_str = args.get("tags").and_then(|v| v.as_str()).unwrap_or("");
        let tags: Option<Vec<String>> = if tags_str.is_empty() {
            None
        } else {
            Some(tags_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        };
        let importance = args.get("importance")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32)
            .filter(|&v| v >= 0);

        if mem_id.is_empty() {
            return ToolResult::error("mem_id is required".to_string());
        }

        match self.system.update(&mem_id, content, tags, importance).await {
            Some(entry) => ToolResult::success(format!("✅ Запись обновлена: {}\nСодержимое: {}...", entry.id, entry.content.chars().take(100).collect::<String>())),
            None => ToolResult::error(format!("Запись {} не найдена", mem_id)),
        }
    }
}

pub struct MemoryDeleteTool {
    system: Arc<MemorySystem>,
}

impl MemoryDeleteTool {
    pub fn new(system: Arc<MemorySystem>) -> Self {
        Self { system }
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for MemoryDeleteTool {
    fn name(&self) -> &'static str {
        "memory_delete"
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let mem_id = args.get("mem_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if mem_id.is_empty() {
            return ToolResult::error("mem_id is required".to_string());
        }

        if self.system.delete(&mem_id).await {
            ToolResult::success(format!("✅ Запись {} удалена из памяти", mem_id))
        } else {
            ToolResult::error(format!("Запись {} не найдена", mem_id))
        }
    }
}

pub struct MemoryListTool {
    system: Arc<MemorySystem>,
}

impl MemoryListTool {
    pub fn new(system: Arc<MemorySystem>) -> Self {
        Self { system }
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for MemoryListTool {
    fn name(&self) -> &'static str {
        "memory_list"
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let limit = args.get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(50) as usize;

        let memories = self.system.list_all(limit).await;

        if memories.is_empty() {
            return ToolResult::success(" Память пуста".to_string());
        }

        let mut lines = vec![format!(" Всего записей: {}\n", memories.len())];
        for mem in memories {
            lines.push(format!("• {}", mem.id));
            lines.push(format!("  [{}] {}...", mem.type_, mem.content.chars().take(80).collect::<String>()));
            lines.push(format!("  Важность: {}/10", mem.importance));
            if !mem.tags.is_empty() {
                lines.push(format!("  Теги: {}", mem.tags.join(", ")));
            }
            lines.push(String::new());
        }

        ToolResult::success(lines.join("\n"))
    }
}

// ══════════════════════════ Файловая память (паттерн MEMORY.md) ═════════
// Память фактами-файлами: ~/.hephaestus/memory/<name>.md с frontmatter
// (type: user|feedback|project|reference) + автоиндекс MEMORY.md.
// Дополняет старую JSON-память (обе доступны модели).


pub fn memory_files_dir() -> PathBuf {
    // HEPHAESTUS_HOME уважается везде (изолированные тесты, кастомный хоум).
    if let Ok(custom) = std::env::var("HEPHAESTUS_HOME") {
        return PathBuf::from(custom).join("memory");
    }
    let mut p = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push(".hephaestus");
    p.push("memory");
    p
}

fn rebuild_memory_index(dir: &PathBuf) -> Result<(), String> {
    let mut lines = vec!["<!-- Автоиндекс; не редактировать вручную -->".to_string()];
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") || p.file_name().map(|f| f == "MEMORY.md").unwrap_or(false) {
                continue;
            }
            if let Ok(txt) = std::fs::read_to_string(&p) {
                let desc = txt
                    .lines()
                    .find_map(|l| l.strip_prefix("description:"))
                    .map(|d| d.trim().to_string())
                    .unwrap_or_default();
                let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
                lines.push(format!("- [{name}]( {}.md ) — {desc}", name));
            }
        }
    }
    std::fs::write(dir.join("MEMORY.md"), lines.join("\n")).map_err(|e| e.to_string())
}

/// Индекс для системного промпта (строки "- name — description").
pub fn memory_index_lines() -> String {
    let dir = memory_files_dir();
    match std::fs::read_to_string(dir.join("MEMORY.md")) {
        Ok(idx) => idx
            .lines()
            .filter(|l| l.starts_with("- ["))
            .take(20)
            .collect::<Vec<_>>()
            .join("\n"),
        Err(_) => String::new(),
    }
}

// ── СТЕЙДЖИНГ ФОНОВЫХ ЗАПИСЕЙ (по образцу tools/write_approval.py Hermes) ──
//
// Фоновый агент (суб-агент, задача из очереди) НЕ должен молча писать в
// долговременную память: он может «запомнить» неверные допущения, и они
// останутся в контексте всех будущих сессий. Такие записи СТЕЙДЖАТСЯ в
// `~/.hephaestus/pending/memory/*.md` и переживают рестарт; пользователь
// утверждает их командой /memory review (перенос в основную память или
// отказ). Прямой вызов из интерактивного хода пишет как раньше.

thread_local! {
    /// true в тасках, которые считаются ФОНОВЫМИ (суб-агенты, очередь).
    /// Ставится goal_mode/subagent при старте фонового выполнения.
    static BACKGROUND_CONTEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Пометить ТЕКУЩУЮ задачу как фоновую: memory_file_save будет стейджить,
/// а не писать в постоянную память. Вызывается до spawn фонового хода
/// (thread_local — распространяется только на этот поток-исполнитель).
pub fn mark_background() {
    BACKGROUND_CONTEXT.with(|c| c.set(true));
}

/// Снять пометку фоновой задачи.
pub fn clear_background() {
    BACKGROUND_CONTEXT.with(|c| c.set(false));
}

/// Мы сейчас в фоновом контексте?
pub fn is_background() -> bool {
    BACKGROUND_CONTEXT.with(|c| c.get())
}

fn pending_memory_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("HEPHAESTUS_HOME") {
        return PathBuf::from(custom).join("pending").join("memory");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hephaestus")
        .join("pending")
        .join("memory")
}

/// Каталог stейдж-записей (для /memory review).
pub fn pending_memory_files() -> Vec<PathBuf> {
    let dir = pending_memory_dir();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Утвердить стейдж-запись: перенести в основную память + индекс.
pub fn approve_pending(name: &str) -> Result<String, String> {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let src = pending_memory_dir().join(format!("{safe}.md"));
    let content = std::fs::read_to_string(&src).map_err(|e| format!("нет stейдж-записи {safe}: {e}"))?;
    let dir = memory_files_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{safe}.md")), &content).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&src);
    rebuild_memory_index(&dir)?;
    Ok(safe)
}

/// Отклонить стейдж-запись (удалить файл).
pub fn reject_pending(name: &str) -> Result<(), String> {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let src = pending_memory_dir().join(format!("{safe}.md"));
    std::fs::remove_file(&src).map_err(|e| format!("{safe}: {e}"))
}

/// memory_file_save — сохранить/обновить факт-файл.
pub struct MemoryFileSaveTool;

#[async_trait::async_trait]
impl Tool for MemoryFileSaveTool {
    fn name(&self) -> &'static str {
        "memory_file_save"
    }

    fn description(&self) -> &'static str {
        "Сохранить долговременный ФАКТ в файловую память (переживает перезапуск). type: user — кто пользователь; feedback — как с ним работать (с 'Why:' и 'How to apply:'); project — цели/ограничения проекта; reference — ссылки на внешние ресурсы. Не сохраняй то, что видно из кода или git."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "kebab-case имя факта"},
                "type": {"type": "string", "enum": ["user", "feedback", "project", "reference"]},
                "description": {"type": "string", "description": "Одна строка — суть (для индекса)"},
                "content": {"type": "string", "description": "Текст факта"}
            },
            "required": ["name", "type", "description", "content"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let (Some(name), Some(ftype), Some(desc), Some(content)) = (
            args.get("name").and_then(|v| v.as_str()),
            args.get("type").and_then(|v| v.as_str()),
            args.get("description").and_then(|v| v.as_str()),
            args.get("content").and_then(|v| v.as_str()),
        ) else {
            return ToolResult::error("нужны name, type, description, content");
        };
        let safe: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect();
        if safe.is_empty() {
            return ToolResult::error("некорректное имя");
        }
        let dir = memory_files_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return ToolResult::error(format!("create_dir: {e}"));
        }
        let file_content = format!(
            "---\nname: {safe}\ntype: {ftype}\ndescription: {}\n---\n{}\n",
            desc.replace('\n', " "),
            content.trim()
        );
        // СТЕЙДЖИНГ (write_approval-модель Hermes): фоновая задача
        // (суб-агент, очередь) не пишет в постоянную память напрямую —
        // запись уходит в pending/, пользователь утверждает /memory review.
        if is_background() {
            let pdir = pending_memory_dir();
            if let Err(e) = std::fs::create_dir_all(&pdir) {
                return ToolResult::error(format!("create_dir: {e}"));
            }
            if let Err(e) = std::fs::write(pdir.join(format!("{safe}.md")), &file_content) {
                return ToolResult::error(format!("write: {e}"));
            }
            return ToolResult::success(format!(
                "📋 Факт {safe} ПОМЕЩЕН НА УТВЕРЖДЕНИЕ (фоновая задача не пишет в память напрямую). \
                 Пользователь увидит его в /memory review и перенесёт в постоянную память при согласии."
            ));
        }
        if let Err(e) = std::fs::write(dir.join(format!("{safe}.md")), &file_content) {
            return ToolResult::error(format!("write: {e}"));
        }
        if let Err(e) = rebuild_memory_index(&dir) {
            return ToolResult::error(format!("index rebuild: {e}"));
        }
        ToolResult::success(format!(
            "✅ Факт сохранён в память: {safe}. Он будет виден в каждой будущей сессии."
        ))
    }
}

/// memory_file_list — индекс.
pub struct MemoryFileListTool;

#[async_trait::async_trait]
impl Tool for MemoryFileListTool {
    fn name(&self) -> &'static str {
        "memory_file_list"
    }

    fn description(&self) -> &'static str {
        "Индекс долговременных фактов в файловой памяти."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: &Value) -> ToolResult {
        let idx = memory_index_lines();
        if idx.is_empty() {
            ToolResult::success("(файловая память пуста)")
        } else {
            ToolResult::success(idx)
        }
    }
}

/// memory_file_load — полный текст одного факта.
pub struct MemoryFileLoadTool;

#[async_trait::async_trait]
impl Tool for MemoryFileLoadTool {
    fn name(&self) -> &'static str {
        "memory_file_load"
    }

    fn description(&self) -> &'static str {
        "Прочитать полный текст факта из файловой памяти по имени."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
            return ToolResult::error("нужен 'name'");
        };
        let path = memory_files_dir().join(format!("{name}.md"));
        match std::fs::read_to_string(&path) {
            Ok(t) => ToolResult::success(t),
            Err(_) => ToolResult::error(format!("факт '{name}' не найден (memory_file_list)")),
        }
    }
}

pub fn create_memory_file_tools() -> Vec<(&'static str, Arc<dyn Tool>)> {
    use std::sync::Arc;
    vec![
        ("memory_file_save", Arc::new(MemoryFileSaveTool)),
        ("memory_file_list", Arc::new(MemoryFileListTool)),
        ("memory_file_load", Arc::new(MemoryFileLoadTool)),
    ]
}
