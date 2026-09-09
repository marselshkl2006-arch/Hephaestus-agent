//! Permissions — система подтверждения опасных действий в духе opencode.
//!
//! Раньше опасная bash-команда просто ОТКЛОНЯЛАСЬ с текстом "повторите
//! вызов с force: true" — модель зацикливалась, пользователь не понимал,
//! что происходит, а флаг force приходилось угадывать. Теперь опасное
//! действие ставит ЗАПРОС на разрешение, и человек выбирает:
//!
//!   • разрешить ОДИН раз (Once)
//!   • разрешить ВСЕГДА для этого ключа (Always — например "всегда для npm")
//!   • запретить (Deny)
//!
//! Как это работает без блокировки интерфейсов:
//!   • Режим Terminal (--simple/--voice) — обычный вопрос y/a/n через
//!     stdin (TUI там нет, терминал свободен).
//!   • Режим Queue (TUI и Telegram) — запрос кладётся в очередь, а
//!     инструмент АСИНХРОННО ждёт ответа. Пока он ждёт:
//!       - остальные инструменты того же хода продолжают выполняться
//!         (Agent::chat гоняет их конкурентно через join_all);
//!       - TUI опрашивает очередь каждый тик (~120мс) и показывает
//!         вопрос + команды /allow once|always, /deny;
//!       - Telegram-бот показывает вопрос в чат, ответ — «1»/«2»/«3».
//!     Ответ не пришёл за WAIT_TIMEOUT → действие отклонено (fail-safe),
//!     диалог агента получает внятную ошибку вместо зависания.
//!
//! Ключ "всегда разрешать" — первый токен команды для bash ("npm", "sudo",
//! "apt"), имя инструмента для остальных. Разрешения живут в течение
//! сессии; катастрофические паттерны security.rs (rm -rf /, fork bomb)
//! НЕ спрашиваются никогда — они заблокированы всегда.

use std::collections::HashSet;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio::sync::oneshot;

/// Сколько ждать ответа человека, прежде чем отклонить действие.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// Спрашивать блокирующе через stdin (--simple, --voice).
    Terminal,
    /// Класть запрос в очередь — ответ приходит из TUI или Telegram.
    Queue,
}

/// Вариант ответа пользователя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Once,
    Always,
    Deny,
}

/// Что получил ожидающий инструмент.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Granted,
    Denied,
    Expired,
}

/// Снимок ожидающего запроса — для отображения в TUI/Telegram.
#[derive(Debug, Clone)]
pub struct PendingRequest {
    pub id: u64,
    pub tool: String,
    pub summary: String,
    pub reason: String,
    pub always_key: String,
}

struct QueuedRequest {
    pending: PendingRequest,
    tx: oneshot::Sender<Outcome>,
}

struct Inner {
    next_id: u64,
    queue: std::collections::VecDeque<QueuedRequest>,
}

pub struct PermissionManager {
    mode: StdMutex<PermissionMode>,
    inner: StdMutex<Inner>,
    /// Ключи, на которые ответили "всегда разрешать" (в течение сессии).
    always_allowed: StdMutex<HashSet<String>>,
}

impl PermissionManager {
    pub fn new() -> Self {
        Self {
            mode: StdMutex::new(PermissionMode::Queue),
            inner: StdMutex::new(Inner {
                next_id: 1,
                queue: std::collections::VecDeque::new(),
            }),
            always_allowed: StdMutex::new(HashSet::new()),
        }
    }

    pub fn set_mode(&self, mode: PermissionMode) {
        if let Ok(mut m) = self.mode.lock() {
            *m = mode;
        }
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode.lock().map(|m| *m).unwrap_or(PermissionMode::Queue)
    }

    /// Проверить ключ "всегда разрешено".
    pub fn is_always_allowed(&self, key: &str) -> bool {
        self.always_allowed.lock().map(|s| s.contains(key)).unwrap_or(false)
    }

