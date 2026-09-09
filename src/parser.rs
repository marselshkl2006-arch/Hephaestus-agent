//! Parser — извлечение ВЫЗОВОВ ИНСТРУМЕНТОВ из обычного текста ответа.
//!
//! Зачем это нужно (реальный кейс, из-за которого модуль воскрешён):
//! провайдер custom (deepseek/qwen через llama.cpp /v1 без --jinja)
//! рендерит вызовы инструментов НЕ как нативные tool_calls в API-ответе,
//! а КАК ОБЫЧНЫЙ ТЕКСТ в content — причём формат от модели к модели
//! плавает:
//!
//!   <tool_call>{"name": "bash", "arguments": {"command": "ls"}}</tool_call>
//!   **tool_call** bash
//!   **arguments** {"command": "touch x"}          ← реальный случай 26.08
//!   tool▁call▁begin-токены deepseek
//!
//! Agent::chat() такой вызов не видел, ничего не исполнял — а модель,
//! не получив результата, ДОГАДЫВАЛА его и врала пользователю
//! ("✅ Файл создан", "Permission denied" — оба выдуманы).
//!
//! Решение: если API вернул ход БЕЗ нативных tool_calls, текст прогоняется
//! через этот парсер. Найденные вызовы выполняются по обычному конвейеру,
//! а из показываемого текста вырезается всё от первого маркера.
//!
//! Извлечение JSON — посимвольным сканером с учётом вложенности и строк,
//! а НЕ регэкспом "не-жадный anything": тот обрывается на первой `}`
//! внутри аргументов и вызов теряется.

use serde_json::Value;

pub struct ParsedToolCalls {
    /// (имя инструмента, аргументы)
    pub calls: Vec<(String, Value)>,
    /// Текст без вызовов и без всего после них — то, что показать человеку.
    pub cleaned: String,
}

/// Максимальная дистанция (в символах) от маркера до JSON — дальше это
/// уже прозаическое упоминание "tool_call" без вызова, не наш клиент.
const MAX_MARKER_TO_JSON: usize = 300;

/// Найти КОНЕЦ ближайшего маркера вызова (без учёта регистра и разметки):
/// покрывает `<tool_call>`, `**tool_call**`, `Tool Call:`, deepseek-токены.
/// Сравнение посимвольно по ASCII-нижнему регистру — длины совпадают,
/// индексы валидны в исходной строке.
fn find_marker_end(s: &str) -> Option<usize> {
    // ВАЖНО: работаем в БАЙТАХ с ASCII-понижением регистра — длина не
    // меняется, многобайтовые (кириллица/юникодные токены) байты не
    // трогаются, поэтому индексы валидны для срезов исходной строки.
    // Раньше здесь был Vec<char> — на русском тексте индексы съезжали
    // и text[..idx] паниковал на границе символа.
    let lb = s.bytes().map(|b| b.to_ascii_lowercase()).collect::<Vec<u8>>();
    const VARIANTS: &[&str] = &[
        "<｜tool▁calls▁begin｜>",
        "<｜tool▁call▁begin｜>",
        "<tool_call>",
        "**tool_call**",
        "**tool call**",
        "__tool_call__",
        "tool_call:",
        "tool_call",
        "tool call:",
        "tool call",
    ];
    let mut best: Option<usize> = None;
    for v in VARIANTS {
        let vb = v.as_bytes();
        let n = vb.len();
        if lb.len() < n {
            continue;
        }
        for i in 0..=(lb.len() - n) {
            if lb[i..i + n] == *vb {
                let end = i + n;
                best = Some(match best {
                    Some(b) => b.min(end),
                    None => end,
                });
                break;
            }
        }
    }
    best
}

/// Точка входа: None — если маркеров вызовов в тексте нет вообще.
pub fn extract_tool_calls(text: &str) -> Option<ParsedToolCalls> {
    // 1) Explicit markers: JSON with name/arguments, **tool_call**.
    if let Some(r) = extract_explicit_calls(text) { return Some(r); }
    // 2) Some custom models emit a textual pseudo-call: bash("ls -la").
    if let Some(r) = extract_function_style_bash_call(text) { return Some(r); }
    // 3) Fallback: text format \u00abExecute `bash` with `ls`\u00bb.
    extract_text_style_calls(text)
}

