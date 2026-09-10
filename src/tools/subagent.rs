//! Суб-агенты — параллельные задачи со СВОИМ контекстом диалога.
//!
//! Отличие от background_run (фон для ОДНОГО инструмента): здесь
//! запускается полноценный агент с чистой историей, который сам
//! планирует шаги и вызывает любые инструменты реестра (bash, file_*,
//! mcp_*...), а в конце возвращает отчёт. Главный диалог не засоряется
//! промежуточными простынями.
//!
//! Изоляция и общее:
//!   • СВОИ: сообщения/контекст, отдельный LLM-клиент;
//!   • ОБЩЕЕ: реестр инструментов, рабочая директория, разрешения
//!     (опасное действие суб-агента спросит у человека как обычно),
//!     план todos, мониторинг.
//!
//! Инструменты:
//!   agent_spawn  {task}            — запустить (id в ответе)
//!   agent_status {}                — список задач
//!   agent_result {id, wait_secs?}  — дождаться (до 120с) и получить отчёт

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use serde_json::Value;

use crate::llm::LLMConfig;
use crate::workdir::WorkDir;
use crate::tools::{Tool, ToolResult};

#[derive(Clone)]
struct Job {
    status: String, // running | done | error
    result: String,
    started: String,
}

/// Доска суб-агентов. Runner хранит всё нужное для постройки
/// изолированного агента на каждый spawn.
pub struct SubAgentRunner {
    pub cfg: LLMConfig,
    pub workdir: WorkDir,
    pub permissions: Arc<crate::permissions::PermissionManager>,
    /// Реестр прикрепляется менеджером после сборки (OnceLock — как в
    /// SharedToolRegistry): на момент создания ToolsEnv реестра ещё нет.
    pub(crate) registry: std::sync::OnceLock<crate::tools::SharedToolRegistry>,
    jobs: StdMutex<HashMap<String, Job>>,
    next_id: AtomicU64,
}