    /// Запросить разрешение у человека. Вызывается из инструментов
    /// (async-контекст). НЕ держит никаких локков во время ожидания —
    /// параллельные инструменты работают.
    pub async fn request(
        &self,
        tool: &str,
        summary: &str,
        reason: &str,
        always_key: &str,
    ) -> Outcome {
        // Уже разрешено "навсегда" — спрашивать не надо.
        if self.is_always_allowed(always_key) {
            return Outcome::Granted;
        }

        match self.mode() {
            PermissionMode::Terminal => self.request_terminal(tool, summary, reason, always_key),
            PermissionMode::Queue => {
                let rx = {
                    let mut g = match self.inner.lock() {
                        Ok(g) => g,
                        Err(_) => return Outcome::Denied,
                    };
                    let id = g.next_id;
                    g.next_id += 1;
                    let (tx, rx) = oneshot::channel();
                    g.queue.push_back(QueuedRequest {
                        pending: PendingRequest {
                            id,
                            tool: tool.to_string(),
                            summary: summary.to_string(),
                            reason: reason.to_string(),
                            always_key: always_key.to_string(),
                        },
                        tx,
                    });
                    rx
                };
                match tokio::time::timeout(WAIT_TIMEOUT, rx).await {
                    Ok(Ok(outcome)) => outcome,
                    // Таймаут ИЛИ упавший канал (очередь сброшена) — отказ.
                    _ => Outcome::Expired,
                }
            }
        }
    }

    /// Блокирующий вопрос через stdin — только для режимов без TUI.
    fn request_terminal(&self, tool: &str, summary: &str, reason: &str, always_key: &str) -> Outcome {
        use std::io::{self, Write};

        // ВАЖНО: печатаем напрямую в stdout. Этот режим используется
        // только когда TUI не запущен (--simple/--voice приостанавливает
        // TUI своим tty_guard), поэтому прямой вывод безопасен.
        println!("\n⚠️  Запрос разрешения ({})", tool);
        if !reason.is_empty() {
            println!("   Причина: {}", reason);
        }
        println!("   Действие: {}", summary);
        print!("Разрешить? [y] один раз / [a] всегда для '{}' / [n] нет: ", always_key);
        let _ = io::stdout().flush();

        let mut answer = String::new();
        let read = std::io::stdin().read_line(&mut answer);
        match read {
            Ok(_) => match answer.trim().to_lowercase().as_str() {
                "y" | "yes" | "д" | "да" | "1" => Outcome::Granted,
                "a" | "always" | "в" | "всегда" | "2" => {
                    self.remember_always(always_key);
                    Outcome::Granted
                }
                _ => Outcome::Denied,
            },
            Err(_) => Outcome::Denied,
        }
    }

    fn remember_always(&self, key: &str) {
        if key.is_empty() {
            return;
        }
        if let Ok(mut s) = self.always_allowed.lock() {
            s.insert(key.to_string());
        }
    }

    /// Текущий (самый старый) ожидающий запрос — для показа в интерфейсе.
    pub fn current_pending(&self) -> Option<PendingRequest> {
        let mut g = self.inner.lock().ok()?;
        // После таймаута или отмены receiver закрывается. Убираем такие
        // записи с головы очереди, иначе TUI будет бесконечно показывать
        // уже недействительное «ЖДЁТ РАЗРЕШЕНИЯ».
        loop {
            let closed = match g.queue.front() {
                Some(q) => q.tx.is_closed(),
                None => return None,
            };
            if closed {
                g.queue.pop_front();
            } else {
                return g.queue.front().map(|q| q.pending.clone());
            }
        }
    }

