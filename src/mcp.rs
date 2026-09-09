//! MCP (Model Context Protocol) — универсальный клиент.
//!
//! Позволяет подключать ЛЮБЫЕ MCP-серверы: локальные (stdio — Blender
//! MCP, filesystem, git, playwright, ...) и удалённые (Streamable HTTP).
//! Все tools сервера автоматически появляются в общем реестре агента с
//! префиксом `mcp_<сервер>_`, и LLM вызывает их как обычные инструменты.
//!
//! Протокол: JSON-RPC 2.0. Жизненный цикл по спецификации:
//!   1) initialize (обмен версиями/возможностями)
//!   2) уведомление notifications/initialized
//!   3) tools/list → регистрация прокси-инструментов
//!   4) tools/call по требованию LLM
//!
//! Транспорты:
//!   • stdio — дочерний процесс, обмен NDJSON через stdin/stdout. stderr
//!     процесса уводится в файловый лог (НЕ наследуется — иначе болтающийся
//!     в stdout мусор от TUI/Telegram гарантирован). Строки stdout, которые
//!     не парсятся как JSON, ПРОПУСКАЮТСЯ с предупреждением в лог — многие
//!     серверы печатают баннеры/логи, это не должно рвать протокол.
//!   • Streamable HTTP — POST JSON-RPC на URL; ответ может быть обычным
//!     JSON ИЛИ потоком SSE (парсится до события с нашим id). Заголовок
//!     mcp-session-id из initialize переиспользуется дальше. Легаси-
//!     транспорт "GET /sse + отдельный POST-endpoint" сознательно НЕ
//!     поддержан (современные официальные SDK все умеют streamable HTTP),
//!     см. примечание у HttpTransport.
//!
//! Конфиг: `~/.hephaestus/mcp.toml` (создаётся с примерами при первом
//! запуске). Управление из REPL: /mcp, /mcp reload.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{Tool, ToolResult, SharedToolRegistry};

const PROTOCOL_VERSION: &str = "2024-11-05";
/// Таймауты: рукопожатие короткое, вызов инструмента — длинный (Blender
/// рендерит сцену минутами; таймаут перекрывается per-server timeout_ms).
const INIT_TIMEOUT: Duration = Duration::from_secs(15);
const LIST_TIMEOUT: Duration = Duration::from_secs(15);
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 120_000;

// ============================================================ конфиг ===

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpServerConfig {
    /// Имя — часть имени инструментов (`mcp_<name>_<tool>`).
    pub name: String,
    /// stdio-транспорт: команда запуска сервера ("uvx", "npx", "python"...).
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Дополнительные переменные окружения процесса (поверх унаследованных).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Рабочая директория процесса сервера (по умолчанию — текущая).
    #[serde(default)]
    pub cwd: Option<String>,
    /// http-транспорт: URL endpoint'а (обычно заканчивается на /mcp).
    #[serde(default)]
    pub url: Option<String>,
    /// Произвольные HTTP-заголовки (например Authorization).
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Таймаут одного вызова инструмента, мс.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl McpServerConfig {
    fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("пустое имя сервера".to_string());
        }
        match (&self.command, &self.url) {
            (Some(_), Some(_)) => {
                Err("заданы И command (stdio), И url (http) — выберите одно".to_string())
            }
            (None, None) => Err("не задан ни command (stdio), ни url (http)".to_string()),
            _ => Ok(()),
        }
    }
}

fn mcp_config_path() -> std::path::PathBuf {
    let mut p = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push(".hephaestus");
    p.push("mcp.toml");
    p
}

const DEFAULT_MCP_CONFIG: &str = r#"# MCP-серверы Гефеста (~/.hephaestus/mcp.toml).
# Любой MCP-сервер: локальный (command = процесс) или удалённый (url = HTTP).
# Инструменты сервера появляются у агента как mcp_<имя>_<инструмент>.
# Применить изменения: /mcp reload
#
# Примеры:
#
# [[servers]]
# name = "blender"
# command = "uvx"
# args = ["blender-mcp"]
#
# [[servers]]
# name = "filesystem"
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/marsel"]
#
# [[servers]]
# name = "remote-tools"
# url = "http://localhost:8000/mcp"
# # headers = { Authorization = "Bearer ..." }
# # timeout_ms = 300000
"#;

