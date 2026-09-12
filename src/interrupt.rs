//! Прерывание хода агента (по образцу opencode/CC): Esc в TUI →
//! выполнение останавливается между итерациями tool-loop, ход
//! возвращает накопленный текст, история сохраняется.
//!
//! МЕХАНИКА: глобальный счётчик «эпох прерываний». Agent::chat() при
//! старте запоминает эпоху, и на каждой точке отмены (перед LLM-вызовом,
//! перед выполнением инструментов) сверяет её; интерфейс (repl по Esc)
//! вызывает `interrupt()` — инкремент. Всё, что уже началось (запущенные
//! инструменты, стрим), ДОЖИВАЁТ до конца текущего await — честный
//! кооперативный стоп: дедлоков нет, а реакции — в пределах секунды.

use std::sync::atomic::{AtomicU64, Ordering};

static INTERRUPT_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Прервать текущий ход (вызывается из интерфейса — Esc в TUI).
/// idempotent: повторные вызовы до нового хода безвредны.
pub fn interrupt() {
    INTERRUPT_EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// Текущая эпоха — для tokio::select! вокруг блокирующих await.
pub fn current_epoch() -> u64 {
    INTERRUPT_EPOCH.load(Ordering::SeqCst)
}

/// Ждать прерывания хода: резолвится, как только эпоха уедет от `epoch`.
/// Живой инцидент (TUI + nvidia): стрим для custom-провайдеров выключен,
/// эссе шло ОДНИМ блокирующим HTTP-вызовом — Esc не мог ничего прервать,
/// пока вызов не завершится сам. Теперь LLM-вызов оборачивается в
/// `tokio::select!` с этим ожиданием: дроп future = отмена HTTP-запроса.
pub async fn wait_interrupt(epoch: u64) {
    loop {
        if INTERRUPT_EPOCH.load(Ordering::SeqCst) != epoch {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Маркер хода: снимок эпохи на старте.
#[derive(Debug)]
pub struct InterruptGuard {
    epoch: u64,
}

impl InterruptGuard {
    pub fn new() -> Self {
        Self { epoch: INTERRUPT_EPOCH.load(Ordering::SeqCst) }
    }

    /// true — ход прерван после старта (эпоха уехала вперёд).
    pub fn is_interrupted(&self) -> bool {
        INTERRUPT_EPOCH.load(Ordering::SeqCst) != self.epoch
    }

    /// Снимок эпохи — для select!-абортов длинных await (см. wait_interrupt).
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl Default for InterruptGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_flips_guard() {
        let g = InterruptGuard::new();
        assert!(!g.is_interrupted());
        interrupt();
        assert!(g.is_interrupted());
        // Новый guard — чистый.
        let g2 = InterruptGuard::new();
        assert!(!g2.is_interrupted());
    }
}
