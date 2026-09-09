use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use serde_json::Value;

/// Флаг: можно ли писать WARNING/ERROR в консоль (eprintln).
/// В TUI-режиме (repl.rs) эхо выключается, чтобы не ломать
/// отрисовку экрана crossterm'ом.
static CONSOLE_ECHO: AtomicBool = AtomicBool::new(true);

/// Включить/выключить консольное эхо логгера.
pub fn set_console_echo(on: bool) {
    CONSOLE_ECHO.store(on, Ordering::Relaxed);
}

/// Текущее состояние консольного эхо.
fn console_echo_enabled() -> bool {
    CONSOLE_ECHO.load(Ordering::Relaxed)
}

/// Уровень логирования
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warning = 2,
    Error = 3,
    Critical = 4,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warning => "WARNING",
            LogLevel::Error => "ERROR",
            LogLevel::Critical => "CRITICAL",
        };
        write!(f, "{}", s)
    }
}

/// Конфигурация логгера
#[derive(Debug, Clone)]
pub struct LoggerConfig {
    pub log_dir: PathBuf,
    pub max_file_size: u64,
    pub backup_count: usize,
    pub console_level: LogLevel,
    pub file_level: LogLevel,
}

impl Default for LoggerConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            log_dir: PathBuf::from(home).join(".hephaestus").join("logs"),
            max_file_size: 10 * 1024 * 1024, // 10MB
            backup_count: 5,
            console_level: LogLevel::Warning,
            file_level: LogLevel::Debug,
        }
    }
}

/// Основная структура логгера
pub struct Logger {
    config: LoggerConfig,
    file_handle: Mutex<std::fs::File>,
    error_handle: Mutex<std::fs::File>,
}