/// Загрузить конфиг; если файла нет — создать с закомментированными
/// примерами (чтобы человек сразу видел формат) и вернуть пустой список.
pub fn load_config() -> Result<Vec<McpServerConfig>, String> {
    let path = mcp_config_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, DEFAULT_MCP_CONFIG);
        return Ok(Vec::new());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        servers: Vec<McpServerConfig>,
    }
    let root: Root = toml::from_str(&content)
        .map_err(|e| format!("{} повреждён: {}", path.display(), e))?;
    for s in &root.servers {
        s.validate().map_err(|e| format!("сервер '{}': {}", s.name, e))?;
    }
    Ok(root.servers.into_iter().filter(|s| s.enabled).collect())
}

// ========================================================= JSON-RPC ===

fn rpc_request(id: u64, method: &str, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

fn rpc_notification(method: &str, params: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

/// Достать result или превратить error в понятную строку.
fn unwrap_rpc_response(v: &Value, what: &str) -> Result<Value, String> {
    if let Some(err) = v.get("error") {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("?");
        return Err(format!("{}: JSON-RPC error {}: {}", what, code, msg));
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| format!("{}: в ответе нет ни result, ни error", what))
}

/// Вытащить полезную нагрузку из строки SSE (`data: ...`) — для http-
/// транспорта, когда сервер отвечает event-stream'ом. Возвращает payload
/// события, содержащего наш id, либо последний data перед закрытием.
fn extract_sse_data(line_block: &[&str]) -> Option<String> {
    line_block
        .iter()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(|d| d.trim_start().to_string())
        .last()
}

// ======================================================= транспорт ===

type PendingMap = Arc<StdMutex<HashMap<u64, tokio::sync::oneshot::Sender<Value>>>>;

enum Transport {
    Stdio {
        child: tokio::process::Child,
        // Mutex, чтобы писать из request() и notify() при &self.
        stdin: Arc<tokio::sync::Mutex<tokio::process::ChildStdin>>,
        pending: PendingMap,
    },
    Http {
        url: String,
        client: reqwest::Client,
        session: StdMutex<Option<String>>,
        headers: HashMap<String, String>,
    },
}

struct RpcOutcome {
    response: Value,
}

impl Transport {
    async fn request(&self, id: u64, body: Value, timeout: Duration) -> Result<RpcOutcome, String> {
        match self {
            Transport::Stdio { stdin, pending, .. } => {
                let tx = {
                    let mut map = pending.lock().map_err(|_| "pending map poisoned")?;
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    map.insert(id, tx);
                    rx
                };
                use tokio::io::AsyncWriteExt;
                let mut line = body.to_string();
                line.push('\n');
                {
                    let mut w = stdin.lock().await;
                    w.write_all(line.as_bytes())
                        .await
                        .map_err(|e| format!("stdin сервера закрыт: {}", e))?;
                    w.flush().await.map_err(|e| e.to_string())?;
                }
                match tokio::time::timeout(timeout, tx).await {
                    Ok(Ok(resp)) => Ok(RpcOutcome { response: resp }),
                    Ok(Err(_)) => Err("сервер закрылся, не ответив".to_string()),
                    Err(_) => Err(format!(
                        "таймаут {}с ожидания ответа от MCP-сервера",
                        timeout.as_secs()
                    )),
                }
            }
            Transport::Http { url, client, session, headers } => {
                let mut req = client.post(url).json(&body).header(
                    "Accept",
                    "application/json, text/event-stream",
                );
                if let Some(sid) = session.lock().ok().and_then(|g| g.clone()) {
                    req = req.header("mcp-session-id", sid);
                }
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = tokio::time::timeout(timeout, req.send())
                    .await
                    .map_err(|_| format!("таймаут {}с HTTP-запроса к {}", timeout.as_secs(), url))?
                    .map_err(|e| format!("HTTP {}: {}", url, e))?;

                // Session id живёт с initialize-ответа и далее.
                if let Some(sid) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
                    if let Ok(mut g) = session.lock() {
                        *g = Some(sid.to_string());
                    }
                }

                let status = resp.status();
                let content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                if !status.is_success() {
                    let body_text = resp.text().await.unwrap_or_default();
                    return Err(format!(
                        "HTTP {}: {}",
                        status,
                        crate::truncate_chars(&body_text, 300)
                    ));
                }

                // Уведомления (без id) обычно получают пустой ответ 202.
                if status.as_u16() == 202 {
                    return Ok(RpcOutcome { response: serde_json::Value::Null });
                }

                if content_type.contains("text/event-stream") {
                    // SSE: читаем события, пока не придёт ответ с нашим id.
                    use futures_util::StreamExt;
                    let mut stream = resp.bytes_stream();
                    let mut buf = String::new();
                    loop {
                        match tokio::time::timeout(timeout, stream.next()).await {
                            Ok(Some(Ok(chunk))) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                            Ok(Some(Err(e))) => return Err(format!("SSE поток: {}", e)),
                            Ok(None) | Err(_) => break,
                        }
                        // Разбираем завершённые блоки (разделитель \n\n).
                        while let Some(pos) = buf.find("\n\n") {
                            // Клонируем блок до drain — иначе borrow conflict
                            // между срезом и мутацией buf.
                            let block_str = buf[..pos].to_string();
                            buf.drain(..pos + 2);
                            let block: Vec<&str> = block_str.lines().collect();
                            if let Some(data) = extract_sse_data(&block) {
                                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                                    if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                                        return Ok(RpcOutcome { response: v });
                                    }
                                }
                            }
                        }
                    }
                    Err("SSE поток закончился без ответа на запрос".to_string())
                } else {
                    let body_text = resp.text().await.map_err(|e| e.to_string())?;
                    let v: Value = serde_json::from_str(body_text.trim())
                        .map_err(|e| format!("не-JSON ответ от {}: {} ({:?})", url, e, crate::truncate_chars(&body_text, 120)))?;
                    Ok(RpcOutcome { response: v })
                }
            }
        }
    }

    /// Отправить уведомление (без id, ответ не ждём).
    async fn notify(&self, body: Value) -> Result<(), String> {
        match self {
            Transport::Stdio { stdin, .. } => {
                use tokio::io::AsyncWriteExt;
                let mut line = body.to_string();
                line.push('\n');
                let mut w = stdin.lock().await;
                w.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
                w.flush().await.map_err(|e| e.to_string())?;
                Ok(())
            }
            Transport::Http { url, client, session, headers } => {
                let mut req = client.post(url).json(&body).header("Accept", "application/json");
                if let Some(sid) = session.lock().ok().and_then(|g| g.clone()) {
                    req = req.header("mcp-session-id", sid);
                }
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                // Ответ (обычно 202) игнорируем — уведомление.
                let _ = req.send().await.map_err(|e| format!("HTTP {}: {}", url, e))?;
                Ok(())
            }
        }
    }
}

