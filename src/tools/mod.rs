use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use async_trait::async_trait;

pub mod file_tools;
pub mod bash_tool;
pub mod git_tool;
pub mod file_cache;
pub mod skill_tools;
pub mod web_tools;
pub mod async_executor;
pub mod workflow_tool;
pub mod background_agent;
pub mod secret_store;
pub mod ask_user;
pub mod diagram_tool;
pub mod result_verifier;
pub mod config_tools;
pub mod test_tools;
pub mod worktree_tools;
pub mod doc_generator;
pub mod subagent;
pub mod md_skills;

use file_tools::*;
use bash_tool::BashTool;
use git_tool::GitTool;
pub use file_cache::FileCache;
use skill_tools::{SkillFindTool, SkillListTool, SkillSaveTool};
use web_tools::{WebFetchTool, WebSearchTool};
use crate::skill_learner::SkillLearner;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
    pub images: Vec<String>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
            images: Vec::new(),
        }
    }



    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
            images: Vec::new(),
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    async fn execute(&self, args: &Value) -> ToolResult;
    fn name(&self) -> &'static str;

    /// Версия реализации инструмента (semver-подобная строка).
    /// Инструменты, которым не важно версионирование, могут не переопределять.
    fn version(&self) -> &'static str {
        "1.0.0"
    }

    /// Версия схемы аргументов инструмента. Увеличивайте при несовместимых
    /// изменениях набора/формата аргументов (`args`), чтобы вызывающий код
    /// (LLM tool-calling слой, кэш описаний инструментов и т.п.) мог
    /// обнаружить рассинхронизацию.
    fn schema_version(&self) -> u32 {
        1
    }

    /// Человеко-читаемое описание — то, что видит модель в списке
    /// доступных инструментов. Дефолт пустой не потому что это ок, а
    /// чтобы существующие реализации Tool продолжали компилироваться,
    /// пока описания не дописаны по одному — см. TOOL_SCHEMAS_TODO.md.
    fn description(&self) -> &'static str {
        ""
    }

    /// JSON Schema параметров в формате `{"type":"object","properties":{...}}`
    /// (то, что уходит как `function.parameters` в OpenAI-совместимый
    /// tool-calling запрос). Дефолт — пустой объект БЕЗ required: модель
    /// сможет вызвать инструмент, но не будет знать точные имена
    /// параметров, пока схема не дописана — лучше, чем инструмент,
    /// который LLM вообще не видит (было так до этого фикса), но это
    /// временное состояние, не финальное.
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
}

/// Метаданные зарегистрированного инструмента — для отображения в REPL
/// (`/tools`), логирования и обнаружения несовместимых версий схемы.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolMetadata {
    pub name: String,
    pub version: &'static str,
    pub schema_version: u32,
}

