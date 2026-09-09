//! Telegram Bot — канал удалённого управления Гефестом.
//!
//! ВЕРСИЯ ПЕРЕПИСАНА С НУЛЯ (жалоба "бот не работает"). Разбор показал,
//! что на машине ОДНОВРЕМЕННО крутились старый Python-Гефест и Rust —
//! оба опрашивали getUpdates одного токена, Telegram отдаёт апдейты
//! только одному из них, поэтому сообщения уходили ПИТОНУ со его
//! старой логикой и лимитами. Уроки вшиты сюда:
//!
//!   • одиночный поллер в процессе (AtomicBool) — повторный запуск
//!     не создаёт второго конкурента;
//!   • проверка токена через getMe ДО цикла — невалидный токен виден
//!     сразу в логе, а не как бесконечное молчание;
//!   • явная диагностика HTTP 409 ("кто-то другой уже опрашивает") —
//!     ровно та ситуация с двумя агентами, теперь видно в логах сразу;
//!   • все сообщения ходят через общую с TUI очередь запросов
//!     (request_queue.rs): можно слать несколько задач подряд;
//!   • пока идёт ход — sendChatAction "typing", чтобы чат не выглядел
//!     мёртвым;
//!   • команды: /start /help /status /reset /workdir (+yes/no),
//!     ответы на запросы разрешений: 1 / 2 / 3.
//!
//! Запуск: `--telegram` отдельным процессом или `/telegram start` из REPL
//! (токен: ~/.hephaestus/telegram_token.txt, затем TELEGRAM_BOT_TOKEN).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::path::PathBuf;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::Agent;

const API_BASE: &str = "https://api.telegram.org/bot";
/// Защита от двух поллеров внутри одного процесса.
static BOT_POLLING: AtomicBool = AtomicBool::new(false);

fn token_file_path() -> PathBuf {
    let mut p = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push(".hephaestus");
    p.push("telegram_token.txt");
    p
}

pub fn save_token(token: &str) -> Result<(), String> {
    let path = token_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, token).map_err(|e| e.to_string())
}

pub fn load_token() -> Option<String> {
    if let Ok(content) = std::fs::read_to_string(token_file_path()) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    std::env::var("TELEGRAM_BOT_TOKEN").ok().filter(|v| !v.is_empty())
}

pub struct TelegramBot {
    token: String,
    client: reqwest::Client,
    agent: Arc<Mutex<Agent>>,
    allowed_chat_id: Option<i64>,
    /// Кандидаты /workdir по чатам, ожидающие yes/no.
    pending_workdir: Mutex<std::collections::HashMap<i64, PathBuf>>,
    /// Последний писавший чат — куда слать запросы разрешений без whitelist.
    last_chat_id: Arc<std::sync::Mutex<Option<i64>>>,
}

impl TelegramBot {
    pub fn new(token: String, agent: Arc<Mutex<Agent>>, allowed_chat_id: Option<i64>) -> Self {
        Self {
            token,
            client: reqwest::Client::new(),
            agent,
            allowed_chat_id,
            pending_workdir: Mutex::new(std::collections::HashMap::new()),
            last_chat_id: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}{}/{}", API_BASE, self.token, method)
    }

    /// Сырой вызов метода API; Err — сеть/HTTP, тело возвращается как Value.
    async fn call(&self, method: &str, payload: Option<Value>) -> Result<Value, String> {
        let mut req = match payload {
            Some(p) => self.client.post(self.api_url(method)).json(&p),
            None => self.client.get(self.api_url(method)),
        };
        if method == "getUpdates" {
            req = req.timeout(std::time::Duration::from_secs(35));
        } else {
            req = req.timeout(std::time::Duration::from_secs(15));
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| format!("не-JSON ответ {}: {}", method, e))?;
        if !status.is_success() {
            // Описание ошибки Telegram (например, 401 Unauthorized при
            // неверном токене) — наружу, а не в бездну.
            let desc = body.get("description").and_then(|d| d.as_str()).unwrap_or("");
            return Err(format!("HTTP {} {}: {}", status.as_u16(), method, desc));
        }
        Ok(body)
    }

    async fn send_message(&self, chat_id: i64, text: &str) {
        for chunk in split_for_telegram(text, 3500) {
            let payload = serde_json::json!({ "chat_id": chat_id, "text": chunk });
            if let Err(e) = self.call("sendMessage", Some(payload)).await {
                crate::logging_system::warning(&format!("[telegram] sendMessage: {}", e));
            }
        }
    }

