//! Конфигурация агента — провайдер/модель/base_url, сохраняется в
//! `~/.hephaestus/config.toml`. Раньше в `main()` эти параметры были
//! жёстко зашиты (`LLMProvider::Ollama` + `"llama3.2:3b"`) и менялись
//! только правкой исходника — отсюда и жалоба "не могу подключить ни
//! одну модель". Теперь: `/provider` и `/model` в REPL меняют их на
//! лету и сохраняют сюда, а при следующем запуске `load()` подхватывает
//! сохранённое.
//!
//! API-ключи облачных провайдеров (Anthropic/OpenAI/OpenRouter) в файл
//! НЕ пишутся — только берутся из переменных окружения, чтобы не
//! хранить секреты в открытом конфиге на диске. Custom — другое дело:
//! у самостоятельно поднятых OpenAI-совместимых прокси ключ часто вообще
//! не проверяется, сюда он пишется как обычная (не секретная) настройка.
//!
//! **Профили** (`profiles`) — по запросу пользователя: "хочу чтобы в
//! конфиге была поддержка нескольких провайдеров" (свой ik-llama.cpp
//! + Ollama + т.д. одновременно, с переключением одной командой вместо
//! повторного ввода base_url каждый раз). Именованные наборы
//! provider+model+base_url+api_key, сохраняются и переключаются через
//! `/provider save <имя>` / `/provider use <имя>` в `repl.rs`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use serde_json::Value;

use crate::llm::{LLMConfig, LLMProvider};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// api_key для Custom-провайдера — В ОТЛИЧИЕ от облачных, сюда
    /// сохраняется (не только из окружения), т.к. у Custom это обычно
    /// не секрет, а произвольная строка-заглушка для прокси, которую
    /// неудобно каждый раз задавать через переменную окружения заново.
    #[serde(default)]
    pub custom_api_key: Option<String>,
    /// Имя переменной окружения для ключа активного Custom-профиля. Это
    /// позволяет иметь NVIDIA_API_KEY, GROQ_API_KEY и ключи своих прокси,
    /// не копируя секрет в config.toml.
    #[serde(default)]
    pub custom_api_key_env: Option<String>,
    /// Hermes-совместимые расширения Custom/OpenAI-compatible профиля.
    /// Секреты в extra_headers не выводятся интерфейсом и предпочтительно
    /// должны ссылаться на переменные окружения, а не храниться в TOML.
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    #[serde(default)]
    pub extra_body: Option<Value>,
    /// Именованные сохранённые профили — см. заголовок файла.
    #[serde(default)]
    pub profiles: HashMap<String, ProviderProfile>,
    /// Рабочая директория инструментов (/work_dir в REPL, /workdir в
    /// Telegram). None — использовать директорию запуска процесса.
    /// Сохраняется, чтобы выбор пережил перезапуск; меняется ТОЛЬКО
    /// явным подтверждением пользователя ("доверяю этой директории"),
    /// не из чата с LLM.
    #[serde(default)]
    pub work_dir: Option<String>,
    /// SMALL MODEL для служебных задач (сжатие контекста, подсказки
    /// следующих действий). Пример: small_model = "qwen2.5:3b" — сводки
    /// делает дешёвая локальная модель, основная жжёт контекст только
    /// на полезную работу. None — использовать основную модель.
    #[serde(default)]
    pub small_model: Option<String>,
    /// PERMISSIONS-AS-DATA: декларативные правила разрешений, решают без
    /// вопроса человеку. Порядок = приоритет, последнее совпадение выигрывает:
    ///
    ///   [[permissions.rules]]
    ///   tool = "bash"        # bash | edit | read | delete | *
    ///   pattern = "git *"    # glob по команде/пути
    ///   action = "allow"     # allow | ask | deny
    ///
    /// Поверх встроенных дефолтов (защита .env/ключей → ask).
    #[serde(default)]
    pub permissions: Vec<crate::permissions::RuleConfig>,
    /// УНИВЕРСАЛЬНЫЕ БД (db_universal.rs): подключения для db_query
    /// (sqlite/postgres/mysql через sqlx) и db_redis. Секреты — через
    /// {env:VAR} в url, не в конфиге:
    ///
    ///   [databases.prod]
    ///   url = "postgres://app:{env:PG_PASSWORD}@10.0.0.5/appdb"
    ///   read_only = true
    ///   description = "прод, только чтение"
    #[serde(default)]
    pub databases: HashMap<String, crate::tools::db_universal::DatabaseConfig>,
}