/// Извлекает только текстовый псевдовызов `bash("команда")`.
/// Это совместимость с моделями, которые не возвращают API `tool_calls`.
/// Аргумент обязан быть JSON-строкой: экранирование проверяется serde_json.
fn extract_function_style_bash_call(text: &str) -> Option<ParsedToolCalls> {
    let marker = "bash(";
    let (call_start, after_marker) = text.match_indices(marker)
        .find_map(|(start, _)| {
            let before = &text[..start];
            let is_identifier_suffix = before.chars().next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            (!is_identifier_suffix).then_some((start, start + marker.len()))
        })?;
    let rest = text[after_marker..].trim_start();
    let json_string = take_json_string(rest)?;
    let command: String = serde_json::from_str(json_string).ok()?;
    if !rest[json_string.len()..].trim_start().starts_with(')') {
        return None;
    }

    let line_start = text[..call_start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    Some(ParsedToolCalls {
        calls: vec![("bash".to_string(), serde_json::json!({ "command": command }))],
        cleaned: text[..line_start].trim().to_string(),
    })
}

/// Возвращает одну JSON-строку, включая кавычки, с корректной обработкой `\`.
fn take_json_string(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(1) {
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'"' {
            return Some(&s[..=i]);
        }
    }
    None
}

/// Explicit markers: JSON, **tool_call**, <tool_call_begin>.
fn extract_explicit_calls(text: &str) -> Option<ParsedToolCalls> {
    let first_end = find_marker_end(text)?;
    // Преамбула — всё до строки, где начинается первый маркер.
    let line_start = text[..first_end].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let preamble = text[..line_start].trim().to_string();

    let mut calls: Vec<(String, Value)> = Vec::new();
    let mut rest = &text[first_end..];
    loop {
        // Имя может стоять рядом с маркером отдельным словом:
        //   "**tool_call** bash\n**arguments** {...}"          → bash
        //   "<tool_call_begin>function<tool_sep>bash\n{...}"   → bash
        // Берём ПОСЛЕДНЕЕ слово-идентификатор перед '{', исключая
        // служебные ("arguments"/"parameters" — заголовки аргументов,
        // "function" — kind-токен deepseek).
        let mut forced_name: Option<String> = None;
        let Some(brace_off) = rest.find('{') else { break };
        if brace_off > MAX_MARKER_TO_JSON {
            break; // слишком далеко — это проза, не вызов
        }
        let mut words: Vec<String> = Vec::new();
        let mut cur = String::new();
        for c in rest[..brace_off].chars() {
            if c.is_ascii_alphanumeric() || c == '_' {
                cur.push(c);
            } else if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            words.push(cur);
        }
        for w in words.iter().rev() {
            let lw = w.to_ascii_lowercase();
            if lw != "arguments" && lw != "parameters" && lw != "function" {
                forced_name = Some(w.clone());
                break;
            }
        }

        rest = &rest[brace_off..];
        let Some(json_str) = take_balanced_json(rest) else { break };
        if let Some((name, args)) = parse_call_json(json_str, forced_name.as_deref()) {
            calls.push((name, args));
        }
        rest = &rest[json_str.len()..];

        // Следующий вызов? Маркер после конца текущего JSON.
        match find_marker_end(rest) {
            Some(next_end) => {
                // Пропускаем мусор между вызовами (закрывающие теги и т.п.),
                // но ограниченно — иначе прозаический хвост замедлит разбор.
                let cut = next_end.min(rest.len());
                rest = &rest[cut..];
            }
            None => break,
        }
    }

    if calls.is_empty() {
        return None;
    }
    Some(ParsedToolCalls { calls, cleaned: preamble })
}

/// Посимвольный извлекатель сбалансированного {...} с начала строки:
/// учитывает вложенность объектов/массивов и строки с экранированием.
fn take_balanced_json(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    debug_assert!(matches!(bytes[0], b'{' | b'['));
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None // незакрытый JSON — обрезанный ответ модели, вызов теряем
}

/// Разбор канонического объекта вызова: name + arguments|parameters.
/// arguments может прийти объектом ИЛИ строкой с JSON внутри (разные
/// шаблоны шлют по-разному) — нормализуем к объекту.
/// `forced_name` — имя из deepseek-формата, где оно лежит вне JSON.
fn parse_call_json(json_str: &str, forced_name: Option<&str>) -> Option<(String, Value)> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .or_else(|| forced_name.map(|s| s.to_string()))?;
    let raw_args = v
        .get("arguments")
        .or_else(|| v.get("parameters"))
        .cloned()
        // deepseek-формат: имя снаружи, а ВЕСЬ объект — аргументы.
        .unwrap_or_else(|| {
            if forced_name.is_some() { v.clone() } else { Value::Object(serde_json::Map::new()) }
        });
    let args = match raw_args {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::Object(serde_json::Map::new())),
        other => other,
    };
    Some((name, args))
}


