//! REPL — текстовый интерфейс агента на `ratatui` + `crossterm`.
//!
//! Функциональность:
//! - скроллбэк истории диалога (по строкам, с учётом переносов, колесо
//!   мыши + PageUp/PageDown), ввод с историей команд (Up/Down) и
//!   нормальным редактированием строки (курсор, Home/End, Ctrl+U/K/W),
//! - спиннер + строка статуса (провайдер/модель, токены, вызовы
//!   инструментов, время работы сессии),
//! - слэш-команды (`/help`, `/reset`, `/save`, `/load`, `/tools`, `/stats`,
//!   `/provider`, `/model`, `/checkpoint`, `/voice`, `/exit`, ...),
//! - graceful shutdown: Ctrl+C (или `/exit`) сохраняет сессию на диск перед выходом.
//!
//! ## История правок этого файла (по жалобам пользователя)
//! 1. "hhhh вместо Backspace" — на части терминалов физическая клавиша
//!    Backspace шлёт код Ctrl+H (0x08), а не `KeyCode::Backspace` (0x7F/DEL).
//!    Старый код обрабатывал только `KeyCode::Backspace` и слепо
//!    вставлял ЛЮБОЙ `KeyCode::Char(c)` в строку ввода, включая тот же
//!    Ctrl+H — из-за чего вместо удаления символа в строку вставлялась
//!    буква 'h'. Исправлено: `KeyCode::Char(c)` с модификатором CONTROL
//!    теперь разбирается отдельно (Ctrl+H/U/K/W/A/E), а не вставляется
//!    как текст.
//! 2. Не было курсора — редактирование работало только "дописать в
//!    конец"/"стереть с конца". Добавлен настоящий курсор (индекс символа)
//!    с Left/Right/Home/End/Delete и вставкой/удалением в произвольном месте.
//! 3. `/model` и `/provider` делали сетевые запросы (пинг LLM, детект
//!    моделей на localhost) СИНХРОННО прямо в цикле обработки событий —
//!    это блокировало вообще весь TUI (клавиатура, спиннер, прокрутка)
//!    на время запроса. Перенесено в `tokio::spawn`, как уже было
//!    сделано для обычных сообщений и `/goal`.
//!  4. Прокрутка истории считалась "по записям" (грубая эвристика
//!     entries/2), из-за чего на длинных ответах прокрутка визуально не
//!     совпадала с реальным содержимым. Переписано на посчёт по
//!     фактическим строкам (с учётом `\n` в тексте). Добавлена прокрутка
//!     колесом мыши.
//! 5. Добавлена строка статуса: провайдер/модель, токены, вызовы
//!    инструментов, время работы сессии — обновляется каждый кадр.
//! 6. `/voice` — голосовой режим теперь доступен прямо из REPL: TUI
//!    временно приостанавливается (raw mode выключается, экран
//!    возвращается в обычный), запускается `VoiceInterface::run_loop()`
//!    на ТОМ ЖЕ `Agent` (см. фикс в voice_interface.rs — раньше он
//!    требовал отдельный владеющий экземпляр `Agent`, теперь разделяет
//!    общий `Arc<Mutex<Agent>>`), после выхода из голосового режима
//!    (`exit`/`quit`) TUI восстанавливается.

use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use tokio::sync::{mpsc, Mutex};

use crate::goal_mode::GoalModeAgent;
use crate::llm::LLMProvider;
use crate::session_store;
use crate::{Agent, LLMMessage};

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SCROLL_STEP: usize = 3;

/// Список команд для автодополнения при вводе "/" — раньше подсказок не
/// было вообще, узнать команды можно было только через уже отправленный
/// `/help`. Держим здесь, а не парсим `/help`, чтобы не завязываться на
/// форматирование текста справки.
const COMMANDS: &[(&str, &str)] = &[
    ("help", "справка по командам"),
    ("reset", "очистить историю диалога"),
    ("save", "сохранить сессию на диск"),
    ("load", "загрузить последнюю сохранённую сессию"),
    ("checkpoint", "<имя> — сохранить диалог под именем"),
    ("checkpoints", "список сохранённых чекпоинтов"),
    ("restore", "<id> — восстановить чекпоинт"),
    ("provider", "[имя] [модель] [URL или ключ] [ключ] — сменить провайдера"),
    ("providers", "список провайдеров"),
    ("model", "[имя] — сменить модель / показать доступные"),
    ("models", "показать доступные модели"),
    ("voice", "голосовой режим"),
    ("telegram", "запустить Telegram-бота в фоне на этом же диалоге"),
    ("goal", "<текст> — режим достижения цели"),
    ("tools", "список инструментов"),
    ("work_dir", "[путь] — рабочая директория инструментов (с подтверждением доверия)"),
    ("allow", "once|always — разрешить опасное действие; /deny — запретить"),
    ("permissions", "постоянные разрешения; /permissions clear — сбросить"),
    ("todos", "текущий план задачи агента (todo_write/todo_read)"),
    ("mcp", "MCP-серверы; /mcp reload — перечитать конфиг и переподключить"),
    ("stats", "статистика сессии"),
    ("compress", "сжать контекст диалога"),
    ("undo", "откатить изменения файлов последнего хода агента"),
    ("exit", "выход"),
    ("quit", "выход"),
];

#[derive(Clone, Copy, PartialEq)]
enum Role {
    User,
    Assistant,
    System,
    Error,
    /// Дифф изменения файла (как в opencode): рендерится построчно —
    /// `+` зелёным, `-` красным, контекст приглушённо.
    Diff,
}

#[derive(Clone)]
struct HistoryEntry {
    role: Role,
    text: String,
    time: String,
}

impl HistoryEntry {
    fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }
}

/// Сообщение из фонового `tokio::spawn`-таска в главный цикл TUI.
enum ReplMsg {
    /// Финальный результат долгой операции (результат /provider или /model)
    /// — снимает флаг `busy`.
    Done(HistoryEntry),
    /// Завершённый ХОД из очереди запросов (request_queue.rs) — уменьшает
    /// счётчик незавершённых ходов и показывает ответ.
    TurnDone(HistoryEntry),
    /// Промежуточное сообщение долгой операции (шаги `/goal`,
    /// "проверяю подключение..." у /provider) — НЕ снимает `busy`.
    Update(HistoryEntry),
    /// Кусочек текста модели ПО МЕРЕ генерации (стриминг): рисуется в
    /// живой панели над вводом и НЕ попадает в историю.
    StreamDelta(String),
}

struct EchoRestore;
impl Drop for EchoRestore {
    fn drop(&mut self) {
        crate::logging_system::set_console_echo(true);
    }
}

/// Запустить REPL. Возвращает управление, когда пользователь вышел
/// (`/exit`, `Ctrl+C`) — сессия к этому моменту уже сохранена на диск.
pub async fn run(agent: Agent) -> io::Result<()> {
    let agent = Arc::new(Mutex::new(agent));
    run_shared(agent).await
}

