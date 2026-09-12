//! RequestQueue — очередь запросов к агенту, как в opencode.
//!
//! Проблема: Agent — общий (TUI + Telegram-бот на одном Arc<Mutex<Agent>>),
//! и лок агента держался НА ВЕСЬ ход chat(). Пока агент отвечал, новые
//! сообщения:
//!   • в TUI — отбивались ("Агент ещё обрабатывает предыдущий запрос");
//!   • в Telegram — молча копились задачами, висящими на мьютексе.
//!
//! Решение: FIFO-очередь + ОДИН фоновый воркер. Любой источник кладёт
//! запрос (`push` → позиция + канал результата) и сразу получает
//! управление; воркер забирает элементы строго по одному и гонит их
//! через Agent::chat. История диалога остаётся последовательной (контекст
//! общий — параллельные ходы недопустимы в принципе), но человек видит
//! свою позицию в очереди и ответ приходит сам, без повторных отправок.
//!
//! Гарантии:
//!   • порядок = FIFO (tokio::sync::Mutex честный);
//!   • ровно один воркер на очередь (spawn_worker идемпотентен);
//!   • если получатель результата ушёл (пользователь вышел), отправка
//!     в закрытый канал просто игнорируется — воркер не падает.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::{Mutex as TokioMutex, Notify, oneshot};

/// Один элемент очереди: текст запроса и источник (для логов/диагностики).
#[derive(Debug, Clone)]
pub struct QueuedTurn {
    pub text: String,
    /// Например "tui" или "telegram:<chat_id>".
    pub source: String,
}

struct QueueItem {
    turn: QueuedTurn,
    tx: oneshot::Sender<String>,
}

pub struct RequestQueue {
    inner: TokioMutex<VecDeque<QueueItem>>,
    notify: Notify,
}

impl Default for RequestQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestQueue {
    pub fn new() -> Self {
        Self {
            inner: TokioMutex::new(VecDeque::new()),
            notify: Notify::new(),
        }
    }

    /// Поставить запрос в очередь. Возвращает позицию с нуля (0 = воркер
    /// свободен, выполнение начинается сейчас) и приёмник финального ответа.
    pub async fn push(&self, turn: QueuedTurn) -> (usize, oneshot::Receiver<String>) {
        let (tx, rx) = oneshot::channel();
        let pos = {
            let mut q = self.inner.lock().await;
            let pos = q.len();
            q.push_back(QueueItem { turn, tx });
            pos
        };
        // Notify хранит разрешение, даже если воркер ещё спит в notified()
        // — уведомление не теряется между unlock и ожиданием.
        self.notify.notify_one();
        (pos, rx)
    }

    /// Забрать следующий запрос (блокируется до появления).
    async fn pop(&self) -> (QueuedTurn, oneshot::Sender<String>) {
        loop {
            {
                let mut q = self.inner.lock().await;
                if let Some(item) = q.pop_front() {
                    return (item.turn, item.tx);
                }
            }
            self.notify.notified().await;
        }
    }

    /// Текущая глубина очереди — для статусной строки TUI.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Запустить фоновый воркер (одна задача на весь срок жизни процесса).
    /// `run_turn` инкапсулирует выполнение хода (лок агента берётся ТОЛЬКО
    /// на время хода; между ходами агент свободен для /provider, /reset и
    /// т.п.). Идемпотентность обеспечивает вызывающая сторона через
    /// `WorkerGuard` — см. ниже, зачем.
    pub fn spawn_worker<F, Fut>(self: &Arc<Self>, run_turn: F) -> WorkerGuard
    where
        F: FnMut(String) -> Fut + Send + 'static,
        Fut: Future<Output = String> + Send,
    {
        let this = self.clone();
        tokio::spawn(async move {
            let mut run_turn = run_turn;
            loop {
                let (turn, tx) = this.pop().await;
                crate::logging_system::info(&format!(
                    "[queue] ход из '{}', в очереди осталось {}",
                    turn.source,
                    this.len().await
                ));
                let reply = run_turn(turn.text).await;
                // Получатель мог выйти — игнорируем ошибку канала.
                let _ = tx.send(reply);
            }
        });
        WorkerGuard
    }
}

/// Маркер запущенного воркера. Нужен только для читаемости вызова;
/// защита от двойного запуска — на стороне владельцев Arc (TUI стартует
/// воркер один раз, Telegram-режим — свой процесс).
pub struct WorkerGuard;

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(text: &str) -> QueuedTurn {
        QueuedTurn { text: text.to_string(), source: "test".into() }
    }

    #[tokio::test]
    async fn test_fifo_order_and_positions() {
        let q = Arc::new(RequestQueue::new());

        let (p1, r1) = q.push(turn("первый")).await;
        let (p2, r2) = q.push(turn("второй")).await;
        let (p3, _) = q.push(turn("третий")).await;

        assert_eq!((p1, p2, p3), (0, 1, 2));
        assert_eq!(q.len().await, 3);

        // Воркер: выполняем "ход" — просто возвращаем текст.
        q.spawn_worker(|text| async move { format!("готово: {}", text) });

        assert_eq!(r1.await.unwrap(), "готово: первый");
        assert_eq!(r2.await.unwrap(), "готово: второй");
        assert_eq!(q.len().await, 0);

        // Очередь жива после опустошения.
        let (pos, rx) = q.push(turn("четвёртый")).await;
        assert_eq!(pos, 0); // всё разгружено — выполнение сразу
        assert_eq!(rx.await.unwrap(), "готово: четвёртый");
    }

    #[tokio::test]
    async fn test_push_before_worker_starts_is_queued() {
        let q = Arc::new(RequestQueue::new());
        let (_, rx) = q.push(turn("ранний")).await;
        assert_eq!(q.len().await, 1);
        q.spawn_worker(|text| async move { text });
        assert_eq!(rx.await.unwrap(), "ранний");
    }
}
