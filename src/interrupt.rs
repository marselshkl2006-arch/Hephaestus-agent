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