/// Вариант для случая, когда Arc<Agent> УЖЕ создан и разделяется
/// (автостарт Telegram-бота в main): TUI, бот и очередь работают на
/// одном агенте.
pub async fn run_shared(agent: Arc<Mutex<Agent>>) -> io::Result<()> {
    // TUI занял терминал — дублирование WARNING/ERROR в консоль ломает
    // отрисовку. Всё пишется в файлы: ~/.hephaestus/logs/
    crate::logging_system::set_console_echo(false);
    let _echo_guard = EchoRestore;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Захват мыши: колесо листает чат ПРИЛОЖЕНИЕМ, а ЛКМ-выделение
    // обрабатывается тоже приложением — с копированием в буфер ОС через
    // OSC52 и автоскроллом у краёв панели. Это заменяет старый костыль
    // с F2-переключателем режимов: теперь "копировать" и "листать"
    // работают одновременно, как в обычном терминале/Claude Code.
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, agent).await;

    // Восстановление терминала — всегда, даже если run_app вернул ошибку.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste);
    let _ = terminal.show_cursor();

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    agent: Arc<Mutex<Agent>>,
) -> io::Result<()> {
    // Очередь запросов (request_queue.rs): один воркер на весь процесс,
    // выполняет ходы из TUI и Telegram строго по одному.
    Agent::spawn_queue_worker(&agent).await;

    // Однократные клоны разделяемых Arc-полей агента — чтобы опрос
    // разрешений и ответы /allow работали БЕЗ лока агента (лок занят
    // воркером ровно во время хода; раньше это давало дедлок: запрос
    // разрешения ждал ответа под локом, а /allow ждал лок).
    let (permissions, _todos, queue) = {
        let a = agent.lock().await;
        (a.permissions.clone(), a.todos.clone(), a.queue.clone())
    };
    let session_start = Instant::now();

    let mut history: Vec<HistoryEntry> = Vec::new();

    // ВОССТАНОВЛЕНИЕ (state_db.rs — канонический стор SQLite):
    // 1) Recovery-заметка от Agent::new (краш прошлого процесса / новая
    //    сессия после 3+ крашей подряд).
    // 2) Недоделанные tool-вызовы прошлой сессии (pending/running).
    // 3) История сообщений — ИЗ БД. session.json используется только как
    //    одноразовая миграция, если БД пуста, а legacy-файл есть.
    {
        let mut a = agent.lock().await;
        if let Some(notice) = &a.recovery_notice {
            history.push(HistoryEntry::new(Role::System, notice.clone()));
        }
        let unfinished = a.unfinished_tool_calls();
        if !unfinished.is_empty() {
            let mut lines = vec![format!(
                "⚠️ В прошлой сессии осталось {} недоделанный(х) вызов(ов) инструментов:",
                unfinished.len()
            )];
            for tc in unfinished.iter().take(10) {
                lines.push(format!("   [{}] {} — статус: {}", tc.name, tc.call_id, tc.status));
            }
            history.push(HistoryEntry::new(Role::System, lines.join("\n")));
        }
        let restored = a.state.load_messages(a.session_id());
        let count = restored.len();
        for msg in &restored {
            history.push(HistoryEntry::new(llm_role_to_repl_role(&msg.role), msg.content.clone()));
        }
        if count > 0 {
            let when = a.state.session_updated_at(a.session_id()).unwrap_or_default();
            history.push(HistoryEntry::new(
                Role::System,
                format!("── Восстановлена сессия #{}: {} сообщений (активность {}) ──", a.session_id(), count, when),
            ));
        }
        // ВАЖНО: восстановленное должно попасть и в LLM-контекст агента
        // (Agent.messages), а не только в видимую историю TUI.
        if !restored.is_empty() {
            a.load_messages(restored).await;
        }
    }

    // Legacy-миграция: БД пуста, но старый session.json существует и ещё
    // не переносился — переносим его содержимое в канонический стор один
    // раз. Сам файл НЕ удаляем (остаётся бэкапом пользователя).
    {
        let migrated = agent.lock().await.state.meta_get(crate::state_db::meta_keys::JSON_MIGRATED);
        let db_empty = agent.lock().await.state.count_messages(agent.lock().await.session_id()) == 0;
        if migrated.is_none() && db_empty {
            if let Some(saved) = session_store::load() {
                if !saved.messages.is_empty() {
                    let count = saved.messages.len();
                    for msg in &saved.messages {
                        history.push(HistoryEntry::new(llm_role_to_repl_role(&msg.role), msg.content.clone()));
                    }
                    agent.lock().await.load_messages(saved.messages.clone()).await;
                    agent.lock().await.state.meta_set(crate::state_db::meta_keys::JSON_MIGRATED, "1");
                    history.push(HistoryEntry::new(
                        Role::System,
                        format!(
                            "── Перенесена старая сессия из session.json: {} сообщений (теперь история хранится в state.db) ──",
                            count
                        ),
                    ));
                } else {
                    agent.lock().await.state.meta_set(crate::state_db::meta_keys::JSON_MIGRATED, "1");
                }
            } else {
                // Файла нет — сразу помечаем, чтобы не проверять при каждом старте.
                agent.lock().await.state.meta_set(crate::state_db::meta_keys::JSON_MIGRATED, "1");
            }
        }
    }

    history.push(HistoryEntry::new(
        Role::System,
        "⚡ Гефест — AI Coding Agent (Rust). Введите запрос или /help для списка команд.",
    ));

    // Ввод — Vec<char>, а не String: индексация по символам, а не байтам
    // (кириллица многобайтная в UTF-8 — байтовые индексы для позиции
    // курсора были бы источником паник/багов на русском тексте).
    let mut input: Vec<char> = Vec::new();
    let mut cursor: usize = 0;
    let mut input_history: Vec<String> = Vec::new();
    let mut history_cursor: Option<usize> = None;
    let mut scroll_from_bottom: usize = 0;
    // Захват мыши ВКЛЮЧЁН ПОСТОЯННО (без F2-переключателя — костыль
    // удалён): колесо листает чат, ЛКМ-выделение копирует через OSC52.
    // Терминал без поддержки OSC52? Текст всё равно показывается
    // выделенным — можно взять любым нативным способом (Shift+мышь).
    let mut busy = false;
    // Незавершённые ходы в очереди (request_queue.rs): сообщения больше
    // не отбиваются "агент занят" — встают в очередь, и пока счётчик > 0
    // или воркер занят, спиннер крутится.
    let mut queued_pending: usize = 0;
    let mut spinner_frame: usize = 0;
    // /work_dir — двухшаговое подтверждение доверия (как в Claude Code):
    // шаг 1 `/work_dir <путь>` запоминает кандидата и спрашивает доверие,
    // шаг 2 `/work_dir ok` применяет (или `/work_dir cancel` отменяет).
    // В TUI нельзя задать y/N через stdin (raw-mode + EventStream уже
    // съедают ввод), поэтому подтверждение — тоже командой.
    let mut pending_workdir: Option<std::path::PathBuf> = None;

    // Кэш статистики для строки статуса — обновляется каждый кадр через
    // try_lock (не блокирует, если агент занят долгим запросом).
    let mut cached_stats: (u64, u64, u64) = (0, 0, 0);
    let mut cached_model: (String, String) = ("?".to_string(), "?".to_string());
    // Ожидающий запрос разрешения (permissions.rs): показываем вопрос в
    // чат ОДИН раз на каждый id и держим индикатор в заголовке ввода.
    let mut last_perm_shown: Option<u64> = None;
    let mut perm_waiting;
    let mut cached_queue_len;

    // ── Выделение мышью как в обычном терминале/Claude Code ──
    // ЛКМ зажата → тащим → отпускаем = копирование в буфер ОС через
    // OSC52 (работает и поверх SSH). Пока тянем у верхнего/нижнего края
    // панели чата, история САМА листается в ту сторону. Состояние:
    //   • sel_anchor/sel_end — абсолютные индексы визуальных строк
    //     all_lines + колонка символа; Some = выделение активно.
    //   • drag_edge — у какого края застрял указатель (-1 верх/+1 низ),
    //     обрабатывается каждый тик (автоскролл).
    //   • plain_lines — посимвольное зеркало отрисованных строк (заполняет
    //     draw_ui), из него вырезается выделенный текст.
    let mut sel_anchor: Option<(usize, usize)> = None;
    let mut sel_end: Option<(usize, usize)> = None;
    let mut mouse_down = false;
    let mut drag_edge: i8 = 0;
    let mut stream_buf = String::new();
    let mut plain_lines: Vec<String> = Vec::new();
    // Геометрия панели чата последнего кадра — для перевода координат
    // мыши в (строка, колонка) БЕЗ пересчёта внутри обработчика событий.
    let mut chat_geom: Option<(u16, u16, u16, u16, usize)> = None; // x,y,w,h,start_idx

    let (tx, mut rx) = mpsc::unbounded_channel::<ReplMsg>();

    // Живой стриминг: агент пересылает сюда кусочки текста модели, а
    // главный цикл превращает их в ReplMsg::StreamDelta.
    {
        let (tx_stream, mut rx_stream) = mpsc::unbounded_channel::<String>();
        if let Ok(mut sink) = agent.lock().await.stream_sink.lock() {
            *sink = Some(tx_stream);
        }
        let tx_fwd = tx.clone();
        tokio::spawn(async move {
            while let Some(chunk) = rx_stream.recv().await {
                let _ = tx_fwd.send(ReplMsg::StreamDelta(chunk));
            }
        });
    }

    // Живой лог вызовов инструментов: раньше во время работы агента
    // пользователь видел ТОЛЬКО спиннер — какие инструменты вызываются,
    // упали ли они, было непонятно ("когда ошибка вылазит не понятно что
    // происходит"). Хук вызывается из Agent::chat после каждого
    // инструмента и шлёт строку в чат. Тот же хук использует goal_mode
    // для своего прогресса — он ставит СВОЙ на время /goal, поэтому
    // после завершения /goal мы возвращаем наш (см. спавн /goal ниже).
    {
        let a = agent.lock().await;
        a.set_tool_hook(Some(tool_progress_hook(tx.clone())));
    }

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(120));

    // WATCHDOG: TUI-режим активен — следим за свежестью тика отрисовки.
    crate::watchdog::set_tui_active(true);

    loop {
        // WATCHDOG: тик основного цикла TUI (каждые ~120мс) — главный
        // источник heartbeat при активном TUI. Сюда же попадает
        // suspended-ожидание ниже.
        crate::watchdog::bump_tui_tick();
        // ИСПРАВЛЕНО (жалоба "bash — опять в кашу"): раньше проверка
        // NEEDS_REDRAW шла ПОСЛЕ старта итерации — но пока терминал
        // передан sudo/ask_user (см. tty_guard.rs), этот цикл как
        // отдельная tokio-задача ПРОДОЛЖАЛ вызывать terminal.draw()
        // каждые ~120мс по тику И читать EventStream — то есть ДВА
        // потребителя одного терминала одновременно. Теперь при
        // is_suspended() цикл вообще не трогает терминал: не рисует,
        // не слушает события — просто ждёт и проверяет флаг снова.
        if crate::tty_guard::is_suspended() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }

        // ИСПРАВЛЕНО (дедлок разрешений): статистика/модель читаются через
        // try_lock, а вот очередь разрешений опрашивается по СВОЕМУ клону
        // Arc — без лока агента. Раньше запрос разрешения, пришедший во
        // время занятого агента, никогда не показывался: try_lock не
        // проходил ровно тогда, когда вопрос ждал ответа.
        if let Ok(a) = agent.try_lock() {
            let (tokens, llm_calls, tool_calls) = a.quick_stats();
            cached_stats = (tokens, llm_calls, tool_calls);
            let (p, m) = a.current_provider_and_model();
            cached_model = (p.to_string(), m.to_string());
        }
        cached_queue_len = queue.len().await;

        // Очередь разрешений: новый запрос → вопрос в историю чата.
        // Инструмент в это время асинхронно ждёт ответа — TUI жив,
        // остальные инструменты хода работают.
        match permissions.current_pending() {
            Some(p) => {
                perm_waiting = true;
                if last_perm_shown != Some(p.id) {
                    history.push(HistoryEntry::new(Role::System, format!(
                        "⚠️ Требуется разрешение — {}\n{}\n{}\n\nРазрешить один раз: /allow once   Разрешить всегда для '{}': /allow always\nЗапретить: /deny",
                        p.tool, p.summary, p.reason, p.always_key
                    )));
                    last_perm_shown = Some(p.id);
                    scroll_from_bottom = 0;
                }
            }
            None => {
                perm_waiting = false;
                last_perm_shown = None;
            }
        }

        // После возврата из приостановки (см. выше) — ratatui не знает,
        // что физический экран поменялся в обход него, и его
        // инкрементальная отрисовка красила бы только "разницу" со
        // своим старым кадром. clear() сбрасывает внутренний кэш и
        // заставляет перерисовать экран целиком.
        if crate::tty_guard::take_needs_redraw() {
            let _ = terminal.clear();
        }

        terminal.draw(|f| {
            draw_ui(
                f,
                &history,
                &input,
                cursor,
                busy || queued_pending > 0,
                spinner_frame,
                scroll_from_bottom,
                session_start.elapsed(),
                &cached_model,
                cached_stats,
                perm_waiting,
                cached_queue_len,
                &stream_buf,
                &mut plain_lines,
                &mut chat_geom,
                sel_anchor,
                sel_end,
            )
        })?;

        tokio::select! {
            // Основной путь graceful shutdown при получении SIGINT извне
            // (в raw-режиме терминал обычно НЕ шлёт SIGINT на Ctrl+C — тот
            // перехватывается ниже как обычная клавиша — но сигнал может
            // прийти, например, от `kill -INT` или из другого источника).
            _ = tokio::signal::ctrl_c() => {
                graceful_save(&agent).await;
                break;
            }
            _ = tick.tick() => {
                if busy || queued_pending > 0 {
                    spinner_frame = (spinner_frame + 1) % SPINNER_FRAMES.len();
                }
                // Автоскролл при выделении у края панели (как в терминале:
                // тянешь выделение к верху — текст листается вверх).
                if mouse_down && drag_edge != 0 {
                    scroll_from_bottom = if drag_edge < 0 {
                        (scroll_from_bottom + SCROLL_STEP).min(usize::MAX)
                    } else {
                        scroll_from_bottom.saturating_sub(SCROLL_STEP)
                    };
                    // Конец выделения следует за прокруткой, чтобы полоса
                    // "растягивалась" вместе с содержимым.
                    if let Some(end) = sel_end.as_mut() {
                        if drag_edge < 0 { end.0 += SCROLL_STEP; } else { end.0 = end.0.saturating_sub(SCROLL_STEP); }
                    }
                }
            }
            Some(msg) = rx.recv() => {
                match msg {
                    ReplMsg::Done(entry) => {
                        busy = false;
                        stream_buf.clear();
                        let is_reply = entry.role == Role::Assistant;
                        history.push(entry);
                        scroll_from_bottom = 0;

                        // Автосохранение после ответа агента (см. комментарий
                        // в исходной версии этого блока).
                        if is_reply {
                            let agent_for_save = agent.clone();
                            tokio::spawn(async move {
                                if let Ok(a) = agent_for_save.try_lock() {
                                    let msgs = a.snapshot_messages().await;
                                    let _ = session_store::save(&msgs);
                                }
                            });
                        }
                    }
                    ReplMsg::TurnDone(entry) => {
                        // Завершён ход из ОЧЕРЕДИ: снимаем один элемент
                        // счётчика; живая панель стрима гаснет.
                        stream_buf.clear();
                        queued_pending = queued_pending.saturating_sub(1);
                        let _is_reply = entry.role != Role::Error;
                        history.push(entry);
                        scroll_from_bottom = 0;
                        // Автосохранение после каждого завершённого хода.
                        let agent_for_save = agent.clone();
                        tokio::spawn(async move {
                            if let Ok(a) = agent_for_save.try_lock() {
                                let msgs = a.snapshot_messages().await;
                                let _ = session_store::save(&msgs);
                            }
                        });
                    }
                    ReplMsg::Update(entry) => {
                        history.push(entry);
                        scroll_from_bottom = 0;
                    }
                    ReplMsg::StreamDelta(chunk) => {
                        stream_buf.push_str(&chunk);
                        // Держим хвост, чтобы панель не росла бесконечно.
                        let chars: Vec<char> = stream_buf.chars().collect();
                        if chars.len() > 1500 {
                            let cut = chars.len() - 1200;
                            stream_buf = chars[cut..].iter().collect();
                        }
                        scroll_from_bottom = 0;
                    }
                }
            }
            maybe_event = events.next() => {
                let Some(Ok(event)) = maybe_event else { continue; };

                match event {
                    Event::Mouse(mouse) => {
                        // ── Нативное выделение мышью ──
                        // Пока включён захват мыши, ЛКМ обрабатываем САМИ:
                        // зажать → тянуть → отпустить = копирование (OSC52).
                        // Колесо — прокрутка чата. F2 отключает захват
                        // целиком (запасной путь для терминалов без OSC52).
                        match mouse.kind {
                            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                if let Some((gx, gy, gw, gh, start_idx)) = chat_geom {
                                    let inner_x = gx + 1;
                                    let inner_y = gy + 1;
                                    let inner_w = gw.saturating_sub(2) as usize;
                                    let h = gh as usize;
                                    if mouse.row >= inner_y && mouse.column >= inner_x && mouse.row < inner_y + gh {
                                        let row = (mouse.row - inner_y) as usize;
                                        let col = ((mouse.column - inner_x) as usize).min(inner_w.saturating_sub(1));
                                        let line_idx = start_idx.saturating_add(row.min(h.saturating_sub(1)));
                                        sel_anchor = Some((line_idx, col));
                                        sel_end = sel_anchor;
                                        mouse_down = true;
                                        drag_edge = 0;
                                    }
                                }
                            }
                            MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                                if mouse_down {
                                    if let Some((gx, gy, gw, gh, start_idx)) = chat_geom {
                                        let inner_x = gx + 1;
                                        let inner_y = gy + 1;
                                        let inner_w = gw.saturating_sub(2) as usize;
                                        let h = gh as usize;
                                        let row_u = mouse.row.max(inner_y).saturating_sub(inner_y) as usize;
                                        let col = (((mouse.column - inner_x) as usize)).min(inner_w);
                                        let line_idx = start_idx.saturating_add(row_u.min(h.saturating_sub(1)));
                                        sel_end = Some((line_idx, col));
                                        // У края? — тик сам листает историю.
                                        drag_edge = if mouse.row <= inner_y { -1 }
                                            else if mouse.row >= inner_y + gh.saturating_sub(1) { 1 }
                                            else { 0 };
                                    }
                                }
                            }
                            MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                                mouse_down = false;
                                drag_edge = 0;
                                if let (Some(a), Some(b)) = (sel_anchor, sel_end) {
                                    if a == b {
                                        // Простой клик — сброс выделения.
                                        sel_anchor = None;
                                        sel_end = None;
                                    } else if !plain_lines.is_empty() {
                                        let text = extract_selection(&plain_lines, a, b);
                                        let method = copy_via_osc52(&text);
                                        history.push(HistoryEntry::new(
                                            Role::System,
                                            format!("✅ Скопировано {} симв. в буфер ОС ({})", text.chars().count(), method),
                                        ));
                                        scroll_from_bottom = 0;
                                        sel_anchor = None;
                                        sel_end = None;
                                    }
                                }
                            }
                            MouseEventKind::ScrollUp => {
                                scroll_from_bottom = scroll_from_bottom.saturating_add(SCROLL_STEP);
                            }
                            MouseEventKind::ScrollDown => {
                                scroll_from_bottom = scroll_from_bottom.saturating_sub(SCROLL_STEP);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    // ИСПРАВЛЕНО (жалоба "вставляешь — тоже баги"): без
                    // bracketed paste терминал отправляет вставляемый
                    // текст как БЫСТРЫЙ поток обычных нажатий. Если в
                    // буфере многострочный текст (команда с \n, код,
                    // путь из блокнота) — каждый \n приходил как Enter,
                    // и ввод отправлялся КУСКАМИ посреди вставки; часть
                    // символов могла потеряться на границе кадров.
                    // EnableBracketedPaste (включён в run()) превращает
                    // всю вставку в ОДНО событие Event::Paste. Переносы
                    // строк заменяем пробелами: модель ввода здесь
                    // однострочная, а случайный ранний Enter от текста
                    // из буфера — худший из возможных исходов.
                    Event::Paste(text) => {
                        for ch in text.chars() {
                            match ch {
                                '\n' | '\r' | '\t' => {
                                    input.insert(cursor, ' ');
                                    cursor += 1;
                                }
                                c if c.is_control() => {}
                                c => {
                                    input.insert(cursor, c);
                                    cursor += 1;
                                }
                            }
                        }
                    }
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }

                        // Ctrl+C в raw-режиме приходит как обычное событие клавиши.
                        // Если активно выделение мышью — Ctrl+C КОПИРУЕТ его
                        // (терминальная привычка), а не выходит; выход —
                        // повторный Ctrl+C без выделения.
                        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            if let (Some(a), Some(b)) = (sel_anchor, sel_end) {
                                if a != b && !plain_lines.is_empty() {
                                    let text = extract_selection(&plain_lines, a, b);
                                    let method = copy_via_osc52(&text);
                                    history.push(HistoryEntry::new(
                                        Role::System,
                                        format!("✅ Скопировано {} симв. в буфер ОС ({})", text.chars().count(), method),
                                    ));
                                    scroll_from_bottom = 0;
                                    sel_anchor = None;
                                    sel_end = None;
                                    continue;
                                }
                            }
                            graceful_save(&agent).await;
                            break;
                        }

                        match key.code {
                            KeyCode::Enter => {
                                let line: String = input.iter().collect::<String>().trim().to_string();
                                input.clear();
                                cursor = 0;
                                history_cursor = None;
                                scroll_from_bottom = 0;
                                if line.is_empty() {
                                    continue;
                                }

                                if line == "/voice" {
                                    input_history.push(line);
                                    run_voice_mode(terminal, &agent, &mut history).await;
                                    continue;
                                }

                                if let Some(goal_text) = line.strip_prefix("/goal ").map(|s| s.trim().to_string()) {
                                    if busy || queued_pending > 0 {
                                        history.push(HistoryEntry::new(Role::System, "Агент ещё занят (или очередь не пуста) — /goal запустится позже вручную."));
                                        continue;
                                    }
                                    if goal_text.is_empty() {
                                        history.push(HistoryEntry::new(Role::System, "Использование: /goal <описание цели>"));
                                        continue;
                                    }
                                    input_history.push(line.clone());
                                    history.push(HistoryEntry::new(Role::User, format!("/goal {}", goal_text)));
                                    busy = true;
                                    spinner_frame = 0;

                                    let agent_clone = agent.clone();
                                    let tx_clone = tx.clone();
                                    tokio::spawn(async move {
                                        let mut a = agent_clone.lock().await;
                                        let mut gm = GoalModeAgent::new(&mut a);
                                        let tx_progress = tx_clone.clone();
                                        gm.on_progress(move |msg| {
                                            let _ = tx_progress.send(ReplMsg::Update(HistoryEntry::new(Role::System, msg.to_string())));
                                        });
                                        let report = gm.pursue(&goal_text, None).await;
                                        // goal_mode на время /goal ставит СВОЙ
                                        // tool_hook и в конце снимает его в None —
                                        // возвращаем наш хук живого лога инструментов.
                                        a.set_tool_hook(Some(tool_progress_hook(tx_clone.clone())));
                                        let _ = tx_clone.send(ReplMsg::Done(HistoryEntry::new(Role::Assistant, report)));
                                    });
                                    continue;
                                }

                                if let Some(cmd) = line.strip_prefix('/') {
                                    input_history.push(line.clone());
                                    if handle_command(cmd, &agent, &permissions, &mut history, &tx, busy, &mut busy, &mut pending_workdir).await {
                                        graceful_save(&agent).await;
                                        break;
                                    }
                                    continue;
                                }
                                input_history.push(line.clone());
                                history.push(HistoryEntry::new(Role::User, line.clone()));
                                scroll_from_bottom = 0;

                                // ОЧЕРЕДЬ ЗАПРОСОВ (как в opencode): вместо
                                // прямого tokio::spawn с chat() кладём ход в
                                // общую FIFO — воркер выполнит его после
                                // текущего (в т.ч. из Telegram). Позиция > 0
                                // значит "перед вами ещё N ходов".
                                let source = "tui".to_string();
                                let (queue_pos, done_rx) = queue
                                    .push(crate::request_queue::QueuedTurn {
                                        text: line.clone(),
                                        source,
                                    })
                                    .await;
                                queued_pending += 1;
                                if queue_pos > 0 {
                                    history.push(HistoryEntry::new(
                                        Role::System,
                                        format!("📥 В очереди — позиция {}. Ответ придёт автоматически.", queue_pos + 1),
                                    ));
                                }

                                let tx_clone = tx.clone();
                                tokio::spawn(async move {
                                    let reply = match tokio::time::timeout(Duration::from_secs(960), done_rx).await {
                                        Ok(Ok(text)) => text,
                                        Ok(Err(_)) => "Ход отменён (воркер очереди завершился).".to_string(),
                                        Err(_) => "⏱️ Превышено время ожидания ответа (16 минут).".to_string(),
                                    };
                                    let _ = tx_clone.send(ReplMsg::TurnDone(HistoryEntry::new(reply_role(&reply), reply)));
                                });
                            }
                            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // ИСПРАВЛЕНО: главный источник бага "hhhh" —
                                // на многих терминалах физический Backspace
                                // шлёт Ctrl+H (0x08), а не KeyCode::Backspace.
                                // Раньше ЛЮБОЙ KeyCode::Char(c) слепо
                                // вставлялся в строку — Ctrl+H вставлял
                                // букву 'h' вместо удаления символа.
                                match c.to_ascii_lowercase() {
                                    'h' => {
                                        if cursor > 0 {
                                            input.remove(cursor - 1);
                                            cursor -= 1;
                                        }
                                    }
                                    'u' => {
                                        // Ctrl+U — стереть от начала строки до курсора (readline-конвенция).
                                        input.drain(0..cursor);
                                        cursor = 0;
                                    }
                                    'k' => {
                                        // Ctrl+K — стереть от курсора до конца строки.
                                        input.truncate(cursor);
                                    }
                                    'w' => {
                                        // Ctrl+W — удалить предыдущее слово.
                                        let start = cursor;
                                        let mut i = cursor;
                                        while i > 0 && input[i - 1] == ' ' {
                                            i -= 1;
                                        }
                                        while i > 0 && input[i - 1] != ' ' {
                                            i -= 1;
                                        }
                                        input.drain(i..start);
                                        cursor = i;
                                    }
                                    'a' => cursor = 0,
                                    'e' => cursor = input.len(),
                                    // Остальные control-комбинации (Ctrl+другое) —
                                    // игнорируем, а не вставляем как текст: именно
                                    // слепая вставка контрольных символов и породила баг.
                                    _ => {}
                                }
                            }
                            KeyCode::Char(c) => {
                                input.insert(cursor, c);
                                cursor += 1;
                            }
                            KeyCode::Backspace => {
                                if cursor > 0 {
                                    input.remove(cursor - 1);
                                    cursor -= 1;
                                }
                            }
                            KeyCode::Delete => {
                                if cursor < input.len() {
                                    input.remove(cursor);
                                }
                            }
                            KeyCode::Left => {
                                cursor = cursor.saturating_sub(1);
                            }
                            KeyCode::Right => {
                                cursor = (cursor + 1).min(input.len());
                            }
                            KeyCode::Home => cursor = 0,
                            KeyCode::End => cursor = input.len(),
                            KeyCode::Up => {
                                if !input_history.is_empty() {
                                    let idx = match history_cursor {
                                        Some(i) if i > 0 => i - 1,
                                        Some(i) => i,
                                        None => input_history.len() - 1,
                                    };
                                    history_cursor = Some(idx);
                                    input = input_history[idx].chars().collect();
                                    cursor = input.len();
                                }
                            }
                            KeyCode::Down => {
                                if let Some(i) = history_cursor {
                                    if i + 1 < input_history.len() {
                                        history_cursor = Some(i + 1);
                                        input = input_history[i + 1].chars().collect();
                                    } else {
                                        history_cursor = None;
                                        input.clear();
                                    }
                                    cursor = input.len();
                                }
                            }
                            KeyCode::PageUp => {
                                scroll_from_bottom = scroll_from_bottom.saturating_add(10);
                            }
                            KeyCode::PageDown => {
                                scroll_from_bottom = scroll_from_bottom.saturating_sub(10);
                            }
                            KeyCode::Esc => {
                                // ДВОЙНАЯ РОЛЬ ESC: если агент занят —
                                // ПРЕРВАТЬ ХОД (как в opencode/CC); иначе —
                                // очистить ввод/выделение, как раньше.
                                if busy {
                                    crate::interrupt::interrupt();
                                    history.push(HistoryEntry::new(
                                        Role::System,
                                        "⏹️ Прерываю ход… (остановка сработает между шагами агента)",
                                    ));
                                    scroll_from_bottom = 0;
                                } else {
                                    input.clear();
                                    cursor = 0;
                                    history_cursor = None;
                                    sel_anchor = None;
                                    sel_end = None;
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // WATCHDOG: TUI больше не активен — глобальные тикеры снова
    // единственный источник heartbeat.
    crate::watchdog::set_tui_active(false);

    Ok(())
}

/// `/voice` — приостанавливает TUI (raw mode выключается, экран
/// возвращается в обычный режим) и запускает голосовой цикл на том же
/// агенте. См. фикс в voice_interface.rs (VoiceInterface теперь
/// разделяет общий Arc<Mutex<Agent>>, а не владеет отдельным Agent).
///
/// ЧЕСТНОЕ ОГРАНИЧЕНИЕ: `VoiceInterface::run_loop()` читает stdin через
/// обычный блокирующий `std::io::stdin().read_line()`, а не через
/// `tokio::io` — вызов здесь блокирует один поток исполнителя tokio на
/// время голосового режима. Так как `#[tokio::main]` в этом проекте
/// поднимает многопоточный executor (несколько worker-потоков), это не
/// приводит к дедлоку, но и не идеальная практика — если когда-нибудь
/// понадобится единственный поток исполнителя, `run_loop()` надо будет
/// переписать на `tokio::task::spawn_blocking` для чтения stdin.
async fn run_voice_mode(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    agent: &Arc<Mutex<Agent>>,
    history: &mut Vec<HistoryEntry>,
) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste);
    let _ = terminal.show_cursor();
    println!("\n(Голосовой режим — говорите 'exit' или 'quit', чтобы вернуться в TUI)\n");
    let _ = io::stdout().flush();

    let voice = crate::voice_interface::create_voice_interface(agent.clone(), None, None, None, None, false);
    voice.run_loop().await;

    let _ = enable_raw_mode();
    let mut stdout = io::stdout();
    let _ = execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste);
    let _ = terminal.clear();
    history.push(HistoryEntry::new(Role::System, "Возврат из голосового режима."));
}

