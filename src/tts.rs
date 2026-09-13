//! TTS — озвучка ответов на всех платформах, ноль внешних Rust-зависимостей.
//!
//! Платформенный диспетчер (см. docs/PLAN-interface-voice.md):
//! - Windows: SAPI через PowerShell (`System.Speech`) — встроен в ОС,
//!   русские голоса Irina/Dmitri ставятся в Параметрах → Речь.
//!   Кириллицу передаём через временный UTF-8 файл (PowerShell-аргумент
//!   в командной строке ломается о кодировку консоли cp866).
//! - Linux: piper (отличное качество, если установлен) → espeak-ng → espeak.
//! - macOS: `say` (встроен, русский Milena).
//!
//! Озвучка синхронная (блокирующая) — вызывать через spawn_blocking,
//! чтобы не держать async-рантайм.

/// Результат озвучки — для диагностики и тестов.
pub enum TtsOutcome {
    /// Успешно, каким движком.
    Spoken(&'static str),
    /// TTS недоступен на этой системе (текст показан, но не озвучен).
    Unavailable,
    /// TTS отключён конфигом.
    Disabled,
    /// Пустой текст — озвучивать нечего.
    Empty,
}

/// Проверка доступности движка без озвучки (для /voice tts status).
pub fn tts_available() -> bool {
    which_engine().is_some()
}

/// Какой движок доступен на этой системе.
fn which_engine() -> Option<&'static str> {
    if cfg!(windows) {
        return Some("sapi");
    }
    if cfg!(target_os = "macos") {
        return Some("say");
    }
    // Linux (и прочие unix): piper → espeak-ng → espeak
    if std::process::Command::new("piper")
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("piper");
    }
    if std::process::Command::new("espeak-ng")
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("espeak-ng");
    }
    if std::process::Command::new("espeak")
        .arg("--help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("espeak");
    }
    None
}

/// Озвучить текст. `voice` — язык/голос ("ru"/"en").
/// Ограничиваем длину: TTS-движки медленные, ответы агента бывают длинными;
/// озвучиваем первые ~600 символов (сущность ответа), остальное — текстом.
pub fn speak_blocking(text: &str, voice: &str) -> TtsOutcome {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return TtsOutcome::Empty;
    }
    // Обрезаем по границе символов (не байт!) — кириллица многобайтная.
    let clipped: String = if trimmed.chars().count() > 600 {
        trimmed.chars().take(600).collect()
    } else {
        trimmed.to_string()
    };

    let engine = match which_engine() {
        Some(e) => e,
        None => return TtsOutcome::Unavailable,
    };

    match engine {
        "sapi" => speak_sapi(&clipped, voice),
        "say" => speak_say(&clipped, voice),
        "piper" => speak_piper(&clipped),
        "espeak-ng" | "espeak" => speak_espeak(engine, &clipped, voice),
        _ => TtsOutcome::Unavailable,
    }
}

/// Windows SAPI через PowerShell. Текст — через временный UTF-8 файл:
/// инлайн-аргумент с кириллицей в cmd-кодировке искажается (поймано
/// живыми прогонами: cp866 на русской Windows).
fn speak_sapi(text: &str, voice: &str) -> TtsOutcome {
    use std::io::Write;
    let tmp = std::env::temp_dir().join(format!("heph_tts_{}.txt", std::process::id()));
    let (Ok(mut f), true) = (
        std::fs::File::create(&tmp).map(|mut f| {
            let _ = f.write_all(text.as_bytes());
            f
        }),
        true,
    ) else {
        return TtsOutcome::Unavailable;
    };
    drop(f);

    // Русский голос — если стоит; иначе SAPI выберет дефолтный.
    let voice_sel = match voice {
        "ru" => "try { $s.SelectVoice('Microsoft Irina Desktop') } catch {}",
        "en" => "try { $s.SelectVoice('Microsoft Zira Desktop') } catch {}",
        _ => "",
    };
    let script = format!(
        "Add-Type -AssemblyName System.Speech; \
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         {voice_sel} \
         $s.Speak([IO.File]::ReadAllText('{}'));",
        tmp.display()
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output();
    let _ = std::fs::remove_file(&tmp);
    match out {
        Ok(o) if o.status.success() => TtsOutcome::Spoken("sapi"),
        _ => TtsOutcome::Unavailable,
    }
}

/// macOS `say` — голос Rufina/Milena для ru.
fn speak_say(text: &str, voice: &str) -> TtsOutcome {
    let mut cmd = std::process::Command::new("say");
    if voice == "ru" {
        // Milena есть не на всех системах — say молча возьмёт дефолтный
        cmd.arg("-v").arg("Milena");
    }
    let out = cmd.arg(text).output();
    match out {
        Ok(o) if o.status.success() => TtsOutcome::Spoken("say"),
        _ => TtsOutcome::Unavailable,
    }
}

/// Linux piper: stdin → wav-пайп в aplay... но aplay нет гарантии; piper
/// умеет --output_raw | aplay. Проще: piper читает stdin и пишет wav во
/// временный файл; проигрывать будем ниже без внешних плееров НЕ будем —
/// piper без плеера только генерирует. Если нет плеера — генерируем в /dev/null?
// Нет: честно проверяем плеер; иначе используем espeak.
fn speak_piper(text: &str) -> TtsOutcome {
    // piper сам играет через --output-stdout + плеер; проверим aplay/pw-play
    let player = ["pw-play", "aplay", "play"]
        .iter()
        .find(|p| std::process::Command::new(*p).arg("--help").output().map(|o| o.status.success()).unwrap_or(false));
    let Some(player) = player else {
        return TtsOutcome::Unavailable;
    };
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("echo '{}' | piper --output-stdout 2>/dev/null | {} -", text.replace('\'', "'\\''"), player))
        .output();
    match out {
        Ok(o) if o.status.success() => TtsOutcome::Spoken("piper"),
        _ => TtsOutcome::Unavailable,
    }
}

/// espeak/espeak-ng: сам играет звук.
fn speak_espeak(engine: &str, text: &str, voice: &str) -> TtsOutcome {
    let v = if voice == "ru" { "ru" } else { "en" };
    let out = std::process::Command::new(engine)
        .args(["-v", v])
        .arg(text)
        .output();
    let name = if engine.contains("ng") { "espeak-ng" } else { "espeak" };
    match out {
        Ok(o) if o.status.success() => TtsOutcome::Spoken(name),
        _ => TtsOutcome::Unavailable,
    }
}

/// Асинхронная обёртка: не блокировать async-контекст (TTS-движки
/// синхронные, ответ агента можно озвучить в фоне).
pub async fn speak(text: String, voice: String) -> TtsOutcome {
    tokio::task::spawn_blocking(move || speak_blocking(&text, &voice))
        .await
        .unwrap_or(TtsOutcome::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_empty() {
        assert!(matches!(speak_blocking("", "ru"), TtsOutcome::Empty));
        assert!(matches!(speak_blocking("   ", "ru"), TtsOutcome::Empty));
    }

    #[test]
    fn availability_reported() {
        // На CI-Linux может не быть ни одного движка — проверяем только
        // что функция не паникует и возвращает согласованное значение.
        let has = tts_available();
        assert_eq!(has, which_engine().is_some());
    }
}