/// Known tools and their params for text-style call extraction.
const TEXT_TOOLS: &[(&str, &[&str])] = &[
    ("bash", &["command"]),
    ("file_read", &["file_path"]),
    ("file_write", &["file_path", "content"]),
    ("file_edit", &["file_path", "old_str", "new_str"]),
    ("file_delete", &["file_path"]),
    ("file_exists", &["file_path"]),
    ("file_move", &["source", "destination"]),
    ("file_copy", &["source", "destination"]),
    ("glob", &["pattern"]),
    ("git", &["command"]),
    ("web_fetch", &["url"]),
    ("web_search", &["query"]),
    ("doc_generator", &["format", "content", "file_path"]),
    ("document_create", &["format", "content", "file_path"]),
    ("run_tests", &["command"]),
    ("todo_write", &["todos"]),
    ("todo_read", &[]),
    ("diagram", &["type", "content"]),
    ("config_get", &["key"]),
    ("config_set", &["key", "value"]),
    ("ask_user", &["question"]),
    ("verify_result", &["result"]),
    ("worktree", &["action"]),
    ("skill_list", &[]),
    ("skill_find", &["query"]),
    ("skill_save", &["name", "content"]),
    ("parallel_exec", &["tasks"]),
    ("workflow_run", &["workflow"]),
    ("background_run", &["task"]),
    ("background_status", &["id"]),
    ("background_list", &[]),
    ("secret_set", &["key", "value"]),
    ("secret_get", &["key"]),
    ("secret_list", &[]),
    ("secret_delete", &["key"]),
];

/// Context hints for parameter name detection (Russian).
const PARAM_HINTS: &[(&str, &str)] = &[
    ("команд", "command"),
    ("шаблон", "pattern"),
    ("пут", "file_path"),
    ("файл", "file_path"),
    ("запро", "query"),
    ("содержим", "content"),
    ("текст", "content"),
    ("источник", "source"),
    ("назначен", "destination"),
    ("ключ", "key"),
    ("значен", "value"),
    ("вопро", "question"),
    ("формат", "format"),
    ("тип", "type"),
    ("результат", "result"),
    ("задач", "task"),
    ("имя", "name"),
    ("url", "url"),
];