/// Хук прогресса инструментов: Agent::chat вызывает его после каждого
/// выполненного инструмента, мы печатаем строку в чат — пользователь
/// видит работу агента в реальном времени, а не только спиннер.
fn tool_progress_hook(
    tx: mpsc::UnboundedSender<ReplMsg>,
) -> Box<dyn Fn(&str, &serde_json::Value, bool, &str) + Send + Sync> {
    Box::new(move |name, args, success, output| {
        // КРАСИВЫЕ ДИФФЫ (как в opencode): file_edit/file_write несут
        // unified-diff в выводе — выносим его ОТДЕЛЬНОЙ записью Role::Diff,
        // чтобы рендер раскрасил +зелёным/−красным построчно.
        if success && matches!(name, "file_edit" | "file_write") {
            if let Some(path) = args.get("file_path").and_then(|v| v.as_str()) {
                // Ищем блок диффа в выводе: от "Изменения (+N/−M):" до
                // конца вывода (дифф — последний блок).
                if let Some(pos) = output.find("Изменения (+") {
                    let after_marker = &output[pos + "Изменения (+".len()..];
                    // пропускаем "+N/−M):" до первого \n
                    let diff_body = after_marker
                        .find('\n')
                        .map(|i| &after_marker[i + 1..])
                        .unwrap_or("");
                    let diff_body = diff_body.trim_end();
                    if !diff_body.is_empty() {
                        let header = HistoryEntry::new(
                            Role::System,
                            format!("📝 {} — {} (+{}/−{})", name, path,
                                diff_body.lines().filter(|l| l.starts_with('+')).count(),
                                diff_body.lines().filter(|l| l.starts_with('-')).count()),
                        );
                        let _ = tx.send(ReplMsg::Update(header));
                        let diff_entry = HistoryEntry::new(
                            Role::Diff,
                            format!("--- {}\n{}", path, diff_body),
                        );
                        let _ = tx.send(ReplMsg::Update(diff_entry));
                        return;
                    }
                }
                // Новый файл — короткая запись с числом строк.
                let entry = HistoryEntry::new(Role::System, format!("📝 {} — создан", path));
                let _ = tx.send(ReplMsg::Update(entry));
                return;
            }
        }
        // Ошибки инструментов — КРАСНЫЕ (Role::Error), а не серые: в TUI
        // провал должен бросаться в глаза так же, как в Telegram.
        let entry = if success {
            HistoryEntry::new(Role::System, format!("🔧 {} — ок", name))
        } else {
            let first_line = output
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .chars()
                .take(160)
                .collect::<String>();
            HistoryEntry::new(
                Role::Error,
                format!(
                    "🔧 {} — ошибка: {}",
                    name,
                    if first_line.is_empty() { "(без подробностей, см. /stats → логи)" } else { &first_line }
                ),
            )
        };
        let _ = tx.send(ReplMsg::Update(entry));
    })
}

/// Классификация финального ответа агента для ОТРИСОВКИ: сообщения об
/// ошибках должны быть красными с меткой "Ошибка", а не зелёным
/// "Гефест" — раньше любая ошибка LLM выглядела как обычный ответ,
/// и было непонятно, что что-то пошло не так.
fn reply_role(text: &str) -> Role {
    let t = text.trim_start();
    if t.starts_with("LLM error")
        || t.starts_with("⏱️")
        || t.starts_with("🛑")
        || t.starts_with("❌")
    {
        Role::Error
    } else {
        Role::Assistant
    }
}

/// Дефолтная модель для провайдера при переключении через `/provider`,
/// если пользователь не указал модель явно (`/provider anthropic gpt-4o`
/// всё равно можно — второй аргумент переопределяет).
fn default_model_for(provider: &LLMProvider) -> &'static str {
    match provider {
        LLMProvider::Anthropic => "claude-3-5-sonnet-20241022",
        LLMProvider::OpenAI => "gpt-4o",
        LLMProvider::OpenRouter => "qwen/qwen-2.5-coder-32b-instruct",
        LLMProvider::Ollama => "llama3.2:3b",
        LLMProvider::KoboldCpp => "local-model",
        LLMProvider::LlamaServer => "local-model",
        LLMProvider::Custom => "custom-model",
    }
}

