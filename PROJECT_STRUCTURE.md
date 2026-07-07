# ⚡ Гефест — Структура проекта

## Дерево файлов

```
Hephaestus/
├── src/
│   │
│   ├── 🔥 ЯДРО АГЕНТА
│   ├── hephaestus.py          — Главный агент: chat(), execute_tool(), _init_tools()
│   ├── hephaestus_logo.py     — Анимация огня при запуске
│   ├── hephaestus_repl.py     — Интерактивный REPL: промпт, команды, спиннер
│   ├── llm_client.py          — Провайдеры LLM: Ollama, Anthropic, OpenAI, OpenRouter
│   │
│   ├── 📁 ФАЙЛОВЫЕ ИНСТРУМЕНТЫ
│   ├── real_tools.py          — file_read/write/edit/delete/copy/move, bash, glob, grep, git
│   │
│   ├── 🌐 ВЕБ
│   ├── web_tools.py           — Базовый web_fetch (requests + BeautifulSoup)
│   ├── web_surfer.py          — Полный веб-сёрфинг: поиск DDG/Bing, парсинг, deep research
│   ├── http_tools.py          — HTTP запросы (GET/POST/PUT/DELETE)
│   │
│   ├── 💾 ПАМЯТЬ И ДАННЫЕ
│   ├── memory_system.py       — Хранилище памяти (~/.hephaestus/memory.json)
│   ├── memory_tools.py        — memory_add/search/list/update/delete
│   ├── vector_search.py       — Векторный поиск по памяти (numpy)
│   │
│   ├── ✅ ЗАДАЧИ
│   ├── task_manager.py        — Хранилище задач (~/.hephaestus/tasks.json)
│   ├── task_tools.py          — task_create/list/update
│   │
│   ├── ⏰ РАСПИСАНИЕ
│   ├── scheduler.py           — Хранилище cron задач (~/.hephaestus/cron.json)
│   ├── cron_tools.py          — cron_create/list/delete
│   │
│   ├── 🔌 НАВЫКИ
│   ├── skill_system.py        — Хранилище навыков (~/.hephaestus/skills.json)
│   ├── skill_tools.py         — skill_register/list/execute/unregister
│   ├── skill_learner.py       — Автообучение из /goal → сохраняет планы как навыки
│   │
│   ├── 📓 NOTEBOOK
│   ├── notebook_tools.py      — notebook_create/read/edit (.ipynb)
│   │
│   ├── 🗄️ БАЗА ДАННЫХ
│   ├── database_tools.py      — db_query/schema (SQLite, PostgreSQL, MySQL)
│   │
│   ├── 🐳 DOCKER
│   ├── docker_tools.py        — docker_list/run/stop/logs/exec
│   │
│   ├── 👁️ OCR
│   ├── ocr_tool.py            — ocr_extract/status/languages (Tesseract)
│   │
│   ├── 📊 ДИАГРАММЫ
│   ├── rich_diagrams.py       — diagram_class/tree/deps/mermaid (Rich ASCII)
│   ├── diagram_generator.py   — Старый генератор (Mermaid код)
│   │
│   ├── 📖 ДОКУМЕНТАЦИЯ
│   ├── doc_generator.py       — doc_generate/readme/docstrings (AST парсинг)
│   │
│   ├── 🗺️ КАРТА РЕПОЗИТОРИЯ
│   ├── repomap.py             — repomap/repomap_file (как в Aider)
│   │
│   ├── 🎯 РЕЖИМ ЦЕЛИ
│   ├── goal_mode.py           — /goal: Plan→Execute→Learn→Report
│   │
│   ├── 🤖 TELEGRAM
│   ├── telegram_bot.py        — Управление Гефестом через Telegram
│   │
│   ├── 💬 СЕССИИ
│   ├── session_manager.py     — /save /load /sessions (~/.hephaestus/sessions/)
│   ├── context_learning.py    — Запоминает успехи/ошибки → подсказки в промпт
│   │
│   ├── ⚙️ GITHUB
│   ├── github_actions_tool.py — github_workflow (создаёт .github/workflows/)
│   │
│   ├── 📦 ПАКЕТЫ И ТЕСТЫ
│   ├── package_tools.py       — package_install/search (pip, npm, apt)
│   ├── test_tools.py          — test_run/generator (pytest, unittest)
│   ├── project_tools.py       — project_analyzer
│   │
│   └── 🔧 ВСПОМОГАТЕЛЬНЫЕ
│       ├── advanced_tools.py      — HttpRequestTool и другие
│       ├── agent_tool.py          — Базовый класс инструмента
│       ├── ask_user_question.py   — Запрос подтверждения у пользователя
│       ├── background_agent.py    — Фоновое выполнение задач
│       ├── caching.py             — Кэширование запросов к LLM
│       ├── command_validator.py   — Валидация bash команд
│       ├── config_tools.py        — Работа с конфигами (yaml/toml)
│       ├── confirmation.py        — Подтверждения опасных операций
│       ├── context_manager.py     — Управление длиной контекста
│       ├── error_handler.py       — Обработка ошибок
│       ├── logging_system.py      — Логирование в файл
│       ├── model_detector.py      — Автоопределение возможностей модели
│       ├── monitoring.py          — Метрики и мониторинг
│       ├── plan_mode.py           — Режим планирования (устарел, см. goal_mode)
│       ├── progress_display.py    — Прогресс-бары
│       ├── result_verifier.py     — Верификация результатов
│       ├── secret_store.py        — Хранилище секретов/ключей
│       ├── security.py            — Проверка безопасности команд
│       ├── smart_compression.py   — Сжатие контекста
│       ├── streaming.py           — Streaming вывод
│       ├── streaming_tool_calling.py — Streaming + tool calling
│       ├── system_tools.py        — Системный мониторинг
│       ├── tool_calling.py        — Базовый tool calling
│       ├── tool_execution_pipeline.py — Пайплайн выполнения инструментов
│       ├── workflow_system.py     — Система воркфлоу
│       └── worktree_tools.py      — Git worktree операции
│
├── requirements.txt    — Зависимости Python
├── README.md           — Документация
├── CHANGELOG.md        — История версий
└── PROJECT_STRUCTURE.md — Этот файл
```