/// Запустить фоновый читатель stdout процесса: строки → pending-map.
/// Мусорные строки (баннеры, логи) пропускаем с записью в файловый лог.
async fn spawn_stdio_reader(
    stdout: tokio::process::ChildStdout,
    pending: PendingMap,
    name: String,
) {
    use tokio::io::AsyncBufReadExt;
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Result<Value, _> = serde_json::from_str(trimmed);
        match parsed {
            Ok(v) => {
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    if let Ok(mut map) = pending.lock() {
                        if let Some(tx) = map.remove(&id) {
                            let _ = tx.send(v);
                        }
                    }
                }
                // Уведомления/запросы от сервера пока просто логируются.
            }
            Err(_) => {
                crate::logging_system::warning(
                    &format!("[mcp:{}] не-JSON строка в stdout (пропущена): {:?}", name, crate::truncate_chars(&trimmed, 120)),
                );
            }
        }
    }
    // Поток кончился — сервер умер. Проваливаем всех ожидающих.
    if let Ok(mut map) = pending.lock() {
        for (_, tx) in map.drain() {
            let _ = tx.send(Value::Null);
        }
    }
    crate::logging_system::warning(&format!("[mcp:{}] stdout закрыт — процесс сервера завершился", name));
}
// Пометка "умер" делается вызывающей стороной через слот (см. McpSlot).

// ====================================================== соединение ===

#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct McpConnection {
    pub server_name: String,
    transport: Transport,
    next_id: AtomicU64,
    pub protocol_version: String,
    pub server_info: String,
    /// Атомарный, т.к. соединение живёт под Arc и заполняется уже после
    /// создания (tools/list идёт ПОСЛЕ initialize).
    pub tool_count: AtomicU64,
    /// Соединение умерло (процесс упал / stdout закрылся / транспорт
    /// отвалился) — сигнал для watchdog'а и lazy-retry в прокси.
    pub dead: AtomicU64,
}

impl McpConnection {
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed) == 1
    }
}

