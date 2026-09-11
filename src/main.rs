mod llm;
mod tools;
mod goal_mode;
mod skill_learner;
mod voice_interface;
mod repl;
mod session_store;
mod session;
mod telegram_bot;
mod security;
mod logging_system;
mod diagram_tools;
mod memory_tools;

// --- Итерация 5 (см. STATUS_RU.md): модули существовали в src/*.rs с
// самого начала миграции, но не были подключены ни через `mod`, ни через
// ToolRegistry — были мёртвым кодом. Обёртки под Tool trait — в
// src/extra_tools.rs.
mod cron_tools;
mod database_tools;
mod docker_tools;
mod github_tools;
mod http_tools;
mod notebook_tools;
mod ocr_tool;
mod task_tools;
mod extra_tools;
mod confirmation;
mod hephaestus_logo;
mod error_handler;

// --- Итерация 6 (продолжение STATUS_RU.md): vector_search, repomap,
// model_detector, monitoring, package_tools, project_analyzer — новые
// возможности, которых не было даже в виде мёртвого кода. Обёртки под
// Tool — в src/extra_tools2.rs. streaming.rs/streaming_tool_calling.rs
// подключены, но НЕ встроены в главный цикл Agent::chat() — см. описание
// в начале самих этих файлов и в STATUS_RU.md.
mod vector_search;
mod repomap;
mod model_detector;
mod monitoring;
mod package_tools;
mod project_analyzer;
mod streaming;
mod extra_tools2;
mod caching;
mod task_manager;
mod learning;
 mod extra_tools3;
 mod tty_guard;
 mod config;
 mod workdir;
 mod permissions;
 mod todo_tools;
 mod mcp;
 mod request_queue;
 mod parser;
 mod system_prompt;
 mod state_db;
 mod tool_guard;
 mod watchdog;
 mod snapshots;
 mod shell;
 mod interrupt;
 mod bootstrap;
#[cfg(test)]
mod integration_tests;

// НЕ подключены сознательно (пересмотрено в итерации 7 — caching.rs,
// task_manager.rs и learning.rs, которые раньше здесь тоже упоминались,
// подключены выше как отдельные полноценные наборы инструментов, не
// дубликаты, см. STATUS_RU.md):
//   context_manager.rs (свой формат Message/SessionMetadata и compress
//     с summary через LLM — параллельная система токен-бюджета поверх
//     ДРУГОГО представления сообщений, чем llm.rs::LLMMessage, которым
//     живёт Agent. Смысла заводить два конкурирующих механизма Smart
//     Compression над двумя разными типами сообщений в одном Agent нет;
//     тот, что уже работает — в Agent::compress_context_if_needed()),
//   llm_client.rs (свой LLMClient как конкретная структура + свои
//     LLMMessage/LLMResponse — полный дубль llm.rs с несовместимым
//     дизайном, ничем не используется, ничего не даёт взамен),
//   parser.rs (текстовые <tool_call> теги — обходной путь под моделями
//     без нативного tool-calling API; сейчас агент всегда использует
//     complete_with_tools(), эта функция не вызывается ниоткуда — тот
//     же случай, что streaming_tool_calling.rs выше, который тоже не
//     встроен в главный цикл по этой причине).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::Mutex;

use futures_util::future::join_all;

use llm::{LLMClient, LLMConfig, LLMMessage, ToolUseBlock, create_llm_client};
use tools::{SharedToolRegistry, ToolResult, ToolsEnv, create_tools};

type ToolHook = Box<dyn Fn(&str, &serde_json::Value, bool, &str) + Send + Sync>;

/// Recovery при старте (по образцу docs/session-lifecycle.md Hermes):
/// - `clean_shutdown=1` в meta → прошлый выход штатный → НОВАЯ сессия.
/// - Иначе (краш/kill) → та же последняя сессия, `resume_pending=1`
///   (снимается после первого успешного хода), заметка для TUI.
/// - Нет прошлой сессии → новая.
/// - 3+ крашей подряд (`crash_streak`) → принудительно НОВАЯ сессия
///   (stuck-loop escalation: возможно, именно контент сессии валит
///   процесс — начинаем с чистого листа).
fn recover_session(
    state: &Arc<state_db::StateDb>,
    provider: &str,
    model: &str,
) -> (state_db::SessionHandle, Option<String>) {
    let clean = state.meta_get(state_db::meta_keys::CLEAN_SHUTDOWN);
    let streak: u32 = state
        .meta_get(state_db::meta_keys::CRASH_STREAK)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Изначально считаем запуск потенциально аварийным: clean_shutdown
    // сбрасываем сразу. При штатном выходе repl вызовет mark_clean_shutdown.
    state.meta_set(state_db::meta_keys::CLEAN_SHUTDOWN, "0");
    let crashed = clean.as_deref() != Some("1");
    // Инкремент ДО решения: streak — число крашей ВКЛЮЧАЯ этот запуск
    // (иначе 3-й краш даёт streak=2 и эскалация опаздывает на один).
    let streak = if crashed { streak + 1 } else { 0 };
    state.meta_set(state_db::meta_keys::CRASH_STREAK, &streak.to_string());

    let last = state.last_session_id();
    let Some(last_id) = last else {
        // Первый запуск: создаём сессию.
        let id = state.new_session(provider, model);
        return (state_db::SessionHandle::new(state.clone(), id), None);
    };

    // Три аварийных старта подряд → контент сессии подозрителен → новый лист.
    let force_fresh = crashed && streak >= 3;

    // ПУСТАЯ прошлая сессия переиспользуется при ЛЮБОМ сценарии: иначе
    // каждый перезапуск (даже штатный) плодил бы пустую сессию в /sessions.
    if state.count_messages(last_id) == 0 {
        return (state_db::SessionHandle::new(state.clone(), last_id), None);
    }

    if !crashed || force_fresh {
        if force_fresh {
            // Эскалация сработала — счётчик обнулён: следующему крашу нужно
            // снова 3 подряд, чтобы принудительно сбросить.
            state.meta_set(state_db::meta_keys::CRASH_STREAK, "0");
            state.set_resume_pending(last_id, false);
            let id = state.new_session(provider, model);
            return (
                state_db::SessionHandle::new(state.clone(), id),
                Some(format!(
                    "⚠️ Предыдущая сессия аварийно завершалась {streak} раз(а) подряд — начал новую сессию (старая #{last_id} сохранена в БД)."
                )),
            );
        }
        let id = state.new_session(provider, model);
        return (state_db::SessionHandle::new(state.clone(), id), None);
    }

    // Краш: продолжаем ТУ ЖЕ сессию (тот же id, прежний транскрипт).
    state.set_resume_pending(last_id, true);
    let count = state.count_messages(last_id);
    let when = state
        .session_updated_at(last_id)
        .unwrap_or_else(|| "неизвестно когда".to_string());
    let notice = if count > 0 {
        Some(format!(
            "── Прошлый процесс не завершился штатно: продолжаю сессию #{last_id} ({count} сообщений, активность {when}). /reset — начать с чистого листа ──"
        ))
    } else {
        let id = state.new_session(provider, model);
        return (state_db::SessionHandle::new(state.clone(), id), None);
    };
    (state_db::SessionHandle::new(state.clone(), last_id), notice)
}

/// Штатный выход: clean_shutdown=1, снимаем resume_pending.
pub fn mark_clean_shutdown(state: &state_db::StateDb, session_id: i64) {
    state.meta_set(state_db::meta_keys::CLEAN_SHUTDOWN, "1");
    state.meta_set(state_db::meta_keys::CRASH_STREAK, "0");
    state.set_resume_pending(session_id, false);
}

/// Грубая оценка токенов: для смеси русского/английского ~3 символа на
/// токен. Нужно как фолбэк, когда провайдер не присылает usage (Ollama
/// его не отдаёт — из-за этого счётчик "работает странно": всегда 0).
pub(crate) fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64 / 3).max(1)
}

/// Безопасная обрезка строки по СИМВОЛАМ UTF-8, а не по байтам.
/// Срез `&s[..n]` паникует ("byte index N is not a char boundary"), если n
/// попадает внутрь многобайтового символа (кириллица — 2 байта, эмодзи — 4).
/// Возвращает строку длиной максимум `max_chars` символов.
pub(crate) fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => s[..idx].to_string(),
        None => s.to_string(),
    }
}

/// Monitoring — простые метрики поверх `logging_system::get_logger()`
/// (файловый структурированный лог, который раньше был написан, но нигде
/// не подключён). Реальных алертов (уведомлений) нет — только счётчики,
/// доступные через `/stats` в REPL.
#[derive(Default)]
pub struct Metrics {
    pub llm_calls: AtomicU64,
    pub llm_errors: AtomicU64,
    pub tool_calls: AtomicU64,
    pub tool_errors: AtomicU64,
    /// Суммарные токены по всем LLM-запросам сессии (см. `Agent::chat()`)
    /// — используется статус-строкой REPL (`/stats`, шапка TUI).
    pub total_tokens: AtomicU64,
}