/// Обработать слэш-команду. Возвращает `true`, если REPL должен завершиться.
/// `busy_out` — команды, которые сами ставят агента "занятым" (спавнят
/// фоновую задачу), выставляют его в `true`; вызывающий код (`run_app`)
/// использует именно это значение, а не то, что было на входе.
/// `pending_workdir` — кандидат в рабочие директории, ожидающий
/// подтверждения доверия (`/work_dir ok`).
/// `permissions` передаётся отдельно от `Agent`: ход очереди удерживает
/// лок агента, пока инструмент ждёт решения человека.
#[allow(clippy::too_many_arguments)]
async fn handle_command(
    cmd: &str,
    agent: &Arc<Mutex<Agent>>,
    permissions: &Arc<crate::permissions::PermissionManager>,
    history: &mut Vec<HistoryEntry>,
    tx: &mpsc::UnboundedSender<ReplMsg>,
    busy: bool,
    busy_out: &mut bool,
    pending_workdir: &mut Option<std::path::PathBuf>,
) -> bool {
    let cmd = cmd.trim();

    // /work_dir — рабочая директория инструментов с подтверждением
    // доверия, как в Claude Code ("Do you trust the files in this
    // folder?"). Смена директории даёт агенту доступ на запись в новое
    // место — это осознанное решение человека, а не то, что LLM может
    // сделать сама (у LLM для этого инструмента нет и не должно быть:
    // см. workdir.rs).
    if cmd == "work_dir" || cmd == "workdir" {
        let cur = match agent.try_lock() {
            Ok(a) => a.work_dir().display().to_string(),
            Err(_) => "(агент занят, попробуйте ещё раз)".to_string(),
        };
        let pending_note = match pending_workdir {
            Some(p) => format!("\nОжидает подтверждения: {} (/work_dir ok или /work_dir cancel)", p.display()),
            None => String::new(),
        };
        history.push(HistoryEntry::new(Role::System, format!(
            "Рабочая директория инструментов: {}\nСменить: /work_dir <путь> — спросит подтверждение доверия.\nВсе инструменты (file_*, bash, git, run_tests...) сразу начнут работать в новой папке.{}",
            cur, pending_note
        )));
        return false;
    }
    if let Some(arg) = cmd.strip_prefix("work_dir ").map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| cmd.strip_prefix("workdir ").map(str::trim).filter(|s| !s.is_empty()))
    {
        let arg = arg.trim_matches('"');
        match arg {
            "ok" | "yes" | "да" => {
                match pending_workdir.take() {
                    Some(path) => match agent.try_lock() {
                        Ok(a) => {
                            a.set_work_dir(path.clone());
                            match crate::config::AgentConfig::set_saved_work_dir(&path) {
                                Ok(_) => history.push(HistoryEntry::new(Role::System, format!(
                                    "✅ Рабочая директория: {}\nСохранено в {}. Инструменты теперь работают здесь.",
                                    path.display(),
                                    crate::config::AgentConfig::config_file_path()
                                ))),
                                Err(e) => history.push(HistoryEntry::new(Role::Error, format!(
                                    "Директория {} применена, но НЕ сохранена в конфиг: {}",
                                    path.display(), e
                                ))),
                            }
                        }
                        Err(_) => {
                            *pending_workdir = Some(path);
                            history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте подтвердить ещё раз через секунду."));
                        }
                    },
                    None => history.push(HistoryEntry::new(Role::System, "Нет ожидающего подтверждения. Сначала: /work_dir <путь>")),
                }
                return false;
            }
            "cancel" | "no" | "нет" => {
                if pending_workdir.take().is_some() {
                    history.push(HistoryEntry::new(Role::System, "Смена рабочей директории отменена."));
                } else {
                    history.push(HistoryEntry::new(Role::System, "Нечего отменять — нет ожидающего подтверждения."));
                }
                return false;
            }
            _ => {}
        }
        // Шаг 1: путь-кандидат. Проверяем существование ДО вопроса,
        // чтобы не подтверждать то, что не существует.
        let expanded = crate::workdir::expand_home(arg);
        match expanded.canonicalize() {
            Ok(path) if path.is_dir() => {
                let file_count = std::fs::read_dir(&path).map(|d| d.count()).unwrap_or(0);
                *pending_workdir = Some(path.clone());
                history.push(HistoryEntry::new(Role::System, format!(
                    "Новая рабочая директория: {}\n(содержимое: {} записей)\n\nДоверия вопрос: агент получит доступ на чтение/запись файлов в этой папке.\nПодтвердить: /work_dir ok   Отменить: /work_dir cancel",
                    path.display(), file_count
                )));
            }
            Ok(_) => history.push(HistoryEntry::new(Role::Error, format!("{} — это не директория.", expanded.display()))),
            Err(e) => history.push(HistoryEntry::new(Role::Error, format!("{}: {}", expanded.display(), e))),
        }
        return false;
    }

    // session.rs: именованные чекпоинты — в отличие от /save (только
    // один слот "последняя сессия"), тут можно хранить несколько
    // сохранённых диалогов под разными именами.
    if let Some(name) = cmd.strip_prefix("checkpoint ") {
        return handle_checkpoint_save(name.trim(), agent, history).await;
    }
    if cmd == "checkpoint" {
        history.push(HistoryEntry::new(Role::System, "Использование: /checkpoint <имя>"));
        return false;
    }
    if cmd == "checkpoints" {
        let mgr = crate::session::SessionManager::new();
        let sessions = mgr.list_sessions(20);
        let text = if sessions.is_empty() {
            "Сохранённых чекпоинтов нет.".to_string()
        } else {
            let mut lines = vec!["Чекпоинты:".to_string()];
            for s in &sessions {
                lines.push(format!(
                    "  {} — {} сообщений — {}",
                    s.get("id").map(|s| s.as_str()).unwrap_or("?"),
                    s.get("messages").map(|s| s.as_str()).unwrap_or("?"),
                    s.get("summary").map(|s| s.as_str()).unwrap_or(""),
                ));
            }
            lines.join("\n")
        };
        history.push(HistoryEntry::new(Role::System, text));
        return false;
    }
    if let Some(id) = cmd.strip_prefix("restore ") {
        let id = id.trim();
        let mgr = crate::session::SessionManager::new();
        match mgr.load(id) {
            Ok(messages) => match agent.try_lock() {
                Ok(mut a) => {
                    let count = messages.len();
                    for m in &messages {
                        history.push(HistoryEntry::new(llm_role_to_repl_role(&m.role), m.content.clone()));
                    }
                    let llm_messages: Vec<LLMMessage> = messages
                        .into_iter()
                        .map(|m| LLMMessage { role: m.role, content: m.content, tool_calls: None, tool_call_id: None })
                        .collect();
                    a.load_messages(llm_messages).await;
                    history.push(HistoryEntry::new(Role::System, format!("── Восстановлен чекпоинт '{}': {} сообщений ──", id, count)));
                }
                Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")),
            },
            Err(e) => history.push(HistoryEntry::new(Role::Error, format!("Не удалось восстановить чекпоинт '{}': {}", id, e))),
        }
        return false;
    }

    // Провайдер/модель — сетевые операции, поэтому спавним фоновую
    // задачу вместо await прямо здесь (см. пункт 3 в шапке файла: раньше
    // это блокировало весь TUI на время запроса).
    if cmd == "provider" || cmd == "providers" {
        handle_provider_list(history);
        return false;
    }
    if let Some(arg) = cmd.strip_prefix("provider ") {
        let arg = arg.trim();
        // Профили — по запросу пользователя: "хочу поддержку нескольких
        // провайдеров в конфиге" (свой ik-llama.cpp + Ollama и т.п.
        // одновременно, переключение одной командой). См. config.rs.
        if let Some(name) = arg.strip_prefix("save ") {
            return handle_provider_save(name.trim(), agent, history).await;
        }
        if let Some(name) = arg.strip_prefix("use ") {
            if busy {
                history.push(HistoryEntry::new(Role::System, "Агент обрабатывает предыдущий запрос — дождитесь ответа перед сменой провайдера."));
                return false;
            }
            spawn_provider_use(name.trim(), agent, tx);
            *busy_out = true;
            return false;
        }
        if let Some(name) = arg.strip_prefix("forget ") {
            return handle_provider_forget(name.trim(), history).await;
        }
        if busy {
            history.push(HistoryEntry::new(Role::System, "Агент обрабатывает предыдущий запрос — дождитесь ответа перед сменой провайдера."));
            return false;
        }
        spawn_provider_switch(arg, agent, tx);
        *busy_out = true;
        return false;
    }
    if cmd == "model" || cmd == "models" {
        if busy {
            history.push(HistoryEntry::new(Role::System, "Агент обрабатывает предыдущий запрос — дождитесь ответа."));
            return false;
        }
        spawn_model_list(agent, tx);
        *busy_out = true;
        return false;
    }
    if let Some(arg) = cmd.strip_prefix("model ") {
        if busy {
            history.push(HistoryEntry::new(Role::System, "Агент обрабатывает предыдущий запрос — дождитесь ответа перед сменой модели."));
            return false;
        }
        spawn_model_switch(arg.trim(), agent, tx);
        *busy_out = true;
        return false;
    }

    // ИСПРАВЛЕНО (жалоба "дал агенту токен телеграма, а он не хочет
    // запускать бота, думает-думает, не помню флаг запуска"): у агента
    // физически не было И НЕ МОГЛО быть инструмента "перезапусти меня с
    // другими флагами" — LLM пыталась что-то придумать в ответ на
    // просьбу в чате и просто зацикливалась. Теперь это отдельная
    // команда REPL, не требующая перезапуска процесса вообще — бот
    // стартует в фоне на ТОМ ЖЕ агенте (тот же диалог/контекст).
    if cmd == "telegram" {
        history.push(HistoryEntry::new(
            Role::System,
            "Использование:\n  /telegram save <токен> — запомнить токен (файл ~/.hephaestus/telegram_token.txt)\n  /telegram start [токен] — запустить бота в фоне (использует сохранённый токен, если не указан)",
        ));
        return false;
    }
    if let Some(token) = cmd.strip_prefix("telegram save ") {
        let token = token.trim();
        if token.is_empty() {
            history.push(HistoryEntry::new(Role::System, "Использование: /telegram save <токен>"));
        } else {
            match crate::telegram_bot::save_token(token) {
                Ok(_) => history.push(HistoryEntry::new(Role::System, "✅ Токен сохранён.")),
                Err(e) => history.push(HistoryEntry::new(Role::Error, format!("Не удалось сохранить токен: {}", e))),
            }
        }
        return false;
    }
    if let Some(rest) = cmd.strip_prefix("telegram start") {
        let explicit_token = rest.trim();
        let token = if explicit_token.is_empty() {
            crate::telegram_bot::load_token()
        } else {
            let _ = crate::telegram_bot::save_token(explicit_token);
            Some(explicit_token.to_string())
        };
        match token {
            Some(token) => {
                let allowed_chat_id = std::env::var("TELEGRAM_ALLOWED_CHAT_ID").ok().and_then(|v| v.parse::<i64>().ok());
                let bot = crate::telegram_bot::TelegramBot::new(token, agent.clone(), allowed_chat_id);
                tokio::spawn(async move { bot.run().await; });
                history.push(HistoryEntry::new(
                    Role::System,
                    "✅ Telegram-бот запущен в фоне на этом же диалоге. Логи — в файле логов (см. /stats), ошибки сети сюда не выводятся.",
                ));
            }
            None => history.push(HistoryEntry::new(
                Role::Error,
                "Токен не найден. Сначала: /telegram save <токен> (получить у @BotFather в Telegram), либо /telegram start <токен> сразу.",
            )),
        }
        return false;
    }

    // /allow /deny — ответы на запросы разрешений ("как в opencode"):
    // /allow once — один раз, /allow always — всегда для этого ключа,
    // /deny — отказ. Пока инструмент ждёт ответа, агент приостановлен
    // ТОЛЬКО на этом вызове: TUI и остальные инструменты работают.
    if cmd == "allow" || cmd == "always" || cmd.starts_with("allow ") || cmd.starts_with("always") || cmd == "deny" || cmd.starts_with("deny ") {
        let choice = match cmd.trim() {
            "allow" | "allow once" | "allow 1" => Some(crate::permissions::Choice::Once),
            "always" | "allow always" | "allow 2" => Some(crate::permissions::Choice::Always),
            "deny" | "deny no" | "allow never" | "deny 3" => Some(crate::permissions::Choice::Deny),
            _ => None,
        };
        let Some(choice) = choice else {
            history.push(HistoryEntry::new(Role::System, "Использование: /allow once | /allow always | /deny"));
            return false;
        };
        // Нельзя брать `agent.try_lock()` здесь: воркер держит этот лок на
        // всём протяжении chat(), включая await запроса разрешения. Сам
        // PermissionManager потокобезопасен и был заранее клонирован именно
        // для ответа без участия Agent.
        if permissions.resolve(choice) {
            history.push(HistoryEntry::new(Role::System, match choice {
                crate::permissions::Choice::Once => "✅ Разрешено (один раз).",
                crate::permissions::Choice::Always => "✅ Разрешено всегда для этого ключа (до конца сессии, см. /permissions).",
                crate::permissions::Choice::Deny => "🚫 Запрещено.",
            }));
        } else {
            history.push(HistoryEntry::new(Role::System, "Нет ожидающих запросов разрешения."));
        }
        return false;
    }

    if cmd == "permissions" || cmd.starts_with("permissions ") {
        if cmd.trim_end() == "permissions clear" {
            permissions.clear_always();
            // Сессийные правила движка («всегда») тоже забываем; правила
            // из config.toml и встроенные дефолты остаются.
            if let Ok(a) = agent.try_lock() {
                a.engine.clear_session_rules();
            }
            history.push(HistoryEntry::new(Role::System, "Все 'всегда разрешено' забыты (правила config.toml остались)."));
            return false;
        }
        if let Some(add_args) = cmd.strip_prefix("permissions add ") {
            // /permissions add <tool> <pattern> <allow|ask|deny>
            let parts: Vec<&str> = add_args.split_whitespace().collect();
            let [tool, pattern, action] = parts.as_slice() else {
                history.push(HistoryEntry::new(Role::Error, "Формат: /permissions add <tool> <pattern> <allow|ask|deny>\nПример: /permissions add bash \"cargo *\" allow"));
                return false;
            };
            let action = match *action {
                "allow" => crate::permissions::RuleAction::Allow,
                "ask" => crate::permissions::RuleAction::Ask,
                "deny" => crate::permissions::RuleAction::Deny,
                other => {
                    history.push(HistoryEntry::new(Role::Error, format!("Неизвестное действие '{other}' (allow|ask|deny)")));
                    return false;
                }
            };
            if let Ok(a) = agent.try_lock() {
                a.engine.add_session_rule(tool, pattern, action);
                history.push(HistoryEntry::new(Role::System, format!("✅ Правило добавлено: {tool} '{pattern}' → {} (действует до конца сессии; для постоянного — [[permissions.rules]] в config.toml)", action.as_str())));
            } else {
                history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже."));
            }
            return false;
        }
        let mut lines = String::new();
        // 1) Правила движка: дефолты + конфиг + сессия.
        if let Ok(a) = agent.try_lock() {
            let rules = a.engine.list_rules();
            lines.push_str(&format!("Правила разрешений ({}) — последнее совпадение выигрывает:\n", rules.len()));
            for (tool, pattern, action, is_session) in &rules {
                let mark = if *is_session { " [сессия]" } else { "" };
                let icon = match action.as_str() {
                    "allow" => "✅",
                    "ask" => "❓",
                    _ => "🚫",
                };
                lines.push_str(&format!("  {icon} {tool}: '{pattern}' → {action}{mark}\n"));
            }
            lines.push_str("\nДобавить на сессию: /permissions add <tool> <\"паттерн\"> <allow|ask|deny>\nПостоянно: [[permissions.rules]] в ~/.hephaestus/config.toml\nСбросить сессийные: /permissions clear\n");
            // 2) Классические always-ключи (кеш ответов «всегда»).
            let keys = permissions.list_always();
            if !keys.is_empty() {
                lines.push_str(&format!("\nРазрешено всегда (сессия):\n  • {}\n", keys.join("\n  • ")));
            }
            // 3) Audit trail — последние решения.
            let log = a.state.list_permission_log(15);
            if !log.is_empty() {
                lines.push_str("\nПоследние решения (audit trail, state.db):\n");
                for (ts, tool, key, action, source) in log.iter().rev() {
                    let short_ts = ts.get(11..19).unwrap_or(ts);
                    let short_key = crate::truncate_chars(key, 60);
                    let src = crate::truncate_chars(source, 40);
                    lines.push_str(&format!("  {short_ts} [{action}] {tool}: {short_key} ({src})\n"));
                }
            }
        } else {
            lines.push_str("Агент занят — попробуйте ещё раз чуть позже.");
        }
        history.push(HistoryEntry::new(Role::System, lines));
        return false;
    }

    // Текущий план задачи агента (todo_write/todo_read).
    if cmd == "todos" || cmd == "todo" {
        match agent.try_lock() {
            Ok(a) => history.push(HistoryEntry::new(Role::System, a.todos.render())),
            Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят.")),
        }
        return false;
    }

    // /mcp — статус MCP-серверов; /mcp reload — переподключить все
    // серверы и пересобрать их инструменты (конфиг перечитывается с диска).
    if cmd == "mcp" {
        match agent.try_lock() {
            Ok(a) => history.push(HistoryEntry::new(Role::System, a.mcp.status_report())),
            Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят.")),
        }
        return false;
    }
    if cmd == "mcp reload" || cmd.starts_with("mcp r") {
        if busy {
            history.push(HistoryEntry::new(Role::System, "Агент занят — подождите."));
            return false;
        }
        let agent_clone = agent.clone();
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let _ = tx_clone.send(ReplMsg::Update(HistoryEntry::new(Role::System, "Перечитываю ~/.hephaestus/mcp.toml…")));
            let (registry, mcp) = {
                let a = agent_clone.lock().await;
                (a.shared_tools(), a.mcp.clone())
            }; // лок не держим во время сети
            mcp.unload_all(&registry);
            let mut lines: Vec<String> = Vec::new();
            match crate::mcp::load_config() {
                Ok(servers) if !servers.is_empty() => {
                    for s in &servers {
                        let report = mcp.connect_and_register(s, &registry).await;
                        let mark = if report.ok { "✅" } else { "⚠️" };
                        lines.push(format!("{} [{}]: {}", mark, report.name, report.detail));
                    }
                }
                Ok(_) => lines.push("Конфиг пуст — добавьте серверы в ~/.hephaestus/mcp.toml (там есть примеры).".to_string()),
                Err(e) => lines.push(format!("❌ {}", e)),
            }
            let _ = tx_clone.send(ReplMsg::Done(HistoryEntry::new(Role::System, format!("MCP reload:\n{}", lines.join("\n")))));
        });
        *busy_out = true;
        return false;
    }

    match cmd {
        "exit" | "quit" | "q" => {
            return true;
        }
        "help" | "h" | "?" => {
            history.push(HistoryEntry::new(
                Role::System,
                [
                    "Команды:",
                    "  /help          — эта справка",
                    "  /reset         — очистить историю диалога",
                    "  /save          — сохранить сессию на диск",
                    "  /load          — загрузить последнюю сохранённую сессию",
                    "  /checkpoint <имя> — сохранить диалог под именем (можно хранить много)",
                    "  /checkpoints   — список сохранённых чекпоинтов",
                    "  /restore <id>  — восстановить чекпоинт по id (см. /checkpoints)",
                    "  /provider      — список провайдеров LLM и статус API-ключей",
                    "  /provider <имя или номер> [модель] [base_url] [ключ] — переключить провайдера",
                    "  /provider custom <модель> [URL] [ключ] — Custom: URL, ключ или оба",
                    "  /provider custom <модель> --url <URL> --key <ключ> — явная форма",
                    "  /provider save <имя> — сохранить текущее подключение как профиль",
                    "  /provider use <имя> — переключиться на сохранённый профиль",
                    "  /provider forget <имя> — удалить сохранённый профиль",
                    "  /model         — текущая модель + обнаруженные доступные",
                    "  /model <имя>   — сменить модель у текущего провайдера",
                    "  /voice         — голосовой режим (микрофон + озвучка ответа)",
                    "  /telegram save <токен> / start [токен] — Telegram-бот в фоне, тот же диалог",
                    "  /goal <текст>  — режим достижения цели (несколько шагов подряд)",
                    "  /tools         — список зарегистрированных инструментов",
                    "  /work_dir      — показать рабочую директорию инструментов",
                    "  /work_dir <путь> — сменить (спросит подтверждение доверия, как в Claude Code:",
                    "                     /work_dir ok — применить и сохранить, /work_dir cancel — отмена)",
                    "  /allow once    — разрешить опасное действие один раз (вопрос приходит,",
                    "                   когда инструмент просит разрешение)",
                    "  /allow always  — разрешить всегда для этого ключа (до конца сессии)",
                    "  /deny          — запретить ожидающее действие",
                    "  /permissions   — список постоянных разрешений; /permissions clear — сброс",
                    "  /todos         — план текущей задачи агента (шаги и статусы)",
                    "  /mcp           — подключённые MCP-серверы и их инструменты",
                    "  /mcp reload    — перечитать ~/.hephaestus/mcp.toml и переподключить",
                    "  /stats         — статистика (токены, вызовы LLM/инструментов)",
                    "  /toolcalls     — журнал вызовов инструментов сессии (статусы, ошибки)",
                    "  /sessions      — список сессий; /sessions open N — открыть; /sessions find <текст> — поиск по всем",
                    "  /compress      — принудительно сжать контекст диалога",
                    "  /exit, /quit   — выход",
                    "",
                    "Редактирование строки: ←/→ — курсор, Home/End, Ctrl+U/K/W — стереть,",
                    "Up/Down — история команд, PageUp/PageDown или колесо мыши — прокрутка чата.",
                    "Копирование: зажмите ЛКМ и ведите — у края панели текст сам листается;",
                    "отпустите (или Ctrl+C) — выделенное уйдёт в буфер ОС через OSC52.",
                    "Esc — снять выделение. Вставка из буфера безопасна (bracketed paste).",
                ]
                .join("\n"),
            ));
        }
        "reset" => {
            if busy {
                history.push(HistoryEntry::new(Role::System, "Нельзя сбросить историю, пока агент отвечает — подождите."));
            } else if let Ok(a) = agent.try_lock() {
                a.reset().await;
                history.push(HistoryEntry::new(Role::System, "История диалога очищена."));
            } else {
                history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже."));
            }
        }
        "save" => match agent.try_lock() {
            Ok(a) => {
                let msgs = a.snapshot_messages().await;
                // Канонический стор (state.db) уже актуален — chat() пишет
                // инкрементально. /save — это легаси-экспорт в session.json.
                match session_store::save(&msgs) {
                    Ok(path) => history.push(HistoryEntry::new(Role::System, format!("Сессия сохранена (экспорт JSON): {}", path.display()))),
                    Err(e) => history.push(HistoryEntry::new(Role::Error, format!("Не удалось сохранить сессию: {}", e))),
                }
            }
            Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")),
        },
        "load" => match session_store::load() {
            Some(data) => match agent.try_lock() {
                Ok(mut a) => {
                    let count = data.messages.len();
                    for msg in &data.messages {
                        history.push(HistoryEntry::new(llm_role_to_repl_role(&msg.role), msg.content.clone()));
                    }
                    // load_messages синхронизирует и память, и каноническую БД.
                    a.load_messages(data.messages).await;
                    history.push(HistoryEntry::new(Role::System, format!("── Загружена сессия: {} сообщений ──", count)));
                }
                Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")),
            },
            None => history.push(HistoryEntry::new(Role::System, "Сохранённая сессия не найдена.")),
        },
        "tools" => match agent.try_lock() {
            Ok(a) => {
                let meta = a.tool_metadata();
                let mut lines = vec![format!("Зарегистрированные инструменты ({}):", meta.len())];
                for m in meta {
                    lines.push(format!("  {:<24} v{}  (schema v{})", m.name, m.version, m.schema_version));
                }
                history.push(HistoryEntry::new(Role::System, lines.join("\n")));
            }
            Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")),
        },
        "stats" => match agent.try_lock() {
            Ok(a) => {
                let report = a.stats_report().await;
                history.push(HistoryEntry::new(Role::System, format!("Статистика:\n{}", report)));
            }
            Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")),
        },
        "toolcalls" => match agent.try_lock() {            Ok(a) => {
                let rows = a.unfinished_tool_calls();
                let all = a.state.list_tool_calls(a.session_id());
                let mut lines = vec![format!("Tool-вызовы сессии #{} (всего {}):", a.session_id(), all.len())];
                let tail: Vec<_> = all.iter().rev().take(25).collect();
                for tc in tail.into_iter().rev() {
                    let mark = match tc.status.as_str() {
                        "completed" => "✅",
                        "error" => "❌",
                        "running" => "⏳",
                        _ => "🕐",
                    };
                    let mut line = format!("  {} [{}] {} ({})", mark, tc.status, tc.name, tc.call_id);
                    if let Some(e) = &tc.error {
                        let short = crate::truncate_chars(e, 80);
                        line.push_str(&format!(" — {}", short));
                    }
                    lines.push(line);
                }
                let pending: Vec<_> = rows.iter().filter(|r| r.status != "error").collect();
                if !pending.is_empty() {
                    lines.push(format!("⚠️ Недоделанных (pending/running): {}", pending.len()));
                }
                history.push(HistoryEntry::new(Role::System, lines.join("\n")));
            }
            Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")),
        },
        "compress" => {
            if busy {
                history.push(HistoryEntry::new(Role::System, "Агент занят — подождите."));
            } else if let Ok(mut a) = agent.try_lock() {
                a.compress_context_if_needed().await;
                history.push(HistoryEntry::new(Role::System, "Сжатие контекста запрошено (сработает, если история достаточно большая)."));
            } else {
                history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже."));
            }
        }
        "sessions" => {
            // /sessions              — список сессий
            // /sessions open N       — переключиться на сессию #N
            // /sessions find <текст> — полнотекстовый поиск по всем сессиям
            let args_str = cmd.strip_prefix("sessions ").unwrap_or("");
            let rest: Vec<&str> = args_str.split_whitespace().collect();
            if busy {
                history.push(HistoryEntry::new(Role::System, "Нельзя переключать сессии, пока агент отвечает."));
                return false;
            }
            match rest.first().map(|s| *s) {
                None => {
                    if let Ok(a) = agent.try_lock() {
                        let list = a.state.list_sessions(20);
                        let cur = a.session_id();
                        let mut lines = vec![format!("Сессии (текущая #{cur}):")];
                        for s in &list {
                            let mark = if s.id == cur { "→" } else { " " };
                            let title = crate::truncate_chars(&s.title, 60);
                            lines.push(format!(
                                "  {} #{:<4} {} — {} сообщ. — {}/{} — {}",
                                mark, s.id, s.updated_at.get(0..16).unwrap_or(""), s.message_count, s.provider, s.model, title
                            ));
                        }
                        lines.push(String::new());
                        lines.push("Открыть: /sessions open <номер>  Поиск: /sessions find <текст>".to_string());
                        history.push(HistoryEntry::new(Role::System, lines.join("\n")));
                    } else {
                        history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже."));
                    }
                }
                Some("find") => {
                    let query = args_str.strip_prefix("find ").unwrap_or("").trim();
                    if query.is_empty() {
                        history.push(HistoryEntry::new(Role::Error, "Формат: /sessions find <текст>"));
                    } else if let Ok(a) = agent.try_lock() {
                        let hits = a.state.search_sessions(query, 10);
                        if hits.is_empty() {
                            history.push(HistoryEntry::new(Role::System, format!("По запросу '{query}' ничего не найдено.")));
                        } else {
                            let list = a.state.list_sessions(50);
                            let mut lines = vec![format!("Найдено в {} сессиях:", hits.len())];
                            for (sid, count) in &hits {
                                let title = list
                                    .iter()
                                    .find(|s| s.id == *sid)
                                    .map(|s| crate::truncate_chars(&s.title, 50))
                                    .unwrap_or_default();
                                lines.push(format!("  #{sid} — {count} совпад. — {title}"));
                            }
                            lines.push("Открыть: /sessions open <номер>".to_string());
                            history.push(HistoryEntry::new(Role::System, lines.join("\n")));
                        }
                    } else {
                        history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже."));
                    }
                }
                Some("open") => {
                    let Some(n) = rest.get(1).and_then(|s| s.parse::<i64>().ok()) else {
                        history.push(HistoryEntry::new(Role::Error, "Формат: /sessions open <номер>"));
                        return false;
                    };
                    if let Ok(mut a) = agent.try_lock() {
                        if !a.state.switch_to(n) {
                            history.push(HistoryEntry::new(Role::Error, format!("Сессии #{n} не существует (см. /sessions)")));
                            return false;
                        }
                        let msgs = a.state.load_messages(n);
                        let count = msgs.len();
                        a.session.set_id(n);
                        a.load_messages(msgs).await;
                        let restored = a.state.load_messages(n);
                        for msg in &restored {
                            history.push(HistoryEntry::new(llm_role_to_repl_role(&msg.role), msg.content.clone()));
                        }
                        history.push(HistoryEntry::new(
                            Role::System,
                            format!("── Открыта сессия #{n}: {count} сообщений — продолжаем ──"),
                        ));
                    } else {
                        history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже."));
                    }
                }
                Some(other) => {
                    history.push(HistoryEntry::new(Role::Error, format!("Неизвестное действие /sessions {other}. Доступно: (пусто), open N, find <текст>")));
                }
            }
        }
        "memory" => {            // /memory            — список стейдж-записей фоновых задач
            // /memory approve N  — перенести запись N в постоянную память
            // /memory reject N   — отклонить
            let args_str = cmd.strip_prefix("memory ").unwrap_or("");
            let rest: Vec<&str> = args_str.split_whitespace().collect();
            match rest.first().map(|s| *s) {
                None => {
                    let pending = crate::memory_tools::pending_memory_files();
                    if pending.is_empty() {
                        history.push(HistoryEntry::new(
                            Role::System,
                            "Нет записей на утверждении. Фоновые задачи (суб-агенты) пишут факты в память только после вашего /memory approve.",
                        ));
                    } else {
                        let mut lines = vec![format!("📋 На утверждении ({}):", pending.len())];
                        for (i, p) in pending.iter().enumerate() {
                            let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                            let first = std::fs::read_to_string(p)
                                .ok()
                                .and_then(|t| {
                                    t.lines().find(|l| l.starts_with("description:"))
                                        .map(|d| d.trim_start_matches("description:").trim().to_string())
                                })
                                .unwrap_or_default();
                            lines.push(format!("  {}. {} — {}", i + 1, name, first));
                        }
                        lines.push(String::new());
                        lines.push("Утвердить: /memory approve <номер или имя>  Отклонить: /memory reject <номер или имя>".to_string());
                        history.push(HistoryEntry::new(Role::System, lines.join("\n")));
                    }
                }
                Some("approve") | Some("reject") => {
                    let is_approve = rest.first() == Some(&"approve");
                    let key = rest.get(1).copied().unwrap_or("");
                    let pending = crate::memory_tools::pending_memory_files();
                    let target: Option<String> = if let Ok(n) = key.parse::<usize>() {
                        pending.get(n.saturating_sub(1)).and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                    } else {
                        pending
                            .iter()
                            .find(|p| p.file_stem().map(|s| s.to_string_lossy() == key).unwrap_or(false))
                            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                    };
                    let Some(name) = target else {
                        history.push(HistoryEntry::new(Role::Error, format!("Запись '{}' не найдена (см. /memory)", key)));
                        return false;
                    };
                    if is_approve {
                        match crate::memory_tools::approve_pending(&name) {
                            Ok(_) => history.push(HistoryEntry::new(Role::System, format!("✅ Факт '{name}' перенесён в постоянную память."))),
                            Err(e) => history.push(HistoryEntry::new(Role::Error, e)),
                        }
                    } else {
                        match crate::memory_tools::reject_pending(&name) {
                            Ok(_) => history.push(HistoryEntry::new(Role::System, format!("🗑 Факт '{name}' отклонён и удалён."))),
                            Err(e) => history.push(HistoryEntry::new(Role::Error, e)),
                        }
                    }
                }
                Some(other) => {
                    history.push(HistoryEntry::new(Role::Error, format!("Неизвестное действие /memory {other}. Доступно: (пусто) — список, approve N, reject N")));
                }
            }
        }
        "undo" => {
            if busy {
                history.push(HistoryEntry::new(Role::System, "Нельзя откатывать, пока агент работает — подождите завершения хода."));
            } else if let Ok(a) = agent.try_lock() {
                match a.snapshots.undo_last() {
                    Ok(report) => {
                        history.push(HistoryEntry::new(Role::Diff, report));
                        history.push(HistoryEntry::new(Role::System, "⚠️ В диалоге агенту стоит напомнить, что изменения отменены — или начните новый ход с уточнением."));
                    }
                    Err(e) => history.push(HistoryEntry::new(Role::Error, format!("Undo не удался: {}", e))),
                }
            } else {
                history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже."));
            }
        }
        "" => {}
        other => {
            history.push(HistoryEntry::new(Role::Error, format!("Неизвестная команда: /{} (см. /help)", other)));
        }
    }
    false
}