impl Logger {
    pub fn new(config: Option<LoggerConfig>) -> Self {
        let config = config.unwrap_or_default();
        
        // Создаём директорию для логов
        if !config.log_dir.exists() {
            let _ = fs::create_dir_all(&config.log_dir);
        }
        
        // Открываем файлы логов
        let log_file = config.log_dir.join("hephaestus.log");
        let error_file = config.log_dir.join("errors.log");
        
        let file_handle = Mutex::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file)
                .expect("Failed to open log file")
        );
        
        let error_handle = Mutex::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&error_file)
                .expect("Failed to open error file")
        );
        
        Self {
            config,
            file_handle,
            error_handle,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(None)
    }

    fn timestamp(&self) -> String {
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
    }

    fn format_extra(&self, kwargs: &HashMap<String, Value>) -> String {
        if kwargs.is_empty() {
            return String::new();
        }
        
        let parts: Vec<String> = kwargs
            .iter()
            .map(|(k, v)| {
                let v_str = if v.is_object() || v.is_array() {
                    serde_json::to_string(v).unwrap_or_default()
                } else if v.is_string() {
                    v.as_str().unwrap_or("").to_string()
                } else {
                    v.to_string()
                };
                format!("{}={}", k, v_str)
            })
            .collect();
        
        format!("[{}]", parts.join(", "))
    }

    fn log(&self, level: LogLevel, message: &str, kwargs: &HashMap<String, Value>) {
        let timestamp = self.timestamp();
        let extra = self.format_extra(kwargs);
        let full_message = if extra.is_empty() {
            format!("{} | {:<8} | {}", timestamp, level, message)
        } else {
            format!("{} | {:<8} | {} {}", timestamp, level, message, extra)
        };
        
        // Пишем в файл (все уровни)
        if level >= self.config.file_level {
            if let Ok(mut handle) = self.file_handle.lock() {
                let _ = writeln!(handle, "{}", full_message);
                let _ = handle.flush();
            }
        }
        
        // Пишем в файл ошибок (только ERROR и CRITICAL)
        if level >= LogLevel::Error {
            if let Ok(mut handle) = self.error_handle.lock() {
                let _ = writeln!(handle, "{}", full_message);
                let _ = handle.flush();
            }
        }
        
        // Пишем в консоль (WARNING и выше) — ТОЛЬКО если эхо разрешено.
        // В TUI-режиме эхо выключается (repl): иначе предупреждения бота
        // печатались прямо поверх отрисованного экрана («наезжают на
        // сессию/провайдера»).
        if level >= self.config.console_level && console_echo_enabled() {
            let console_message = match level {
                LogLevel::Error | LogLevel::Critical => {
                    format!("\x1b[31m{}: {}\x1b[0m", level, message)
                }
                LogLevel::Warning => {
                    format!("\x1b[33m{}: {}\x1b[0m", level, message)
                }
                _ => format!("{}: {}", level, message),
            };
            eprintln!("{}", console_message);
        }
    }

    pub fn debug(&self, message: &str, kwargs: Option<HashMap<String, Value>>) {
        self.log(LogLevel::Debug, message, &kwargs.unwrap_or_default());
    }

    pub fn info(&self, message: &str, kwargs: Option<HashMap<String, Value>>) {
        self.log(LogLevel::Info, message, &kwargs.unwrap_or_default());
    }

    pub fn warning(&self, message: &str, kwargs: Option<HashMap<String, Value>>) {
        self.log(LogLevel::Warning, message, &kwargs.unwrap_or_default());
    }

    pub fn error(&self, message: &str, error: Option<&str>, kwargs: Option<HashMap<String, Value>>) {
        let mut all_kwargs = kwargs.unwrap_or_default();
        if let Some(err) = error {
            all_kwargs.insert("error".to_string(), Value::String(err.to_string()));
        }
        self.log(LogLevel::Error, message, &all_kwargs);
    }

    pub fn critical(&self, message: &str, error: Option<&str>, kwargs: Option<HashMap<String, Value>>) {
        let mut all_kwargs = kwargs.unwrap_or_default();
        if let Some(err) = error {
            all_kwargs.insert("error".to_string(), Value::String(err.to_string()));
        }
        self.log(LogLevel::Critical, message, &all_kwargs);
    }

    /// Логировать выполнение инструмента
    pub fn log_tool_execution(
        &self,
        tool_name: &str,
        success: bool,
        duration_ms: Option<f64>,
        error: Option<&str>,
    ) {
        let mut kwargs = HashMap::new();
        if let Some(dur) = duration_ms {
            kwargs.insert("duration_ms".to_string(), Value::Number(
                serde_json::Number::from_f64(dur).unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap())
            ));
        }
        
        if success {
            self.info(&format!("Tool executed: {}", tool_name), Some(kwargs));
        } else {
            if let Some(err) = error {
                kwargs.insert("error".to_string(), Value::String(err.to_string()));
            }
            self.error(&format!("Tool failed: {}", tool_name), None, Some(kwargs));
        }
    }

    /// Логировать запрос к LLM
    pub fn log_llm_request(
        &self,
        provider: &str,
        model: &str,
        tokens: Option<u64>,
        duration_ms: Option<f64>,
    ) {
        let mut kwargs = HashMap::new();
        if let Some(t) = tokens {
            kwargs.insert("tokens".to_string(), Value::Number(t.into()));
        }
        if let Some(dur) = duration_ms {
            kwargs.insert("duration_ms".to_string(), Value::Number(
                serde_json::Number::from_f64(dur).unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap())
            ));
        }
        
        self.info(
            &format!("LLM request: {}/{}", provider, model),
            Some(kwargs)
        );
    }

    /// Логировать ошибку LLM
    pub fn log_llm_error(
        &self,
        provider: &str,
        model: &str,
        error: &str,
    ) {
        self.error(
            &format!("LLM error: {}/{}", provider, model),
            Some(error),
            None
        );
    }

    /// Логировать событие сессии
    pub fn log_session_event(&self, event: &str, session_id: Option<&str>) {
        let mut kwargs = HashMap::new();
        if let Some(id) = session_id {
            kwargs.insert("session_id".to_string(), Value::String(id.to_string()));
        }
        self.info(&format!("Session event: {}", event), Some(kwargs));
    }

    /// Получить путь к файлу логов
    pub fn get_log_file_path(&self, log_type: &str) -> PathBuf {
        match log_type {
            "errors" => self.config.log_dir.join("errors.log"),
            _ => self.config.log_dir.join("hephaestus.log"),
        }
    }

    /// Ротация логов (проверка размера)
    pub fn rotate_logs(&self) -> Result<(), String> {
        self.rotate_file(&self.config.log_dir.join("hephaestus.log"))?;
        self.rotate_file(&self.config.log_dir.join("errors.log"))?;
        Ok(())
    }

    fn rotate_file(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        
        let metadata = fs::metadata(path)
            .map_err(|e| format!("Failed to read file metadata: {}", e))?;
        
        if metadata.len() > self.config.max_file_size {
            // Ротируем: удаляем самый старый бэкап, сдвигаем остальные
            for i in (1..self.config.backup_count).rev() {
                let src = path.with_extension(format!("log.{}", i));
                let dst = path.with_extension(format!("log.{}", i + 1));
                if src.exists() {
                    let _ = fs::rename(&src, &dst);
                }
            }
            // Перемещаем текущий файл в .log.1
            let backup = path.with_extension("log.1");
            let _ = fs::rename(path, &backup);
            // Создаём новый пустой файл
            let _ = fs::File::create(path);
        }
        
        Ok(())
    }
}