impl Metrics {
    pub fn summary(&self) -> String {
        format!(
            "LLM: {} запросов, {} ошибок, {} токенов\nИнструменты: {} вызовов, {} ошибок\nЛоги: {}",
            self.llm_calls.load(Ordering::Relaxed),
            self.llm_errors.load(Ordering::Relaxed),
            self.total_tokens.load(Ordering::Relaxed),
            self.tool_calls.load(Ordering::Relaxed),
            self.tool_errors.load(Ordering::Relaxed),
            logging_system::get_logger().get_log_file_path("main").display(),
        )
    }
}

pub struct Agent {
    llm: Box<dyn LLMClient>,
    tools: SharedToolRegistry,
    messages: Arc<Mutex<Vec<LLMMessage>>>,
    max_iterations: usize,
    auto_approve: bool,
    debug: bool,
    /// Хук, вызываемый после каждого выполненного инструмента —
    /// используется `goal_mode::GoalModeAgent` для отслеживания шагов цели.
    tool_hook: std::sync::Mutex<Option<ToolHook>>,
    pub metrics: Metrics,
    llm_model_label: String,
    llm_provider_name: &'static str,
    llm_model_name: String,
    /// monitoring.rs (итерация 6) — тот же `Arc`, что видят инструменты
    /// `monitoring_dashboard`/`monitoring_health` (см. create_tools()),
    /// поэтому дашборд теперь реально наполняется данными из чата, а не
    /// стоит пустой, как было сразу после подключения модуля.
    pub monitoring: Arc<monitoring::MonitoringSystem>,
    /// learning.rs (итерация 7) — статистика успех/неудача по каждому
    /// инструменту + предпочтения пользователя, копится в течение сессии
    /// и на диске (`~/.hephaestus/learning.json`).
    learning: learning::ContextLearning,
    /// Динамическая рабочая директория (src/workdir.rs) — один и тот же
    /// хендл у Agent и у ВСЕХ инструментов. `/work_dir` в REPL и
    /// `/workdir <путь>` в Telegram меняют её на лету; с этого момента
    /// file_*/bash/git/run_tests/glob работают в новом месте.
    workdir: workdir::WorkDir,
    /// Подтверждения опасных действий как в opencode ("один раз / всегда /
    /// нет"). Публичное поле: TUI и Telegram-бот читают очередь запросов
    /// и доставляют ответ человека.
    pub permissions: Arc<permissions::PermissionManager>,
    /// Движок декларативных правил (permissions-as-data): config.toml
    /// [[permissions.rules]] + правила сессии. Публичный: /permissions
    /// в REPL показывает правила и audit trail.
    pub engine: Arc<permissions::PermissionEngine>,
    /// Рабочий план задачи агента (todo_write/todo_read) — подсказка
    /// подмешивается в system prompt каждый ход, чтобы модель шла по плану.
    pub todos: Arc<todo_tools::TodoBoard>,
    /// MCP-серверы (src/mcp.rs): их инструменты регистрируются в общий
    /// реестр динамически; /mcp reload пересобирает набор.
    pub mcp: Arc<mcp::McpManager>,
    /// Очередь запросов (src/request_queue.rs) — как в opencode: сообщения
    /// из TUI и Telegram встают в FIFO и выполняются строго по одному
    /// фоновым воркером, вместо отбоя "Агент занят".
    pub queue: Arc<request_queue::RequestQueue>,
    /// Живой стриминг: сюда воркер интерфейса кладёт отправитель, а
    /// Agent::chat пересылает туда кусочки текста модели ПО МЕРЕ генерации
    /// (complete_with_tools_stream). None — стрим просто не доставляется,
    /// ответ всё равно собирается целиком.
    pub stream_sink: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
    /// Конфиг LLM — сохраняется для постройки изолированных суб-агентов.
    pub llm_config: LLMConfig,
    /// Суб-агенты: доска задач + фабрика изолированных агентов.
    pub subagents: Arc<tools::subagent::SubAgentRunner>,
    /// Момент старта — для аптайма в /stats.
    pub started: std::time::Instant,
    /// Каноническое хранилище (src/state_db.rs, SQLite WAL) — сообщения и
    /// tool-calls пишутся инкрементально ПО МЕРЕ хода, а не после ответа.
    pub state: Arc<state_db::StateDb>,
    /// Текущая сессия в state_db (меняется /reset-ом — создаётся новая).
    session: state_db::SessionHandle,
    /// Заметка recovery для интерфейса (TUI показывает при старте):
    /// «сессия не завершилась штатно — продолжаю» и т.п.
    pub recovery_notice: Option<String>,
    /// Снапшоты воркспейса (src/snapshots.rs) для /undo: коммит-снимок в
    /// теневом репозитории перед каждым ходом.
    pub snapshots: std::sync::Arc<snapshots::SnapshotManager>,
    /// SMALL MODEL для служебных задач (сводка контекста, подсказки
    /// следующих действий) — опционально задаётся в config.toml
    /// (`small_model = "qwen2.5:3b"`). Если не задана — используется
    /// основная модель. Зачем: сводка сжимает историю дешёвой моделью,
    /// основная модель не жжёт контекст/деньги на черновую работу
    /// (по образцу small_model opencode).
    pub small_model: Option<String>,
}

impl Agent {
    pub fn new(config: LLMConfig, auto_approve: bool, workdir: workdir::WorkDir) -> Self {
        Self::new_with_rules(config, auto_approve, workdir, Vec::new())
    }

    /// Полная фабрика: правила разрешений из config.toml
    /// ([[permissions.rules]]) приходят отдельным параметром — Agent::new
    /// сохраняет старую сигнатуру для тестов/суб-агентов.
    pub fn new_with_rules(
        config: LLMConfig,
        auto_approve: bool,
        workdir: workdir::WorkDir,
        permission_rules: Vec<permissions::RuleConfig>,
    ) -> Self {
        Self::new_full(config, auto_approve, workdir, permission_rules, None)
    }

    /// Полная фабрика: правила разрешений + БД-подключения из config.toml.
    pub fn new_full(
        config: LLMConfig,
        auto_approve: bool,
        workdir: workdir::WorkDir,
        permission_rules: Vec<permissions::RuleConfig>,
        databases: Option<tools::db_universal::DbConnections>,
    ) -> Self {
        let monitoring = Arc::new(monitoring::MonitoringSystem::new());
        let permissions = Arc::new(permissions::PermissionManager::new());
        // ДВИЖОК ПРАВИЛ (permissions-as-data): дефолты (.env/ключи → ask)
        // + правила пользователя из config.toml. Audit trail подключается
        // ниже, когда открыта state.db.
        let engine = Arc::new(permissions::PermissionEngine::new(permission_rules));
        permissions.set_engine(engine.clone());
        // БД-подключения ([databases.NAME] из config.toml) или пустой набор.
        let databases = Arc::new(databases.unwrap_or_else(tools::db_universal::DbConnections::empty));
        let todos = Arc::new(todo_tools::TodoBoard::new());
        let llm_config = config.clone();
        // Суб-агенты: фабрика создаётся здесь, чтобы ToolsEnv мог
        // зарегистрировать agent_* инструменты и прикрепить реестр.
        let subagents = tools::subagent::SubAgentRunner::new(
            llm_config.clone(),
            workdir.clone(),
            permissions.clone(),
        );
        let tools = create_tools(ToolsEnv {
            workdir: workdir.clone(),
            monitoring: monitoring.clone(),
            permissions: permissions.clone(),
            engine: engine.clone(),
            databases: databases.clone(),
            todos: todos.clone(),
            llm_config: llm_config.clone(),
            subagents: subagents.clone(),
        });
        let llm_provider_name = config.provider.as_str();
        let llm_model_name = config.model.clone();
        let llm_model_label = format!("{}/{}", llm_provider_name, config.model);
        // ИСПРАВЛЕНО: раньше здесь было безусловно `OllamaClient::new(config)`
        // — LLMConfig.provider полностью игнорировался, так что запуск с
        // `--provider anthropic/openai/openrouter/koboldcpp/custom` тихо
        // подключался бы к Ollama на localhost:11434 вместо выбранного
        // провайдера (или падал бы с ошибкой соединения, если Ollama не
        // запущен). `create_llm_client()` в llm.rs уже делает правильный
        // выбор клиента по `config.provider` — просто не был здесь вызван.
        let llm: Box<dyn LLMClient> = create_llm_client(config.clone());

        // Каноническое хранилище: SQLite WAL (state_db.rs). Здесь же —
        // решение recovery (шаг «надёжности»): штатно ли завершился
        // прошлый процесс и продолжать ли ту же сессию.
        let state = state_db::StateDb::open_default();
        let (session, recovery_notice) = recover_session(&state, config.provider.as_str(), &config.model);
        let snapshots = std::sync::Arc::new(snapshots::SnapshotManager::new(workdir.get()));
        // AUDIT TRAIL разрешений в state.db (таблица permission_log).
        engine.set_audit(state.clone());

        Self {
            llm,
            tools,
            messages: Arc::new(Mutex::new(Vec::new())),
            max_iterations: 50,
            auto_approve,
            debug: false,
            tool_hook: std::sync::Mutex::new(None),
            metrics: Metrics::default(),
            llm_model_label,
            llm_provider_name,
            llm_model_name,
            monitoring,
            learning: learning::ContextLearning::new(),
            workdir,
            permissions,
            engine,
            todos,
            mcp: Arc::new(mcp::McpManager::new()),
            queue: Arc::new(request_queue::RequestQueue::new()),
            stream_sink: std::sync::Mutex::new(None),
            llm_config,
            subagents,
            started: std::time::Instant::now(),
            state,
            session,
            recovery_notice,
            snapshots,
            small_model: None,
        }
    }