fn handle_provider_list(history: &mut Vec<HistoryEntry>) {
    let mut lines = vec!["Провайдеры (/provider <имя или номер> [модель] [base_url] [ключ]; Custom также принимает один ключ):".to_string()];
    for (i, p) in LLMProvider::all().iter().enumerate() {
        let key_status = match p.env_key_var() {
            Some(var) => match std::env::var(var) {
                Ok(v) if !v.is_empty() => "ключ найден".to_string(),
                _ => format!("нужен {}", var),
            },
            None => "ключ не нужен".to_string(),
        };
        lines.push(format!("  {}) {} — {}", i + 1, p.display_name(), key_status));
    }

    // Сохранённые профили — см. config.rs. "/provider save <имя>" сохраняет
    // ТЕКУЩЕЕ подключение под именем, "/provider use <имя>" переключает.
    let profile_names = match crate::config::AgentConfig::load() {
        crate::config::LoadResult::Loaded(cfg) => cfg.list_profile_names(),
        _ => Vec::new(),
    };
    if profile_names.is_empty() {
        lines.push("\nСохранённых профилей нет. /provider save <имя> — сохранить текущее подключение.".to_string());
    } else {
        lines.push("\nСохранённые профили (/provider use <имя>):".to_string());
        for name in profile_names {
            lines.push(format!("  • {}", name));
        }
        lines.push("Удалить: /provider forget <имя>".to_string());
    }

    lines.push(format!("\nКонфиг: {}", crate::config::AgentConfig::config_file_path()));
    history.push(HistoryEntry::new(Role::System, lines.join("\n")));
}