/// Один сохранённый профиль подключения — тот же набор полей, что и
/// "активные" в `AgentConfig`, но под именем, которое можно вызвать
/// одной командой (`/provider use ik-llama`), не вводя base_url заново.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    #[serde(default)]
    pub extra_body: Option<Value>,
}

fn default_temperature() -> f32 {
    0.1
}
fn default_max_tokens() -> u32 {
    4096
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            provider: "ollama".to_string(),
            model: "llama3.2:3b".to_string(),
            base_url: Some("http://localhost:11434".to_string()),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            custom_api_key: None,
            custom_api_key_env: None,
            extra_headers: HashMap::new(),
            extra_body: None,
            profiles: HashMap::new(),
            work_dir: None,
            small_model: None,
            permissions: Vec::new(),
            databases: HashMap::new(),
        }
    }
}

fn config_path() -> PathBuf {
    let mut p = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    p.push(".hephaestus");
    let _ = std::fs::create_dir_all(&p);
    p.push("config.toml");
    p
}

/// Результат загрузки — отдельным типом, а не `(Self, bool)`, чтобы
/// различать "файла нет" (нормально, первый запуск) и "файл ЕСТЬ, но
/// битый" (нужно явно сказать пользователю ПОЧЕМУ, а не тихо съехать
/// на дефолтную модель, которой у него может не быть — именно это
/// произошло с "почему определяется llama3.2:3b, которой у меня нет").
pub enum LoadResult {
    /// Конфиг успешно прочитан.
    Loaded(AgentConfig),
    /// Файла не было — это нормально, дефолт ожидаем.
    NotFound(AgentConfig),
    /// Файл есть, но не распарсился — вероятно, испорчен вручную
    /// (например, задвоены ключи `provider =` на верхнем уровне TOML —
    /// именно так выглядел присланный пользователем config.toml).
    ParseError { default: AgentConfig, error: String },
}