/// Глобальный экземпляр логгера
static LOGGER: std::sync::OnceLock<Logger> = std::sync::OnceLock::new();

/// Получить глобальный логгер
pub fn get_logger() -> &'static Logger {
    LOGGER.get_or_init(Logger::with_defaults)
}

/// Удобные функции для быстрого доступа

pub fn debug(message: &str) {
    get_logger().debug(message, None);
}

pub fn debug_with(message: &str, kwargs: HashMap<String, Value>) {
    get_logger().debug(message, Some(kwargs));
}

pub fn info(message: &str) {
    get_logger().info(message, None);
}

pub fn info_with(message: &str, kwargs: HashMap<String, Value>) {
    get_logger().info(message, Some(kwargs));
}

pub fn warning(message: &str) {
    get_logger().warning(message, None);
}

pub fn warning_with(message: &str, kwargs: HashMap<String, Value>) {
    get_logger().warning(message, Some(kwargs));
}

pub fn error(message: &str) {
    get_logger().error(message, None, None);
}

pub fn error_with(message: &str, error: &str) {
    get_logger().error(message, Some(error), None);
}

pub fn critical(message: &str) {
    get_logger().critical(message, None, None);
}

pub fn critical_with(message: &str, error: &str) {
    get_logger().critical(message, Some(error), None);
}

// Макросы для удобства
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::logging_system::debug(&format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::logging_system::info(&format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::logging_system::warning(&format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::logging_system::error(&format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_logger_new() {
        let logger = Logger::with_defaults();
        assert!(logger.config.log_dir.exists());
    }

    #[test]
    fn test_log_level_display() {
        assert_eq!(format!("{}", LogLevel::Debug), "DEBUG");
        assert_eq!(format!("{}", LogLevel::Info), "INFO");
        assert_eq!(format!("{}", LogLevel::Warning), "WARNING");
        assert_eq!(format!("{}", LogLevel::Error), "ERROR");
        assert_eq!(format!("{}", LogLevel::Critical), "CRITICAL");
    }

    #[test]
    fn test_format_extra() {
        let logger = Logger::with_defaults();
        let mut kwargs = HashMap::new();
        kwargs.insert("key".to_string(), Value::String("value".to_string()));
        let result = logger.format_extra(&kwargs);
        assert_eq!(result, "[key=value]");
    }

    #[test]
    fn test_logger_config_default() {
        let config = LoggerConfig::default();
        assert!(config.log_dir.to_string_lossy().contains(".hephaestus"));
        assert_eq!(config.max_file_size, 10 * 1024 * 1024);
        assert_eq!(config.backup_count, 5);
    }
}