async fn handle_provider_save(name: &str, agent: &Arc<Mutex<Agent>>, history: &mut Vec<HistoryEntry>) -> bool {
    if name.is_empty() {
        history.push(HistoryEntry::new(Role::System, "Использование: /provider save <имя>"));
        return false;
    }
    // Само подключение (провайдер/модель/base_url/ключ) уже лежит в
    // config.toml — его туда пишет каждый успешный /provider или /model
    // (см. apply_llm_switch). Здесь просто копируем его под именем.
    let _ = agent; // текущее состояние агента уже отражено в конфиге на диске
    match crate::config::AgentConfig::load() {
        crate::config::LoadResult::Loaded(mut cfg) => match cfg.save_profile(name) {
            Ok(_) => history.push(HistoryEntry::new(
                Role::System,
                format!("✅ Профиль '{}' сохранён: {}/{}{}", name, cfg.provider, cfg.model, cfg.base_url.as_deref().map(|u| format!(" ({})", u)).unwrap_or_default()),
            )),
            Err(e) => history.push(HistoryEntry::new(Role::Error, format!("Не удалось сохранить профиль: {}", e))),
        },
        _ => history.push(HistoryEntry::new(Role::Error, "Конфиг пуст или повреждён — сначала подключитесь через /provider.".to_string())),
    }
    false
}

async fn handle_provider_forget(name: &str, history: &mut Vec<HistoryEntry>) -> bool {
    if name.is_empty() {
        history.push(HistoryEntry::new(Role::System, "Использование: /provider forget <имя>"));
        return false;
    }
    match crate::config::AgentConfig::load() {
        crate::config::LoadResult::Loaded(mut cfg) => match cfg.forget_profile(name) {
            Ok(true) => history.push(HistoryEntry::new(Role::System, format!("Профиль '{}' удалён.", name))),
            Ok(false) => history.push(HistoryEntry::new(Role::System, format!("Профиль '{}' не найден.", name))),
            Err(e) => history.push(HistoryEntry::new(Role::Error, format!("Не удалось сохранить конфиг: {}", e))),
        },
        _ => history.push(HistoryEntry::new(Role::System, "Профилей нет — конфиг пуст.".to_string())),
    }
    false
}

fn spawn_provider_use(name: &str, agent: &Arc<Mutex<Agent>>, tx: &mpsc::UnboundedSender<ReplMsg>) {
    let name = name.to_string();
    let agent = agent.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        if name.is_empty() {
            let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::System, "Использование: /provider use <имя>. Список: /provider")));
            return;
        }
        let cfg = match crate::config::AgentConfig::load() {
            crate::config::LoadResult::Loaded(cfg) => cfg,
            _ => {
                let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::Error, "Конфиг пуст или повреждён — сохранённых профилей нет.")));
                return;
            }
        };
        let Some(profile) = cfg.profiles.get(&name).cloned() else {
            let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::Error, format!("Профиль '{}' не найден. Список: /provider", name))));
            return;
        };
        let Some(provider) = LLMProvider::from_str(&profile.provider) else {
            let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::Error, format!("Профиль '{}' повреждён: неизвестный провайдер '{}'.", name, profile.provider))));
            return;
        };
        apply_llm_switch(
            provider,
            profile.model,
            profile.base_url,
            profile.api_key,
            Some(profile.api_key_env),
            Some(profile.extra_headers),
            Some(profile.extra_body),
            &agent,
            &tx,
        ).await;
    });
}

/// Разбор аргументов `/provider`/`/model` с поддержкой кавычек — на
/// случай имени модели с пробелом (`/provider custom "My Model" http://host/v1`).
/// Без этого `arg.split_whitespace()` растащило бы одно имя на несколько
/// позиционных аргументов и всё бы съехало (именно так у пользователя
/// разъехалась команда `/provider custom qwen 3.8 http://...` — "qwen"
/// стало моделью, "3.8" по ошибке стало base_url).
fn split_args(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in s.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Для Custom один третий позиционный аргумент может быть либо URL, либо
/// ключом. Ключ сам по себе допустим, только если адрес уже сохранён либо
/// задан в CUSTOM_BASE_URL; проверка этого выполняется в `to_llm_config`.
/// Флаги позволяют убрать неоднозначность и принимать URL/ключ в любом
/// порядке: `--url <...> --key <...>`.
fn parse_custom_connection_args(parts: &[String]) -> Result<(Option<String>, Option<String>), String> {
    let mut base_url = None;
    let mut api_key = None;

    if parts.iter().any(|arg| arg.starts_with("--")) {
        let mut i = 0;
        while i < parts.len() {
            let flag = &parts[i];
            let value = parts.get(i + 1).filter(|v| !v.starts_with("--"))
                .ok_or_else(|| format!("После '{}' требуется значение.", flag))?;
            match flag.as_str() {
                "--url" | "--base-url" if base_url.is_none() => base_url = Some(value.clone()),
                "--key" | "--api-key" if api_key.is_none() => api_key = Some(value.clone()),
                "--url" | "--base-url" | "--key" | "--api-key" => {
                    return Err(format!("Параметр '{}' указан повторно.", flag));
                }
                _ => return Err(format!("Неизвестный параметр '{}'. Используйте --url и/или --key.", flag)),
            }
            i += 2;
        }
        return Ok((base_url, api_key));
    }

    match parts {
        [] => Ok((None, None)),
        [value] if value.starts_with("http://") || value.starts_with("https://") => {
            Ok((Some(value.clone()), None))
        }
        [value] => Ok((None, Some(value.clone()))),
        [url, key] if url.starts_with("http://") || url.starts_with("https://") => {
            Ok((Some(url.clone()), Some(key.clone())))
        }
        _ => Err(
            "Custom: используйте /provider custom <модель> [base_url] [ключ], либо флаги --url <URL> --key <ключ>."
                .to_string(),
        ),
    }
}

fn spawn_model_list(agent: &Arc<Mutex<Agent>>, tx: &mpsc::UnboundedSender<ReplMsg>) {
    let agent = agent.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let (provider, model) = match agent.try_lock() {
            Ok(a) => {
                let (p, m) = a.current_provider_and_model();
                (p.to_string(), m.to_string())
            }
            Err(_) => {
                let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")));
                return;
            }
        };
        let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
            Role::System,
            format!("Текущая модель: {}/{}\n\nИщу доступные локальные/облачные модели…", provider, model),
        )));
        // model_detector.rs — реальное обнаружение того, что сейчас
        // доступно (Ollama/KoboldCPP на localhost + облачные по
        // переменным окружения), а не просто статический список.
        let detected = crate::model_detector::describe_available_models().await;
        let _ = tx.send(ReplMsg::Done(HistoryEntry::new(
            Role::System,
            format!("{}\n\nПереключить: /model <имя_модели>  (провайдер: /provider <имя>)", detected),
        )));
    });
}

fn spawn_provider_switch(arg: &str, agent: &Arc<Mutex<Agent>>, tx: &mpsc::UnboundedSender<ReplMsg>) {
    if arg.is_empty() {
        let _ = tx.send(ReplMsg::Done(HistoryEntry::new(
            Role::System,
            "Использование: /provider <имя или номер> [модель] [base_url] [api_key]\n\
             Custom: URL, ключ или оба: /provider custom <модель> [base_url] [ключ]\n\
             Для ясности также: /provider custom <модель> --url <URL> --key <ключ>\n\
             Если передан только ключ, URL берётся из сохранённой Custom-конфигурации или CUSTOM_BASE_URL.\n\
             Профили: /provider save <имя> сохраняет текущее подключение, /provider use <имя> переключает.",
        )));
        return;
    }
    // Поддерживаются кавычки (см. split_args). У Custom третий аргумент
    // распознаётся как URL либо ключ, поэтому любой OpenAI-совместимый
    // провайдер подключается без добавления отдельного типа в LLMProvider.
    let parts = split_args(arg);
    let selector = parts.first().cloned().unwrap_or_default();
    let explicit_model = parts.get(1).cloned().filter(|s| !s.is_empty());

    let provider = if let Ok(idx) = selector.parse::<usize>() {
        LLMProvider::all().get(idx.saturating_sub(1)).cloned()
    } else {
        LLMProvider::from_str(&selector)
    };
    let Some(provider) = provider else {
        let tx = tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::Error, format!("Неизвестный провайдер: '{}'. Список: /provider", selector))));
        });
        return;
    };
    let model = explicit_model.unwrap_or_else(|| default_model_for(&provider).to_string());
    let (explicit_base_url, explicit_api_key) = if provider == LLMProvider::Custom {
        match parse_custom_connection_args(&parts[2..]) {
            Ok(connection) => connection,
            Err(error) => {
                let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::Error, error)));
                return;
            }
        }
    } else {
        (
            parts.get(2).cloned().filter(|s| !s.is_empty()),
            parts.get(3).cloned().filter(|s| !s.is_empty()),
        )
    };

    let agent = agent.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        apply_llm_switch(provider, model, explicit_base_url, explicit_api_key, None, None, None, &agent, &tx).await;
    });
}

fn spawn_model_switch(arg: &str, agent: &Arc<Mutex<Agent>>, tx: &mpsc::UnboundedSender<ReplMsg>) {
    if arg.is_empty() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::System, "Использование: /model <имя модели>. Список доступных: /model")));
        });
        return;
    }
    let model = arg.to_string();
    let agent = agent.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let provider = match agent.try_lock() {
            Ok(a) => {
                let (p_str, _) = a.current_provider_and_model();
                LLMProvider::from_str(p_str).unwrap_or(LLMProvider::Ollama)
            }
            Err(_) => {
                let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")));
                return;
            }
        };
        apply_llm_switch(provider, model, None, None, None, None, None, &agent, &tx).await;
    });
}