    /// Изолированный агент для суб-агента: общий реестр инструментов и
    /// права, СВОЙ LLM-клиент и пустая история. Вызывается из
    /// subagent::spawn через crate::build_sub_agent.
    pub fn build_sub_agent(runner: &Arc<tools::subagent::SubAgentRunner>) -> Self {
        let monitoring = Arc::new(monitoring::MonitoringSystem::new());
        let sub_session;
        Self {
            llm: create_llm_client(runner.cfg.clone()),
            tools: runner.registry.get().cloned().expect("реестр привязан при старте"),
            messages: Arc::new(Mutex::new(Vec::new())),
            max_iterations: 50,
            auto_approve: true, // суб-агент автономен; опасное всё равно спросит человека
            debug: false,
            tool_hook: std::sync::Mutex::new(None),
            metrics: Metrics::default(),
            llm_model_label: format!("{}/{}", runner.cfg.provider.as_str(), runner.cfg.model),
            llm_provider_name: runner.cfg.provider.as_str(),
            llm_model_name: runner.cfg.model.clone(),
            monitoring,
            learning: learning::ContextLearning::new(),
            workdir: runner.workdir.clone(),
            permissions: runner.permissions.clone(),
            // Реестр инструментов суб-агента общий с родителем — там уже
            // настоящий движок с правилами. Полю же нужен просто валидный
            // Arc: /permissions у суб-агента не вызывается.
            engine: Arc::new(permissions::PermissionEngine::new(vec![])),
            todos: Arc::new(todo_tools::TodoBoard::new()),
            mcp: Arc::new(mcp::McpManager::new()),
            queue: Arc::new(request_queue::RequestQueue::new()),
            stream_sink: std::sync::Mutex::new(None),
            llm_config: runner.cfg.clone(),
            subagents: Arc::clone(runner),
            started: std::time::Instant::now(),
            // Суб-агент: эфемерное in-memory хранилище — диалоги суб-агентов
            // не должны попадать в каноническую БД основной сессии.
            state: {
                let sub = state_db::StateDb::open_in_memory();
                sub_session = state_db::SessionHandle::new(sub.clone(), -1);
                sub
            },
            session: sub_session,
            recovery_notice: None,
            // Суб-агент НЕ делает снимков воркспейса: общий workdir, снимки
            // ведёт основной агент перед каждым ходом.
            snapshots: std::sync::Arc::new(snapshots::SnapshotManager::new(
                std::path::PathBuf::from("/nonexistent"),
            )),
            small_model: None,
        }
    }

    /// Запустить воркер очереди на этом агенте (вызывается ОДИН раз на
    /// процесс — из repl::run_app либо из --telegram-старта). Лок агента
    /// берётся только на время хода; между ходами агент свободен для
    /// /provider, /reset и т.п.
    pub async fn spawn_queue_worker(agent: &Arc<Mutex<Agent>>) {
        let queue = agent.lock().await.queue.clone();
        let agent = agent.clone();
        queue.spawn_worker(move |text| {
            let agent = agent.clone();
            async move {
                // Тот же таймаут на весь ход, что стоял у прямых вызовов
                // в repl.rs (защита от бесконечного "агент думает").
                let fut = async {
                    let mut a = agent.lock().await;
                    a.chat(&text, 50, true).await
                };
                match tokio::time::timeout(std::time::Duration::from_secs(900), fut).await {
                    Ok(reply) => reply,
                    Err(_) => "⏱️ Превышено время ожидания ответа (15 минут) — модель зависла или сеть недоступна. \
                               Проверьте /provider и /model, либо что локальный сервер вообще запущен."
                        .to_string(),
                }
            }
        });
    }

    /// Клон общего реестра — для динамической регистрации инструментов
    /// MCP-серверов (см. mcp::McpManager::connect_and_register).
    pub fn shared_tools(&self) -> SharedToolRegistry {
        self.tools.clone()
    }

    /// Текущая рабочая директория инструментов — для `/work_dir` (REPL)
    /// и `/workdir` (Telegram).
    pub fn work_dir(&self) -> std::path::PathBuf {
        self.workdir.get()
    }

    /// Сменить рабочую директорию на лету. Вызывается ТОЛЬКО после
    /// явного подтверждения пользователя ("доверяю") — из REPL или
    /// из Telegram-команды, никогда по инициативе LLM.
    pub fn set_work_dir(&self, path: std::path::PathBuf) {
        self.workdir.set(path);
    }

    /// Установить (или снять — `None`) хук, вызываемый после каждого
    /// выполненного инструмента: `(name, args, success, output_or_error)`.
    pub fn set_tool_hook(&self, hook: Option<ToolHook>) {
        if let Ok(mut guard) = self.tool_hook.lock() {
            *guard = hook;
        }
    }

    /// Полный отчёт для /stats: метрики сессии, очередь, суб-агенты,
    /// план задачи и дашборд мониторинга (топ инструментов по времени).
    pub async fn stats_report(&self) -> String {
        let messages = self.messages.lock().await;
        let total_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        let est_tokens: u64 = messages.iter().map(|m| estimate_tokens(&m.content)).sum();
        drop(messages);

        let secs = self.started.elapsed().as_secs();
        let uptime = format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60);

        let mut out = format!(
            "⏱ Сессия: {} | Провайдер: {}/{}\nТокены (счётчик): {} | LLM вызовов: {} (ошибок {})\nИнструментов вызывано: {} (ошибок {})\nСообщений в контексте: {} (~{} симв., ~{} токенов)\n📥 Очередь ходов: {}\n🤖 Суб-агентов активно: {}",
            uptime,
            self.llm_provider_name,
            self.llm_model_name,
            self.metrics.total_tokens.load(Ordering::Relaxed),
            self.metrics.llm_calls.load(Ordering::Relaxed),
            self.metrics.llm_errors.load(Ordering::Relaxed),
            self.metrics.tool_calls.load(Ordering::Relaxed),
            self.metrics.tool_errors.load(Ordering::Relaxed),
            {
                let m = self.messages.lock().await;
                m.len()
            },
            total_chars,
            est_tokens,
            self.queue.len().await,
            self.subagents.active_count(),
        );

        // get_dashboard возвращает ToolResult — достаём текст.
        let dash = self.monitoring.get_dashboard();
        let dash_text = if dash.success { dash.output } else { String::new() };
        if !dash_text.trim().is_empty() {
            let lines: Vec<&str> = dash_text.lines().take(25).collect();
            out.push_str("\n\n📊 Мониторинг:\n");
            out.push_str(&lines.join("\n"));
        }

        if let Some(t) = self.todos.context_hint() {
            out.push_str("\n\n📋 Активный план задачи есть — см. /todos");
            let _ = t;
        }