impl McpConnection {
    /// Подключиться: транспорт + initialize + notifications/initialized.
    pub async fn connect(cfg: &McpServerConfig) -> Result<Arc<Self>, String> {
        cfg.validate()?;
        let transport = if let Some(cmd) = &cfg.command {
            Self::spawn_stdio(cfg, cmd).await?
        } else if let Some(url) = &cfg.url {
            Transport::Http {
                url: url.clone(),
                client: reqwest::Client::new(),
                session: StdMutex::new(None),
                headers: cfg.headers.clone(),
            }
        } else {
            unreachable!("validate() гарантирует command или url");
        };

        let mut conn = Self {
            server_name: cfg.name.clone(),
            transport,
            next_id: AtomicU64::new(1),
            protocol_version: PROTOCOL_VERSION.to_string(),
            server_info: String::new(),
            tool_count: AtomicU64::new(0),
            dead: AtomicU64::new(0),
        };

        conn.initialize().await?;
        Ok(Arc::new(conn))
    }

    async fn spawn_stdio(cfg: &McpServerConfig, cmd: &str) -> Result<Transport, String> {
        let mut command = tokio::process::Command::new(cmd);
        command
            .args(&cfg.args)
            .envs(&cfg.env)
            .stdin(std::process::Stdio::piped())
            // stderr — в ПАЙП (читаем в фоновой задаче в лог), никогда не
            // inherit: прямая запись в терминал ломает TUI (см. STATUS_RU).
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &cfg.cwd {
            command.current_dir(cwd);
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("не удалось запустить '{} {}': {}", cmd, cfg.args.join(" "), e))?;

        let stdout = child
            .stdout
            .take()
            .ok_or("нет stdout у процесса сервера")?;
        // stderr глотаем в фоне в лог (иначе пайп переполнится и сервер
        // заблокируется на записи).
        if let Some(stderr) = child.stderr.take() {
            let name = cfg.name.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    if !l.trim().is_empty() {
                        crate::logging_system::info(&format!("[mcp:{} stderr] {}", name, l));
                    }
                }
            });
        }

        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
        tokio::spawn(spawn_stdio_reader(stdout, pending.clone(), cfg.name.clone()));

        let stdin = child.stdin.take().ok_or("нет stdin")?;
        Ok(Transport::Stdio {
            child,
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            pending,
        })
    }

    async fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn raw_request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id().await;
        let outcome = self.transport.request(id, rpc_request(id, method, params), timeout).await?;
        unwrap_rpc_response(&outcome.response, &format!("[{}] {}", self.server_name, method))
    }

    async fn initialize(&mut self) -> Result<(), String> {
        let id = self.next_id().await;
        let outcome = self
            .transport
            .request(
                id,
                rpc_request(
                    id,
                    "initialize",
                    serde_json::json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": {},
                        "clientInfo": {"name": "hephaestus", "version": env!("CARGO_PKG_VERSION")},
                    }),
                ),
                INIT_TIMEOUT,
            )
            .await?;

        let result = unwrap_rpc_response(&outcome.response, &format!("[{}] initialize", self.server_name))?;
        if let Some(ver) = result.get("protocolVersion").and_then(|v| v.as_str()) {
            self.protocol_version = ver.to_string();
        }
        self.server_info = result
            .get("serverInfo")
            .map(|si| {
                format!(
                    "{} {}",
                    si.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                    si.get("version").and_then(|v| v.as_str()).unwrap_or("")
                )
                .trim()
                .to_string()
            })
            .unwrap_or_default();

        // Сигнализируем о готовности — после этого можно работать.
        self.transport
            .notify(rpc_notification("notifications/initialized", serde_json::json!({})))
            .await?;
        Ok(())
    }

    /// Список инструментов сервера.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, String> {
        let result = self.raw_request("tools/list", serde_json::json!({}), LIST_TIMEOUT).await?;
        let Some(arr) = result.get("tools").and_then(|t| t.as_array()) else {
            return Ok(Vec::new());
        };
        Ok(arr
            .iter()
            .filter_map(|t| {
                Some(McpToolDef {
                    name: t.get("name")?.as_str()?.to_string(),
                    description: t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                    input_schema: t.get("inputSchema").cloned().unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}})),
                })
            })
            .collect())
    }

    /// Вызов инструмента: content[] склеивается в текст; isError → ошибка.
    pub async fn call_tool(&self, tool: &str, args: Value, timeout: Duration) -> ToolResult {
        match self
            .raw_request(
                "tools/call",
                serde_json::json!({"name": tool, "arguments": args}),
                timeout,
            )
            .await
        {
            Ok(result) => {
                let is_error = result
                    .get("isError")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                let mut texts: Vec<String> = Vec::new();
                if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
                    for item in content {
                        match item.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                                    texts.push(t.to_string());
                                }
                            }
                            Some("image") => texts.push("(изображение в ответе — текстом не передать)".to_string()),
                            Some(other) => texts.push(format!("(content: {})", other)),
                            None => {}
                        }
                    }
                }
                // structuredContent (новые версии протокола) — фолбэк.
                if texts.is_empty() {
                    if let Some(sc) = result.get("structuredContent") {
                        texts.push(sc.to_string());
                    }
                }
                let joined = if texts.is_empty() { "(пустой ответ)".to_string() } else { texts.join("\n") };
                if is_error {
                    ToolResult::error(joined)
                } else {
                    ToolResult::success(joined)
                }
            }
            Err(e) => ToolResult::error(e),
        }
    }
}