/// Общая часть /provider и /model: собрать конфиг, проверить, что LLM
/// реально отвечает (пробный запрос через ВРЕМЕННЫЙ клиент, отдельный
/// от `agent.llm`), и только тогда применить и сохранить. Если пинг не
/// прошёл — рабочий клиент агента не трогаем.
///
/// `base_url`/`api_key` — явно заданные пользователем (третий/четвёртый
/// аргумент `/provider`). Если не заданы и провайдер — Custom, пробуем
/// взять их из УЖЕ СОХРАНЁННОГО конфига (тот же провайдер), чтобы можно
/// было переключаться на свой прокси командой `/provider custom` без
/// повторного набора base_url каждый раз после первого раза.
async fn apply_llm_switch(
    provider: LLMProvider,
    model: String,
    base_url: Option<String>,
    api_key: Option<String>,
    profile_key_env: Option<Option<String>>,
    profile_headers: Option<std::collections::HashMap<String, String>>,
    profile_extra_body: Option<Option<serde_json::Value>>,
    agent: &Arc<Mutex<Agent>>,
    tx: &mpsc::UnboundedSender<ReplMsg>,
) {
    // URL и ключ независимы: при передаче только одного из них второе
    // значение берём из сохранённого подключения того же провайдера. Это
    // делает рабочими оба сценария: «новый URL + прежний ключ» и «новый
    // ключ + прежний URL».
    let saved = match crate::config::AgentConfig::load() {
        crate::config::LoadResult::Loaded(saved) if saved.provider == provider.as_str() => Some(saved),
        _ => None,
    };
    let resolved_base_url = base_url
        .or_else(|| saved.as_ref().and_then(|cfg| cfg.base_url.clone()))
        .or_else(|| provider.default_base_url().map(|s| s.to_string()));
    let resolved_api_key = api_key
        .or_else(|| profile_key_env.as_ref().and_then(|key_env| key_env.as_deref()).and_then(|name| std::env::var(name).ok()))
        .or_else(|| saved.and_then(|cfg| cfg.custom_api_key));

    // ИСПРАВЛЕНО: раньше здесь строился НОВЫЙ AgentConfig с пустыми
    // profiles и сохранялся целиком — каждая смена провайдера ЗАТИРАЛА
    // все сохранённые профили (и work_dir). Теперь берём свежий конфиг
    // с диска и меняем только поля подключения; профили и рабочая
    // директория остаются как были.
    let mut cfg = match crate::config::AgentConfig::load() {
        crate::config::LoadResult::Loaded(c) => c,
        _ => crate::config::AgentConfig::default(),
    };
    cfg.provider = provider.as_str().to_string();
    cfg.model = model.clone();
    cfg.base_url = resolved_base_url;
    cfg.custom_api_key = resolved_api_key;
    if provider != LLMProvider::Custom {
        // Настройки Custom не должны попасть в официальный провайдер при
        // обычном переключении (особенно Authorization из proxy-профиля).
        cfg.extra_headers.clear();
        cfg.extra_body = None;
        cfg.custom_api_key_env = None;
    } else {
        // `Some(empty)` означает намеренно очистить настройки при выборе
        // сохранённого профиля; `None` — оставить настройки текущего Custom.
        if let Some(headers) = profile_headers {
            cfg.extra_headers = headers;
        }
        if let Some(key_env) = profile_key_env {
            cfg.custom_api_key_env = key_env;
        }
        if let Some(body) = profile_extra_body {
            cfg.extra_body = body;
        }
    }

    let llm_config = match cfg.to_llm_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::Error, e)));
            return;
        }
    };

    let _ = tx.send(ReplMsg::Update(HistoryEntry::new(Role::System, format!("Проверяю подключение к {}/{}…", provider.as_str(), model))));

    let probe_client = crate::llm::create_llm_client(llm_config.clone());
    let probe = vec![LLMMessage::user("ping".to_string())];
    if let Err(e) = probe_client.complete(&probe, Some("Ответь одним словом: pong")).await {
        let _ = tx.send(ReplMsg::Done(HistoryEntry::new(
            Role::Error,
            format!("Не удалось подключиться к {}/{}: {}\nПрежняя модель агента не тронута.", provider.as_str(), model, e),
        )));
        return;
    }

    let mut a = agent.lock().await;
    a.reconfigure_llm(llm_config);
    let msg = match cfg.save() {
        Ok(_) => format!("✅ Подключено: {}/{} (сохранено в {})", provider.as_str(), model, crate::config::AgentConfig::config_file_path()),
        Err(e) => format!("✅ Подключено: {}/{} (не удалось сохранить конфиг: {})", provider.as_str(), model, e),
    };
    let _ = tx.send(ReplMsg::Done(HistoryEntry::new(Role::System, msg)));
}

async fn handle_checkpoint_save(name: &str, agent: &Arc<Mutex<Agent>>, history: &mut Vec<HistoryEntry>) -> bool {
    if name.is_empty() {
        history.push(HistoryEntry::new(Role::System, "Использование: /checkpoint <имя>"));
        return false;
    }
    match agent.try_lock() {
        Ok(a) => {
            let msgs = a.snapshot_messages().await;
            let session_messages: Vec<crate::session::Message> = msgs
                .iter()
                .map(|m| crate::session::Message { role: m.role.clone(), content: m.content.clone() })
                .collect();
            let mgr = crate::session::SessionManager::new();
            match mgr.save(&session_messages, Some(name)) {
                Ok(id) => history.push(HistoryEntry::new(Role::System, format!("Чекпоинт сохранён: {} (восстановить: /restore {})", id, id))),
                Err(e) => history.push(HistoryEntry::new(Role::Error, format!("Не удалось сохранить чекпоинт: {}", e))),
            }
        }
        Err(_) => history.push(HistoryEntry::new(Role::System, "Агент занят — попробуйте ещё раз чуть позже.")),
    }
    false
}

async fn graceful_save(agent: &Arc<Mutex<Agent>>) {
    // Штатный выход: clean_shutdown-маркер в state.db (recovery при
    // следующем старте увидит «выход был нормальный», не станет
    // продолжать сессию и обнулит счётчик крашей).
    if let Ok(a) = agent.try_lock() {
        let sid = a.session_id();
        crate::mark_clean_shutdown(&a.state, sid);
        // Легаси-экспорт session.json — сохранён как побочный бэкап.
        let msgs = a.snapshot_messages().await;
        let _ = session_store::save(&msgs);
    }
}

/// Вырезать текст выделения из зеркала отрисованных строк.
/// a/b — (индекс визуальной строки, колонка) в любом порядке.
fn extract_selection(plain: &[String], a: (usize, usize), b: (usize, usize)) -> String {
    let ((l0, c0), (l1, c1)) = if a <= b { (a, b) } else { (b, a) };
    let mut out: Vec<String> = Vec::new();
    for l in l0..=l1.min(plain.len().saturating_sub(1)) {
        if l >= plain.len() {
            break;
        }
        let chars: Vec<char> = plain[l].chars().collect();
        let from = if l == l0 { c0 } else { 0 };
        let to = if l == l1 { c1 } else { chars.len() };
        let from = from.min(chars.len());
        let to = to.max(from).min(chars.len());
        out.push(chars[from..to].iter().collect());
    }
    out.join("\n")
}

/// Минимальный base64 (standard, с паддингом) — чтобы не тащить крейт
/// ради одной функции для OSC52.
fn base64_encode(data: &[u8]) -> String {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TBL[(n >> 18) as usize & 63] as char);
        out.push(TBL[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TBL[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TBL[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Копировать текст в буфер обмена ОС, пробуя ВСЕ доступные каналы:
///   1. wl-copy (Wayland) — самый частый десктоп Linux сегодня;
///   2. xclip → xsel (X11);
///   3. OSC52 — escape-последовательность для терминалов с поддержкой
///      (kitty/WezTerm/Alacritty/tmux), работает даже через SSH.
/// Возвращает метку сработавшего канала или подсказку, если ничего.
/// (Раньше был только OSC52: сообщение "✅ скопировано" печаталось,
/// а терминал пользователя последовательность молча игнорировал —
/// в буфере пусто. Теперь локальные утилиты дают гарантированный
/// результат на машине, а OSC52 остаётся бонусом для SSH-сценария.)
fn copy_via_osc52(text: &str) -> &'static str {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let feed = |mut cmd: Command| -> bool {
        match cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                match child.wait() {
                    Ok(st) => st.success(),
                    Err(_) => false,
                }
            }
            Err(_) => false,
        }
    };

    // WINDOWS: powershell Set-Clipboard — нативно, без держателя (Windows
    // хранит буфер системно, эффект "испарения" — только Wayland/GTK).
    if cfg!(windows) {
        let mut ps = Command::new("powershell");
        ps.args(["-NoProfile", "-Command", "Set-Clipboard -Value $input"]);
        if feed(ps) {
            return "powershell";
        }
        return "OSC52";
    }

    // 0. GTK через python3-gi. ОСОБЕННОСТЬ KDE/Wayland (наш случай):
    // без живого держателя содержимое буфера ИСПАРЯЕТСЯ после выхода
    // источника — поэтому запускаем отсоединённый процесс-держатель
    // (setsid), который владеет буфером 15 минут; каждый новый копи-
    // пейст убивает предыдущего держателя. Проверено: write+read ок,
    // а "пусто через минуту" лечится именно держателем.
    let pid_file = dirs::home_dir()
        .map(|h| h.join(".hephaestus/.clip_holder.pid"))
        .unwrap_or_else(|| std::env::temp_dir().join(".hephaestus-clip-holder.pid"));
    if let Ok(old) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = old.trim().parse::<u32>() {
            let _ = Command::new("kill").arg(pid.to_string()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
    const HOLDER_CODE: &str = "\
import os, gi
gi.require_version('Gtk','3.0')
from gi.repository import Gtk, Gdk, GLib
t = os.environ.get('CLIP_TEXT', '')
cb = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
cb.set_text(t, -1)
cb.store()
GLib.timeout_add(15 * 60 * 1000, Gtk.main_quit)
Gtk.main()
";
    let spawn_holder = |use_setsid: bool| -> Option<u32> {
        let mut cmd = if use_setsid {
            let mut c = Command::new("setsid");
            c.arg("python3");
            c
        } else {
            let c = Command::new("python3");
            c
        };
        cmd.arg("-c").arg(HOLDER_CODE).env("CLIP_TEXT", text);
        match cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(child) => Some(child.id()),
            Err(_) => None,
        }
    };
    let holder_pid = spawn_holder(true).or_else(|| spawn_holder(false));
    if let Some(pid) = holder_pid {
        let _ = std::fs::write(&pid_file, pid.to_string());
        return "gtk";
    }

    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        if feed(Command::new("wl-copy")) {
            return "wl-copy";
        }
    }
    if std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()) {
        if feed({ let mut c = Command::new("xclip"); c.arg("-selection").arg("clipboard"); c }) {
            return "xclip";
        }
        if feed({ let mut c = Command::new("xsel"); c.args(["--clipboard", "--input"]); c }) {
            return "xsel";
        }
    }

    // OSC52 — последним: работает не во всех терминалах.
    let seq = format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()));
    let mut out = io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
    "OSC52"
}

fn role_style(role: Role) -> (Style, &'static str) {
    match role {
        Role::User => (Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD), "Вы"),
        Role::Assistant => (Style::default().fg(Color::Green).add_modifier(Modifier::BOLD), "Гефест"),
        Role::System => (Style::default().fg(Color::DarkGray), "•"),
        Role::Error => (Style::default().fg(Color::Red).add_modifier(Modifier::BOLD), "Ошибка"),
        Role::Diff => (Style::default().fg(Color::DarkGray), "diff"),
    }
}

/// Построчная раскраска содержимого записи (markdown-подмножество, как в
/// opencode): заголовки — жирные цветные, код-блоки — приглушённые,
/// таблицы — синие, цитаты — серые. Возвращает Style для ВСЕЙ строки
/// (спаны рвутся hard-wrap'ом, построчная окраска — самый надёжный
/// уровень без переустройства рендера).
fn content_line_style(line: &str, in_code: bool) -> ratatui::style::Style {
    let t = line.trim_start();
    if in_code {
        return Style::default().fg(Color::Cyan);
    }
    if let Some(h) = t.strip_prefix("### ") {
        let _ = h;
        return Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    }
    if let Some(h) = t.strip_prefix("## ") {
        let _ = h;
        return Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    }
    if let Some(h) = t.strip_prefix("# ") {
        let _ = h;
        return Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    }
    if t.starts_with("```") {
        return Style::default().fg(Color::DarkGray);
    }
    if t.starts_with('|') {
        return Style::default().fg(Color::Blue);
    }
    if t.starts_with("> ") {
        return Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC);
    }
    Style::default()
}