/// Extract all backtick-quoted segments with their byte positions.
fn find_backtick_segments(text: &str) -> Vec<(usize, usize, String)> {
    let mut result = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'`' {
                end += 1;
            }
            if end > start {
                result.push((start, end, text[start..end].to_string()));
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    result
}

/// Fallback: model writes text like «Execute `bash` with `ls`»
/// instead of native tool_call. Parses backtick-quoted tool names and args.
fn extract_text_style_calls(text: &str) -> Option<ParsedToolCalls> {
    let segments = find_backtick_segments(text);
    if segments.len() < 2 {
        return None;
    }

    // Find first backtick matching a known tool name
    let mut tool_idx: Option<usize> = None;
    let mut tool_params: &[&str] = &[];
    for (i, _, seg) in &segments {
        let lower = seg.to_ascii_lowercase();
        for &(name, params) in TEXT_TOOLS {
            if lower == name {
                tool_idx = Some(*i);
                tool_params = params;
                break;
            }
        }
        if tool_idx.is_some() { break; }
    }
    let tool_idx = tool_idx?;
    let tool_name = segments.iter()
        .find(|(s, _, _)| *s == tool_idx)
        .unwrap().2.clone();

    // Collect arg values from subsequent backticks
    let arg_segments: Vec<_> = segments.iter()
        .filter(|(s, _, _)| *s > tool_idx)
        .collect();
    if arg_segments.is_empty() && !tool_params.is_empty() {
        return None;
    }

    let mut args = serde_json::Map::new();
    for (idx, &&(seg_start, _seg_end, ref seg_text)) in arg_segments.iter().enumerate() {
        let prev_end = if idx == 0 { tool_idx } else { arg_segments[idx - 1].1 };
        let context = text[prev_end..seg_start].to_ascii_lowercase();

        let mut param_name = None;
        for &(hint, pname) in PARAM_HINTS {
            if context.contains(hint) {
                param_name = Some(pname.to_string());
                break;
            }
        }
        let param_name = param_name.unwrap_or_else(|| {
            tool_params.get(idx)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("arg{}", idx))
        });
        args.insert(param_name, Value::String(seg_text.clone()));
    }

    // Preamble: text before the tool name line
    let bt_start = segments.iter().find(|(s,_,_)| *s == tool_idx).unwrap().0;
    let line_start = text[..bt_start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let preamble = text[..line_start].trim().to_string();

    Some(ParsedToolCalls {
        calls: vec![(tool_name, Value::Object(args))],
        cleaned: preamble,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_function_style_bash_call_from_custom_model() {
        let text = "Проверяю каталог.\n'bash(\"ls -la /tmp\")'";
        let parsed = extract_tool_calls(text).expect("псевдовызов bash должен распознаться");
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].0, "bash");
        assert_eq!(parsed.calls[0].1["command"], json!("ls -la /tmp"));
        assert_eq!(parsed.cleaned, "Проверяю каталог.");
    }

    #[test]
    fn test_qwen_style_single_call_with_hallucinated_result() {
        let text = "Сейчас создам файл.\n<tool_call>{\"name\": \"bash\", \"arguments\": {\"command\": \"touch /home/marsel/test.txt && echo OK\"}}</tool_call>\n✅ Файл создан: ... (галлюцинация)";
        let parsed = extract_tool_calls(text).expect("должен найти вызов");
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].0, "bash");
        // Вложенные/экранированные скобки не рвут JSON.
        assert_eq!(
            parsed.calls[0].1["command"].as_str().unwrap(),
            "touch /home/marsel/test.txt && echo OK"
        );
        // Галлюцинация после маркера вырезана.
        assert_eq!(parsed.cleaned, "Сейчас создам файл.");
        assert!(!parsed.cleaned.contains("Файл создан"));
    }

    #[test]
    fn test_multiple_calls_and_nested_braces() {
        let text = r##"<tool_call>{"name":"file_write","arguments":{"file_path":"a.md","content":"# Заголовок\n{шаблон}"}}</tool_call>
<tool_call>{"name":"bash","arguments":{"command":"echo '{\"x\":1}'"}}</tool_call>"##;
        let parsed = extract_tool_calls(text).unwrap();
        assert_eq!(parsed.calls.len(), 2);
        assert_eq!(parsed.calls[0].0, "file_write");
        assert!(parsed.calls[0].1["content"].as_str().unwrap().contains("{шаблон}"));
        assert_eq!(parsed.calls[1].0, "bash");
    }

    #[test]
    fn test_markdown_style_from_real_transcript() {
        // Реальный формат из Telegram-переписки 26.08 (deepseek через
        // llama.cpp): имя после маркера, аргументы под "**arguments**".
        let text = "Сейчас создам файл.\n\
                    **tool_call** bash\n\
                    **arguments** {\"command\":\"touch /home/marsel/test_create.txt && echo ok\"}";
        let parsed = extract_tool_calls(text).expect("markdown-вызов должен распознаваться");
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].0, "bash");
        assert!(parsed.calls[0].1["command"]
            .as_str()
            .unwrap()
            .contains("touch /home/marsel/test_create.txt"));
        assert_eq!(parsed.cleaned, "Сейчас создам файл.");
    }

    #[test]
    fn test_prose_mention_without_json_is_ignored() {
        // Слово "tool_call" в прозе БЕЗ JSON рядом — не вызов.
        let text = "Я могу использовать tool_call механизм, но сейчас просто отвечаю текстом.";
        assert!(extract_tool_calls(text).is_none());
    }

    #[test]
    fn test_deepseek_style_tokens() {
        // Формат deepseek: имя инструмента идёт ОТДЕЛЬНОЙ строкой после
        // разделителя <tool_sep>, аргументы — следующим JSON-объектом.
        let text = "Вызываю инструмент.\n<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>function<｜tool▁sep｜>bash\n{\"command\": \"pwd\"}<｜tool▁call▁end｜><｜tool▁calls▁end｜>";
        let parsed = extract_tool_calls(text).expect("deepseek-вызов должен распознаться");
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].0, "bash");
        assert_eq!(parsed.calls[0].1["command"].as_str().unwrap(), "pwd");
        assert_eq!(parsed.cleaned, "Вызываю инструмент.");
    }

    #[test]
    fn test_arguments_as_json_string() {
        let text = r#"<tool_call>{"name":"glob","arguments":"{\"pattern\":\"**/*.rs\"}"}</tool_call>"#;
        let parsed = extract_tool_calls(text).unwrap();
        assert_eq!(parsed.calls[0].1["pattern"].as_str().unwrap(), "**/*.rs");
    }

    #[test]
    fn test_parameters_key_also_accepted() {
        let text = r#"<tool_call>{"name":"t","parameters":{"a":1}}</tool_call>"#;
        let parsed = extract_tool_calls(text).unwrap();
        assert_eq!(parsed.calls[0].1["a"], json!(1));
    }

    #[test]
    fn test_no_false_positive_on_plain_text_or_code() {
        assert!(extract_tool_calls("Обычный ответ без вызовов.").is_none());
        // Обычный код-блок без name/arguments — НЕ вызов.
        assert!(extract_tool_calls("Пример:\n```json\n{\"foo\": 1}\n```").is_none());
    }

    #[test]
    fn test_unterminated_json_is_dropped_safely() {
        let text = r#"<tool_call>{"name": "bash", "arguments": {"command": "echo hi""#;
        assert!(extract_tool_calls(text).is_none());
    }
}

