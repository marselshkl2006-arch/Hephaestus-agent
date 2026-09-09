/// Voice Interface — голосовой интерфейс на чистом Rust
///
/// Использует:
/// - cpal для записи с микрофона
/// - rwhisper (https://docs.rs/rwhisper) для распознавания речи (STT) —
///   чистый Rust STT на candle, без Python и без внешнего whisper.cpp
/// - piper или espeak-ng для озвучки (TTS)

use std::io::{BufRead, Cursor};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::WavWriter;
use futures_util::StreamExt;
use rwhisper::{Whisper, WhisperBuilder, WhisperSource, WhisperLanguage};

use crate::Agent;

/// Конфигурация голосового интерфейса
#[derive(Clone)]
pub struct VoiceConfig {
    /// Язык распознавания (например "ru", "en"). Пусто/"auto" — автоопределение.
    pub whisper_language: String,
    /// Размер модели whisper (tiny/base/small/medium/large...), см. `parse_whisper_source`
    pub whisper_model: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub piper_model_path: Option<String>,
    pub tts_voice: String,
    pub no_tts: bool,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            // ИСПРАВЛЕНО (жалоба "voice работает только на английском,
            // нужен и русский и английский сразу"): было "ru" — это
            // ЖЁСТКО фиксирует распознавание на одном языке через
            // `.with_language(Some(...))` ниже (см. `ensure_whisper`),
            // так что второй язык просто не распознавался бы вообще.
            // "auto" — пропускает `.with_language(...)` и оставляет
            // автоопределение языка в самой модели Whisper (она
            // многоязычная, см. `parse_whisper_source` — multilingual
            // выбирается для любого языка кроме "en") — она сама
            // определяет русскую или английскую речь по звучанию,
            // не нужно переключать заранее.
            whisper_language: "auto".to_string(),
            whisper_model: "small".to_string(),
            sample_rate: 16000,
            channels: 1,
            piper_model_path: None,
            tts_voice: "ru".to_string(),
            no_tts: false,
        }
    }
}

/// Сопоставляет строковый идентификатор модели с `WhisperSource` из rwhisper.
/// rwhisper сам скачивает и кэширует веса модели при первом запуске
/// (см. `WhisperBuilder::with_cache` — по умолчанию `DATA_DIR/kalosm/cache`).
fn parse_whisper_source(name: &str, language: &str) -> WhisperSource {
    let multilingual = !language.eq_ignore_ascii_case("en");
    match name.to_ascii_lowercase().as_str() {
        "tiny" => if multilingual { WhisperSource::Tiny } else { WhisperSource::TinyEn },
        "base" => if multilingual { WhisperSource::Base } else { WhisperSource::BaseEn },
        "small" => if multilingual { WhisperSource::Small } else { WhisperSource::SmallEn },
        "medium" => if multilingual { WhisperSource::Medium } else { WhisperSource::MediumEn },
        "large" | "large-v2" => WhisperSource::LargeV2,
        "large-v3" => WhisperSource::LargeV2,
        "distil-large-v3" => WhisperSource::DistilLargeV3,
        _ => if multilingual { WhisperSource::Small } else { WhisperSource::SmallEn },
    }
}

/// Голосовой интерфейс
pub struct VoiceInterface {
    // ИСПРАВЛЕНО: было `agent: Mutex<Agent>` — VoiceInterface владел
    // СВОИМ отдельным экземпляром Agent, из-за чего его было невозможно
    // подключить как команду `/voice` внутри repl.rs (там уже есть
    // Agent, обёрнутый в свой Arc<Mutex<Agent>> — Agent не Clone, второй
    // экземпляр взять неоткуда). Теперь VoiceInterface просто разделяет
    // ТОТ ЖЕ Arc<Mutex<Agent>>, что и REPL/Telegram-бот — тот же паттерн,
    // что уже используется в repl.rs и telegram_bot.rs.
    agent: Arc<Mutex<Agent>>,
    config: VoiceConfig,
    /// Модель whisper загружается лениво при первом распознавании и кэшируется.
    whisper: Arc<Mutex<Option<Whisper>>>,
}