impl AgentConfig {
    /// Загрузить сохранённую конфигурацию. См. `LoadResult` — в отличие
    /// от прежней версии, различает "файла нет" и "файл битый", и во
    /// втором случае возвращает текст ошибки парсинга, а не молчит.
    pub fn load() -> LoadResult {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<AgentConfig>(&content) {
                Ok(cfg) => LoadResult::Loaded(cfg),
                Err(e) => LoadResult::ParseError { default: Self::default(), error: e.to_string() },
            },
            Err(_) => LoadResult::NotFound(Self::default()),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path();
        let content = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, content).map_err(|e| e.to_string())
    }

    pub fn config_file_path() -> String {
        config_path().display().to_string()
    }

    /// Сохранить ТЕКУЩИЕ активные provider/model/base_url/api_key как
    /// именованный профиль — и сразу на диск. `/provider save <имя>`.
    pub fn save_profile(&mut self, name: &str) -> Result<(), String> {
        let profile = ProviderProfile {
            provider: self.provider.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key: self.custom_api_key.clone(),
            api_key_env: self.custom_api_key_env.clone(),
            extra_headers: self.extra_headers.clone(),
            extra_body: self.extra_body.clone(),
        };
        self.profiles.insert(name.to_string(), profile);
        self.save()
    }

    /// Удалить именованный профиль. `/provider forget <имя>`.
    pub fn forget_profile(&mut self, name: &str) -> Result<bool, String> {
        let existed = self.profiles.remove(name).is_some();
        if existed {
            self.save()?;
        }
        Ok(existed)
    }

    pub fn list_profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.profiles.keys().cloned().collect();
        names.sort();
        names
    }

    /// Сохранить рабочую директорию в конфиг на диске. ВАЖНО: читает
    /// СВЕЖИЙ конфиг с диска и меняет только `work_dir` — так смена
    /// директории не затирает профили/подключение, сохранённые из
    /// другого места (REPL/Telegram) между нашими чтением и записью.
    pub fn set_saved_work_dir(path: &std::path::Path) -> Result<(), String> {
        let mut cfg = match Self::load() {
            LoadResult::Loaded(c) => c,
            LoadResult::NotFound(_) => Self::default(),
            LoadResult::ParseError { error, .. } => {
                return Err(format!("конфиг повреждён, не могу сохранить work_dir: {}", error));
            }
        };
        cfg.work_dir = Some(path.display().to_string());
        cfg.save()
    }

    /// Собрать `LLMConfig` для `create_llm_client()`.
    ///
    /// Облачные провайдеры (Anthropic/OpenAI/OpenRouter) требуют ключ
    /// в переменной окружения — если её нет, возвращаем `Err` с понятным
    /// сообщением, а не идём в API без ключа за невнятной 401.
    ///
    /// Custom работает с любым OpenAI-совместимым API: адрес берётся из
    /// `base_url` или `CUSTOM_BASE_URL`, а ключ — из `custom_api_key` или
    /// `CUSTOM_API_KEY`. Это позволяет отдельно задавать URL, ключ либо оба
    /// значения через `/provider custom`, не добавляя отдельный enum для
    /// каждого совместимого поставщика.
    pub fn to_llm_config(&self) -> Result<LLMConfig, String> {
        let provider = LLMProvider::from_str(&self.provider)
            .ok_or_else(|| format!("Неизвестный провайдер в конфиге: '{}'", self.provider))?;

        let api_key = if provider == LLMProvider::Custom {
            Some(
                self.custom_api_key
                    .clone()
                    .or_else(|| {
                        self.custom_api_key_env.as_deref().and_then(|name| {
                            (!name.trim().is_empty()).then(|| std::env::var(name).ok()).flatten()
                        })
                    })
                    .or_else(|| std::env::var("CUSTOM_API_KEY").ok())
                    .unwrap_or_else(|| "not-needed".to_string()),
            )
        } else {
            match provider.env_key_var() {
                Some(var) => match std::env::var(var) {
                    Ok(v) if !v.is_empty() => Some(v),
                    _ => {
                        return Err(format!(
                            "Провайдер '{}' требует API-ключ в переменной окружения {} — она не задана.\n\
                             export {}=\"...\" и повторите, либо выберите другого провайдера через /provider.",
                            provider.display_name(),
                            var,
                            var
                        ));
                    }
                },
                None => None,
            }
        };

        let base_url = self.base_url.clone()
            .or_else(|| {
                (provider == LLMProvider::Custom)
                    .then(|| std::env::var("CUSTOM_BASE_URL").ok())
                    .flatten()
                    .filter(|url| !url.trim().is_empty())
            })
            .or_else(|| provider.default_base_url().map(|s| s.to_string()));

        // Custom без base_url — явная ошибка ДО сетевого запроса, а не
        // необъяснимый 401 от чужого сервиса (см. `llm.rs::OpenAICompatibleClient::endpoint()`
        // — при пустом base_url молча уходит на api.openai.com).
        if provider == LLMProvider::Custom && base_url.as_deref().unwrap_or("").is_empty() {
            return Err(
                "Custom требует API-адрес (base_url): одного ключа недостаточно, чтобы понять, куда отправлять запрос.\n\
                 Укажите URL вместе с ключом: /provider custom <модель> <base_url> [ключ]\n\
                 либо сохраните адрес в CUSTOM_BASE_URL и передайте только ключ: /provider custom <модель> <ключ>\n\
                 Например для NVIDIA: /provider custom <модель> https://integrate.api.nvidia.com/v1 <ключ>\n\
                 ВАЖНО: base_url — это API-эндпоинт сервера (обычно оканчивается на /v1),\n\
                 а НЕ адрес веб-интерфейса чата в браузере (тот часто выглядит как\n\
                 http://host:port/#/chat/... — такой адрес рабочим НЕ будет: '#...' —\n\
                 это внутренняя маршрутизация браузерной страницы, браузер даже не\n\
                 отправляет эту часть на сервер, для API она бессмысленна)."
                    .to_string(),
            );
        }

        Ok(LLMConfig {
            provider,
            model: self.model.clone(),
            api_key,
            base_url,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            stream: false,
            extra_headers: self.extra_headers.clone(),
            extra_body: self.extra_body.clone(),
        })
    }
}