    /// "печатает..." во время обработки хода.
    fn spawn_typing_loop(&self, chat_id: i64, mut done: tokio::sync::oneshot::Receiver<()>) {
        let client = self.client.clone();
        let base = self.api_url("");
        tokio::spawn(async move {
            loop {
                let _ = client
                    .post(format!("{}sendChatAction", base))
                    .json(&serde_json::json!({"chat_id": chat_id, "action": "typing"}))
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await;
                // typing живёт ~5с — обновляем чуть раньше; выход — как
                // только канал хода закрылся (ответ отправлен).
                match tokio::time::timeout(std::time::Duration::from_secs(4), &mut done).await {
                    Ok(_) => return,
                    Err(_) => continue,
                }
            }
        });
    }

    /// Основной цикл long polling.
    pub async fn run(&self) {
        if BOT_POLLING.swap(true, Ordering::SeqCst) {
            crate::logging_system::warning("[telegram] бот уже запущен в этом процессе — второй поллер не нужен");
            return;
        }

        // 1. Проверка токена ДО цикла: невалидный токен = сразу понятная
        // ошибка в логе, а не вечное молчание.
        match self.call("getMe", None).await {
            Ok(me) => {
                let name = me.pointer("/result/username").and_then(|u| u.as_str()).unwrap_or("?");
                crate::logging_system::info(&format!("[telegram] подключён как @{}", name));
            }
            Err(e) => {
                crate::logging_system::warning(&format!(
                    "[telegram] ТОКЕН НЕ ПРИНЯТ: {}. Бот остановлен. Проверьте ~/.hephaestus/telegram_token.txt",
                    e
                ));
                BOT_POLLING.store(false, Ordering::SeqCst);
                return;
            }
        }

        // Клоны разделяемых полей агента — работа без лока агента.
        let (permissions, queue) = {
            let a = self.agent.lock().await;
            (a.permissions.clone(), a.queue.clone())
        };

        // Вотчер запросов разрешений → сообщение в последний активный чат.
        {
            let perms = permissions.clone();
            let client = self.client.clone();
            let token = self.token.clone();
            let allowed_chat_id = self.allowed_chat_id;
            let last_chat = self.last_chat_id.clone();
            tokio::spawn(async move {
                let mut last_shown: Option<u64> = None;
                loop {
                    if let Some(p) = perms.current_pending() {
                        if last_shown != Some(p.id) {
                            let target = allowed_chat_id.or_else(|| last_chat.lock().ok().and_then(|g| *g));
                            if let Some(chat_id) = target {
                                let text = format!(
                                    "⚠️ Требуется разрешение — {}\n{}\n{}\n\n1 — разрешить один раз\n2 — разрешить всегда для '{}'\n3 — запретить",
                                    p.tool, p.summary, p.reason, p.always_key
                                );
                                send_message_raw(&client, &token, chat_id, &text).await;
                            }
                            last_shown = Some(p.id);
                        }
                    } else {
                        last_shown = None;
                    }
                    // WATCHDOG: poll-цикл бота — тикер живости.
                    crate::watchdog::bump("telegram-permission-poll");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            });
        }

        let mut offset: i64 = 0;
        let mut error_streak: u32 = 0;

        loop {
            // WATCHDOG: getUpdates возвращает каждые ~25с (long poll) —
            // основной тикер живости при включённом боте.
            crate::watchdog::bump("telegram-getupdates");
            let body = match self
                .call(
                    "getUpdates",
                    Some(serde_json::json!({
                        "timeout": 25,
                        "offset": offset,
                        "allowed_updates": ["message"],
                    })),
                )
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    // Особый случай: 409 = другой процесс уже опрашивает
                    // этот токен (наш кейс "два Гефеста"). Говорим прямо.
                    if e.contains("409") {
                        crate::logging_system::warning(
                            "[telegram] КОНФЛИКТ 409: этот бот уже опрашивается ДРУГИМ ПРОЦЕССОМ (старый агент?). Остановите его — иначе сообщения уходят туда.",
                        );
                    } else {
                        crate::logging_system::warning(&format!("[telegram] getUpdates: {}", e));
                    }
                    error_streak += 1;
                    let backoff = std::time::Duration::from_secs((5 * error_streak.min(6)) as u64);
                    tokio::time::sleep(backoff).await;
                    continue;
                }
            };
            error_streak = 0;

            let Some(updates) = body.get("result").and_then(|v| v.as_array()) else {
                continue;
            };

            for update in updates {
                if let Some(update_id) = update.get("update_id").and_then(|v| v.as_i64()) {
                    offset = update_id + 1;
                }
                let Some(message) = update.get("message") else { continue };
                let Some(chat_id) = message.pointer("/chat/id").and_then(|v| v.as_i64()) else { continue };
                let Some(text) = message.get("text").and_then(|v| v.as_str()) else { continue };

                // Запоминаем, откуда пишут (для вотчера разрешений).
                if let Ok(mut g) = self.last_chat_id.lock() {
                    *g = Some(chat_id);
                }

                if let Some(allowed) = self.allowed_chat_id {
                    if chat_id != allowed {
                        self.send_message(chat_id, "Доступ ограничён — этот бот привязан к другому пользователю.").await;
                        continue;
                    }
                }

                let t = text.trim();
                crate::logging_system::info(&format!("[telegram:<{}>] {}", chat_id, t));

                // ---------- команды ----------
                let lower = t.to_lowercase();
                if lower == "/start" || lower == "/help" {
                    self.send_message(chat_id, HELP_TEXT).await;
                    continue;
                }
                if t == "/reset" {
                    self.agent.lock().await.reset().await;
                    self.send_message(chat_id, "История диалога очищена.").await;
                    continue;
                }
                if lower == "/status" {
                    let a = self.agent.lock().await;
                    let (p, m) = a.current_provider_and_model();
                    let msg = format!(
                        "Провайдер: {}/{}\nРабочая директория: {}\nВ очереди ходов: {}\n\n{}",
                        p,
                        m,
                        a.work_dir().display(),
                        queue.len().await,
                        a.mcp.status_report()
                    );
                    drop(a);
                    self.send_message(chat_id, &msg).await;
                    continue;
                }
                if self.handle_workdir_command(chat_id, t).await {
                    continue;
                }
                let normalized = lower.as_str();
                if matches!(normalized, "yes" | "да" | "no" | "нет") {
                    if self.handle_workdir_answer(chat_id, normalized).await {
                        continue;
                    }
                }
                if matches!(t, "1" | "2" | "3") {
                    let choice = match t {
                        "1" => crate::permissions::Choice::Once,
                        "2" => crate::permissions::Choice::Always,
                        _ => crate::permissions::Choice::Deny,
                    };
                    let resolved = permissions.resolve(choice);
                    let reply = if !resolved {
                        "Нет ожидающих запросов разрешения."
                    } else {
                        match choice {
                            crate::permissions::Choice::Once => "✅ Разрешено (один раз).",
                            crate::permissions::Choice::Always => "✅ Разрешено всегда для этого ключа.",
                            crate::permissions::Choice::Deny => "🚫 Запрещено.",
                        }
                    };
                    self.send_message(chat_id, reply).await;
                    continue;
                }

                // ---------- обычное сообщение → очередь ----------
                let (queue_pos, done_rx) = queue
                    .push(crate::request_queue::QueuedTurn {
                        text: t.to_string(),
                        source: format!("telegram:{}", chat_id),
                    })
                    .await;
                if queue_pos > 0 {
                    self.send_message(chat_id, &format!("📥 В очереди — позиция {}. Ответ придёт автоматически.", queue_pos + 1)).await;
                }

                let bot_client = self.client.clone();
                let token = self.token.clone();
                let agent_for_suggest = self.agent.clone();
                let (done_tx, done_signal) = tokio::sync::oneshot::channel::<()>();
                self.spawn_typing_loop(chat_id, done_signal);

                tokio::spawn(async move {
                    let reply = match tokio::time::timeout(std::time::Duration::from_secs(960), done_rx).await {
                        Ok(Ok(text)) => text,
                        Ok(Err(_)) => "Ход отменён.".to_string(),
                        Err(_) => "⏱️ Превышено время ожидания ответа (16 минут).".to_string(),
                    };
                    let _ = done_tx.send(());

                    // КРАСИВЫЕ ОШИБКИ в чате: неудачный ход оформляется
                    // заголовком и обрезанным дампом, а не простынёй
                    // сырого текста (жалоба "криво как-то").
                    let is_error = {
                        let t = reply.trim_start();
                        t.starts_with("LLM error")
                            || t.starts_with("⏱️")
                            || t.starts_with("🚫")
                            || t.starts_with("❌")
                            || t.starts_with("🌐")
                            || t.starts_with("⏳")
                            || t.starts_with("🔒")
                            || t.starts_with("⚙️")
                    };
                    let out = if is_error {
                        let mut body: String = reply.chars().take(1200).collect();
                        if reply.chars().count() > 1200 {
                            body.push_str("\n\n…полный текст — в логах (~/.hephaestus/logs/)");
                        }
                        format!("⚠️ Не получилось\n\n{}", body.trim())
                    } else {
                        reply
                    };

                    for chunk in split_for_telegram(&out, 3500) {
                        let payload = serde_json::json!({ "chat_id": chat_id, "text": chunk });
                        let url = format!("{}{}/sendMessage", API_BASE, token);
                        let _ = bot_client.post(&url).json(&payload).send().await;
                    }

                    // 💡 Подсказки следующих действий (prompt-suggestion):
                    // только для неошибочных ответов; сбои молча глотаются.
                    if !is_error {
                        if let Some(sug) = agent_for_suggest.lock().await.suggest_next().await {
                            send_message_raw(&bot_client, &token, chat_id, &format!("💡 Дальше:\n{sug}")).await;
                        }
                    }
                });
            }
        }
    }

    // ---------------- /workdir ----------------

    async fn handle_workdir_command(&self, chat_id: i64, text: &str) -> bool {
        let is_bare = text == "/workdir" || text == "/work_dir";
        let arg = text
            .strip_prefix("/workdir ")
            .or_else(|| text.strip_prefix("/work_dir "))
            .map(str::trim)
            .unwrap_or("");
        if !is_bare && arg.is_empty() {
            return false;
        }

        if is_bare {
            let cur = match self.agent.try_lock() {
                Ok(a) => a.work_dir().display().to_string(),
                Err(_) => "(агент занят)".to_string(),
            };
            let pending_note = match self.pending_workdir.lock().await.get(&chat_id) {
                Some(p) => format!("\nОжидает подтверждения: {} (ответьте yes или no)", p.display()),
                None => String::new(),
            };
            self.send_message(
                chat_id,
                &format!(
                    "Рабочая директория инструментов: {}\nСменить: /workdir <путь>\nНапример: /workdir ~/projects/myapp{}",
                    cur, pending_note
                ),
            )
            .await;
            return true;
        }

        match crate::workdir::validate_candidate(arg.trim_matches('"')) {
            Ok(path) => {
                let file_count = std::fs::read_dir(&path).map(|d| d.count()).unwrap_or(0);
                self.pending_workdir.lock().await.insert(chat_id, path.clone());
                self.send_message(
                    chat_id,
                    &format!(
                        "Новая рабочая директория: {}\n(содержимое: {} записей)\n\n⚠️ Вы доверяете этой директории? Агент получит доступ на чтение и запись файлов в ней.\n\nОтветьте yes — применить и сохранить, no — отменить.",
                        path.display(),
                        file_count
                    ),
                )
                .await;
            }
            Err(e) => self.send_message(chat_id, &format!("❌ {}", e)).await,
        }
        true
    }

    async fn handle_workdir_answer(&self, chat_id: i64, answer: &str) -> bool {
        let pending = self.pending_workdir.lock().await.remove(&chat_id);
        let Some(path) = pending else { return false };
        if answer == "yes" || answer == "да" {
            {
                let a = self.agent.lock().await;
                a.set_work_dir(path.clone());
            }
            match crate::config::AgentConfig::set_saved_work_dir(&path) {
                Ok(_) => {
                    self.send_message(
                        chat_id,
                        &format!(
                            "✅ Рабочая директория: {}\nСохранено. Все инструменты теперь работают здесь.",
                            path.display()
                        ),
                    )
                    .await;
                }
                Err(e) => {
                    self.send_message(chat_id, &format!("Применена, но НЕ сохранена: {}", e)).await;
                }
            }
        } else {
            self.send_message(chat_id, "Смена рабочей директории отменена.").await;
        }
        true
    }
}