    /// Ответить на самый старый запрос. true — ответ доставлен.
    pub fn resolve(&self, choice: Choice) -> bool {
        let req = {
            let mut g = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return false,
            };
            // Достаём самый старый; если отправитель уже ушёл по таймауту
            // (rx выброшен), пропускаем дальше по очереди.
            loop {
                let Some(q) = g.queue.pop_front() else { return false };
                if !q.tx.is_closed() {
                    break q;
                }
            }
        };
        if choice == Choice::Always {
            self.remember_always(&req.pending.always_key);
        }
        let outcome = match choice {
            Choice::Once | Choice::Always => Outcome::Granted,
            Choice::Deny => Outcome::Denied,
        };
        req.tx.send(outcome).is_ok()
    }

    /// Список ключей "всегда разрешено" — для /permissions.
    pub fn list_always(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .always_allowed
            .lock()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Забыть все "всегда разрешено" (/permissions clear).
    pub fn clear_always(&self) {
        if let Ok(mut s) = self.always_allowed.lock() {
            s.clear();
        }
    }
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn new_arc() -> Arc<PermissionManager> {
        Arc::new(PermissionManager::new())
    }

    #[tokio::test]
    async fn test_queue_resolve_once() {
        let pm = new_arc();
        let task = {
            let pm = pm.clone();
            tokio::spawn(async move { pm.request("bash", "ls -la", "тест", "ls").await })
        };
        // Даём задаче время положить запрос в очередь.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(pm.current_pending().is_some());
        assert!(pm.resolve(Choice::Once));
        assert_eq!(task.await.unwrap(), Outcome::Granted);
        assert!(pm.current_pending().is_none());
        // Once НЕ запоминает ключ.
        assert!(!pm.is_always_allowed("ls"));
    }

    #[tokio::test]
    async fn test_queue_resolve_always_and_deny() {
        let pm = new_arc();

        let t1 = {
            let pm = pm.clone();
            tokio::spawn(async move { pm.request("bash", "npm install", "", "npm").await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(pm.resolve(Choice::Always));
        assert_eq!(t1.await.unwrap(), Outcome::Granted);
        assert!(pm.is_always_allowed("npm"));

        // Второй запрос с тем же ключом проходит БЕЗ вопроса.
        assert_eq!(pm.request("bash", "npm test", "", "npm").await, Outcome::Granted);

        let t2 = {
            let pm = pm.clone();
            tokio::spawn(async move { pm.request("bash", "rm x", "", "rm").await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(pm.resolve(Choice::Deny));
        assert_eq!(t2.await.unwrap(), Outcome::Denied);
        assert!(!pm.is_always_allowed("rm"));
    }

    #[tokio::test]
    async fn test_fifo_order() {
        let pm = new_arc();
        let t1 = { let pm = pm.clone(); tokio::spawn(async move { pm.request("bash", "первый", "", "a").await }) };
        tokio::time::sleep(Duration::from_millis(50)).await;
        let t2 = { let pm = pm.clone(); tokio::spawn(async move { pm.request("bash", "второй", "", "b").await }) };
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Самый старый — "первый".
        assert_eq!(pm.current_pending().unwrap().summary, "первый");
        assert!(pm.resolve(Choice::Deny));
        assert_eq!(t1.await.unwrap(), Outcome::Denied);
        assert_eq!(pm.current_pending().unwrap().summary, "второй");
        assert!(pm.resolve(Choice::Once));
        assert_eq!(t2.await.unwrap(), Outcome::Granted);
    }

    #[tokio::test]
    async fn test_resolve_empty_queue_is_false() {
        let pm = PermissionManager::new();
        assert!(!pm.resolve(Choice::Once));
    }

    #[tokio::test]
    async fn test_current_pending_discards_cancelled_request() {
        let pm = new_arc();
        let task = {
            let pm = pm.clone();
            tokio::spawn(async move { pm.request("bash", "ожидает", "", "bash").await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(pm.current_pending().is_some());

        // Отмена ожидающего инструмента закрывает receiver oneshot.
        task.abort();
        let _ = task.await;
        assert!(pm.current_pending().is_none());
    }

    #[test]
    fn test_clear_always() {
        let pm = PermissionManager::new();
        pm.remember_always("cargo");
        assert!(pm.is_always_allowed("cargo"));
        pm.clear_always();
        assert!(!pm.is_always_allowed("cargo"));
    }
}
