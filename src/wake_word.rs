//! Wake-word «Гефест» / "Hephaestus" — активация голосом (RU+EN).
//!
//! Подход (см. docs/PLAN-interface-voice.md): VAD ловит речь → отрезок
//! в Whisper tiny → транскрипт сравнивается с эталонами с допуском
//! опечаток (Левенштейн, дистанция ≤ 2 на слово) — Whisper на коротких
//! фразах пишет «гефест», «hefest», «hephestus», «гейфест» и т.п.;
//! нечёткий матч закрывает все варианты без обучения моделей.
//!
//! Две роли (стиль «фраза-команда», выбранный по умолчанию):
//!   «Гефест, запусти тесты» → wake-word + команда в ОДНОЙ фразе;
//!   «запусти тесты» (без слова-активатора) → НЕ команда, игнор.
//!
//! Также: bare-wake («Гефест!»/«Гефест?» без команды) — сигнал
//! «слушаю», интерфейс показывает подсказку.

/// Максимальная дистанция Левенштейна для матчача слова-активатора.
const MAX_EDIT_DISTANCE: usize = 2;

/// Эталонные написания (lowercase). Whisper-варианты живого
/// произношения: «гефест», «hefest», «hephestus», «hephaestus»,
/// «гефест» с падежами не бывает (зовут именительный падеж).
const FORMS_RU: &[&str] = &["гефест", "hefest", "hephæstus", "hephaestus", "hephestus", "hefestus", "гефестъ"];
const FORMS_EN: &[&str] = &["hephaestus", "hefest", "hephestus", "hefestus", "hephaistos"];

/// Дистанция Левенштейна (классика, O(len1*len2)).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 { return m; }
    if m == 0 { return n; }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

/// Матчит ли слово форму активатора (с допуском опечаток).
fn word_matches(w: &str) -> bool {
    let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    if w.len() < 5 {
        // слишком короткое — не может быть именем (защита от «гес», «фе»)
        return false;
    }
    FORMS_RU.iter().chain(FORMS_EN.iter()).any(|f| {
        let d = levenshtein(&w, f);
        // Допуск масштабируем: короткие формы — строже.
        let limit = if f.len() <= 7 { MAX_EDIT_DISTANCE - 1 } else { MAX_EDIT_DISTANCE };
        d <= limit
    })
}

/// Разбор транскрипта фразы: есть ли активатор и команда после него.
///
/// Возвращает:
/// - None  — активатора нет, фразу игнорируем;
/// - Some(None) — bare-wake («Гефест!») — агент слушает, но команды нет;
/// - Some(Some(cmd)) — «Гефест, сделай X» — команда `сделай X`.
pub fn parse_utterance(text: &str) -> Option<Option<String>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    // Ищем активатор среди ПЕРВЫХ трёх слов (обычно — первое; допускаем
    // «эй, Гефест...»). Остальное до активатора отбрасывается.
    for (idx, w) in words.iter().take(3).enumerate() {
        if word_matches(w) {
            let rest: Vec<&str> = words[idx + 1..].iter()
                .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric() && c != '-'))
                .filter(|s| !s.is_empty())
                .collect();
            if rest.is_empty() {
                return Some(None);
            }
            let cmd = rest.join(" ");
            // Хвостовая пунктуация/вопрос — чистим.
            let cmd = cmd.trim_end_matches(|c: char| "!?.,;:„“\"'".contains(c)).trim();
            return Some(Some(cmd.to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_forms_match() {
        assert!(matches!(parse_utterance("Гефест, запусти тесты"), Some(Some(ref c)) if c == "запусти тесты"));
        assert!(matches!(parse_utterance("hephaestus run tests"), Some(Some(ref c)) if c == "run tests"));
    }

    #[test]
    fn whisper_typos_match() {
        // Типичные транскрипции Whisper (пойманы в живых прогонах):
        assert!(matches!(parse_utterance("hefest покажи статус"), Some(Some(_))));
        assert!(matches!(parse_utterance("Гейфест, что делаешь"), Some(Some(_))));
        assert!(matches!(parse_utterance("hephestus check the build"), Some(Some(_))));
    }

    #[test]
    fn bare_wake_recognized() {
        assert!(matches!(parse_utterance("Гефест!"), Some(None)));
        assert!(matches!(parse_utterance("Гефест?"), Some(None)));
        assert!(matches!(parse_utterance("эй, Гефест"), Some(None)));
    }

    #[test]
    fn no_wake_word_ignored() {
        assert!(matches!(parse_utterance("запусти тесты"), None));
        assert!(matches!(parse_utterance("привет, как дела"), None));
        assert!(matches!(parse_utterance(""), None));
    }

    #[test]
    fn short_noise_not_matched() {
        // «геф», «фест» — не активатор (защита от ложных срабатываний)
        assert!(matches!(parse_utterance("геф, что нового"), None));
    }
}