        out
    }


    pub async fn chat(&mut self, user_input: &str, _max_iterations: usize, _auto_approve: bool) -> String {
        // WATCHDOG: ход начался — живость подтверждена (важно для
        // suspended-пауз TUI во время sudo/ask_user: тик отрисовки стоит,
        // но процесс жив).
        crate::watchdog::bump("chat-start");
        // UNDO-СНИМОК перед ходом: если агент испортит файлы, /undo
        // вернёт состояние на этот момент. Best effort, ход не рвёт.
        self.snapshots.snapshot_before_turn(&format!(
            "before turn: {}",
            user_input.chars().take(60).collect::<String>()
        ));
        let user_msg = LLMMessage::user(user_input.to_string());
        // ИНКРЕМЕНТАЛЬНАЯ ЗАПИСЬ (state_db.rs): сообщение пользователя
        // попадает в SQLite СРАЗУ — краш в любой момент хода не теряет его.
        self.session.append_message(&user_msg);
        let mut messages = self.messages.lock().await;
        messages.push(user_msg);
        drop(messages);

        // INTERRUPT (interrupt.rs): маркер эпохи на старте хода — Esc в
        // TUI инкрементирует счётчик, кооперативные точки отмены ниже
        // останавливают ход между итерациями.
        let interrupt_guard = interrupt::InterruptGuard::new();

        // Smart Compression: если контекст слишком разросся, сжимаем
        // старую часть истории в одну сводку через сам LLM, прежде чем
        // отправлять следующий запрос. См. `compress_context`.
        self.compress_context_if_needed().await;

        let mut turn_tools: Vec<String> = Vec::new();
        // DOOM-LOOP DETECTOR (tool_guard.rs): живёт весь ход — если модель
        // вызывает один и тот же инструмент с теми же аргументами больше
        // DOOM_LOOP_LIMIT раз ПОДРЯД, вызов блокируется с объяснением.
        let mut doom_detector = tool_guard::DoomLoopDetector::new();
        for iteration in 0..self.max_iterations {
            // ТОЧКА ОТМЕНЫ 1: Esc между итерациями — ход останавливается,
            // накопленное сохранено (история пишется инкрементально).
            if interrupt_guard.is_interrupted() {
                logging_system::info("[interrupt] ход прерван пользователем (Esc)");
                return "⏹️ Ход прерван (Esc). Часть работы до прерывания сохранена в истории.".to_string();
            }
            if self.debug {
                // ИСПРАВЛЕНО: было `eprintln!` — в TUI-режиме (repl.rs,
                // ratatui + alternate screen) прямая запись в stderr из
                // фонового tokio::spawn-таска летит поверх отрисованного
                // ratatui экрана мимо его буфера и ломает рамки/курсор —
                // именно это выглядело как "[DEBUG] Iteration 0" внутри
                // рамки поля ввода и разъезжающийся Backspace на скриншоте
                // пользователя. Debug теперь идёт в файловый лог
                // (`logging_system.rs`, тот же `~/.hephaestus/logs/`),
                // который и так уже читает `/logs`-подобный тулинг, а не
                // на голый терминал.
                logging_system::debug(&format!("Iteration {}", iteration));
            }

            // mut — retry-политика CompressAndRetry перезагружает guard
            // после сжатия (ниже). tool_schemas идёт за holder-переменной:
            // после сжатия набор схем не меняется, но guard сообщений
            // пересоздаётся, поэтому обе живут в мутабельных локальных.
            let mut messages = self.messages.lock().await;
            let started = Instant::now();
            // Раньше здесь был self.llm.chat(&messages) — тот вызывает
            // LLMClient::complete() без единой схемы инструмента в поле
            // "tools" запроса. Модель физически не могла вызвать ни
            // один инструмент, сколько бы их ни было зарегистрировано в
            // self.tools — не баг конкретного инструмента, а разрыв во
            // всей цепочке. См. ToolRegistry::to_api_schemas в tools/mod.rs.
            let tool_schemas_holder = self.tools.to_api_schemas();
            // learning.rs (итерация 7): если по прошлому опыту сессии
            // какой-то инструмент часто падает или известны предпочтения
            // пользователя — подмешиваем это системным подсказом. Пусто,
            // пока опыта не накопилось (см. get_context_hint()).
            //
            // + todo_tools: активный план задачи (todo_write) тоже идёт в
            // system prompt КАЖДЫЙ ход — так модель не забывает про план
            // после сжатия контекста и длинных цепочек инструментов.
            // Базовое ядро (system_prompt.rs, md-файл) + ЖИВЫЕ данные:
            // git-ветка/грязные файлы, глоссарий инструментов из learning,
            // AVOID-подсказки; затем SESSION HINTS. Лимит держит build().
            let wd_path = self.workdir.get();
            let mut extras = system_prompt::EnvExtras::default();
            if wd_path.join(".git").exists() {
                let run = |a: &[&str]| -> Option<String> {
                    std::process::Command::new("git").args(a).current_dir(&wd_path)
                        .output().ok().filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                };
                extras.git_branch = run(&["branch", "--show-current"])
                    .map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                if let Some(st) = run(&["status", "--porcelain"]) {
                    extras.dirty_files = st.lines().filter(|l| !l.trim().is_empty())
                        .take(5)
                        .map(|l| l.get(3..).unwrap_or(l).trim().to_string())
                        .collect();
                }
            }
            extras.glossary = self.learning.tool_glossary(8);
            extras.avoid = self.learning.avoid_hints();

            let mut hints: Vec<String> = Vec::new();
            // GEFEST.md — постоянные инструкции проекта (аналог CLAUDE.md):
            // грузится каждый ход, лимит 2000 символов.
            if let Ok(pi) = std::fs::read_to_string(wd_path.join("GEFEST.md")) {
                let trimmed: String = pi.trim().chars().take(2000).collect();
                if !trimmed.is_empty() {
                    hints.push(format!("PROJECT INSTRUCTIONS (GEFEST.md):\n{trimmed}"));
                }
            }
            let learning_hint = self.learning.get_context_hint();
            if !learning_hint.is_empty() {
                hints.push(learning_hint);
            }
            if let Some(t) = self.todos.context_hint() {
                hints.push(t);
            }
            let hints_joined = if hints.is_empty() { None } else { Some(hints.join("\n")) };
            let system_prompt = system_prompt::build(
                &wd_path,
                self.llm_provider_name,
                &self.llm_model_name,
                &extras,
                hints_joined.as_deref(),
            );

            // СТРИМИНГ: кусочки текста модели уходят в sink интерфейса
            // (TUI рисует живую строку "модель пишет") по мере генерации.
            let sink = self.stream_sink.lock().ok().and_then(|g| g.clone());
            let on_text: std::sync::Arc<dyn Fn(String) + Send + Sync> = std::sync::Arc::new(move |s: String| {
                if let Some(tx) = &sink {
                    let _ = tx.send(s);
                }
            });

            // Флаг: сжатие по переполнению контекста уже сделано в этом
            // ходе — второй раз не делаем (иначе цикл сжатий).
            let mut compressed_this_turn = false;

            // RETRY/BACKOFF по ПОЛИТИКЕ КЛАССИФИКАТОРА (error_handler.rs):
            // Network/Timeout/RateLimit — повтор с backoff 400мс → 800мс,
            // ContextOverflow — ОДНО сжатие контекста и повтор (переполнение
            // окна не лечится ожиданием), Auth/Billing/404/Validation —
            // FailFast (повтор воспроизведёт ту же ошибку).
            // Пустой ответ ×2 подряд — детерминированный баг провайдера
            // (empty_response_guard из Hermes): ретраи пропускаем.
            // Стрим включаем только для провайдеров с честным SSE —
            // node-шлюзы в stream:true теряют корректную обработку tools.
            let stream_supported = !matches!(
                self.llm_provider_name,
                "custom" | "llama_server" | "koboldcpp"
            );
            let response = {
                let mut attempt: u32 = 0;
                let mut consecutive_empty: u32 = 0;
                loop {
                    attempt += 1;
                    // WATCHDOG: старт LLM-вызова — веха живости.
                    crate::watchdog::bump("llm-call-start");
                    let call = if stream_supported {
                        self.llm.complete_with_tools_stream(
                            &messages,
                            &tool_schemas_holder,
                            Some(system_prompt.as_str()),
                            std::sync::Arc::clone(&on_text),
                        )
                        .await
                    } else {
                        self.llm.complete_with_tools(
                            &messages,
                            &tool_schemas_holder,
                            Some(system_prompt.as_str()),
                        )
                        .await
                    };
                    match call {
                        Ok(r) => {
                            crate::watchdog::bump("llm-call-done");
                            // EMPTY-RESPONSE GUARD: пустой контент И без
                            // tool-вызовов — ничто. Один пустой ответ —
                            // случайность (модель грузится), два подряд —
                            // детерминированный сбой endpoint'а.
                            if r.content.trim().is_empty() && r.tool_use_blocks.is_empty() {
                                consecutive_empty += 1;
                                if consecutive_empty >= 2 || attempt >= 3 {
                                    let raw = format!(
                                        "LLM вернул пустой ответ {} раз подряд (endpoint: {}/{}). Проверьте /provider — сервер жив, но отдаёт пустоту.",
                                        consecutive_empty, self.llm_provider_name, self.llm_model_name
                                    );
                                    logging_system::get_logger().log_llm_error(
                                        &self.llm_model_label, "chat", &raw,
                                    );
                                    return raw;
                                }
                                logging_system::warning("[llm] пустой ответ — одна попытка повторить (empty-response guard)");
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                continue;
                            }
                            break r;
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            self.metrics.llm_errors.fetch_add(1, Ordering::Relaxed);
                            let category = crate::error_handler::classify_llm(&msg);
                            // Compress-and-retry: переполнение окна лечим
                            // сжатием — но только ОДИН раз за ход, иначе
                            // цикл "сжали — всё равно не влезло" крутится
                            // бесконечно. compress_context_if_needed сам
                            // синхронизирует канонический транскрипт в БД.
                            if category.llm_policy() == crate::error_handler::Policy::CompressAndRetry
                                && !compressed_this_turn
                            {
                                logging_system::warning(
                                    "[llm] переполнение контекста — сжимаю историю и повторяю (один раз за ход)",
                                );
                                drop(messages);
                                self.compress_context_if_needed().await;
                                compressed_this_turn = true;
                                messages = self.messages.lock().await;
                                attempt = 0; // свежий бюджет попыток после сжатия
                                continue;
                            }
                            if attempt >= 3 || category.llm_policy() == crate::error_handler::Policy::FailFast {
                                logging_system::get_logger().log_llm_error(
                                    &self.llm_model_label, "chat", &msg,
                                );
                                drop(messages);
                                return format!("LLM error ({}): {msg}", crate::error_handler::friendly_message(&msg));
                            }
                            let delay = std::time::Duration::from_millis(400 * 2u64.pow(attempt - 1));
                            logging_system::info(&format!(
                                "[llm] попытка {attempt} не удалась ({category}) — повтор через {}мс",
                                delay.as_millis()
                            ));
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            };
            let mut response = response;
            let tool_schemas = &tool_schemas_holder;
            // CUSTOM ADAPTER: нативный tool_calls всегда приоритетен. Если
            // endpoint отдал формальный `tool_name(json_arg, ...)` в content,
            // переводим его по тем же schemas, которые отправляли в запрос.
            // Это совместимость с proxy/model template, а не свободный парсинг
            // произвольного текста.
            if self.llm_provider_name == "custom" && response.tool_use_blocks.is_empty() {
                if let Some(parsed) = parser::extract_schema_function_call(&response.content, &tool_schemas) {
                    let names = parsed.calls.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ");
                    logging_system::warning(&format!(
                        "[custom/adapter] текстовый function-call адаптирован по schema: {names}"
                    ));
                    response.tool_use_blocks = parsed.calls.into_iter().enumerate().map(|(i, (name, input))| ToolUseBlock {
                        id: format!("custom_text_call_{i}"), name, input,
                    }).collect();
                    response.content = parsed.cleaned;
                } else if let Some(parsed) = parser::extract_tool_calls(&response.content) {
                    let names = parsed.calls.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ");
                    logging_system::warning(&format!(
                        "[custom/tools] нераспознанный текстовый псевдовызов ({names}); ничего не выполнено"
                    ));
                    response.content = format!(
                        "⚠️ Custom endpoint вернул псевдовызов ({names}) в тексте, но его аргументы не соответствуют schema инструмента. Действие не выполнено."
                    );
                }
            }
            // Диагностика инструментов: видно сразу, вернула ли модель
            // вызовы (нативные или распознанные парсером) или только текст.
            logging_system::info(&format!(
                "[llm] ход: tool_calls={} content_len={} head={:?}",
                response.tool_use_blocks.len(),
                response.content.chars().count(),
                response.content.chars().take(160).collect::<String>()
            ));
            // ЧЕСТНЫЙ СЧЁТЧИК: если провайдер прислал usage — берём его;
            // иначе оцениваем (промпт + ответ), чтобы счётчик двигался
            // всегда, а не показывал нули на Ollama.
            let api_tokens: u64 = response.usage.values().copied().sum();
            let total_tokens = if api_tokens > 0 {
                api_tokens as f64
            } else {
                let prompt_est: u64 = messages.iter().map(|m| estimate_tokens(&m.content)).sum();
                (prompt_est + estimate_tokens(&response.content)) as f64
            };
            self.metrics.total_tokens.fetch_add(total_tokens as u64, Ordering::Relaxed);
                        drop(messages);
            self.metrics.llm_calls.fetch_add(1, Ordering::Relaxed);
            let llm_duration_ms = started.elapsed().as_secs_f64() * 1000.0;
self.monitoring.record_llm_request(self.llm_provider_name, &self.llm_model_name, total_tokens, llm_duration_ms);
            logging_system::get_logger().log_llm_request(
                &self.llm_model_label,
                "chat",
                None,
                Some(llm_duration_ms),
            );

            if !response.tool_use_blocks.is_empty() {
                // ТОЧКА ОТМЕНЫ 2: Esc после ответа LLM, но до запуска
                // инструментов — вызовы не выполняются вовсе.
                if interrupt_guard.is_interrupted() {
                    logging_system::info("[interrupt] ход прерван до выполнения инструментов");
                    return "⏹️ Ход прерван (Esc) до выполнения запрошенных инструментов — ничего не запущено.".to_string();
                }
                let tool_calls = response.tool_use_blocks.clone();

                if self.debug {
                    for tc in &tool_calls {
                        logging_system::debug(&format!("Calling tool: {} with args: {:?}", tc.name, tc.input));
                    }
                }

                // DOOM-LOOP: идентичные вызовы подряд блокируются ДО
                // выполнения (блокировка записывается как tool-результат,
                // чтобы модель видела объяснение и меняла подход).
                let mut blocked: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
                for (i, tc) in tool_calls.iter().enumerate() {
                    if let Err(expl) = doom_detector.check(&tc.name, &tc.input) {
                        logging_system::warning(&format!(
                            "[doom-loop] блокирован вызов {} ({}): {} повторов подряд",
                            tc.name, tc.id, tool_guard::DOOM_LOOP_LIMIT
                        ));
                        blocked.insert(i, expl);
                    }
                }

                // PERSISTENT STATE-MACHINE (state_db.rs): каждый вызов
                // инструмента записывается как pending ДО выполнения и
                // переводится в running/completed/error по факту. После
                // краша `unfinished_tool_calls` покажет, что было
                // недоделано (по образцу tool-part'ов opencode).
                for tc in &tool_calls {
                    self.state
                        .record_tool_call(self.session.id(), &tc.id, &tc.name, &tc.input);
                }

                // Инструменты одного хода выполняются конкурентно
                // (Async Executor) — реестр сейчас Arc-обёрнутый
                // (`SharedToolRegistry`), поэтому клонирование дёшево и
                // безопасно для параллельных вызовов. Заблокированные
                // doom-loop'ом не выполняются вовсе — сразу error-результат.
                turn_tools.extend(tool_calls.iter().map(|tc| tc.name.clone()));
                let state_for_tools = self.state.clone();
                let futures = tool_calls.iter().enumerate().map(|(i, tc)| {
                    let tools = self.tools.clone();
                    let name = tc.name.clone();
                    let input = tc.input.clone();
                    let call_id = tc.id.clone();
                    let state_db = state_for_tools.clone();
                    let blocked_msg = blocked.get(&i).cloned();
                    async move {
                        match blocked_msg {
                            Some(expl) => {
                                state_db.finish_tool_call(&call_id, false, &format!("doom-loop: {expl}"), 0.0);
                                (name, input, ToolResult::error(expl), 0.0)
                            }
                            None => {
                                state_db.mark_tool_running(&call_id);
                                let started = Instant::now();
                                let result = tools.execute(&name, &input).await;
                                // WATCHDOG: инструмент завершился — веха живости
                                // (сам инструмент может не тикать, но ход в
                                // целом ограничен таймаутом 15 минут).
                                crate::watchdog::bump("tool-done");
                                let result = match result {
                                    Some(r) => r,
                                    None => ToolResult::error(format!("Tool not found: {}", name)),
                                };
                                let dur = started.elapsed().as_secs_f64() * 1000.0;
                                let ok = result.success;
                                let out_or_err = if ok {
                                    result.output.clone()
                                } else {
                                    result.error.clone().unwrap_or_else(|| "Unknown error".to_string())
                                };
                                // BOUND TOOL OUTPUT (tool_guard.rs): большой
                                // вывод не тащится ни в БД, ни в контекст —
                                // полный текст выгружается в файл, дальше
                                // идёт голова+хвост с путём. Единая точка:
                                // и транскрипт tool_calls, и история, и
                                // learning/hook видят обрезанную версию.
                                let bounded = tool_guard::bound_tool_output(&call_id, &out_or_err);
                                state_db.finish_tool_call(&call_id, ok, &bounded, dur);
                                let result = if ok {
                                    ToolResult::success(bounded)
                                } else {
                                    ToolResult::error(bounded)
                                };
                                (name, input, result, dur)
                            }
                        }
                    }
                });
                let executed: Vec<(String, serde_json::Value, ToolResult, f64)> = join_all(futures).await;

                // Monitoring: счётчики + структурированный лог на каждый вызов.
                for (name, input, result, duration_ms) in &executed {
                    self.metrics.tool_calls.fetch_add(1, Ordering::Relaxed);
                    if !result.success {
                        self.metrics.tool_errors.fetch_add(1, Ordering::Relaxed);
                    }
                    self.monitoring.record_tool_execution(name, result.success, *duration_ms);
                    logging_system::get_logger().log_tool_execution(
                        name,
                        result.success,
                        Some(*duration_ms),
                        result.error.as_deref(),
                    );

                    // learning.rs (итерация 7): накопление статистики
                    // успех/неудача по каждому инструменту — используется
                    // инструментами learning_stats/learning_hint.
                    let params: std::collections::HashMap<String, serde_json::Value> = input
                        .as_object()
                        .map(|o| o.clone().into_iter().collect())
                        .unwrap_or_default();
                    if result.success {
                        self.learning.record_success(name, &params, &self.llm_model_label);
                    } else {
                        self.learning.record_failure(name, result.error.as_deref().unwrap_or("unknown error"), &params);
                    }
                }

                // Хук (для goal_mode и подобного) вызывается синхронно
                // после того, как все параллельные вызовы завершились —
                // порядок соответствует порядку tool_use-блоков в ответе LLM.
                if let Ok(hook_guard) = self.tool_hook.lock() {
                    if let Some(hook) = hook_guard.as_ref() {
                        for (name, input, result, _) in &executed {
                            let output_or_error = if result.success {
                                result.output.clone()
                            } else {
                                result.error.clone().unwrap_or_default()
                            };
                            hook(name, input, result.success, &output_or_error);
                        }
                    }
                }

                let tool_results: Vec<(String, String, ToolResult)> = tool_calls
                    .iter()
                    .zip(executed.into_iter())
                    .map(|(tc, (name, _, result, _))| (tc.id.clone(), name, result))
                    .collect();

                // ИСПРАВЛЕНО ("команды из telegram не доходят", часть 2):
                // история tool-хода теперь в СТАНДАРТНОМ формате протокола,
                // а не самодельным plain-text'ом:
                //   1) assistant-сообщение несёт ВЫЗОВЫ С РЕАЛЬНЫМИ
                //      аргументами (раньше реконструировалось с input:{}
                //      — модель на следующей итерации видела свои вызовы
                //      без параметров и путалась/повторялась);
                //   2) каждый результат — отдельное role:"tool" сообщение
                //      с tool_call_id (требование OpenAI-диалекта; строгие
                //      серверы вроде llama.cpp /v1 после assistant с
                //      tool_calls ждут именно tool-сообщения, а не user —
                //      иначе поведение не определено/ошибка).
                let mut messages = self.messages.lock().await;
                let assistant_tools_msg = LLMMessage::assistant_with_tools(
                    response.content.clone(),
                    tool_results
                        .iter()
                        .zip(tool_calls.iter())
                        .map(|((id, name, _), tc)| ToolUseBlock {
                            id: id.clone(),
                            name: name.clone(),
                            input: tc.input.clone(),
                        })
                        .collect(),
                );
                messages.push(assistant_tools_msg.clone());
                self.session.append_message(&assistant_tools_msg);
                let mut tool_msgs: Vec<LLMMessage> = Vec::new();
                for (id, name, result) in &tool_results {
                    let text = if result.success {
                        format!("[{}] {}", name, result.output)
                    } else {
                        format!("[{}] ОШИБКА: {}", name, result.error.clone().unwrap_or_else(|| "Unknown error".to_string()))
                    };
                    let m = LLMMessage::tool_result(id.clone(), text);
                    messages.push(m.clone());
                    tool_msgs.push(m);
                }
                drop(messages);
                for m in &tool_msgs {
                    self.session.append_message(m);
                }
            } else {
                // ИТЕРАЦИОННОЕ НАПОМИНАНИЕ (как system-reminder в CC): если
                // финальный текст ЗАЯВЛЯЕТ успех, а верифицирующего
                // инструмента в этом хода не было — честно дописываем это
                // в конец ответа. Строка попадает и в историю, так что
                // модель на следующем ходе видит свой недочёт.
                let mut text = response.content;
                let claims_success = ["успешн", "готово", "создан", "✅", "done", "success"]
                    .iter().any(|k| text.to_lowercase().contains(k));
                let verified = turn_tools.iter().any(|t| {
                    matches!(t.as_str(), "bash" | "run_tests" | "verify_result")
                });
                if claims_success && !verified {
                    text.push_str("\n\n⚠️ Самопроверка: успех заявлен, но верифицирующая команда в этом ходе не запускалась.");
                }
                let final_msg = LLMMessage::assistant(text.clone());
                let mut messages = self.messages.lock().await;
                messages.push(final_msg.clone());
                drop(messages);
                // Успешный финал хода: снимаем resume_pending и обнуляем
                // счётчик крашей (Hermes: флаг живёт только до успешного
                // хода — иначе один сбой навсегда «повисшего» статуса).
                self.state.set_resume_pending(self.session.id(), false);
                self.state.meta_set(state_db::meta_keys::CRASH_STREAK, "0");
                self.session.append_message(&final_msg);
                return text;
            }
        }

        "Max iterations reached".to_string()
    }

    /// Обёртка над `chat()` для терминального вывода "как будто печатает" —
    /// использует `streaming.rs` (итерация 6). Настоящий токен-за-токеном
    /// стриминг ВО ВРЕМЯ генерации несовместим с тем, как здесь устроен
    /// tool calling (нужен полный ответ модели, чтобы понять, вызывает ли
    /// она инструмент — см. комментарий в начале `chat()` и в
    /// `streaming.rs`), поэтому это пост-фактум эмуляция чанками уже
    /// готового финального текста через `streaming::stream_emulated` —
    /// тот же приём, что `streaming.rs` использует для не-Ollama
    /// провайдеров. Используется в `run_simple_loop()` (`--simple`).
    pub async fn chat_streaming<F: FnMut(&str)>(
        &mut self,
        user_input: &str,
        max_iterations: usize,
        auto_approve: bool,
        on_chunk: F,
    ) -> String {
        let text = self.chat(user_input, max_iterations, auto_approve).await;
        streaming::stream_emulated(&text, 5, on_chunk).await;
        text
    }

    /// ИСПРАВЛЕНО (тихая смерть Telegram-бота): раньше здесь был
    /// `blocking_lock()` — вызов из async-контекста ПАНИКУЕТ ("Cannot
    /// block the current thread from within a runtime"). Первый же
    /// /reset из Telegram убивал фоновый таск бота без следа: бот жив,
    /// но молчит. Теперь обычный async-лок.
    pub async fn reset(&self) {
        let mut messages = self.messages.lock().await;
        messages.clear();
        drop(messages);
        // /reset — новая сессия в каноническом хранилище (старая остаётся
        // в БД как архив, транскрипты не уничтожаются).
        let new_id = self.state.new_session(&self.llm_provider_name, &self.llm_model_name);
        self.session.set_id(new_id);
    }

    /// Ленивая фабрика служебного клиента: small_model если задана, иначе
    /// основной LLMConfig. Вызывается per-use — клиент лёгкий.
    fn aux_client(&self) -> Box<dyn LLMClient> {
        let mut cfg = self.llm_config.clone();
        if let Some(sm) = &self.small_model {
            cfg.model = sm.clone();
        }
        create_llm_client(cfg)
    }

    /// Smart Compression (micro-compaction по образцу Hermes):
    /// - старая часть истории сжимается в одну сводку через LLM,
    /// - ПОЛЬЗОВАТЕЛЬСКИЕ сообщения НЕ СУММАРИЗУЮТСЯ никогда — «то, что вы
    ///   сказали, — источник истины»; пересказ разрушает намерение. Они
    ///   остаются в хвосте (переносятся к recent), сжимается только
    ///   assistant/tool-часть середины,
    /// - вытесненные сообщения АРХИВИРУЮТСЯ в state.db (messages_archive),
    ///   ничего не теряется.
    ///
    /// Пороги консервативные: по оценке токенов ИЛИ по числу сообщений.
    pub async fn compress_context_if_needed(&mut self) {
        const COMPACT_THRESHOLD_MSGS: usize = 60;
        // Бюджет в ОЦЕНЁННЫХ токенах: сжимаем, когда контекст подходит к
        // типичному лимиту локальных моделей (~24k), а не когда "много
        // сообщений" — 60 коротких реплик и 60 простыней кода не равны.
        const COMPACT_TOKEN_BUDGET: u64 = 24_000;
        const KEEP_RECENT: usize = 20;

        let (should_compress, reason) = {
            let messages = self.messages.lock().await;
            let est: u64 = messages.iter().map(|m| estimate_tokens(&m.content)).sum();
            if est > COMPACT_TOKEN_BUDGET {
                (true, format!("~{} токенов > бюджета {COMPACT_TOKEN_BUDGET}", est))
            } else if messages.len() > COMPACT_THRESHOLD_MSGS {
                (true, format!("{} сообщений > {COMPACT_THRESHOLD_MSGS}", messages.len()))
            } else {
                (false, String::new())
            }
        };
        if !should_compress {
            return;
        }
        logging_system::info(&format!("[compress] сжатие контекста: {reason}"));

        // РАЗДЕЛЕНИЕ: старая часть (кандидат на сжатие) и защищённый хвост.
        // User-сообщения из старой части НЕ сжимаются — они переносятся в
        // начало хвоста как есть (кратко: пользовательские инструкции живут
        // дольше пересказов).
        let (old_messages, mut recent_messages) = {
            let messages = self.messages.lock().await;
            let split = messages.len().saturating_sub(KEEP_RECENT);
            (messages[..split].to_vec(), messages[split..].to_vec())
        };

        if old_messages.is_empty() {
            return;
        }

        let (users_preserved, compressible): (Vec<LLMMessage>, Vec<LLMMessage>) = old_messages
            .into_iter()
            .partition(|m| m.role == "user" && m.tool_calls.is_none());
        let protected_users_count = users_preserved.len();

        // Защищённые user-сообщения идут в начало хвоста (в хронологическом
        // порядке), сжимается только остальное.
        let mut protected_tail = users_preserved;
        protected_tail.append(&mut recent_messages);
        let recent_messages = protected_tail;

        if compressible.is_empty() {
            // Сжимать нечего (только user-реплики) — не тратим LLM-вызов.
            return;
        }

        let summary_prompt = LLMMessage::user(format!(
            "Сожми следующую историю диалога (реплики ассистента и результаты инструментов) в краткую сводку (5-8 пунктов): \
             ключевые решения, сделанные изменения, текущее состояние задачи. \
             Пиши по-русски, без преамбулы.\n\n---\n{}\n---",
            compressible
                .iter()
                .map(|m| format!("{}: {}", m.role, m.content))
                .collect::<Vec<_>>()
                .join("\n\n")
        ));

        match self.aux_client().complete(&[summary_prompt], Some("Ты помощник, который кратко и точно резюмирует диалоги.")).await {
            Ok(resp) => {
                let mut messages = self.messages.lock().await;
                let mut new_messages = vec![LLMMessage {
                    role: "system".to_string(),
                    content: format!(
                        "[Сжатая сводка предыдущей части диалога]\n{}\n\n[Реплики пользователя сохранены дословно ниже]",
                        resp.content
                    ),
                    tool_calls: None,
                    tool_call_id: None,
                }];
                new_messages.extend(recent_messages.clone());
                let compressed_count = new_messages.len();
                *messages = new_messages.clone();
                drop(messages);
                // СИНХРОНИЗАЦИЯ С БД (по образцу archive_and_compact Hermes):
                // активная таблица messages перезаписывается сводкой+хвостом,
                // вытесненные assistant/tool-сообщения уходят в messages_archive
                // — канонический транскрипт не теряется, resume после краша
                // согласован с памятью.
                self.session.replace_messages(&new_messages);
                let archived = self
                    .state
                    .archive_head_messages(self.session.id(), compressible.len());
                if self.debug {
                    logging_system::debug(&format!(
                        "Контекст сжат: было {} сообщений, стало {} (user-реплик защищено: {}, заархивировано в БД: {})",
                        compressed_count + archived,
                        compressed_count,
                        protected_users_count,
                        archived
                    ));
                }
            }
            Err(e) => {
                // Не удалось сжать через LLM — не блокируем диалог, просто
                // оставляем историю как есть на этот раз.
                logging_system::get_logger().log_llm_error(&self.llm_model_label, "compress_context", &e.to_string());
            }
        }
    }

    /// Снимок истории сообщений — для сохранения сессии (graceful shutdown, `/save`).
    pub async fn snapshot_messages(&self) -> Vec<LLMMessage> {
        self.messages.lock().await.clone()
    }

    /// Восстановить историю сообщений — для `/load` и восстановления сессии при старте.
    pub async fn load_messages(&mut self, msgs: Vec<LLMMessage>) {
        let mut messages = self.messages.lock().await;
        *messages = msgs.clone();
        drop(messages);
        // Канонический транскрипт сессии приводим к загруженному набору.
        self.session.replace_messages(&msgs);
    }

    /// Переключить провайдера/модель на лету (REPL-команды `/provider`,
    /// `/model`) — без перезапуска процесса. Диалог (`self.messages`) не
    /// трогаем: переключение модели посреди разговора — обычный сценарий
    /// (например, начали на Ollama, перешли на Claude для сложного шага).
    pub fn reconfigure_llm(&mut self, config: LLMConfig) {
        self.llm_provider_name = config.provider.as_str();
        self.llm_model_name = config.model.clone();
        self.llm_model_label = format!("{}/{}", self.llm_provider_name, config.model);
        self.llm = create_llm_client(config);
        // Смена провайдера/модели фиксируется в текущей сессии БД.
        self.state.set_session_provider_model(self.session.id(), self.llm_provider_name, &self.llm_model_name);
    }

    /// Идентификатор текущей сессии в state.db — для recovery-логики REPL.
    pub fn session_id(&self) -> i64 {
        self.session.id()
    }

    /// Недоделанные tool-вызовы текущей сессии (после краша) — для заметки
    /// recovery в TUI.
    pub fn unfinished_tool_calls(&self) -> Vec<state_db::ToolCallRow> {
        self.state.unfinished_tool_calls(self.session.id())
    }

    pub fn current_provider_and_model(&self) -> (&'static str, &str) {
        (self.llm_provider_name, &self.llm_model_name)
    }

    /// Быстрые счётчики для шапки REPL (не блокирует — атомарные счётчики,
    /// без запроса к LLM/сети, в отличие от `stats_report()`/`ping_llm()`).
    pub fn quick_stats(&self) -> (u64, u64, u64) {
        (
            self.metrics.total_tokens.load(Ordering::Relaxed),
            self.metrics.llm_calls.load(Ordering::Relaxed),
            self.metrics.tool_calls.load(Ordering::Relaxed),
        )
    }

    /// Быстрая проверка, что текущий LLM реально отвечает — используется
    /// `/provider`/`/model`, чтобы сказать "подключено" по факту успешного
    /// запроса, а не просто "конфиг сохранён и будем посмотреть при
    /// следующем сообщении".
    pub async fn ping_llm(&self) -> Result<(), String> {
        let probe = vec![LLMMessage::user("ping".to_string())];
        self.llm
            .complete(&probe, Some("Ответь одним словом: pong"))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Подсказки следующих действий: короткий LLM-вызов по хвосту
    /// диалога. None — если генерировать не из чего/не удалось.
    pub async fn suggest_next(&self) -> Option<String> {
        let msgs = self.messages.lock().await;
        let take = msgs.len().min(4);
        if take == 0 {
            return None;
        }
        let mut convo = String::new();
        for m in &msgs[msgs.len() - take..] {
            convo.push_str(&format!(
                "{}: {}\n",
                m.role,
                m.content.chars().take(400).collect::<String>()
            ));
        }
        drop(msgs);

        let prompt = LLMMessage::user(format!(
            "Предложи 2-3 коротких логичных следующих действия пользователя после этого диалога. \
             По-русски, каждое с новой строки, без нумерации, без пояснений.\n\n{convo}"
        ));
        let resp = self
            .aux_client()
            .complete(&[prompt], Some("Ты генератор коротких подсказок следующих действий"))
            .await
            .ok()?;
        let t = resp.content.trim().to_string();
        (!t.is_empty()).then_some(t)
    }

    /// Метаданные зарегистрированных инструментов (имя, версия, версия схемы) — для `/tools`.
    pub fn tool_metadata(&self) -> Vec<tools::ToolMetadata> {
        self.tools.list_metadata()
    }
}

#[tokio::main]
async fn main() {
    // BOOTSTRAP первого запуска: примеры конфигов вшиты в бинарник и
    // распаковываются в ~/.hephaestus/ у каждого пользователя свои.
    // Существующие файлы никогда не перезаписываются.
    let bootstrapped = bootstrap::ensure_examples();
    if !bootstrapped.is_empty() {
        eprintln!("📁 Первый запуск: созданы примеры конфигов:");
        for p in &bootstrapped {
            eprintln!("   {}", p.display());
        }
        let home = std::env::var("HEPHAESTUS_HOME")
            .unwrap_or_else(|_| dirs::home_dir().map(|h| h.join(".hephaestus")).unwrap_or_default().to_string_lossy().to_string());
        eprintln!("   Токен Telegram: /telegram save <токен> (или {home}/telegram_token.txt)");
        eprintln!();
    }
    // ИСПРАВЛЕНО: раньше здесь был жёстко зашитый LLMConfig на Ollama —
    // единственный способ сменить провайдера/модель был править исходник
    // и пересобирать. Теперь конфиг читается из ~/.hephaestus/config.toml
    // (config.rs), а поменять его на лету можно командами /provider и
    // /model прямо в REPL — они же сохраняют выбор обратно в этот файл.
    //
    // ИСПРАВЛЕНО (жалоба "почему определяется llama3.2:3b, которой у
    // меня нет"): раньше "файла нет" и "файл ЕСТЬ, но битый" (например,
    // задвоенные ключи `provider =` на верхнем уровне TOML) тихо
    // схлопывались в один и тот же дефолт без объяснения. Теперь для
    // случая "файл битый" явно печатается текст ошибки парсинга —
    // пользователь видит, что дело не в отсутствующей модели, а в
    // невалидном config.toml.
    let agent_config = match config::AgentConfig::load() {
        config::LoadResult::Loaded(cfg) => cfg,
        config::LoadResult::NotFound(cfg) => {
            eprintln!(
                "ℹ️  Конфиг не найден ({}), использую дефолт: {}/{}. Настроить: /provider в REPL.\n",
                config::AgentConfig::config_file_path(),
                cfg.provider,
                cfg.model
            );
            cfg
        }
        config::LoadResult::ParseError { default, error } => {
            eprintln!(
                "⚠️  Не удалось разобрать {} — файл повреждён (например, задвоены ключи на верхнем уровне TOML):",
                config::AgentConfig::config_file_path()
            );
            eprintln!("    {}", error);
            eprintln!(
                "Использую дефолт: {}/{}, пока вы не поправите файл или не зададите провайдера через /provider.\n",
                default.provider, default.model
            );
            default
        }
    };
    let config = match agent_config.to_llm_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("⚠️  {}", e);
            eprintln!("Использую дефолт: ollama/llama3.2:3b (локально, localhost:11434).");
            eprintln!("Поменять провайдера/модель можно из REPL командой /provider или /model.\n");
            config::AgentConfig::default()
                .to_llm_config()
                .expect("дефолтный конфиг всегда валиден — Ollama не требует API-ключа")
        }
    };

    // Рабочая директория инструментов: сохранённая в конфиге
    // (/work_dir, /workdir) → иначе директория запуска процесса.
    // Если сохранённая папка удалена/переименована — предупреждаем и
    // падаем на CWD, а не молча пишем файлы мимо.
    let workdir = match agent_config.work_dir.as_deref().map(workdir::expand_home) {
        Some(p) if p.is_dir() => workdir::WorkDir::new(p),
        Some(p) => {
            eprintln!(
                "⚠️  Сохранённая рабочая директория {} не существует (удалена или переименована).\n    Использую директорию запуска. Задать заново: /work_dir <путь>\n",
                p.display()
            );
            workdir::WorkDir::from_cwd()
        }
        None => workdir::WorkDir::from_cwd(),
    };

    let mut agent = Agent::new_full(
        config,
        true,
        workdir,
        agent_config.permissions.clone(),
        Some(tools::db_universal::DbConnections::new(agent_config.databases.clone())),
    );
    // SMALL MODEL (config.toml: small_model = "...") — служебные вызовы
    // (сводка контекста, подсказки) идут через дешёвую модель, основная
    // экономит контекст.
    agent.small_model = agent_config.small_model.clone();

    // WATCHDOG (watchdog.rs): std::thread-наблюдатель за живостью
    // рантайма. 15 минут тишины от ВСЕХ тикеров (TUI-отрисовка, Telegram
    // poll, MCP-watchdog) = зависание процесса → exit(75).
    watchdog::spawn();

    // Ловушка паник: паника в фоновом tokio::spawn-таске (Telegram-бот,
    // воркер очереди) НЕ убивает процесс — она тихо глушит только сам
    // таск. Снаружи это выглядит как "бот просто не отвечает". Теперь
    // каждая паника попадает в файловый лог с местом.
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            crate::logging_system::error(&format!(
                "ПАНИКА: {} ({})",
                info,
                info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default()
            ));
            default_hook(info);
        }));
    }

    // ИСПРАВЛЕНО: раньше здесь был жёстко `agent.debug = true` — включаем debug-лог
    // только по явному флагу `--debug`, и он теперь пишет в файл
    // (см. фикс eprintln! выше по коду), а не в терминал TUI.
    let args: Vec<String> = std::env::args().collect();
    agent.debug = args.iter().any(|a| a == "--debug");

    // Режим запросов разрешений: в TUI и Telegram запрос кладётся в
    // очередь — интерфейс показывает его человеку (/allow, /deny или
    // "1"/"2"/"3" в чате). В --simple/--voice терминал свободен —
    // спрашиваем напрямую через stdin (y/a/n).
    let interactive_tui_absent = args.iter().any(|a| a == "--simple" || a == "--voice");
    agent.permissions.set_mode(if interactive_tui_absent {
        permissions::PermissionMode::Terminal
    } else {
        permissions::PermissionMode::Queue
    });

    // confirmation.rs (итерация 5): `--yes`/`-y` включает глобальный
    // автоподтверждение — тот же смысл, что `"force": true` у BashTool
    // (см. security.rs), но на уровне всего процесса, а не отдельного
    // вызова инструмента. bash_tool.rs проверяет force ИЛИ этот флаг.
    if args.iter().any(|a| a == "--yes" || a == "-y") {
        confirmation::set_auto_confirm(true);
    }

    // MCP-серверы (~/.hephaestus/mcp.toml): подключаем ДО входа в TUI —
    // здесь ещё обычный терминал, отчёты о подключении можно печатать
    // в stderr. Упавший сервер НЕ рвёт запуск: предупреждение и дальше.
    match mcp::load_config() {
        Ok(servers) if !servers.is_empty() => {
            eprintln!("🔌 MCP: подключаю {} сервер(ов)…", servers.len());
            for s in &servers {
                let report = agent.mcp.connect_and_register(s, &agent.shared_tools()).await;
                let mark = if report.ok { "✅" } else { "⚠️" };
                eprintln!("{} MCP [{}]: {}", mark, report.name, report.detail);
            }
            eprintln!();
        }
        Ok(_) => {}
        Err(e) => eprintln!("⚠️ MCP конфиг: {}\n", e),
    }
    // Watchdog MCP: раз в минуту поднимает упавшие серверы (идемпотентно).
    {
        let mcp = agent.mcp.clone();
        let registry = agent.shared_tools();
        mcp.spawn_watchdog(registry);
    }

    // ── Автостарт Telegram-бота ──
    // Частая причина "бот не работает": агент перезапущен, а /telegram
    // start никто не ввёл вручную — бот мёртв до команды. Если токен
    // сохранён, бот поднимается САМ. Отключить: --no-telegram.
    if !args.iter().any(|a| a == "--no-telegram" || a == "--simple" || a == "--voice" || a == "--telegram")
        && telegram_bot::load_token().is_some()
    {
        let agent_arc = Arc::new(tokio::sync::Mutex::new(agent));
        Agent::spawn_queue_worker(&agent_arc).await;
        let allowed_chat_id =
            std::env::var("TELEGRAM_ALLOWED_CHAT_ID").ok().and_then(|v| v.parse::<i64>().ok());
        let bot = telegram_bot::TelegramBot::new(
            telegram_bot::load_token().unwrap_or_default(),
            agent_arc.clone(),
            allowed_chat_id,
        );
        tokio::spawn(async move {
            bot.run().await;
            crate::logging_system::warning("[telegram] ПОТОК БОТА ЗАВЕРШИЛСЯ (вернулся из run() — см. причину выше в логе)");
        });
        eprintln!("✅ Telegram-бот запущен автоматически (отключить: --no-telegram)\n");
        // TUI работает на ТОМ ЖЕ общем агенте — одна очередь/диалог.
        if let Err(e) = repl::run_shared(agent_arc).await {
            eprintln!("Ошибка REPL: {}", e);
        }
        return;
    }

    // Логотип — только для полноэкранного REPL (не для --simple/--telegram,
    // где смешивать ANSI-анимацию с построчным выводом не нужно).
    if !args.iter().any(|a| a == "--simple" || a == "--telegram" || a == "--voice" || a == "--no-logo") {
        hephaestus_logo::show_logo_animated(1.5).await;
    }

    if args.iter().any(|a| a == "--voice") {
        // WATCHDOG: в voice-режиме нет тика TUI — живость доказывает
        // runtime-тикер (простой пользователя на read_line — норма).
        watchdog::spawn_runtime_ticker();
        let agent_arc = std::sync::Arc::new(tokio::sync::Mutex::new(agent));
        let voice = voice_interface::create_voice_interface(
            agent_arc.clone(),
            None, // модель whisper по умолчанию ("small")
            None, // язык по умолчанию ("ru")
            None, // piper не настроен — используется espeak(-ng), если есть
            None,
            false,
        );
        voice.run_loop().await;
        // Штатный выход (--voice).
        if let Ok(a) = agent_arc.try_lock() {
            mark_clean_shutdown(&a.state, a.session_id());
        }
        return;
    }

    if args.iter().any(|a| a == "--simple") {
        watchdog::spawn_runtime_ticker();
        run_simple_loop(agent).await;
        return;
    }

    if args.iter().any(|a| a == "--telegram") {
        watchdog::spawn_runtime_ticker();
        telegram_bot::run_from_env(agent).await;
        return;
    }

    if let Err(e) = repl::run(agent).await {
        eprintln!("Ошибка REPL: {}", e);
    }
}

/// Старый простой текстовый цикл — оставлен как запасной вариант
/// (`--simple`) на случай, если терминал не поддерживает TUI (например,
/// при перенаправлении stdin/stdout не в tty).
async fn run_simple_loop(mut agent: Agent) {
    println!("⚡ Гефест — AI Coding Agent (Rust) [простой режим]");
    println!("Введите ваш запрос (или 'exit' для выхода):\n");

    let mut input = String::new();
    loop {
        print!("> ");
        std::io::Write::flush(&mut std::io::stdout()).unwrap();
        input.clear();
        std::io::stdin().read_line(&mut input).unwrap();
        let input = input.trim();
        if input == "exit" || input == "quit" {
            break;
        }
        if input.is_empty() {
            continue;
        }

        print!("\n");
        let _response = agent
            .chat_streaming(input, 50, true, |chunk| {
                print!("{}", chunk);
                std::io::Write::flush(&mut std::io::stdout()).ok();
            })
            .await;
        println!("\n");
    }
    // ШТАТНЫЙ ВЫХОД (--simple тоже чистый выход): иначе следующий запуск
    // считал процесс крашнутым и продолжал «аварийную» сессию.
    mark_clean_shutdown(&agent.state, agent.session_id());
}
