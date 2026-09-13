//! Голосовой воркер внутри TUI (docs/PLAN-interface-voice.md, п.1–2).
//!
//! Дизайн: голос — НЕ отдельный режим, а фоновая tokio-задача, живущая
//! вместе с TUI. Она НЕ трогает терминал (raw-mode у ratatui): всё
//! общение — через mpsc-канал ReplMsg, а распознанная команда кладётся
//! в ту же request_queue, что и печатаемый ввод (см. repl.rs: «ОЧЕРЕДЬ
//! ЗАПРОСОВ»). Выход старого `run_voice_mode` с LeaveAlternateScreen
//! отменён — TUI остаётся единственным интерфейсом.
//!
//! Конвейер (план, п.2): кольцевой буфер PCM ~10с → энергетический VAD
//! («идёт речь?») → по окончании речи отрезок в rwhisper tiny → wake_word::
//! parse_utterance: активатор «Гефест» + команда в одной фразе. Без
//! активатора фраза игнорируется (это главный фильтр ложных срабатываний
//! — телевизор/разговор в комнате не дёргает агента).
//!
//! CPU-трюк: Whisper крутится ТОЛЬКО когда VAD увидел речь и она
//! закончилась (тишина ≥0.7с). В тишине конвейер спит.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use crate::repl::{HistoryEntry, ReplMsg, Role};
use crate::request_queue::QueuedTurn;
use crate::voice_interface::VoiceInterface;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Порог RMS-энергии: ниже — тишина (подобран по живым микрофонам;
/// регулируется, т.к. шумные комнаты разные).
const VAD_RMS_THRESHOLD: f32 = 0.012;
/// Сколько секунд тишины после речи считаем «фраза закончена».
const VAD_SILENCE_SECS: f32 = 0.7;
/// Максимальная длина одной фразы (дальше ресемпл «долины» в sleep): кольцо
/// просто выталкивает старое.
const PHRASE_MAX_SECS: f32 = 10.0;

/// Состояние воркера для /voice status.
#[derive(Clone, Copy, PartialEq)]
pub enum VoiceWorkerState {
    Off,
    /// Микрофон слушает, ждём активатора.
    Listening,
    /// Whisper распознаёт фразу.
    Transcribing,
}

/// Управление воркером из TUI-команд: канал команд — статический, чтобы
/// /voice из repl.rs не таскал структуры по всему циклу.
static VOICE_CMD: std::sync::Mutex<Vec<VoiceCmd>> = std::sync::Mutex::new(Vec::new());

pub enum VoiceCmd {
    Start(UnboundedSender<ReplMsg>),
    Stop,
    TtsToggle,
}

/// Отдать накопленные команды воркеру (вызывает сам воркер каждый цикл).
fn take_cmds() -> Vec<VoiceCmd> {
    let mut g = VOICE_CMD.lock().unwrap();
    std::mem::take(&mut *g)
}

pub fn send_cmd(cmd: VoiceCmd) {
    // Если воркер ещё не стартовал и его Send-канал лежит в команде —
    // просто копим: старт разберёт. Stop/TtsToggle при мёртвом воркере
    // безвредны.
    VOICE_CMD.lock().unwrap().push(cmd);
}

/// Текущее состояние (для /voice status) — обновляет сам воркер.
static VOICE_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
fn set_state(s: VoiceWorkerState) {
    let v = match s {
        VoiceWorkerState::Off => 0,
        VoiceWorkerState::Listening => 1,
        VoiceWorkerState::Transcribing => 2,
    };
    VOICE_STATE.store(v, std::sync::atomic::Ordering::Relaxed);
}
pub fn voice_state() -> VoiceWorkerState {
    match VOICE_STATE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => VoiceWorkerState::Listening,
        2 => VoiceWorkerState::Transcribing,
        _ => VoiceWorkerState::Off,
    }
}

/// TTS-ответы: выкл/вкл (флаг общий для TUI-ответов и воркера).
static TTS_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn tts_enabled() -> bool {
    TTS_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}