pub struct ToolRegistry {
    // RwLock (а не владеющий HashMap) — реестр должен допускать
    // ДИНАМИЧЕСКУЮ регистрацию после первичной сборки: инструменты с
    // MCP-серверов (src/mcp.rs) подключаются при старте и по /mcp reload,
    // когда SharedToolRegistry уже отдан Agent'у. Критические секции
    // короткие — только доступ к map, никаких await под локом.
    tools: std::sync::RwLock<HashMap<String, Arc<dyn Tool>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: std::sync::RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, name: &str, tool: Arc<dyn Tool>) {
        let mut tools = match self.tools.write() {
            Ok(t) => t,
            Err(_) => return,
        };
        if let Some(existing) = tools.get(name) {
            if existing.schema_version() != tool.schema_version() {
                // Не eprintln! — регистрация тоже может случиться уже
                // после входа в TUI (напр. hot-reload набора инструментов
                // в будущем), безопаснее сразу писать через файловый лог.
                crate::logging_system::get_logger().warning(
                    &format!(
                        "инструмент '{}' переопределён с другой версией схемы ({} -> {})",
                        name,
                        existing.schema_version(),
                        tool.schema_version()
                    ),
                    None,
                );
            }
        }
        tools.insert(name.to_string(), tool);
    }

    /// Убрать инструмент из реестра (используется /mcp reload перед
    /// переподключением серверов). true — был и удалён.
    pub fn remove(&self, name: &str) -> bool {
        self.tools
            .write()
            .map(|mut t| t.remove(name).is_some())
            .unwrap_or(false)
    }

    pub async fn execute(&self, name: &str, args: &Value) -> Option<ToolResult> {
        // Клонируем Arc под коротким read-локом; сам вызов инструмента —
        // БЕЗ лока (await с RwLockReadGuard был бы препятствием для
        // параллельных вызовов и риском дедлока).
        let tool = self.tools.read().ok()?.get(name).cloned()?;
        let mut result = tool.execute(args).await;
        // error_handler.rs (итерация 7): единая точка, через которую
        // проходит вызов КАЖДОГО инструмента — добавляем категорию
        // ошибки и подсказку "можно повторить", не трогая ни один
        // из ~88 отдельных инструментов по одному.
        if !result.success {
            if let Some(raw) = &result.error {
                result.error = Some(crate::error_handler::friendly_message(raw));
            }
        }
        Some(result)
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.read().map(|t| t.keys().cloned().collect()).unwrap_or_default()
    }

    /// Список инструментов с версией реализации и версией схемы аргументов.
    pub fn list_metadata(&self) -> Vec<ToolMetadata> {
        let mut out: Vec<ToolMetadata> = self
            .tools
            .read()
            .map(|t| {
                t.iter()
                    .map(|(name, tool)| ToolMetadata {
                        name: name.clone(),
                        version: tool.version(),
                        schema_version: tool.schema_version(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Схемы всех зарегистрированных инструментов в формате OpenAI
    /// function-calling ({"type":"function","function":{...}}) — то,
    /// что напрямую уходит в поле "tools" запроса к LLM API. Раньше
    /// этот массив нигде не строился вообще, и agent::chat() отправлял
    /// запросы без единого инструмента в поле "tools" — LLM физически
    /// не могла вызвать ни один инструмент, независимо от того, сколько
    /// их зарегистрировано в реестре (см. CHANGELOG/STATUS).
    pub fn to_api_schemas(&self) -> Vec<Value> {
        let tools = match self.tools.read() {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        let mut names: Vec<&String> = tools.keys().collect();
        names.sort(); // стабильный порядок — не обязательно для API, но упрощает диагностику/логи
        names
            .into_iter()
            .map(|name| {
                let tool = &tools[name];
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": tool.description(),
                        "parameters": tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }
}

/// Разделяемый снаружи реестр инструментов — обёртка над `Arc<OnceLock<ToolRegistry>>`.
///
/// Нужен для «мета-инструментов», которым для работы требуется вызывать
/// другие инструменты реестра (`parallel_exec`, `workflow_run`,
/// `background_run`): сам реестр ещё строится в момент, когда такие
/// инструменты в него регистрируются, поэтому прямая ссылка на готовый
/// `ToolRegistry` в момент конструирования недоступна — `OnceLock`
/// заполняется один раз, когда регистрация полностью завершена
/// (см. `create_tools`).
#[derive(Clone)]
pub struct SharedToolRegistry(Arc<OnceLock<ToolRegistry>>);

impl SharedToolRegistry {
    pub async fn execute(&self, name: &str, args: &Value) -> Option<ToolResult> {
        match self.0.get() {
            Some(registry) => registry.execute(name, args).await,
            None => Some(ToolResult::error("Реестр инструментов ещё не инициализирован")),
        }
    }

    /// Динамическая регистрация ПОСЛЕ сборки реестра — так в общий набор
    /// попадают инструменты с MCP-серверов (см. src/mcp.rs).
    pub fn register(&self, name: &str, tool: Arc<dyn Tool>) {
        if let Some(registry) = self.0.get() {
            registry.register(name, tool);
        } else {
            crate::logging_system::warning(
                &format!("register('{}') до инициализации реестра — проигнорировано", name),
            );
        }
    }

    /// Удалить инструмент (например, при /mcp reload).
    pub fn remove(&self, name: &str) -> bool {
        self.0.get().map(|r| r.remove(name)).unwrap_or(false)
    }

    pub fn list(&self) -> Vec<String> {
        self.0.get().map(|r| r.list()).unwrap_or_default()
    }

    pub fn list_metadata(&self) -> Vec<ToolMetadata> {
        self.0.get().map(|r| r.list_metadata()).unwrap_or_default()
    }

    /// См. ToolRegistry::to_api_schemas — то же самое через Shared-обёртку,
    /// т.к. agent::Agent держит именно SharedToolRegistry, не голый ToolRegistry.
    pub fn to_api_schemas(&self) -> Vec<Value> {
        self.0.get().map(|r| r.to_api_schemas()).unwrap_or_default()
    }
}

/// Окружение для create_tools — один параметр вместо растущего списка:
/// всё, что нужно инструментам, кроме аргументов вызова.
pub struct ToolsEnv {
    pub workdir: crate::workdir::WorkDir,
    pub monitoring: Arc<crate::monitoring::MonitoringSystem>,
    pub permissions: Arc<crate::permissions::PermissionManager>,
    /// Общий план задачи: ОДИН экземпляр на Agent и инструменты
    /// (todo_write пишет его, подсказка в system prompt читает).
    pub todos: Arc<crate::todo_tools::TodoBoard>,
    /// Конфиг LLM + фабрика суб-агентов (agent_spawn/status/result).
    pub llm_config: crate::llm::LLMConfig,
    pub subagents: Arc<crate::tools::subagent::SubAgentRunner>,
}

/// `env.workdir` — общая ДИНАМИЧЕСКАЯ рабочая директория (src/workdir.rs):
/// все инструменты, работающие с путями/процессами, резолвят
/// относительные пути через неё. Смена `/work_dir` (REPL) или
/// `/workdir <путь>` (Telegram) действует на все инструменты сразу,
/// без пересоздания реестра — именно этого не хватало, когда корень
/// был заморожен на момент старта процесса и Telegram-бот физически
/// не мог работать с файлами там, где ждёт пользователь.
///
/// `env.permissions` — подтверждения опасных действий как в opencode
/// ("один раз / всегда / нет"): см. src/permissions.rs.
pub fn create_tools(env: ToolsEnv) -> SharedToolRegistry {
    let ToolsEnv { workdir, monitoring, permissions, todos, llm_config: _, subagents } = env;
    let registry = ToolRegistry::new();

    let cache = FileCache::default();
    registry.register("file_read", Arc::new(FileReadTool::with_cache(cache, workdir.clone())));
    registry.register("file_write", Arc::new(FileWriteTool::new(workdir.clone())));
    registry.register("file_edit", Arc::new(FileEditTool::new(workdir.clone())));
    registry.register("file_delete", Arc::new(FileDeleteTool::new(workdir.clone(), permissions.clone())));
    registry.register("file_move", Arc::new(FileMoveTool::new(workdir.clone())));
    registry.register("file_copy", Arc::new(FileCopyTool::new(workdir.clone())));
    registry.register("file_exists", Arc::new(FileExistsTool::new(workdir.clone())));
    registry.register("glob", Arc::new(GlobTool::new(workdir.clone())));
    registry.register("bash", Arc::new(BashTool::new(workdir.clone(), permissions.clone())));
    registry.register("git", Arc::new(GitTool::new(workdir.clone())));

    // План задачи агента (todo_write/todo_read) — общий список с Agent.
    registry.register("todo_write", Arc::new(crate::todo_tools::TodoWriteTool::new(todos.clone())));
    registry.register("todo_read", Arc::new(crate::todo_tools::TodoReadTool::new(todos)));

    // Суб-агенты: параллельные задачи со своим контекстом.
    for (name, tool) in crate::tools::subagent::create_subagent_tools(subagents.clone()) {
        registry.register(name, tool);
    }

    // MD-скиллы (прогрессивная подгрузка).
    for (name, tool) in crate::tools::md_skills::create_md_skill_tools() {
        registry.register(name, tool);
    }

    // Файловая память фактами + MEMORY.md индекс.
    for (name, tool) in crate::memory_tools::create_memory_file_tools() {
        registry.register(name, tool);
    }


    let skill_learner = Arc::new(std::sync::Mutex::new(SkillLearner::new()));
    registry.register("skill_list", Arc::new(SkillListTool::new(skill_learner.clone())));
    registry.register("skill_find", Arc::new(SkillFindTool::new(skill_learner.clone())));
    registry.register("skill_save", Arc::new(SkillSaveTool::new(skill_learner)));

    registry.register("web_fetch", Arc::new(WebFetchTool::new()));
    registry.register("web_search", Arc::new(WebSearchTool::new()));

    // memory_tools.rs — на верхнем уровне src/ (не tools/), т.к. историю
    // памяти логично держать вне дерева "исполняемых инструментов". Общий
    // Arc<MemorySystem> строится внутри create_memory_tools() один раз —
    // все 5 memory_* инструментов работают с одним и тем же хранилищем,
    // не пятью независимыми копиями.
    for (name, tool) in crate::memory_tools::create_memory_tools(None) {
        registry.register(name, tool);
    }

    // Мета-инструменты — держат "пустую" ссылку на реестр (OnceLock), которая
    // становится валидной сразу после `cell.0.set(registry)` ниже.
    let cell: SharedToolRegistry = SharedToolRegistry(Arc::new(OnceLock::new()));

    registry.register("parallel_exec", Arc::new(async_executor::ParallelExecTool::new(cell.clone())));
    registry.register("workflow_run", Arc::new(workflow_tool::WorkflowRunTool::new(cell.clone())));

    let board = background_agent::BackgroundBoard::default();
    registry.register(
        "background_run",
        Arc::new(background_agent::BackgroundRunTool::new(cell.clone(), board.clone())),
    );
    registry.register(
        "background_status",
        Arc::new(background_agent::BackgroundStatusTool::new(board.clone())),
    );
    registry.register(
        "background_list",
        Arc::new(background_agent::BackgroundListTool::new(board)),
    );

    registry.register("secret_set", Arc::new(secret_store::SecretSetTool::new()));
    registry.register("secret_get", Arc::new(secret_store::SecretGetTool::new()));
    registry.register("secret_list", Arc::new(secret_store::SecretListTool::new()));
    registry.register("secret_delete", Arc::new(secret_store::SecretDeleteTool::new()));

    registry.register("ask_user", Arc::new(ask_user::AskUserTool::new()));

    registry.register("diagram", Arc::new(diagram_tool::DiagramTool::new()));
    registry.register("verify_result", Arc::new(result_verifier::VerifyResultTool::new()));
    registry.register("config_get", Arc::new(config_tools::ConfigGetTool::new()));
    registry.register("config_set", Arc::new(config_tools::ConfigSetTool::new()));
    registry.register("run_tests", Arc::new(test_tools::RunTestsTool::new(workdir.clone())));
    registry.register("worktree", Arc::new(worktree_tools::WorktreeTool::new()));
    registry.register("doc_generator", Arc::new(doc_generator::DocGeneratorTool::new()));
    // Документы офисных форматов из markdown (docx/odt/xlsx/csv/html/md).
    registry.register("document_create", Arc::new(doc_generator::DocumentCreateTool::new(workdir.clone())));

    // --- Итерация 5: инструменты, ранее написанные в src/*.rs, но никогда
    // не подключённые ни через `mod`, ни через ToolRegistry (см. STATUS_RU.md).
    // Обёртки над ними — в src/extra_tools.rs (crate::extra_tools), т.к. сами
    // модули (cron_tools, database_tools, ...) объявлены на верхнем уровне
    // src/, а не под src/tools/.
    for (name, tool) in crate::extra_tools::create_extra_tools() {
        registry.register(name, tool);
    }

    // --- Итерация 6: vector_search, repomap, model_detector, monitoring,
    // package_tools, project_analyzer — обёртки в src/extra_tools2.rs.
    for (name, tool) in crate::extra_tools2::create_extra_tools2(workdir.clone(), monitoring) {
        registry.register(name, tool);
    }

    // --- Итерация 7: caching.rs, task_manager.rs (доска "board_*"),
    // learning.rs — обёртки в src/extra_tools3.rs.
    for (name, tool) in crate::extra_tools3::create_extra_tools3() {
        registry.register(name, tool);
    }

    // Прикрепляем реестр суб-агентам (для build_sub_agent).
    subagents.attach_registry(cell.clone());

    // Реестр построен полностью — публикуем его для мета-инструментов.
    let _ = cell.0.set(registry);
    cell
}