impl VoiceInterface {
    pub fn new(agent: Arc<Mutex<Agent>>, config: VoiceConfig) -> Self {
        Self {
            agent,
            config,
            whisper: Arc::new(Mutex::new(None)),
        }
    }

    /// Проверить наличие TTS
    pub fn check_tts(&self) -> bool {
        if self.config.no_tts {
            return false;
        }

        if let Some(piper_model) = &self.config.piper_model_path {
            if Path::new(piper_model).exists()
                && std::process::Command::new("piper").arg("--help").output().is_ok()
            {
                return true;
            }
        }

        std::process::Command::new("espeak-ng").arg("--help").output().is_ok()
            || std::process::Command::new("espeak").arg("--help").output().is_ok()
    }

    /// Загрузить (или вернуть уже загруженную) модель whisper.
    /// Первый вызов может занять время — rwhisper скачивает веса в локальный кэш.
    async fn ensure_whisper(&self) -> Result<(), String> {
        let mut guard = self.whisper.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let source = parse_whisper_source(&self.config.whisper_model, &self.config.whisper_language);

        let mut builder: WhisperBuilder = Whisper::builder().with_source(source);

        if !self.config.whisper_language.is_empty()
            && !self.config.whisper_language.eq_ignore_ascii_case("auto")
        {
            if let Ok(lang) = self.config.whisper_language.parse::<WhisperLanguage>() {
                builder = builder.with_language(Some(lang));
            }
            // Неизвестный код языка — просто оставляем автоопределение
        }

        let model = builder
            .build()
            .await
            .map_err(|e| format!("Ошибка загрузки модели Whisper: {}", e))?;

        *guard = Some(model);
        Ok(())
    }

