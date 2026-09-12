//! Гефест — Logo Animation. Порт `hephaestus_logo.py`.
//!
//! Рисуется до захода в `ratatui`-REPL (см. `main.rs`) — поэтому не через
//! `ratatui::Frame`, а напрямую в терминал через `crossterm`, как и в
//! Python-версии через `rich.console.Console`. `crossterm` уже есть в
//! зависимостях (используется REPL-ом), новых крейтов не требуется.
//!
//! Улучшения относительно оригинала:
//! - Кадры собираются в один `String` и пишутся одним `stdout().write_all`
//!   вместо построчных `console.print()` — на реальном терминале это
//!   заметно меньше мерцания при 8 fps.
//! - Добавлен `оранжевый` (`Color::Rgb`) переход между красным и жёлтым
//!   в `fire_char`, вместо жёсткой границы red -> yellow.
//! - `show_logo_animated` — `async fn` на `tokio::time::sleep` (main.rs уже
//!   `#[tokio::main]`), не блокирует executor так, как заблокировал бы
//!   `std::thread::sleep`.

use std::io::{stdout, Write};

use crossterm::{
    execute, queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
};

/// Крошечный self-contained xorshift64 — специально ВМЕСТО крейта `rand`,
/// который в Cargo.toml проекта пока не подключён, а тянуть его только
/// ради "случайно моргнуть символом огня" не оправдано. Не криптографический,
/// но для визуального шума этого более чем достаточно.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x2545F4914F6CDD1D)
            | 1;
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn gen_bool(&mut self, p: f64) -> bool {
        (self.next_u64() as f64 / u64::MAX as f64) < p
    }

    fn gen_range(&mut self, n: usize) -> usize {
        (self.next_u64() as usize) % n.max(1)
    }
}

const LOGO_ART: [&str; 6] = [
    "  ██╗  ██╗███████╗██████╗ ██╗  ██╗███████╗███████╗████████╗",
    "  ██║  ██║██╔════╝██╔══██╗██║  ██║██╔════╝██╔════╝╚══██╔══╝",
    "  ███████║█████╗  ██████╔╝███████║█████╗  ███████╗   ██║   ",
    "  ██╔══██║██╔══╝  ██╔═══╝ ██╔══██║██╔══╝  ╚════██║   ██║   ",
    "  ██║  ██║███████╗██║     ██║  ██║███████╗███████║   ██║   ",
    "  ╚═╝  ╚═╝╚══════╝╚═╝     ╚═╝  ╚═╝╚══════╝╚══════╝   ╚═╝   ",
];

const SPARKS: [&str; 5] = ["*", "·", "✦", "◦", "∘"];

/// Символ + цвет одной "искры" пламени в позиции (col, row) на данном кадре.
/// Логика тождественна `_fire_char` из Python-версии; добавлена пятая
/// оранжевая полоса между red и yellow вместо резкого скачка цвета.
fn fire_char(rng: &mut Rng, frame: i64, col: i64, row: i64) -> (&'static str, Color) {
    let intensity = (frame + col * 3 + row * 7).rem_euclid(20);

    if intensity < 5 {
        (if rng.gen_bool(0.5) { "▓" } else { "█" }, Color::Red)
    } else if intensity < 9 {
        (if rng.gen_bool(0.5) { "▒" } else { "▓" }, Color::DarkRed)
    } else if intensity < 13 {
        (if rng.gen_bool(0.5) { "▒" } else { "░" }, Color::Rgb { r: 255, g: 140, b: 0 })
    } else if intensity < 17 {
        (if rng.gen_bool(0.5) { "░" } else { "▒" }, Color::Yellow)
    } else {
        (if rng.gen_bool(0.5) { "·" } else { " " }, Color::Rgb { r: 255, g: 255, b: 153 })
    }
}

/// Одна строка "огня" шириной `width`, с вероятностью `density` того, что
/// в каждой колонке вообще что-то нарисуется (иначе — пробел, как в
/// оригинале, чтобы огонь не был сплошной стеной).
fn fire_row(out: &mut String, rng: &mut Rng, frame: i64, row: i64, width: i64, density: f64) {
    for col in 0..width {
        if rng.gen_bool(density) {
            let (ch, color) = fire_char(rng, frame, col, row);
            queue_colored(out, ch, color);
        } else {
            out.push(' ');
        }
    }
    out.push('\n');
}

fn queue_colored(out: &mut String, text: &str, color: Color) {
    // Пишем raw ANSI прямо в буфер строки, а не через `queue!` на каждый
    // символ — на кадр из ~4000 символов (62 колонки × ~5 строк огня + лого)
    // это на порядок меньше системных вызовов записи.
    out.push_str(&ansi_fg(color));
    out.push_str(text);
    out.push_str("\x1b[0m");
}

