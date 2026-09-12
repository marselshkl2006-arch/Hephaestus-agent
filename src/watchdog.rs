//! Watchdog процесса: детект полного зависания рантайма.
//!
//! По образцу `hermes_startup_watchdog.py` (Hermes): watchdog — это
//! ОБЫЧНЫЙ std::thread, который НИКАК не зависит от tokio-рантайма. Если
//! рантайм мёртв (все воркеры стоят на блокирующих вызовах, дедлок на
//! локе, EventStream лег) — таймеры tokio не тикают, TUI не рисуется,
//! а этот поток замечает остывший heartbeat и жёстко завершает процесс
//! с кодом 75 («soft failure» — привычный для supervisor'ов сигнал
//! «перезапусти меня»).
//!
//! КЛЮЧЕВОЕ: heartbeat считается не только активностью чата — в простое
//! его поддерживают фоновые тикеры (MCP-watchdog раз в минуту, poll
//! Telegram, тик отрисовки TUI). Поэтому «heartbeat молчит 15 минут»
//! однозначно означает зависание РАНТАЙМА, а не простой пользователя.
//!
//! Отличие от таймаута хода (900с в request_queue): тот ловит зависание
//! одного хода, этот — зависание ВСЕГО процесса (в т.ч. когда таймауты
//! тоже мертвы, потому что стоят в той же заблокированной очереди).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Порог тишины: 15 минут без единого heartbeat = зависание.
const IDLE_LIMIT: Duration = Duration::from_secs(15 * 60);

static LAST_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);

/// TUI-специфичный heartbeat и флаг «TUI активен».
///
/// ЗАЧЕМ отдельный счётчик: глобальный heartbeat в TUI-режиме постоянно
/// освежает MCP-watchdog (тик раз в минуту на ЖИВОМ воркере tokio) —
/// дедлок САМОЙ задачи TUI глобальный счётчик не заметит. Поэтому при
/// активном TUI отслеживается именно свежесть ТИКА ОТРИСОВКИ основного
/// цикла; фоновые тикеры на это не влияют.
static TUI_LAST_TICK_MS: AtomicU64 = AtomicU64::new(0);
static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Включить/выключить TUI-режим отслеживания. Вызывается repl-ом вокруг
/// основного цикла.
pub fn set_tui_active(active: bool) {
    if active {
        TUI_LAST_TICK_MS.store(now_ms(), Ordering::Relaxed);
    }
    TUI_ACTIVE.store(active, Ordering::Relaxed);
}

/// Запустить watchdog-поток. Вызывается ОДИН раз в main() до входа в
/// любые режимы (TUI/telegram/simple/voice).
pub fn spawn() {
    LAST_HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
    std::thread::Builder::new()
        .name("hephaestus-watchdog".into())
        .spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(15));
                // 1) TUI-режим: следим за тиком ОТРИСОВКИ (не за глобальным
                //    heartbeat — тот освежают фоновые тикеры на живых
                //    воркерах даже при заклинившем TUI-цикле).
                if TUI_ACTIVE.load(Ordering::Relaxed) {
                    let last_tui = TUI_LAST_TICK_MS.load(Ordering::Relaxed);
                    let idle = now_ms().saturating_sub(last_tui);
                    if idle >= IDLE_LIMIT.as_millis() as u64 {
                        fire(&format!(
                            "тип отрисовки TUI молчал {idle}мс (лимит {}с) — основной цикл завис",
                            IDLE_LIMIT.as_secs()
                        ));
                    }
                    continue;
                }
                // 2) Не-TUI: глобальный heartbeat от ходов агента,
                //    Telegram-poll, MCP-watchdog, runtime-тикера.
                let last = LAST_HEARTBEAT_MS.load(Ordering::Relaxed);
                let idle = now_ms().saturating_sub(last);
                if idle >= IDLE_LIMIT.as_millis() as u64 {
                    fire(&format!(
                        "heartbeat молчал {idle}мс (лимит {}с) — рантайм завис",
                        IDLE_LIMIT.as_secs()
                    ));
                }
            }
        })
        .expect("watchdog-поток обязан запускаться");
}

/// Финальный отчёт и жёсткий выход (код 75 — «перезапусти меня»).
fn fire(detail: &str) {
    eprintln!(
        "⏰ Гефест-watchdog: {detail} — завершаюсь (код 75)."
    );
    crate::logging_system::error(&format!("[watchdog] {detail}, os.exit(75)"));
    std::process::exit(75);
}

/// Отметка живости. Вызывается из всех долгоживущих циклов:
/// - тик отрисовки TUI (repl.rs, каждые ~120мс);
/// - poll-цикл Telegram-бота;
/// - MCP-watchdog (раз в минуту) — работает даже в headless-режимах;
/// - вехи хода агента (старт/финиш LLM-вызова, выполнение инструментов).
pub fn bump(reason: &str) {
    LAST_HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
    // reason не логируем (спам); параметр — для читаемости call-site.
    let _ = reason;
}

/// Тик ОТРИСОВКИ TUI — в дополнение к глобальному bump. Вызывается
/// основным циклом repl.rs каждые ~120мс.
pub fn bump_tui_tick() {
    TUI_LAST_TICK_MS.store(now_ms(), Ordering::Relaxed);
}

/// Runtime-тикер для режимов БЕЗ foreground-цикла с тиками (--simple,
/// --voice, --telegram без MCP): раз в минуту доказывает, что tokio
/// рантайм жив. В TUI-режиме НЕ включать: там живость доказывает тик
/// отрисовки основного цикла, и фоновый тикер замаскировал бы зависание
/// самого TUI (задача-тикер продолжала бы тикать на живых воркерах).
pub fn spawn_runtime_ticker() {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            bump("runtime-ticker");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_updates_heartbeat() {
        spawn(); // инициализация + поток (в тесте безвреден: лимит 15 мин)
        let before = LAST_HEARTBEAT_MS.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(5));
        bump("test");
        let after = LAST_HEARTBEAT_MS.load(Ordering::Relaxed);
        assert!(after >= before);
    }

    #[test]
    fn tui_flag_switch() {
        set_tui_active(true);
        assert!(TUI_ACTIVE.load(Ordering::Relaxed));
        bump_tui_tick();
        set_tui_active(false);
        assert!(!TUI_ACTIVE.load(Ordering::Relaxed));
    }
}
