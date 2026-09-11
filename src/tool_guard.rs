//! Защита хода от деградации: детектор зацикливания агента (doom-loop)
//! и выгрузка больших выводов инструментов на диск.
//!
//! Два источника:
//! - **doom_loop** opencode: N одинаковых вызовов инструмента подряд —
//!   признак зацикливания модели (жжёт токены и время; у Гефеста лимит
//!   хода 50 итераций, каждая с полным контекстом). Здесь: вызовы с
//!   ИДЕНТИЧНЫМИ (имя + аргументы) выполняются максимум `DOOM_LOOP_LIMIT`
//!   раз подряд, дальше вызов НЕ выполняется, модели уходит внятная
//!   ошибка с подсказкой изменить подход.
//! - **tool-output вне контекста** opencode: большой вывод инструмента не
//!   тащится в контекст целиком — голова+хвост в сообщении, полный текст
//!   в файле (`~/.hephaestus/tool-output/`), путь указан модели. Полный
//!   вывод остаётся доступен (человек/file_read), контекст не раздувается.


/// Сколько ИДЕНТИЧНЫХ вызовов подряд выполняются прежде, чем детектор
/// начнёт блокировать (1-й, 2-й, 3-й — выполняются; 4-й идентичный — нет).
pub const DOOM_LOOP_LIMIT: usize = 3;

/// Порог выгрузки вывода на диск (в символах, не байтах — кириллица).
pub const OFFLOAD_THRESHOLD: usize = 12_000;
/// Голова вывода, остающаяся в контексте.
pub const KEEP_HEAD: usize = 4_000;
/// Хвост вывода, остающийся в контексте (конец часто важнее — итоги).
pub const KEEP_TAIL: usize = 2_000;

/// Детектор зацикливания в рамках ОДНОГО хода (`Agent::chat`). Кросс-ход
/// не считает: модель вправе повторить вызов в новом ходе после анализа.
pub struct DoomLoopDetector {
    recent: Vec<(String, String)>,
}

impl Default for DoomLoopDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl DoomLoopDetector {
    pub fn new() -> Self {
        Self { recent: Vec::new() }
    }

    /// Проверить вызов ДО выполнения. Ok — выполнять; Err(объяснение) —
    /// НЕ выполнять, отдать модели текст ошибки.
    pub fn check(&mut self, name: &str, args: &serde_json::Value) -> Result<(), String> {
        let key = (name.to_string(), args.to_string());
        // Длина текущей серии идентичных вызовов, включая этот.
        let streak = self.recent.iter().rev().take_while(|r| **r == key).count() + 1;
        if streak > DOOM_LOOP_LIMIT {
            let last_outputs: Vec<String> = self
                .recent
                .iter()
                .rev()
                .take(DOOM_LOOP_LIMIT)
                .map(|(n, _)| n.clone())
                .collect();
            return Err(format!(
                "Защита от зацикливания (doom-loop): вызов `{name}` с ИДЕНТИЧНЫМИ аргументами уже выполнялся {} раз подряд \
                 (лимит {DOOM_LOOP_LIMIT}; последние: {}). Результат не меняется от повтора — \
                 измени аргументы, выбери другой инструмент или запроси у пользователя уточнение через ask_user.",
                streak - 1,
                last_outputs.join(", ")
            ));
        }
        self.recent.push(key);
        Ok(())
    }
}

/// Каталог для выгрузки больших выводов.
pub fn tool_output_dir() -> std::path::PathBuf {
    if let Ok(custom) = std::env::var("HEPHAESTUS_HOME") {
        return std::path::PathBuf::from(custom).join("tool-output");
    }
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".hephaestus")
        .join("tool-output")
}

/// Безопасное имя файла из call_id (call_xxx от провайдеров — ASCII, но
/// custom-адаптер мог сгенерировать иное; чистим до [A-Za-z0-9_-]).
fn sanitize_file_stem(id: &str) -> String {
    let s: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() { "call".to_string() } else { crate::truncate_chars(&s, 64) }
}

/// Обрезать вывод инструмента для контекста. Короткий — как есть;
/// длинный: полный текст → файл, в контекст голова + маркер + хвост.
pub fn bound_tool_output(call_id: &str, output: &str) -> String {
    let len = output.chars().count();
    if len <= OFFLOAD_THRESHOLD {
        return output.to_string();
    }
    let dir = tool_output_dir();
    let file = dir.join(format!("{}.txt", sanitize_file_stem(call_id)));
    let write_result = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, output));
    let head: String = output.chars().take(KEEP_HEAD).collect();
    let tail: String = {
        let skip = len.saturating_sub(KEEP_TAIL);
        output.chars().skip(skip).collect()
    };
    match write_result {
        Ok(_) => format!(
            "{head}\n\n…[вывод обрезан: всего {len} символов; ПОЛНЫЙ вывод сохранён в файл {} — прочитай нужный фрагмент file_read-ом, не тащи весь]…\n\n{tail}",
            file.display()
        ),
        Err(e) => format!(
            "{head}\n\n…[вывод обрезан: всего {len} символов; файл сохранить не удалось: {e}]…\n\n{tail}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doom_loop_allows_up_to_limit_then_blocks() {
        let mut d = DoomLoopDetector::new();
        let args = serde_json::json!({"command": "ls"});
        for i in 0..DOOM_LOOP_LIMIT {
            assert!(d.check("bash", &args).is_ok(), "вызов #{i} должен выполняться");
        }
        let err = d.check("bash", &args).unwrap_err();
        assert!(err.contains("doom-loop") || err.contains("зацикливание"));
        // Другой вызов — не блокируется.
        assert!(d.check("file_read", &serde_json::json!({"file_path": "x"})).is_ok());
    }

    #[test]
    fn doom_loop_counts_only_consecutive() {
        let mut d = DoomLoopDetector::new();
        let a = serde_json::json!({"command": "ls"});
        let b = serde_json::json!({"command": "pwd"});
        for _ in 0..DOOM_LOOP_LIMIT {
            assert!(d.check("bash", &a).is_ok());
        }
        // Один другой вызов разрывает серию — следующий идентичный `a` снова ок.
        assert!(d.check("bash", &b).is_ok());
        assert!(d.check("bash", &a).is_ok());
    }

    #[test]
    fn short_output_passes_through() {
        assert_eq!(bound_tool_output("c1", "привет"), "привет");
    }

    #[test]
    fn long_output_offloaded_to_file() {
        // Тест меняет процессный env — берём общий тестовый лок.
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Гарантируем изолированный каталог: HEPHAESTUS_HOME читается на
        // каждый вызов.
        let tmp = std::env::temp_dir().join(format!("hefest-toolout-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HEPHAESTUS_HOME", &tmp);

        let big = "ж".repeat(OFFLOAD_THRESHOLD + 500);
        let out = bound_tool_output("call_test_1", &big);
        assert!(out.contains("вывод обрезан"));
        assert!(out.contains("call_test_1.txt"));
        // Голова+хвост на месте.
        assert!(out.contains("жжжж"));
        let expected_file = tool_output_dir().join("call_test_1.txt");
        let full = std::fs::read_to_string(&expected_file).unwrap();
        assert_eq!(full.chars().count(), OFFLOAD_THRESHOLD + 500);

        std::env::remove_var("HEPHAESTUS_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sanitize_handles_weird_ids() {
        use std::collections::HashMap;
        let mut map: HashMap<String, &str> = HashMap::new();
        map.insert("a/b c".to_string(), "a_b_c");
        for (raw, clean) in map {
            assert_eq!(sanitize_file_stem(&raw), clean);
        }
    }
}