    /// Записать аудио с микрофона в WAV данные
    pub async fn record_audio(&self, duration_secs: f32) -> Result<Vec<u8>, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "Нет устройства ввода".to_string())?;

        let config = device
            .default_input_config()
            .map_err(|e| format!("Ошибка конфигурации: {}", e))?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        let temp_file = tempfile::NamedTempFile::new()
            .map_err(|e| format!("Ошибка создания временного файла: {}", e))?;
        let temp_path = temp_file.path().to_path_buf();

        let writer = Arc::new(Mutex::new(Some(
            WavWriter::create(
                &temp_path,
                hound::WavSpec {
                    channels: channels as u16,
                    sample_rate,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .map_err(|e| format!("Ошибка создания WAV: {}", e))?,
        )));

        let writer_clone = writer.clone();
        let is_recording = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let is_recording_clone = is_recording.clone();

        let stream = device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !is_recording_clone.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    let samples: Vec<i16> = data
                        .iter()
                        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                        .collect();
                    if let Ok(mut w_guard) = writer_clone.try_lock() {
                        if let Some(w) = w_guard.as_mut() {
                            for &sample in &samples {
                                let _ = w.write_sample(sample);
                            }
                        }
                    }
                },
                |err| eprintln!("Ошибка записи: {}", err),
                None,
            )
            .map_err(|e| format!("Ошибка создания потока: {}", e))?;

        stream.play().map_err(|e| format!("Ошибка запуска записи: {}", e))?;

        tokio::time::sleep(tokio::time::Duration::from_secs_f32(duration_secs)).await;

        is_recording.store(false, std::sync::atomic::Ordering::SeqCst);
        drop(stream);

        let mut w_guard = writer.lock().await;
        let writer_inner = w_guard.take().ok_or("Writer is None")?;
        drop(w_guard);
        writer_inner.finalize().map_err(|e| format!("Ошибка сохранения WAV: {}", e))?;

        std::fs::read(&temp_path).map_err(|e| format!("Ошибка чтения WAV: {}", e))
    }

    /// Распознать речь из WAV данных через rwhisper (чистый Rust, без Python)
    pub async fn transcribe(&self, wav_data: &[u8]) -> Result<String, String> {
        self.ensure_whisper().await?;

        let guard = self.whisper.lock().await;
        let model = guard.as_ref().ok_or_else(|| "Модель Whisper не загружена".to_string())?;

        let decoder = rodio::Decoder::new(Cursor::new(wav_data.to_vec()))
            .map_err(|e| format!("Ошибка чтения WAV: {}", e))?;

        let mut task = model.transcribe(decoder);

        let mut text = String::new();
        while let Some(segment) = task.next().await {
            let s = segment.text().trim();
            if !s.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(s);
            }
        }

        Ok(text.trim().to_string())
    }

    /// Озвучить текст через TTS
    pub async fn speak(&self, text: &str) {
        if self.config.no_tts || text.is_empty() {
            return;
        }

        let truncated: String = if text.chars().count() > 1000 {
            text.chars().take(1000).collect()
        } else {
            text.to_string()
        };
        let text = truncated.as_str();

        if let Some(piper_model) = &self.config.piper_model_path {
            if Path::new(piper_model).exists() {
                let output = std::process::Command::new("piper")
                    .arg("--model")
                    .arg(piper_model)
                    .arg("--output_file")
                    .arg("/dev/null")
                    .arg("--help")
                    .output();

                if output.is_ok() {
                    println!(" (TTS piper) {}", text);
                    return;
                }
            }
        }

        let espeak_bin = if std::process::Command::new("espeak-ng").arg("--help").output().is_ok() {
            "espeak-ng"
        } else if std::process::Command::new("espeak").arg("--help").output().is_ok() {
            "espeak"
        } else {
            println!(" (TTS недоступен) {}", text);
            return;
        };

        let _ = std::process::Command::new(espeak_bin)
            .arg("-v")
            .arg(&self.config.tts_voice)
            .arg(text)
            .output();

        println!(" (TTS {}) {}", espeak_bin, text);
    }

    /// Главный цикл голосового интерфейса (push-to-talk)
    pub async fn run_loop(&self) {
        println!("⚡ Гефест — голосовой интерфейс (чистый Rust, rwhisper)");
        println!(
            "⏳ Загружаю модель Whisper ({}) — при первом запуске модель скачивается...",
            self.config.whisper_model
        );

        if let Err(e) = self.ensure_whisper().await {
            println!("❌ Не удалось загрузить модель Whisper: {}", e);
            return;
        }
        println!("✅ Модель Whisper загружена");

        if !self.check_tts() {
            println!("⚠️  TTS не доступен — ответы будут только текстом");
            println!("   Установите piper или espeak-ng для озвучки");
        }

        println!("️  Нажмите Enter, чтобы начать запись...");
        println!("   (запись длится 3 секунды)");

        let stdin = std::io::stdin();
        let mut input = String::new();

        loop {
            println!("\nНажми Enter, чтобы говорить...");
            input.clear();
            if stdin.lock().read_line(&mut input).is_err() {
                break;
            }

            if input.trim() == "exit" || input.trim() == "quit" {
                println!(" Выход.");
                break;
            }

            println!("️  Запись... (3 секунды)");
            let wav_data = match self.record_audio(3.0).await {
                Ok(data) => data,
                Err(e) => {
                    println!("❌ Ошибка записи: {}", e);
                    continue;
                }
            };

            println!("⏳ Распознаю речь...");
            let text = match self.transcribe(&wav_data).await {
                Ok(t) if !t.is_empty() => t,
                Ok(_) => {
                    println!("(не расслышал, попробуй ещё раз)");
                    continue;
                }
                Err(e) => {
                    println!("❌ Ошибка распознавания: {}", e);
                    continue;
                }
            };

            println!("️  Ты сказал: {}", text);

            let response = self.agent.lock().await.chat(&text, 50, true).await;
            println!("⚡ Гефест: {}\n", response);

            self.speak(&response).await;
        }
    }
}

/// Создать голосовой интерфейс с конфигурацией по умолчанию
pub fn create_voice_interface(
    agent: Arc<Mutex<Agent>>,
    whisper_model: Option<String>,
    whisper_language: Option<String>,
    piper_model_path: Option<String>,
    tts_voice: Option<String>,
    no_tts: bool,
) -> VoiceInterface {
    let mut config = VoiceConfig::default();

    if let Some(model) = whisper_model {
        config.whisper_model = model;
    }
    if let Some(lang) = whisper_language {
        config.whisper_language = lang;
    }
    if let Some(path) = piper_model_path {
        config.piper_model_path = Some(path);
    }
    if let Some(voice) = tts_voice {
        config.tts_voice = voice;
    }

    config.no_tts = no_tts;

    VoiceInterface::new(agent, config)
}