/// Построчная раскраска ДИФФА: добавленное — зелёным, удалённое —
/// красным, заголовки ханков/файлов и контекст — приглушённо.
fn diff_line_style(line: &str) -> ratatui::style::Style {
    if line.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::default().fg(Color::Red)
    } else if line.starts_with("@@") || line.starts_with("...") {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Роль из `LLMMessage.role` ("user"/"assistant"/"system"/"tool"/...) —
/// в `Role` для отображения в TUI. system/tool показываем как System
/// (серым, префикс "•") — они не диалог с человеком, но их всё равно
/// стоит видеть при восстановлении сессии, а не скрывать полностью.
fn llm_role_to_repl_role(role: &str) -> Role {
    match role {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => Role::System,
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

#[allow(clippy::too_many_arguments)]
fn draw_ui(
    f: &mut Frame,
    history: &[HistoryEntry],
    input: &[char],
    cursor: usize,
    busy: bool,
    spinner_frame: usize,
    scroll_from_bottom: usize,
    uptime: Duration,
    model: &(String, String),
    stats: (u64, u64, u64),
    perm_waiting: bool,
    queue_len: usize,
    stream_tail: &str,
    plain_out: &mut Vec<String>,
    geom_out: &mut Option<(u16, u16, u16, u16, usize)>,
    sel_anchor: Option<(usize, usize)>,
    sel_end: Option<(usize, usize)>,
) {
    let area = f.area();

    let typed_cmd: Option<String> = if input.first() == Some(&'/') {
        Some(input[1..].iter().collect())
    } else {
        None
    };
    let suggestions: Vec<&(&str, &str)> = match &typed_cmd {
        Some(typed) => {
            let word = typed.split_whitespace().next().unwrap_or("");
            if typed.contains(' ') {
                Vec::new()
            } else {
                COMMANDS.iter().filter(|(name, _)| name.starts_with(word)).take(6).collect()
            }
        }
        None => Vec::new(),
    };
    let has_stream = !stream_tail.is_empty() && busy;

    // Стрим рисуем ВНУТРИ области чата (хвостом истории), а не отдельной
    // панелью: раньше панель сдвигала раскладку и на низких терминалах
    // прятала поле ввода вместе с сообщением пользователя.
    let chunks = if suggestions.is_empty() {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(suggestions.len() as u16 + 2),
                Constraint::Length(3),
            ])
            .split(area)
    };
    let chat_idx = 2;
    let sugg_idx = if suggestions.is_empty() { None } else { Some(3) };
    let input_idx = if suggestions.is_empty() { 3 } else { 4 };

    let title = Line::from(vec![Span::styled(
        " ⚡ Гефест — AI Coding Agent ",
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    )]);
    f.render_widget(Paragraph::new(title), chunks[0]);

    // Строка статуса: провайдер/модель, токены, вызовы LLM/инструментов,
    // время работы сессии. Пункт запроса пользователя — раньше такого
    // не было вообще, узнать текущую модель можно было только по /model.
    let (tokens, llm_calls, tool_calls) = stats;
    let status = Line::from(vec![
        Span::styled(format!(" {}/{} ", model.0, model.1), Style::default().fg(Color::Magenta)),
        Span::raw("│ "),
        Span::styled(format!("токены: {}", tokens), Style::default().fg(Color::Blue)),
        Span::raw(" │ "),
        Span::styled(format!("LLM: {}", llm_calls), Style::default().fg(Color::Blue)),
        Span::raw(" │ "),
        Span::styled(format!("инструменты: {}", tool_calls), Style::default().fg(Color::Blue)),
        Span::raw(" │ "),
        Span::styled(format!("сессия: {}", format_duration(uptime)), Style::default().fg(Color::DarkGray)),
        Span::raw(" │ "),
        // Индикатор очереди запросов — виден только когда есть ожидание.
        if queue_len > 0 {
            Span::styled(format!("📥 очередь: {}", queue_len), Style::default().fg(Color::Yellow))
        } else {
            Span::raw("")
        },
        Span::raw(if queue_len > 0 { " │ " } else { "" }),
        Span::styled(
            "🖱 ЛКМ-выделение → копируется (Ctrl+C тоже)",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(status), chunks[1]);

    // Прокрутка — по фактическим ОТРИСОВАННЫМ строкам, с явным переносом
    // по ширине панели. ИСПРАВЛЕНО (жалоба "последние сообщения
    // обрезаются", видно на скриншоте): раньше здесь считалось, что
    // одна логическая строка `entry.text` (разбитая только по `\n`) —
    // это ОДНА визуальная строка. Но `Paragraph::wrap()` ниже сам
    // переносит длинные строки по ширине терминала — если сообщение
    // было длинным (что для ответов LLM почти всегда так), одна
    // логическая строка превращалась в НЕСКОЛЬКО визуальных уже ПОСЛЕ
    // среза `all_lines[start..end]` по количеству строк — окно среза
    // считало "20 строк = 20 логических", а реально рисовалось,
    // скажем, 35 визуальных (из-за переноса), и всё, что не влезло в
    // высоту панели, ratatui просто обрезает по краю Rect. Теперь перенос
    // считается явно, ЗДЕСЬ, посимвольно по реальной ширине панели —
    // после этого "одна строка в all_lines" ГАРАНТИРОВАННО означает
    // "одна визуальная строка".
    let available_width = chunks[chat_idx].width.saturating_sub(2) as usize; // минус рамка слева/справа
    let continuation_indent = "         "; // 9 пробелов — совпадает с прежним отступом продолжения
    let cont_indent_len = continuation_indent.chars().count();

    fn hard_wrap(s: &str, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![s.to_string()];
        }
        let chars: Vec<char> = s.chars().collect();
        if chars.is_empty() {
            return vec![String::new()];
        }
        chars.chunks(width).map(|c| c.iter().collect()).collect()
    }

    // Зеркало отрисованных строк ПОСИМВОЛЬНО (1:1 с all_lines) — из него
    // вырезается выделенный мышью текст. Заполняется в том же цикле.
    plain_out.clear();
    let mut all_lines: Vec<Line> = Vec::new();
    for entry in history {
        let (style, label) = role_style(entry.role);
        // ИСПРАВЛЕНО: было `entry.text.lines()` без проверки на пустую
        // строку — `"".lines()` даёт ПУСТОЙ итератор (0 элементов), а не
        // одну пустую строку, поэтому запись с пустым `text` не рисовала
        // вообще ничего, даже метку роли/времени. Именно это делало
        // ответ агента "невидимым" (см. фикс в llm.rs — теперь Ollama
        // пустой content возвращает как ошибку, а не пустой Ok(...), но
        // рендер всё равно не должен молчать на пустом тексте в принципе).
        if entry.text.is_empty() {
            let plain = format!("[{}] {}: (пустой ответ)", entry.time, label);
            all_lines.push(Line::from(vec![
                Span::styled(format!("[{}] ", entry.time), Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}: ", label), style),
                Span::styled("(пустой ответ)", Style::default().fg(Color::DarkGray)),
            ]));
            plain_out.push(plain);
        } else {
            let prefix_len = format!("[{}] {}: ", entry.time, label).chars().count();
            let mut is_first_nl_line = true;
            // Отслеживаем код-блоки для построчной раскраски (```...```),
            // дифф раскрашивается по префиксам строк.
            let mut in_code = false;
            for raw_line in entry.text.lines() {
                if entry.role == Role::Assistant || entry.role == Role::System {
                    if raw_line.trim_start().starts_with("```") {
                        in_code = !in_code;
                    }
                }
                let base_style = if entry.role == Role::Diff {
                    diff_line_style(raw_line)
                } else if entry.role == Role::Assistant || entry.role == Role::System {
                    content_line_style(raw_line, in_code)
                } else {
                    Style::default()
                };
                let this_width = if is_first_nl_line {
                    available_width.saturating_sub(prefix_len).max(1)
                } else {
                    available_width.saturating_sub(cont_indent_len).max(1)
                };
                let pieces = hard_wrap(raw_line, this_width);
                for (j, piece) in pieces.into_iter().enumerate() {
                    if is_first_nl_line && j == 0 {
                        all_lines.push(Line::from(vec![
                            Span::styled(format!("[{}] ", entry.time), Style::default().fg(Color::DarkGray)),
                            Span::styled(format!("{}: ", label), style),
                            Span::styled(piece.clone(), base_style),
                        ]));
                        plain_out.push(format!("[{}] {}: {}", entry.time, label, piece));
                    } else {
                        all_lines.push(Line::from(vec![Span::styled(
                            format!("{}{}", continuation_indent, piece),
                            base_style,
                        )]));
                        plain_out.push(format!("{}{}", continuation_indent, piece));
                    }
                }
                is_first_nl_line = false;
            }
        }
        all_lines.push(Line::from(""));
        plain_out.push(String::new());
    }

    // ЖИВОЙ СТРИМ: хвост генерируемого текста прямо в ленте чата —
    // видно сразу, раскладка не сдвигается, ввод не прячется.
    if has_stream {
        let flat = stream_tail.replace('\n', " ⏎ ");
        let chars: Vec<char> = flat.chars().collect();
        let take = available_width.saturating_sub(4).max(20);
        let cut = chars.len().saturating_sub(take);
        let tail_line: String = chars[cut..].iter().collect();
        all_lines.push(Line::from(vec![
            Span::styled("⌨ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(tail_line.clone(), Style::default().fg(Color::Yellow)),
        ]));
        plain_out.push(format!("⌨ {}", tail_line));
    }

    let inner_height = chunks[chat_idx].height.saturating_sub(2) as usize;
    let total = all_lines.len();
    // ИСПРАВЛЕНО (жалоба "прокрутка вверх обрывается"): было
    // `end = total.saturating_sub(scroll_from_bottom.min(total))` — при
    // scroll_from_bottom == total (прокрутили до самого начала истории)
    // получалось end=0, start=0.saturating_sub(inner_height)=0, то есть
    // `all_lines[0..0]` — ПУСТОЙ срез. Экран становился пустым вместо
    // того, чтобы остановиться на самых первых строках истории — это и
    // выглядело как "обрывается". Теперь прокрутка зажата так, чтобы
    // всегда показывать ровно `inner_height` строк (или всё, что есть,
    // если истории меньше), даже в самом начале.
    let max_scroll = total.saturating_sub(inner_height);
    let effective_scroll = scroll_from_bottom.min(max_scroll);
    let end = total.saturating_sub(effective_scroll);
    let start = end.saturating_sub(inner_height);

    let chat_title = if scroll_from_bottom > 0 {
        if effective_scroll >= max_scroll && max_scroll > 0 {
            " Диалог (начало истории — PageDown к концу) ".to_string()
        } else {
            " Диалог (прокручено — PageDown к концу) ".to_string()
        }
    } else {
        " Диалог ".to_string()
    };
    // СКРУГЛЁННЫЕ РАМКИ (как в opencode): ╭─╮ │ ╰─╯ вместо ┌─┐ └─┘,
    // рамка чата — приглушённо-серая, не чёрная.
    let chat_block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title_style(Style::default().fg(Color::Gray))
        .title(chat_title);
    let chat = Paragraph::new(Text::from(all_lines[start..end].to_vec()))
        .block(chat_block)
        .wrap(Wrap { trim: false });
    f.render_widget(chat, chunks[chat_idx]);

    // Геометрия для обработчика мыши + ПОДСВЕТКА выделения: инвертируем
    // ячейки буфера в выбранном диапазоне (ratatui рисует поверх, поэтому
    // правим буфер после render_widget — самый надёжный способ).
    *geom_out = Some((
        chunks[chat_idx].x,
        chunks[chat_idx].y,
        chunks[chat_idx].width,
        chunks[chat_idx].height,
        start,
    ));
    if let (Some(a), Some(b)) = (sel_anchor, sel_end) {
        if a != b && !plain_out.is_empty() {
            let ((l0, c0), (l1, c1)) = if a <= b { (a, b) } else { (b, a) };
            let inner_x = chunks[chat_idx].x + 1;
            let inner_y = chunks[chat_idx].y + 1;
            let inner_w = chunks[chat_idx].width.saturating_sub(2);
            let inner_h = chunks[chat_idx].height.saturating_sub(2);
            for row in 0..inner_h as usize {
                let abs = start.saturating_add(row);
                if abs < l0 || abs > l1 || abs >= plain_out.len() {
                    continue;
                }
                let line_len = plain_out[abs].chars().count();
                let col_from = if abs == l0 { c0.min(line_len) } else { 0 };
                let col_to = if abs == l1 { c1.min(line_len) } else { line_len };
                let x0 = inner_x + col_from as u16;
                let x1 = inner_x + (col_to.max(col_from) as u16).min(inner_w);
                for x in x0..x1 {
                    if let Some(cell) = f.buffer_mut().cell_mut((x, inner_y + row as u16)) {
                        cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
                    }
                }
            }
        }
    }

    if !suggestions.is_empty() {
        let lines: Vec<Line> = suggestions
            .iter()
            .map(|(name, desc)| {
                Line::from(vec![
                    Span::styled(format!("/{}", name), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw("  "),
                    Span::styled(*desc, Style::default().fg(Color::DarkGray)),
                ])
            })
            .collect();
        let suggest_widget = Paragraph::new(Text::from(lines)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray))
                .title_style(Style::default().fg(Color::Yellow))
                .title(" Команды "),
        );
        if let Some(si) = sugg_idx { f.render_widget(suggest_widget, chunks[si]); }
    }

    let input_title = if perm_waiting {
        " ⏳ ЖДЁТ РАЗРЕШЕНИЯ — /allow once · /allow always · /deny ".to_string()
    } else if busy {
        format!(" {} агент думает... ", SPINNER_FRAMES[spinner_frame])
    } else {
        " Ввод (Enter — отправить, /help — команды, Ctrl+C — выход) ".to_string()
    };
    // Рамка ввода меняет ЦВЕТ по состоянию: ждёт разрешения — жёлтая,
    // агент думает — голубая, свободен — приглушённо-серая.
    let input_border = if perm_waiting {
        Style::default().fg(Color::Yellow)
    } else if busy {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Курсор рисуем явным разбиением строки на "до" и "после" — ratatui
    // Paragraph сам курсор не подсвечивает.
    //
    // ИСПРАВЛЕНО (жалоба "когда пишешь длинный текст, его не видно"):
    // раньше рисовался ВЕСЬ ввод одной строкой — Paragraph без wrap
    // просто ОБРЕЗАЛ всё, что не влезло в ширину панели, причём вместе
    // с курсором. Теперь это честный горизонтальный скролл: окно из
    // `vis` символов, сдвигающееся так, чтобы курсор был всегда виден
    // (как в любом readline/редакторе).
    let inner_w = chunks[input_idx].width.saturating_sub(2) as usize; // рамка слева/справа
    let prompt_len = 2usize; // "> "
    let vis = inner_w.saturating_sub(prompt_len).max(1);
    let scroll_x = if cursor >= vis { cursor - vis + 1 } else { 0 };
    let win_start = scroll_x.min(input.len());
    let win_end = input.len().min(scroll_x + vis);
    let window = &input[win_start..win_end];
    let cursor_rel = cursor - win_start;

    let before: String = window[..cursor_rel.min(window.len())].iter().collect();
    let at: String = window.get(cursor_rel).map(|c| c.to_string()).unwrap_or_default();
    let after: String = if cursor_rel + 1 < window.len() {
        window[cursor_rel + 1..].iter().collect()
    } else {
        String::new()
    };

    let mut spans = vec![Span::styled("> ", Style::default().fg(Color::Yellow)), Span::raw(before)];
    if at.is_empty() {
        spans.push(Span::styled(" ", Style::default().bg(Color::Gray)));
    } else {
        spans.push(Span::styled(at, Style::default().bg(Color::Gray).fg(Color::Black)));
    }
    spans.push(Span::raw(after));

    let input_widget = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(input_border)
            .title_style(input_border.add_modifier(Modifier::BOLD))
            .title(input_title),
    );
    f.render_widget(input_widget, chunks[input_idx]);
}

#[cfg(test)]
mod custom_provider_argument_tests {
    use super::parse_custom_connection_args;

    #[test]
    fn custom_accepts_a_key_without_url() {
        let args = vec!["nvapi-example".to_string()];
        assert_eq!(
            parse_custom_connection_args(&args).unwrap(),
            (None, Some("nvapi-example".to_string()))
        );
    }

    #[test]
    fn custom_accepts_url_and_key() {
        let args = vec![
            "https://integrate.api.nvidia.com/v1".to_string(),
            "nvapi-example".to_string(),
        ];
        assert_eq!(
            parse_custom_connection_args(&args).unwrap(),
            (
                Some("https://integrate.api.nvidia.com/v1".to_string()),
                Some("nvapi-example".to_string()),
            )
        );
    }

    #[test]
    fn custom_named_url_and_key_are_order_independent() {
        let args = vec![
            "--key".to_string(), "nvapi-example".to_string(),
            "--url".to_string(), "https://integrate.api.nvidia.com/v1".to_string(),
        ];
        assert_eq!(
            parse_custom_connection_args(&args).unwrap(),
            (
                Some("https://integrate.api.nvidia.com/v1".to_string()),
                Some("nvapi-example".to_string()),
            )
        );
    }
}