/// Адаптер для Custom endpoint, который печатает формальный вызов функции
/// в `content` вместо OpenAI `message.tool_calls`. В отличие от эвристики,
/// список имён и порядок аргументов берутся из реально отправленных schemas.
/// Принимается только синтаксис `tool_name(json_arg, ...)`; обычный текст
/// никогда не интерпретируется как команда.
pub fn extract_schema_function_call(text: &str, schemas: &[Value]) -> Option<ParsedToolCalls> {
    let mut candidate: Option<(usize, String, Vec<String>)> = None;
    for schema in schemas {
        let function = schema.get("function")?;
        let name = function.get("name")?.as_str()?;
        let mut params: Vec<String> = function.pointer("/parameters/required")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        // Optional fields can follow required positional arguments. Sort makes
        // the fallback deterministic; standard schemas should mark positional
        // arguments as required.
        if let Some(properties) = function.pointer("/parameters/properties").and_then(Value::as_object) {
            let mut optional: Vec<String> = properties.keys().filter(|key| !params.contains(*key)).cloned().collect();
            optional.sort();
            params.extend(optional);
        }
        let needle = format!("{name}(");
        for (start, _) in text.match_indices(&needle) {
            let valid_boundary = text[..start].chars().next_back()
                .map(|c| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(true);
            if valid_boundary && candidate.as_ref().map(|(best, _, _)| start < *best).unwrap_or(true) {
            candidate = Some((start, name.to_owned(), params.clone()));
            }
        }
    }
    let (call_start, name, params) = candidate?;
    let after_open = call_start + name.len() + 1;
    let (input, consumed) = parse_positional_json_arguments(&text[after_open..], &params)?;
    let tail = &text[after_open + consumed..];
    if !tail.trim_start().starts_with(')') {
        return None;
    }
    let line_start = text[..call_start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    Some(ParsedToolCalls {
        calls: vec![(name, Value::Object(input))],
        cleaned: text[..line_start].trim().to_owned(),
    })
}

fn parse_positional_json_arguments(s: &str, names: &[String]) -> Option<(serde_json::Map<String, Value>, usize)> {
    let mut values = serde_json::Map::new();
    let mut cursor = 0usize;
    loop {
        let rest = &s[cursor..];
        let skipped = rest.len() - rest.trim_start().len();
        cursor += skipped;
        let rest = &s[cursor..];
        if rest.starts_with(')') {
            return Some((values, cursor));
        }
        let key = names.get(values.len())?.clone();
        let (value, consumed) = take_json_argument(rest)?;
        values.insert(key, value);
        cursor += consumed;
        let rest = &s[cursor..];
        let skipped = rest.len() - rest.trim_start().len();
        cursor += skipped;
        if s[cursor..].starts_with(',') {
            cursor += 1;
        } else if s[cursor..].starts_with(')') {
            return Some((values, cursor));
        } else {
            return None;
        }
    }
}

fn take_json_argument(s: &str) -> Option<(Value, usize)> {
    let raw = match s.as_bytes().first()? {
        b'"' => take_json_string(s)?,
        b'{' | b'[' => take_balanced_json(s)?,
        _ => {
            let end = s.find(|c: char| c == ',' || c == ')').unwrap_or(s.len());
            s[..end].trim_end()
        }
    };
    Some((serde_json::from_str(raw).ok()?, raw.len()))
}

#[cfg(test)]
mod custom_adapter_tests {
    use super::*;
    use serde_json::json;

    fn schemas() -> Vec<Value> {
        vec![json!({"type":"function","function":{"name":"file_write","parameters":{"type":"object","properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"]}}})]
    }

    #[test]
    fn schema_adapter_parses_multi_argument_custom_call() {
        let parsed = extract_schema_function_call(
            "Создаю файл.\n`file_write(\"Tests/test.txt\", \"Привет, мир\")`", &schemas()
        ).expect("формальный вызов должен распознаться");
        assert_eq!(parsed.calls[0].0, "file_write");
        assert_eq!(parsed.calls[0].1["file_path"], json!("Tests/test.txt"));
        assert_eq!(parsed.calls[0].1["content"], json!("Привет, мир"));
        assert_eq!(parsed.cleaned, "Создаю файл.");
    }
}