// ======================================================== менеджер ===

/// Слот одного сервера: конфиг + живое соединение + занятые имена в
/// реестре. Существование слота позволяет ПЕРЕПОДКЛЮЧАТЬ сервер на лету:
/// watchdog замечает мёртвое соединение и поднимает его заново с теми же
/// именами инструментов; прокси-инструменты при ошибке пробуют оживить
/// слот и повторить вызов один раз.
pub struct McpSlot {
    pub cfg: McpServerConfig,
    conn: StdMutex<Option<Arc<McpConnection>>>,
    names: StdMutex<Vec<String>>,
    /// Реестр для перерегистрации инструментов при reconnect (заполняет
    /// менеджер в момент создания слота).
    registry: std::sync::OnceLock<SharedToolRegistry>,
    timeout_ms: u64,
}

impl McpSlot {
    pub fn name(&self) -> &str {
        &self.cfg.name
    }

    pub fn get_conn(&self) -> Option<Arc<McpConnection>> {
        self.conn.lock().ok().and_then(|g| g.clone()).filter(|c| !c.is_dead())
    }

    fn set_conn(&self, c: Option<Arc<McpConnection>>) {
        if let Ok(mut g) = self.conn.lock() {
            *g = c;
        }
    }

    pub fn mark_dead(&self) {
        if let Some(c) = self.conn.lock().ok().and_then(|g| g.clone()) {
            c.dead.store(1, Ordering::Relaxed);
        }
    }

    fn clear_tools(&self) {
        let reg = match self.registry.get() {
            Some(r) => r,
            None => return,
        };
        if let Ok(names) = self.names.lock() {
            for n in names.iter() {
                reg.remove(n);
            }
        }
        if let Ok(mut n) = self.names.lock() {
            n.clear();
        }
    }

    /// Подключиться заново и зарегистрировать инструменты. Возвращает
    /// число инструментов или ошибку.
    async fn revive(self: &Arc<Self>) -> Result<usize, String> {
        self.clear_tools();
        self.set_conn(None);
        let conn = McpConnection::connect(&self.cfg).await?;
        let defs = conn.list_tools().await?;
        let count = defs.len();
        let reg = self.registry.get().ok_or("реестр недоступен")?;
        for def in &defs {
            let proxy = McpProxyTool::new(Arc::clone(self), &def, self.timeout_ms);
            let n = proxy.registry_name();
            reg.register(n, Arc::new(proxy));
            if let Ok(mut list) = self.names.lock() {
                list.push(n.to_string());
            }
        }
        conn.tool_count.store(count as u64, Ordering::Relaxed);
        conn.dead.store(0, Ordering::Relaxed);
        self.set_conn(Some(conn));
        Ok(count)
    }
}

/// Прокси держит СЛОТ, а не соединение: после reconnect вызовы продолжают
/// попадать в тот же инструмент без пересоздания реестра.
pub struct McpProxyTool {
    slot: Arc<McpSlot>,
    mcp_tool: String,
    static_name: &'static str,
    static_description: &'static str,
    schema: Value,
    timeout_ms: u64,
}

impl McpProxyTool {
    pub fn new(slot: Arc<McpSlot>, def: &McpToolDef, timeout_ms: u64) -> Self {
        let reg_name =
            sanitize_registry_name(&format!("mcp_{}_{}", slot.cfg.name, def.name));
        let description = format!("[MCP:{}] {}", slot.cfg.name, def.description);
        Self {
            slot,
            mcp_tool: def.name.clone(),
            static_name: Box::leak(reg_name.into_boxed_str()),
            static_description: Box::leak(description.into_boxed_str()),
            schema: def.input_schema.clone(),
            timeout_ms,
        }
    }