## Хранилище данных (~/.hephaestus/)

```
~/.hephaestus/
├── memory.json          — Долговременная память агента
├── tasks.json           — Задачи
├── cron.json            — Расписание cron задач
├── skills.json          — Зарегистрированные навыки
├── learned_skills.json  — Навыки усвоенные из /goal
├── learning.json        — Контекстное обучение (успехи/ошибки)
├── telegram_token.txt   — Токен Telegram бота
├── telegram_users.json  — Разрешённые пользователи бота
└── sessions/
    ├── ses_20260614_120000.json
    └── ses_20260615_183000.json
```

## Провайдеры LLM (llm_client.py)

| Провайдер | Переменная окружения | Пример модели |
|-----------|---------------------|---------------|
| Ollama | `OLLAMA_HOST` | `qwen2.5-coder:7b-instruct-q4_K_M` |
| Anthropic | `ANTHROPIC_API_KEY` | `claude-sonnet-4-6` |
| OpenAI | `OPENAI_API_KEY` | `gpt-4o` |
| OpenRouter | `OPENROUTER_API_KEY` | `qwen/qwen-2.5-coder-32b-instruct` |
| KoboldCPP | `KOBOLDCPP_URL` | `local-model` |

## Запуск

```bash
# Стандартный запуск
python3 -m src.hephaestus --provider ollama --model qwen2.5-coder:7b-instruct-q4_K_M

# С debug режимом
python3 -m src.hephaestus --debug

# С автокоммитами в git
python3 -m src.hephaestus --auto-commit

# Telegram бот
python3 -m src.telegram_bot --token YOUR_TOKEN

# Одиночный запрос
python3 -m src.hephaestus "создай hello.py"
```

## REPL команды

| Команда | Описание |
|---------|----------|
| `/help` | Справка |
| `/tools` | Все 52 инструмента |
| `/goal <цель>` | Goal Mode — планирует и выполняет |
| `/skills` | Усвоенные навыки |
| `/repomap` | Карта репозитория |
| `/sessions` | Сохранённые сессии |
| `/save` | Сохранить текущую сессию |
| `/load <id>` | Загрузить сессию |
| `/reset` | Очистить историю |
| `/clear` | Очистить экран |
| `/learn` | Статистика обучения |
| `/exit` | Выход (сессия сохраняется) |