fn ansi_fg(color: Color) -> String {
    match color {
        Color::Rgb { r, g, b } => format!("\x1b[38;2;{r};{g};{b}m"),
        Color::Red => "\x1b[31m".to_string(),
        Color::DarkRed => "\x1b[31;2m".to_string(),
        Color::Yellow => "\x1b[93m".to_string(),
        Color::White => "\x1b[97m".to_string(),
        _ => "\x1b[39m".to_string(),
    }
}

/// Анимированный старт с огнём. Если что-то пойдёт не так с терминалом
/// (нет tty, `crossterm` не смог получить размер и т.п.) — тихо
/// откатывается на `show_logo_simple()`, как и в Python-версии.
pub async fn show_logo_animated(duration_secs: f32) {
    if let Err(_) = try_show_logo_animated(duration_secs).await {
        show_logo_simple();
    }
}

async fn try_show_logo_animated(duration_secs: f32) -> std::io::Result<()> {
    let frames = (duration_secs * 8.0) as i64;
    let width: i64 = 62;
    let mut stdout = stdout();
    let mut rng = Rng::new();

    for frame in 0..frames.max(1) {
        let mut buf = String::new();
        buf.push_str("\x1b[2J\x1b[H"); // clear + home, дешевле чем execute!(Clear) каждый кадр

        // Огонь сверху
        for row in 0..3 {
            fire_row(&mut buf, &mut rng, frame, row, width, 0.4);
        }

        // Логотип с мерцанием отдельных символов
        for (i, art_line) in LOGO_ART.iter().enumerate() {
            for (j, ch) in art_line.chars().enumerate() {
                if ch == ' ' {
                    buf.push(' ');
                    continue;
                }
                let flicker = (frame + i as i64 + j as i64) % 7 == 0;
                let color = if flicker { Color::Yellow } else { Color::Red };
                queue_colored(&mut buf, &ch.to_string(), color);
            }
            buf.push('\n');
        }

        // Огонь снизу
        for row in 0..2 {
            fire_row(&mut buf, &mut rng, frame + 5, row, width, 0.35);
        }

        // Подпись
        let spark = SPARKS[rng.gen_range(SPARKS.len())];
        buf.push('\n');
        queue_colored(
            &mut buf,
            &format!("  {spark} AI Coding Agent · God of Forge & Tools {spark}"),
            Color::Yellow,
        );
        buf.push('\n');

        stdout.write_all(buf.as_bytes())?;
        stdout.flush()?;

        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }

    Ok(())
}

/// Простой логотип без анимации — то же, что финальный кадр `show_logo_animated`,
/// но статично и без риска не иметь tty/цветов.
pub fn show_logo_simple() {
    let mut out = stdout();
    let _ = execute!(out, ResetColor);
    for line in LOGO_ART.iter() {
        let _ = queue!(out, SetForegroundColor(Color::Red), Print("  "), Print(line), Print("\n"));
    }
    let _ = execute!(
        out,
        ResetColor,
        SetForegroundColor(Color::Yellow),
        Print("\n  ⚡ AI Coding Agent · God of Forge & Tools ⚡\n\n"),
        ResetColor,
    );
}

/// Баннер после логотипа — рамка + сводка (провайдер/модель/кол-во
/// инструментов/рабочая папка). В оригинале использовалась `rich.Panel`;
/// здесь просто рисуем рамку вручную через box-drawing символы, чтобы не
/// тащить в проект ratatui ради экрана, который живёт три кадра до
/// запуска настоящего REPL.
pub fn show_startup_banner(provider: &str, model: &str, tools: usize, workspace: &str) {
    let lines = [
        format!("Провайдер:    {provider}"),
        format!("Модель:       {model}"),
        format!("Инструменты:  {tools}"),
        format!("Рабочая папка: {workspace}"),
        String::new(),
        "/help — команды  ·  /tools — инструменты  ·  /exit — выход".to_string(),
    ];

    let title = " ⚡ HEPHAESTUS ⚡ ";
    let inner_width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(title.chars().count()) + 4;

    let mut out = stdout();
    let _ = execute!(out, SetForegroundColor(Color::Red));

    let pad_title = inner_width.saturating_sub(title.chars().count());
    let left_pad = pad_title / 2;
    let right_pad = pad_title - left_pad;
    println!("╭{}{}{}╮", "─".repeat(left_pad), title, "─".repeat(right_pad));

    println!("│{}│", " ".repeat(inner_width));
    let _ = execute!(out, ResetColor, SetForegroundColor(Color::Yellow));
    println!(
        "│  Гефест — Бог кузнечного дела и инструментов{}│",
        " ".repeat(inner_width.saturating_sub(46))
    );
    println!("│{}│", " ".repeat(inner_width));
    let _ = execute!(out, ResetColor);

    for line in &lines {
        let pad = inner_width.saturating_sub(line.chars().count() + 2);
        println!("│  {line}{}│", " ".repeat(pad));
    }

    println!("│{}│", " ".repeat(inner_width));
    let _ = execute!(out, SetForegroundColor(Color::Red));
    println!("╰{}╯", "─".repeat(inner_width));
    let _ = execute!(out, ResetColor);
    println!();
}