pub fn set_tts_enabled(v: bool) {
    TTS_ENABLED.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Озвучить ответ агента, если TTS включён (вызывается после Done/TurnDone).
pub async fn speak_reply(text: &str) {
    if !tts_enabled() || text.trim().is_empty() {
        return;
    }
    // Язык TTS: кириллица в первых 200 символах → ru.
    let head: Vec<char> = text.chars().take(200).collect();
    let ru = head.iter().any(|c| matches!(*c as u32, 0x400..=0x4FF));
    let voice = if ru { "ru" } else { "en" };
    let _ = crate::tts::speak(text.to_string(), voice.to_string()).await;
}

/// Запустить голосовой воркер (идемпотентно: повторный Start игнор).
/// `agent` — для Whisper-модели (VoiceInterface), `queue` — куда класть
/// распознанные команды.
pub async fn spawn_voice_worker(
    agent: Arc<tokio::sync::Mutex<crate::Agent>>,
    queue: Arc<crate::request_queue::RequestQueue>,
    tx: UnboundedSender<ReplMsg>,
) {
    if voice_state() != VoiceWorkerState::Off {
        return; // уже жив
    }
    // Модель: SMALL (уже в кэше ~/.local/share/kalosm — 925MB, качать
    // не надо; tiny пробовали — huggingface отдаёт 3.6KB/s, 5 часов).
    // Small грузится ~45с при старте — это цена первого /voice on за
    // сессию, зато качество распознавания команд заметно выше, а
    // wake_word::parse_utterance всё равно терпит опечатки (Левенштейн).
    let vi = crate::voice_interface::create_voice_interface(agent.clone(), None, None, None, None, false);
    let tx_info = tx.clone();
    let _ = tx_info.send(ReplMsg::Update(HistoryEntry::new(
        Role::System,
        "🎤 Голосовой воркер запущен: слушаю активатор «Гефест» (VAD + Whisper tiny). /voice off — выключить.",
    )));
    set_state(VoiceWorkerState::Listening);

    tokio::spawn(async move {
        // cpal::Stream — НЕ Send (внутри *mut ()), его нельзя держать
        // через .await в tokio-задаче. Классическое решение: стрим живёт
        // в отдельном std::thread и умирает вместе с ним; thread-тред
        // слушает atomic-флаг останова. Кольцо и индексы — Arc, они
        // Send и видны обоим.
        let host = cpal::default_host();
        let Some(device) = host.default_input_device() else {
            let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                Role::Error,
                "🎤 Нет микрофона — голосовой воркер остановлен.",
            )));
            set_state(VoiceWorkerState::Off);
            return;
        };
        let Ok(cfg) = device.default_input_config() else {
            let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                Role::Error,
                "🎤 Микрофон не сконфигурирован — голосовой воркер остановлен.",
            )));
            set_state(VoiceWorkerState::Off);
            return;
        };
        let sample_rate = cfg.sample_rate().0 as usize;
        let channels = cfg.channels() as usize;

        // Кольцевой буфер PCM mono f32 (микс каналов) ~10 сек.
        let ring_cap = sample_rate * 10;
        let ring: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(vec![0.0f32; ring_cap]));
        let write_idx: Arc<std::sync::atomic::AtomicUsize> = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let total_written: Arc<std::sync::atomic::AtomicUsize> = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stop_flag: Arc<std::sync::atomic::AtomicBool> = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let mic_ok: Arc<std::sync::atomic::AtomicBool> = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mic_err: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));

        {
            let ring_cb = ring.clone();
            let widx_cb = write_idx.clone();
            let tw_cb = total_written.clone();
            let stop_cb = stop_flag.clone();
            let stop_wait = stop_flag.clone();
            let ok_cb = mic_ok.clone();
            let err_cb = mic_err.clone();
            let cfg_clone = cfg.clone();
            // device тоже не Send — НО его можно получить заново в треде.
            // std::panic::catch_unwind: паника ЗДЕСЬ не должна убивать
            // весь процесс (TUI живёт своей жизнью) и не должна печатать
            // поверх raw-терминала — перехватываем и кладём в err_cb.
            std::thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let host = cpal::default_host();
                    let Some(device) = host.default_input_device() else {
                        *err_cb.lock().unwrap() = Some("нет устройства ввода".into());
                        return;
                    };
                    let build = device.build_input_stream(
                        &cfg_clone.into(),
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            if stop_cb.load(std::sync::atomic::Ordering::Relaxed) {
                                return;
                            }
                            // Микс в mono.
                            let mut mono = Vec::with_capacity(data.len() / channels.max(1));
                            for frame in data.chunks(channels.max(1)) {
                                let s = frame.iter().sum::<f32>() / channels as f32;
                                mono.push(s.clamp(-1.0, 1.0));
                            }
                            let mut r = ring_cb.lock().unwrap();
                            let w = widx_cb.load(std::sync::atomic::Ordering::Relaxed);
                            for (i, s) in mono.iter().enumerate() {
                                r[(w + i) % ring_cap] = *s;
                            }
                            widx_cb.store((w + mono.len()) % ring_cap, std::sync::atomic::Ordering::Relaxed);
                            tw_cb.fetch_add(mono.len(), std::sync::atomic::Ordering::Relaxed);
                        },
                        |_err| {},
                        None,
                    );
                    match build {
                        Ok(s) => {
                            if s.play().is_ok() {
                                ok_cb.store(true, std::sync::atomic::Ordering::Relaxed);
                                // Держим стрим живым, пока не попросят стоп.
                                while !stop_wait.load(std::sync::atomic::Ordering::Relaxed) {
                                    std::thread::sleep(std::time::Duration::from_millis(100));
                                }
                                drop(s);
                            } else {
                                *err_cb.lock().unwrap() = Some("stream.play() не удался".into());
                            }
                        }
                        Err(e) => {
                            *err_cb.lock().unwrap() = Some(format!("{e}"));
                        }
                    }
                }));
                if result.is_err() {
                    *err_cb.lock().unwrap() =
                        Some("паника в микрофонном треде (cpal/candle)".into());
                    crate::logging_system::warning("[voice] паника в микрофонном треде — перехвачена");
                }
            });
        }

        // Ждём вердикта микрофонного треда (до ~3с).
        let mut mic_live = false;
        for _ in 0..30 {
            if mic_ok.load(std::sync::atomic::Ordering::Relaxed) {
                mic_live = true;
                break;
            }
            if mic_err.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !mic_live {
            let why = mic_err.lock().unwrap().clone()
                .unwrap_or_else(|| "микрофон не ответил за 3 сек".into());
            let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                Role::Error,
                format!("🎤 Не удалось открыть микрофон: {why} — воркер остановлен."),
            )));
            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            set_state(VoiceWorkerState::Off);
            return;
        }

        // VAD-автомат.
        let mut in_speech = false;
        let mut silence_start: Option<std::time::Instant> = None;
        let mut phrase_start: Option<std::time::Instant> = None;
        let check_every = Duration::from_millis(100);

        loop {
            // Команды /voice из TUI.
            for cmd in take_cmds() {
                match cmd {
                    VoiceCmd::Stop => {
                        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        set_state(VoiceWorkerState::Off);
                        let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                            Role::System,
                            "🎤 Голосовой воркер остановлен (/voice on — снова запустить).",
                        )));
                        return;
                    }
                    VoiceCmd::TtsToggle => {
                        let now = tts_enabled();
                        set_tts_enabled(!now);
                        let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                            Role::System,
                            if !now { "🔊 TTS-ответы ВКЛючены." } else { "🔇 TTS-ответы ВЫКЛючены." },
                        )));
                    }
                    VoiceCmd::Start(_) => {} // уже жив — игнор
                }
            }

            tokio::time::sleep(check_every).await;

            // Команды, пришедшие за время сна, обработаем сверху цикла.
            let written = total_written.swap(0, std::sync::atomic::Ordering::Relaxed);
            if written == 0 {
                continue; // микрофон молчит совсем (нет колбэков)
            }
            let rms = {
                let r = ring.lock().unwrap();
                let w = write_idx.load(std::sync::atomic::Ordering::Relaxed);
                // RMS последних ~100мс.
                let n = (sample_rate / 10).min(r.len());
                let mut sum = 0.0f32;
                for i in 0..n {
                    let s = r[(w + ring_cap - i) % ring_cap];
                    sum += s * s;
                }
                (sum / n as f32).sqrt()
            };

            let speech_now = rms > VAD_RMS_THRESHOLD;
            match (in_speech, speech_now) {
                (false, true) => {
                    // Начало речи.
                    in_speech = true;
                    phrase_start = Some(std::time::Instant::now());
                    silence_start = None;
                }
                (true, true) => {
                    // Речь продолжается: следим за максимальной длиной.
                    if let Some(ps) = phrase_start {
                        if ps.elapsed().as_secs_f32() > PHRASE_MAX_SECS {
                            // Слишком длинно — форсируем «конец фразы».
                            in_speech = false;
                            silence_start = None;
                            if let Some(phrase) = drain_phrase(&ring, ring_cap, &write_idx, sample_rate) {
                                transcribe_and_dispatch(&vi, &queue, &tx, phrase, sample_rate).await;
                            }
                            phrase_start = None;
                        }
                    }
                }
                (true, false) => {
                    // Тишина после речи.
                    let since = *silence_start.get_or_insert(std::time::Instant::now());
                    if since.elapsed().as_secs_f32() >= VAD_SILENCE_SECS {
                        // Фраза закончена — распознаём.
                        in_speech = false;
                        silence_start = None;
                        phrase_start = None;
                        if let Some(phrase) = drain_phrase(&ring, ring_cap, &write_idx, sample_rate) {
                            transcribe_and_dispatch(&vi, &queue, &tx, phrase, sample_rate).await;
                        }
                    }
                }
                (false, false) => {}
            }
        }
    });
}