const HELP_TEXT: &str = "⚡ Гефест — удалённый агент.

Просто пишите задачи текстом — выполняю на машине (файлы, bash, git...). Можно слать несколько сообщений подряд — встанут в очередь.

Команды:
/status — провайдер, директория, очередь
/workdir <путь> — сменить рабочую директорию (спрошу доверие)
/reset — очистить историю диалога

Опасные действия спрашивают подтверждение: отвечайте 1 (один раз), 2 (всегда), 3 (запретить).";

fn split_for_telegram(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() {
        return vec!["(пустой ответ)".to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(max_chars).map(|c| c.iter().collect()).collect()
}

async fn send_message_raw(client: &reqwest::Client, token: &str, chat_id: i64, text: &str) {
    let url = format!("{}{}/sendMessage", API_BASE, token);
    for chunk in split_for_telegram(text, 3500) {
        let payload = serde_json::json!({ "chat_id": chat_id, "text": chunk });
        let _ = client.post(&url).json(&payload).send().await;
    }
}

/// Запуск отдельным процессом (--telegram).
pub async fn run_from_env(agent: Agent) {
    let token = match load_token() {
        Some(t) => t,
        None => {
            eprintln!(
                "❌ Токен не найден ни в {}, ни в TELEGRAM_BOT_TOKEN.\nСохранить: /telegram save <токен> из REPL.",
                token_file_path().display()
            );
            return;
        }
    };
    let allowed_chat_id = std::env::var("TELEGRAM_ALLOWED_CHAT_ID")
        .ok()
        .and_then(|v| v.parse::<i64>().ok());

    let agent = Arc::new(Mutex::new(agent));
    Agent::spawn_queue_worker(&agent).await;
    let bot = TelegramBot::new(token, agent, allowed_chat_id);
    bot.run().await;
}