    pub fn registry_name(&self) -> &'static str {
        self.static_name
    }
}

/// Имя инструмента реестра: [a-z0-9_], остальное — в '_'; подряд '_' —
/// в один. Некоторые серверы называют инструменты с точками/дефисами.
pub fn sanitize_registry_name(raw: &str) -> String {
    let lowered = raw.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut prev_us = false;
    for c in lowered.chars() {
        let ok = c.is_ascii_alphanumeric() || c == '_';
        if ok {
            out.push(c);
            prev_us = c == '_';
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_matches('_').to_string()
}

#[async_trait::async_trait]
impl Tool for McpProxyTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        // LAZY RECONNECT: если соединение мертво — одна попытка оживить
        // прямо перед вызовом, чтобы пользователь не ждал минуту вотчера.
        if self.slot.get_conn().is_none() {
            crate::logging_system::info(&format!(
                "[mcp:{}] соединение мертво — переподключаюсь перед вызовом {}",
                self.slot.name(), self.mcp_tool
            ));
            let _ = self.slot.revive().await;
        }
        let Some(conn) = self.slot.get_conn() else {
            return ToolResult::error(format!("MCP-сервер '{}' недоступен (не удалось переподключиться)", self.slot.name()));
        };
        let res = conn.call_tool(&self.mcp_tool, args.clone(), Duration::from_millis(self.timeout_ms)).await;
        // Транспортная смерть посреди вызова — помечаем; watchdog/следующий
        // вызов поднимут сервер.
        if res.error.as_deref().map(|e| {
            e.contains("закрыт") || e.contains("сервер закрылся") || e.contains("таймаут")
        }).unwrap_or(false) {
            conn.dead.store(1, Ordering::Relaxed);
        }
        res
    }

    fn name(&self) -> &'static str {
        self.static_name
    }

    fn description(&self) -> &'static str {
        self.static_description
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }
}

// ======================================================== менеджер ===

/// Владеет слотами всех серверов; /mcp reload пересобирает набор целиком.
#[derive(Default)]
pub struct McpManager {
    slots: StdMutex<Vec<Arc<McpSlot>>>,
    watchdog_started: AtomicBool,
}

pub struct ServerReport {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl McpManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status_report(&self) -> String {
        let slots = self.slots.lock().map(|s| s.clone()).unwrap_or_default();
        if slots.is_empty() {
            return "MCP-серверы не подключены (конфиг: ~/.hephaestus/mcp.toml, применить: /mcp reload)".to_string();
        }
        let mut lines = vec![format!("MCP-серверов: {}", slots.len())];
        for s in slots {
            match s.get_conn() {
                Some(c) => lines.push(format!(
                    "  • ✅ {} — {}, протокол {}, инструментов: {}",
                    s.name(),
                    if c.server_info.is_empty() { "?" } else { &c.server_info },
                    c.protocol_version,
                    c.tool_count.load(Ordering::Relaxed)
                )),
                None => lines.push(format!("  • ⚠️ {} — НЕ ПОДКЛЮЧЁН (watchdog попробует поднять)", s.name())),
            }
        }
        lines.join("\n")
    }

    /// Подключить один сервер и зарегистрировать его инструменты через слот.
    pub async fn connect_and_register(
        &self,
        cfg: &McpServerConfig,
        registry: &SharedToolRegistry,
    ) -> ServerReport {
        let timeout_ms = cfg.timeout_ms.unwrap_or(DEFAULT_CALL_TIMEOUT_MS);
        let slot = Arc::new(McpSlot {
            cfg: cfg.clone(),
            conn: StdMutex::new(None),
            names: StdMutex::new(Vec::new()),
            registry: std::sync::OnceLock::new(),
            timeout_ms,
        });
        let _ = slot.registry.set(registry.clone());

        match slot.revive().await {
            Ok(count) => {
                if let Ok(mut s) = self.slots.lock() {
                    s.retain(|x| x.name() != cfg.name); // перезапись дублей
                    s.push(slot);
                }
                ServerReport { name: cfg.name.clone(), ok: true, detail: format!("{count} инструмент(ов)") }
            }
            Err(e) => {
                // Слёт сохраняем со статусом "вниз" — watchdog будет пытаться.
                if let Ok(mut s) = self.slots.lock() {
                    s.retain(|x| x.name() != cfg.name);
                    s.push(slot);
                }
                ServerReport { name: cfg.name.clone(), ok: false, detail: e }
            }
        }
    }

