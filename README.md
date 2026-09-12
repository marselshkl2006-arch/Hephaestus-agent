# Гефест (Hephaestus) — AI Coding Agent

AI-агент для разработки на Rust: TUI-интерфейс, 40+ инструментов (файлы, bash,
git, БД sqlite/postgres/mysql/redis, web, cron, суб-агенты), голосовой ввод,
Telegram-бот, undo через теневые git-снапшоты, SQLite-состояние с recovery.

> Python-версия (v14) сохранена в ветке `legacy-python`.

## Windows — быстрый старт (PowerShell)

```powershell
# 1. Скачать последний релиз: https://github.com/marselshkl2006-arch/Hephaestus-agent/releases
#    файл hephaestus-rs.exe — распаковать/сохранить в любую папку

# 2. Разрешить запуск (если SmartScreen ругается):
Unblock-File .\hephaestus-rs.exe

# 3. Запустить:
.\hephaestus-rs.exe
```

Первый запуск создаст `~\.hephaestus\` с примерами конфигов
(`config.example.toml`, `mcp.example.toml`) — впишите свой LLM-провайдер
(OpenAI/Ollama/custom OpenAI-совместимый) в `config.toml` и запустите снова.

Без конфигурации работает с локальной Ollama по умолчанию.

## Linux / macOS

```bash
git clone https://github.com/marselshkl2006-arch/Hephaestus-agent.git
cd Hephaestus-agent
cargo build --release   # или: cargo install --path .
./target/release/hephaestus-rs
```

## Сборка из исходников (Windows)

```powershell
git clone https://github.com/marselshkl2006-arch/Hephaestus-agent.git
cd Hephaestus-agent
cargo build --release --target x86_64-pc-windows-gnu   # нужен mingw-w64
```

## Возможности
- TUI: диффы, markdown, очередь ходов, Esc-прерывание, /sessions, /undo
- Разрешения: [[permissions.rules]] в config.toml, аудит в permission_log
- Состояние: SQLite (WAL), recovery после крашей, kill -9 переживает
- Голос: Whisper STT; Telegram-бот с автозапуском
- Безопасность: hard-block паттерны, подтверждение rm/db_write

## Статус
151 unit-тест · живые pty-прогоны TUI · Windows exe (x86_64-pc-windows-gnu,
только системные DLL) — подробности в test/REPORT.md.