/// Достать последние ~6 сек из кольца (хвост фразы + запас контекста).
fn drain_phrase(
    ring: &Arc<std::sync::Mutex<Vec<f32>>>,
    cap: usize,
    write_idx: &Arc<std::sync::atomic::AtomicUsize>,
    sample_rate: usize,
) -> Option<Vec<f32>> {
    let take = (sample_rate * 6).min(cap);
    let r = ring.lock().ok()?;
    let w = write_idx.load(std::sync::atomic::Ordering::Relaxed);
    let mut out = Vec::with_capacity(take);
    for i in 0..take {
        out.push(r[(w + cap - take + i) % cap]);
    }
    if out.iter().all(|s| s.abs() < 1e-4) {
        return None; // чистая тишина — не грузим Whisper
    }
    Some(out)
}

/// Фраза → WAV → rwhisper → wake_word::parse_utterance → очередь ходов.
async fn transcribe_and_dispatch(
    vi: &VoiceInterface,
    queue: &Arc<crate::request_queue::RequestQueue>,
    tx: &UnboundedSender<ReplMsg>,
    pcm: Vec<f32>,
    sample_rate: usize,
) {
    set_state(VoiceWorkerState::Transcribing);

    // PCM f32 → WAV 16бит во временный файл (hound сам так делает в
    // record_audio; у WavWriter нет into_inner, из памяти буфер не
    // забрать без дропа writer'а).
    let tmp = std::env::temp_dir().join(format!("heph_vad_{}.wav", std::process::id()));
    let writer_result = hound::WavWriter::create(&tmp, hound::WavSpec {
        channels: 1,
        sample_rate: sample_rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    });
    let mut writer = match writer_result {
        Ok(w) => w,
        Err(e) => {
            let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                Role::Error,
                format!("🎤 WAV-кодер: {e}"),
            )));
            set_state(VoiceWorkerState::Listening);
            return;
        }
    };
    for s in &pcm {
        let _ = writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }
    let wav_res = writer.finalize().map(|_| ());
    let wav_buf = match (wav_res, std::fs::read(&tmp)) {
        (Ok(()), Ok(b)) => {
            let _ = std::fs::remove_file(&tmp);
            b
        }
        (res, _) => {
            let _ = std::fs::remove_file(&tmp);
            let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                Role::Error,
                format!("🎤 WAV-финализация: {res:?}"),
            )));
            set_state(VoiceWorkerState::Listening);
            return;
        }
    };

    // Whisper (rwhisper): транскрибация — самая тяжёлая часть, CPU.
    let text = match vi.transcribe(&wav_buf).await {
        Ok(t) if !t.trim().is_empty() => t,
        Ok(_) => {
            set_state(VoiceWorkerState::Listening);
            return; // не расслышал — ничего не делаем (не спамим чат)
        }
        Err(e) => {
            let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                Role::Error,
                format!("🎤 Whisper: {e}"),
            )));
            set_state(VoiceWorkerState::Listening);
            return;
        }
    };
    set_state(VoiceWorkerState::Listening);

    // Wake-word: активатор «Гефест» + команда в одной фразе (план п.2).
    match crate::wake_word::parse_utterance(&text) {
        Some(Some(cmd)) => {
            let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                Role::User,
                format!("🎤 {cmd}"),
            )));
            let source = "voice".to_string();
            let (queue_pos, _done_rx) = queue.push(QueuedTurn { text: cmd, source }).await;
            if queue_pos > 0 {
                let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                    Role::System,
                    format!("🎤 В очереди — позиция {} (ответ придёт автоматически).", queue_pos + 1),
                )));
            }
        }
        Some(None) => {
            // bare-wake: «Гефест!» — показать, что услышал.
            let _ = tx.send(ReplMsg::Update(HistoryEntry::new(
                Role::System,
                "🎤 Слышу тебя! Скажи: «Гефест, <команда>».",
            )));
        }
        None => {} // без активатора — игнор (телевизор не рулит агентом)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VAD-порог на синтетике: тишина не должна считаться речью.
    #[test]
    fn vad_silence_not_speech() {
        let silence = vec![0.0f32; 4410]; // 0.1с @ 44.1к
        let rms = (silence.iter().map(|s| s * s).sum::<f32>() / silence.len() as f32).sqrt();
        assert!(rms <= VAD_RMS_THRESHOLD, "тишина {rms} не должна проходить VAD");
    }

    /// Громкий тон — речь по VAD.
    #[test]
    fn vad_tone_is_speech() {
        let tone: Vec<f32> = (0..4410).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let rms = (tone.iter().map(|s| s * s).sum::<f32>() / tone.len() as f32).sqrt();
        assert!(rms > VAD_RMS_THRESHOLD, "тон {rms} должен проходить VAD");
    }

    /// Кольцо: drain_phrase достаёт последние сэмплы по порядку.
    #[test]
    fn drain_phrase_returns_tail() {
        let cap = 1000;
        let data: Vec<f32> = (0..cap / 2).map(|i| 0.1 * (i as f32 / 10.0).sin()).collect();
        // Заполняем кольцо половиной, потом ещё: tail должен быть концом.
        let ring = Arc::new(std::sync::Mutex::new(vec![0.0f32; cap]));
        let widx = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let mut r = ring.lock().unwrap();
            for (i, s) in data.iter().enumerate() {
                r[i % cap] = *s;
            }
            widx.store(data.len() % cap, std::sync::atomic::Ordering::Relaxed);
        }
        let sr = 100; // 6 «сек» = 600 сэмплов < cap
        let tail = drain_phrase(&ring, cap, &widx, sr).unwrap();
        assert_eq!(tail.len(), 600);
        // Хвост кольца — последние 600 сэмплов data (там ненулевые синусы).
        assert!(tail.iter().any(|s| s.abs() > 1e-4));
    }

    /// WAV-путь: hound пишет/читает (конвейер rwhisper-транскрибации
    /// требует валидный WAV; smoke на сам энкодер).
    #[test]
    fn wav_roundtrip_valid() {
        let tmp = std::env::temp_dir().join("heph_vad_test.wav");
        let mut w = hound::WavWriter::create(&tmp, hound::WavSpec {
            channels: 1, sample_rate: 16000, bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        }).unwrap();
        for i in 0..16000 {
            let s = ((i as f32) * 0.05).sin() * 0.3;
            w.write_sample((s * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();
        let mut r = hound::WavReader::open(&tmp).unwrap();
        assert_eq!(r.duration(), 16000, "длительность должна совпасть");
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod live_tests {
    /// ЖИВОЙ прогон (cargo test --release -- --ignored): транскрибация
    /// тишины напрямую через rwhisper — тот же путь, что в
    /// VoiceInterface::transcribe (ensure_whisper → transcribe).
    #[tokio::test]
    #[ignore]
    async fn whisper_live_transcribe_silence() {
        use futures_util::StreamExt;
        use rwhisper::{Whisper, WhisperBuilder, WhisperSource};
        let t0 = std::time::Instant::now();
        let model = Whisper::builder()
            .with_source(WhisperSource::Tiny)
            .build()
            .await
            .expect("модель Whisper должна загрузиться (кэш ~/.local/share/kalosm)");
        println!("модель загрузилась за {:?}", t0.elapsed());

        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut buf), hound::WavSpec {
                channels: 1, sample_rate: 16000, bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            }).unwrap();
            for i in 0..16000 {
                // Не идеальная тишина — лёгкий шум, реалистичнее.
                let s = ((i % 7) as f32 - 3.0) * 0.001;
                w.write_sample((s * i16::MAX as f32) as i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let t1 = std::time::Instant::now();
        let dec = rodio::Decoder::new(std::io::Cursor::new(buf))
            .expect("rodio должен декодировать наш WAV");
        let mut task = model.transcribe(dec);
        let mut text = String::new();
        while let Some(seg) = task.next().await {
            text.push_str(seg.text().trim());
        }
        println!("транскрипт: '{text}' за {:?} — КОНВЕЙЕР РАБОТАЕТ", t1.elapsed());
        assert!(t1.elapsed().as_secs() < 60, "тишина 1с не должна распознаваться дольше минуты");
    }
}