    /// Фоновый watchdog: раз в минуту поднимает упавшие серверы.
    /// Идемпотентен (второй вызов не создаёт второй цикл).
    pub fn spawn_watchdog(self: &Arc<Self>, _registry: SharedToolRegistry) {
        if self.watchdog_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                // WATCHDOG: этот тикер — главный источник heartbeat в
                // headless-режимах (без TUI), работает всегда.
                crate::watchdog::bump("mcp-watchdog-tick");
                let slots = this.slots.lock().map(|s| s.clone()).unwrap_or_default();
                for slot in slots {
                    if slot.get_conn().is_some() {
                        continue;
                    }
                    crate::logging_system::info(&format!(
                        "[mcp:{}] watchdog: пытаюсь переподключить сервер", slot.name()
                    ));
                    match slot.revive().await {
                        Ok(n) => crate::logging_system::info(&format!(
                            "[mcp:{}] восстановлен, инструментов: {n}", slot.name())),
                        Err(e) => crate::logging_system::warning(&format!(
                            "[mcp:{}] reconnect не удался: {e}", slot.name())),
                    }
                }
            }
        });
    }

    /// Снять все mcp_* инструменты и забыть слоты (/mcp reload).
    pub fn unload_all(&self, _registry: &SharedToolRegistry) {
        if let Ok(slots) = self.slots.lock() {
            for s in slots.iter() {
                s.clear_tools();
            }
        }
        if let Ok(mut s) = self.slots.lock() {
            s.clear();
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_parse_stdio_and_http() {
        // Корень конфига — таблица { servers = [...] }, парсим так же,
        // как это делает load_config().
        #[derive(Deserialize)]
        struct Root {
            #[serde(default)]
            servers: Vec<McpServerConfig>,
        }
        let root: Root = toml::from_str(
            r#"
[[servers]]
name = "blender"
command = "uvx"
args = ["blender-mcp"]

[[servers]]
name = "remote"
url = "http://127.0.0.1:8000/mcp"
headers = { Authorization = "Bearer tok" }
timeout_ms = 300000
enabled = false
"#,
        )
        .unwrap();
        let cfg = root.servers;

        assert_eq!(cfg.len(), 2);
        assert_eq!(cfg[0].command.as_deref(), Some("uvx"));
        assert_eq!(cfg[0].args, vec!["blender-mcp"]);
        assert!(cfg[0].enabled);

        assert_eq!(cfg[1].url.as_deref(), Some("http://127.0.0.1:8000/mcp"));
        assert_eq!(cfg[1].headers.get("Authorization").map(|s| s.as_str()), Some("Bearer tok"));
        assert_eq!(cfg[1].timeout_ms, Some(300000));
        assert!(!cfg[1].enabled);
    }

    #[test]
    fn test_validate_requires_exactly_one_transport() {
        let mut s = McpServerConfig { name: "x".into(), ..Default::default() };
        assert!(s.validate().is_err()); // ничего не задано
        s.command = Some("uvx".into());
        assert!(s.validate().is_ok());
        s.url = Some("http://x/mcp".into());
        assert!(s.validate().is_err()); // оба сразу
        s.command = None;
        assert!(s.validate().is_ok());
    }

    #[test]
    fn test_sanitize_registry_name() {
        assert_eq!(sanitize_registry_name("mcp_blender_get_scene_info"), "mcp_blender_get_scene_info");
        assert_eq!(sanitize_registry_name("MCP-Server.Get.Scene"), "mcp_server_get_scene");
        assert_eq!(sanitize_registry_name("a--b..c"), "a_b_c");
    }

    #[test]
    fn test_rpc_envelopes_and_unwrap() {
        let req = rpc_request(7, "tools/list", serde_json::json!({}));
        assert_eq!(req["id"], 7);
        assert_eq!(req["method"], "tools/list");

        let notif = rpc_notification("notifications/initialized", serde_json::json!({}));
        assert!(notif.get("id").is_none());

        let ok = unwrap_rpc_response(&serde_json::json!({"result": {"tools": []}}), "t");
        assert!(ok.is_ok());

        let err = unwrap_rpc_response(
            &serde_json::json!({"error": {"code": -32601, "message": "no such tool"}}),
            "tools/call",
        );
        assert!(err.unwrap_err().contains("-32601"));
    }

    #[test]
    fn test_sse_data_extraction() {
        let block = vec!["event: message", "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{}}", ""];
        let data = extract_sse_data(&block).unwrap();
        let v: Value = serde_json::from_str(&data).unwrap();
        assert_eq!(v["id"], 3);
    }

    #[tokio::test]
    async fn test_stdio_roundtrip_against_fake_server() {
        // Полноценный интеграционный тест протокола на «настоящем»
        // MCP-сервере — это /bin/cat с препроцессором: echo'ит наши
        // запросы обратно (id сохраняется), что достаточно для
        // initialize/tools/list механики транспорта.
        if which_exists("cat") {
            let cfg = McpServerConfig {
                name: "echo".into(),
                command: Some("/bin/cat".into()),
                ..Default::default()
            };
            // cat вернёт наш initialize как result-подобный объект? Нет:
            // он вернёт сам запрос, у которого нет "result" → initialize
            // завершится ошибкой unwrap. Проверяем именно ТРАНСПОРТ:
            // ответ приходит и распознаётся как JSON-RPC error.
            match McpConnection::connect(&cfg).await {
                Err(e) => assert!(e.contains("[echo] initialize"), "неожиданная ошибка: {}", e),
                Ok(_) => panic!("cat не должен проходить initialize"),
            }
        }
    }

    fn which_exists(bin: &str) -> bool {
        std::path::Path::new("/bin").join(bin).exists()
            || std::path::Path::new("/usr/bin").join(bin).exists()
    }

    /// ПОЛНЫЙ протокольный тест: настоящий дочерний процесс-«сервер» на
    /// python3, говорящий по MCP поверх stdio (initialize → initialized →
    /// tools/list → tools/call). Это проверяет транспорт, рукопожатие,
    /// разбор ответов и маппинг content[] — то есть всё, кроме HTTP.
    #[tokio::test]
    async fn test_full_protocol_over_stdio_with_python_server() {
        let Ok(python) = std::process::Command::new("python3")
            .arg("--version")
            .output()
        else {
            return; // нет python3 — тест неприменим
        };
        if !python.status.success() {
            return;
        }

        let dir = tempfile::TempDir::new().unwrap();
        let script = dir.path().join("fake_mcp.py");
        std::fs::write(&script, r#"
import sys, json

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    if "id" not in msg:
        continue  # уведомление (notifications/initialized) — ответа не требует
    mid = msg["id"]
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake", "version": "0.1"},
        }})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": [{
            "name": "echo",
            "description": "Эхо-тест",
            "inputSchema": {"type": "object",
                            "properties": {"text": {"type": "string"}},
                            "required": ["text"]},
        }]}})
    elif method == "tools/call":
        text = msg["params"]["arguments"]["text"]
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "content": [{"type": "text", "text": f"echo: {text}"}],
            "isError": False,
        }})