impl SubAgentRunner {
    pub fn new(
        cfg: LLMConfig,
        workdir: WorkDir,
        permissions: Arc<crate::permissions::PermissionManager>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            workdir,
            permissions,
            registry: std::sync::OnceLock::new(),
            jobs: StdMutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// Пустой раннер для суб-агентов второго уровня (не используется
    /// инструментами, только чтобы build_sub_agent мог собрать структуру).
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            cfg: LLMConfig {
                provider: crate::llm::LLMProvider::Custom,
                model: "sub".into(),
                api_key: None,
                base_url: None,
    extra_body: None,
    extra_headers: HashMap::new(),
                temperature: 0.1,
                max_tokens: 1024,
                stream: false,
            },
            workdir: WorkDir::from_cwd(),
            permissions: Arc::new(crate::permissions::PermissionManager::new()),
            registry: std::sync::OnceLock::new(),
            jobs: StdMutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// Прикрепить общий реестр (вызывает create_tools).
    pub fn attach_registry(&self, reg: crate::tools::SharedToolRegistry) {
        let _ = self.registry.set(reg);
    }

    fn set_status(&self, id: &str, status: &str, result: String) {
        if let Ok(mut j) = self.jobs.lock() {
            if let Some(job) = j.get_mut(id) {
                job.status = status.to_string();
                job.result = result;
            }
        }
    }

    fn snapshot(&self) -> Vec<(String, Job)> {
        self.jobs
            .lock()
            .map(|j| j.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// Активные (running) задачи — для /stats.
    pub fn active_count(&self) -> usize {
        self.jobs
            .lock()
            .map(|j| j.values().filter(|v| v.status == "running").count())
            .unwrap_or(0)
    }
}

fn now_label() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

pub struct AgentSpawnTool {
    runner: Arc<SubAgentRunner>,
}

impl AgentSpawnTool {
    pub fn new(runner: Arc<SubAgentRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Tool for AgentSpawnTool {
    fn name(&self) -> &'static str {
        "agent_spawn"
    }

    fn description(&self) -> &'static str {
        "Запустить СУБ-АГЕНТА параллельно: отдельный агент с чистым контекстом получит задачу и выполнит её любыми инструментами (файлы, bash, mcp...). Используй для независимых подзадач (исследование, рефакторинг модуля, поиск по кодовой базе), чтобы не засорять основной диалог; после делегации НЕ выполняй ту же работу сам. read_only=true запрещает суб-агенту изменять файлы и состояние (только поиск/анализ). breadth задаёт глубину поиска. Вернёт id; результат забери agent_result."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "Полная формулировка задачи для суб-агента (он НЕ видит ваш диалог — всё нужное опиши здесь)"},
                "read_only": {"type": "boolean", "description": "true — только поиск и анализ, без изменения файлов/состояния"},
                "breadth": {"type": "string", "enum": ["quick", "medium", "very thorough"], "description": "Ширина поиска для исследовательских задач"},
                "persona": {"type": "string", "enum": ["general", "planner", "explorer", "worker"], "description": "Роль суб-агента: planner/explorer — только анализ и план; worker — исполнитель"}
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let Some(task) = args.get("task").and_then(|t| t.as_str()).filter(|s| !s.trim().is_empty()) else {
            return ToolResult::error("нужен непустой параметр 'task'");
        };
        let task = task.trim().to_string();
        let mut read_only = args.get("read_only").and_then(|v| v.as_bool()).unwrap_or(false);
        let persona = args.get("persona").and_then(|p| p.as_str()).unwrap_or("general").to_string();

        // Персоны по образцу agents/*.md из Claude Code (Plan/worker/Explore).
        let persona_directive = match persona.as_str() {
            "planner" => {
                read_only = true;
                Some("ROLE: Software architect & planning specialist (READ-ONLY).\n\
Explore the codebase, identify critical files and architectural trade-offs, \
then output a STEP-BY-STEP IMPLEMENTATION PLAN. Do not modify anything.")
            }
            "explorer" => {
                read_only = true;
                Some("ROLE: Research specialist (READ-ONLY).\n\
Methodology: decompose the question -> sweep breadth first -> drill into promising leads -> \
verify key facts in >=2 places -> report findings WITH evidence (file paths or URLs).")
            }
            "worker" => {
                Some("ROLE: Worker executing an assigned task.\n\
Complete EXACTLY what was asked; unrelated issues you discover are follow-up suggestions only. \
If file state is confusing because of someone else's changes — stop and report instead of fixing. \
If you changed files, say clearly what changed and show verification results.")
            }
            _ => None,
        };
        let breadth = args.get("breadth").and_then(|v| v.as_str()).unwrap_or("").to_string();

        // Директивы приклеиваются ПЕРЕД задачей: суб-агент получает их как
        // часть первого сообщения (паттерн Explore.md из Claude Code).
        let mut full_task = String::new();
        if let Some(d) = persona_directive {
            full_task.push_str(d);
            full_task.push_str("\n\n");
        }
        if read_only {
            full_task.push_str(
                "STRICT READ-ONLY MODE: do not create, modify or delete any files; no redirects to files, no heredocs, no state-changing commands. Search and analyze only.\n\n",
            );
        }
        match breadth.as_str() {
            "quick" => full_task.push_str("Search breadth: quick — a single targeted lookup.\n\n"),
            "medium" => full_task.push_str("Search breadth: medium — moderate exploration.\n\n"),
            "very thorough" => full_task.push_str("Search breadth: very thorough — multiple locations and naming conventions.\n\n"),
            _ => {}
        }
        full_task.push_str(&task);
        let task = full_task;

        let id = format!("agent_{}", self.runner.next_id.fetch_add(1, Ordering::Relaxed));
        if let Ok(mut j) = self.runner.jobs.lock() {
            j.insert(id.clone(), Job {
                status: "running".into(),
                result: String::new(),
                started: now_label(),
            });
        }

        let runner = self.runner.clone();
        let id2 = id.clone();
        let task2 = task.clone();
        tokio::spawn(async move {
            // СТЕЙДЖИНГ ПАМЯТИ (write_approval-модель Hermes): суб-агент
            // фоновый — его memory_file_save НЕ пишет в постоянную память
            // напрямую, а кладёт запись в pending/ на утверждение
            // (/memory approve в TUI). Пометка ставится здесь, внутри
            // spawned-таска; thread_local живёт в этом таске-исполнителе.
            crate::memory_tools::mark_background();
            // Изолированный агент: свой клиент + пустая история; инструменты/
            // права/директория — общие с основным.
            let mut sub = crate::Agent::build_sub_agent(&runner);
            let out = sub.chat(&task2, 12, true).await;
            crate::memory_tools::clear_background();
            let (status, _) = if out.trim_start().starts_with("LLM error") {
                ("error", ())
            } else {
                ("done", ())
            };
            runner.set_status(&id2, status, out);
        });

        ToolResult::success(format!(
            "Суб-агент запущен: {id}\nЗабрать отчёт: agent_result {{\"id\":\"{id}\",\"wait_secs\":120}}"
        ))
    }
}

pub struct AgentStatusTool {
    runner: Arc<SubAgentRunner>,
}
impl AgentStatusTool {
    pub fn new(runner: Arc<SubAgentRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Tool for AgentStatusTool {
    fn name(&self) -> &'static str {
        "agent_status"
    }

    fn description(&self) -> &'static str {
        "Список суб-агентов: id, статус (running/done/error), начало работы."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _args: &Value) -> ToolResult {
        let mut jobs = self.runner.snapshot();
        if jobs.is_empty() {
            return ToolResult::success("(суб-агентов пока не было)");
        }
        jobs.sort_by(|a, b| a.0.cmp(&b.0));
        let lines: Vec<String> = jobs
            .iter()
            .map(|(id, j)| format!("{id} [{}] с {} — {}", j.status, j.started, first_line(&j.result)))
            .collect();
        ToolResult::success(lines.join("\n"))
    }
}

pub struct AgentResultTool {
    runner: Arc<SubAgentRunner>,
}
impl AgentResultTool {
    pub fn new(runner: Arc<SubAgentRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Tool for AgentResultTool {
    fn name(&self) -> &'static str {
        "agent_result"
    }

    fn description(&self) -> &'static str {
        "Дождаться завершения суб-агента и получить его отчёт. wait_secs — сколько ждать, если ещё работает (до 120с)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "wait_secs": {"type": "integer", "description": "по умолчанию 60"}
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let Some(id) = args.get("id").and_then(|i| i.as_str()) else {
            return ToolResult::error("нужен 'id'");
        };
        let wait = args.get("wait_secs").and_then(|w| w.as_u64()).unwrap_or(60).min(120);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait);
        loop {
            {
                let jobs = match self.runner.jobs.lock() {
                    Ok(j) => j,
                    Err(_) => return ToolResult::error("jobs poisoned"),
                };
                match jobs.get(id) {
                    None => return ToolResult::error(format!("задача '{id}' не найдена (см. agent_status)")),
                    Some(j) if j.status != "running" => {
                        let head: String = j.result.chars().take(6000).collect();
                        return ToolResult::success(head);
                    }
                    Some(_) => {}
                }
            }
            if std::time::Instant::now() >= deadline {
                return ToolResult::success(format!("{id}: всё ещё работает — вызовите agent_result позже"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

pub fn create_subagent_tools(runner: Arc<SubAgentRunner>) -> Vec<(&'static str, Arc<dyn Tool>)> {
    vec![
        ("agent_spawn", Arc::new(AgentSpawnTool::new(runner.clone()))),
        ("agent_status", Arc::new(AgentStatusTool::new(runner.clone()))),
        ("agent_result", Arc::new(AgentResultTool::new(runner))),
    ]
}