"#).unwrap();

        let cfg = McpServerConfig {
            name: "fake".into(),
            command: Some("python3".into()),
            args: vec![script.display().to_string()],
            ..Default::default()
        };

        // 1. Подключение + initialize.
        let conn = McpConnection::connect(&cfg).await.expect("connect failed");
        assert_eq!(conn.protocol_version, "2024-11-05");
        assert!(conn.server_info.contains("fake"));

        // 2. tools/list.
        let tools = conn.list_tools().await.expect("tools/list failed");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");

        // 3. tools/call — happy path и ошибка инструмента.
        let res = conn.call_tool("echo", serde_json::json!({"text": "привет"}), Duration::from_secs(5)).await;
        assert!(res.success, "{:?}", res.error);
        assert_eq!(res.output, "echo: привет");

        // 4. Прокси-инструмент поверх слота (без реестра — revive вернёт
        // ошибку, но прямой вызов работает): имя санитизируется.
        let slot = Arc::new(McpSlot {
            cfg: cfg.clone(),
            conn: StdMutex::new(Some(conn.clone())),
            names: StdMutex::new(Vec::new()),
            registry: std::sync::OnceLock::new(),
            timeout_ms: 5000,
        });
        let proxy = McpProxyTool::new(slot, &tools[0], 5000);
        assert_eq!(proxy.registry_name(), "mcp_fake_echo");
        let via_proxy = proxy.execute(&serde_json::json!({"text": "proxy"})).await;
        assert!(via_proxy.success);
        assert_eq!(via_proxy.output, "echo: proxy");

        // Соединение роняем вместе с TempDir — child убьётся kill_on_drop.
        drop(conn);
    }
}
