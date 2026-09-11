# ОТЧЁТ: полное живое тестирование Гефеста

Дата: 2026-09-11 · 150 тестов · прогоны с реальной LLM (Nvidia nemotron-3-super-120b)

## 1. Сборка и статика
| Проверка | Результат |
|---|---|
| cargo build --release | ✅ (20 МБ, LTO) |
| cargo test | ✅ 150 passed (8 прогонов подряд без флейков) |
| cargo clippy | 141 warning — стилистика, 0 ошибок |
| cargo check --target x86_64-pc-windows-gnu | ✅ 0 ошибок |
| Сборка Windows-бинарника | ✅ hephaestus-rs.exe (19 МБ, PE32+) |

## 2. Живой e2e прогон 1 (файлы, bash, БД, документы)
Задачи через `--simple` с реальной моделью. Все tool-вызовы — completed:

| Инструмент | Проверка | Артефакт |
|---|---|---|
| file_write | создал task1.txt = E2E-OK | test/results/task1.txt |
| file_read | прочитал, подтвердил содержимое | — |
| file_edit | E2E-OK → E2E-EDITED | на диске подтверждено |
| grep | нашёл слово в каталоге | completed |
| bash | echo → bash_out.txt = BASH-RUN | на диске |
| file_exists | подтвердил существование | completed |
| db_query (universal) | CREATE + INSERT + SELECT | results/e2e.db: kv=('alpha','one') |
| db_connections | показал подключения, креды маской | completed |
| document_create pptx | валидный PPTX | report.pptx: 2 слайда, ZIP-структура ок |

## 3. Живой e2e прогон 2 (todo, память, секреты, git, web, кэш, анализ)
| Инструмент | Статус |
|---|---|
| todo_write / todo_read | ✅ |
| memory_file_save / memory_file_list | ✅ |
| secret_set / secret_get / secret_list | ✅ |
| git status --short | ✅ (после фикса ниже) |
| web_fetch (https://example.com) | ✅ |
| cache_set / cache_get | ✅ |
| repomap_scan | ✅ |
| project_analyze | ✅ |

## 4. Тесты инструментов, не покрытых LLM-прогоном (12 новых live-тестов)
cache_set/get/stats · board_create/list/update_status · workflow_run (2 шага) ·
parallel_exec (2 параллельных) · background_run/status · verify_result (ok+fail
ветки) · secret_store roundtrip · cron_create/list/delete · git с аргументами ·
diagram pub-структуры.

## 5. Найденные и исправленные баги (все — через живое тестирование)

| # | Баг | Фикс |
|---|---|---|
| 1 | MCP/конфиг/telegram/monitoring/memory/learning/логи игнорировали HEPHAESTUS_HOME | единый механизм во всех модулях |
| 2 | --simple/--voice/--telegram не ставили clean-shutdown → шт. выход считался крашем | mark_clean_shutdown во всех режимах |
| 3 | Каждый перезапуск плодил пустую сессию | пустая переиспользуется |
| 4 | Конфликт: старый sqlite db_query затирал универсальный | старые db_* убраны из регистрации |
| 5 | [[permissions.rules]] в TOML не матчился на Vec<RuleConfig> | обёртка PermissionSettings |
| 6 | **Snapshot-лавина: снапшоты внутри work-tree включали сами себя → 96 ГБ и oom-killer (git 12 ГБ RSS)** | exclude HEPHAESTUS_HOME/снапшотов + регресс-тест |
| 7 | git-tool не принимал "status --short" | первый токен = action |
| 8 | diagram не видел `pub struct` | опциональный pub-префикс в regex |
| 9 | run_tests мог стать жертвой/виновником OOM при параллельных сборках | memory-guard: отказ при MemAvailable < 1.5 ГБ |
| 10 | env-флейки в тестах (гонка HEPHAESTUS_HOME) | общий TEST_ENV_LOCK + уникальные имена |

## 6. Windows-совместимость (статическое ревью + сборка)
- ✅ Компилируется весь проект, собран exe
- ✅ Оболочка: PowerShell/cmd через shell.rs, паттерны безопасности Windows
- ✅ Буфер: Set-Clipboard; пути через dirs; SQLite bundled; crossterm native
- ⚠️ Требует живой проверки на Windows-машине: TUI-рендер, rwhisper/voice, MCP-stdio, telegram long-poll
- ⚠️ sudo-интерактив отсутствует по дизайну (UAC — отдельно); ssh работает

## 7. Артефакты
- test/results/ — task1.txt, bash_out.txt, report.pptx, e2e.db, run*.log, tasks*.txt
- test/live-home*/ — изолированные дома прогонов (state.db с транскриптами)

## 8. Итого
**150 тестов, 8 стабильных прогонов, 2 живых e2e-прогона с реальной LLM,
10 исправленных багов, Windows-бинарник собран.** Функционал подтверждён:
файлы, шелл, БД (3 СУБД + Redis), документы/презентации, память/скиллы,
permissions-as-data (audit trail), recovery, сессии, MCP, git, web, кэш,
планы, фоновые задачи, векторный поиск.

## Реальное время: TUI через pty (итоговая серия pty3–pty7)

Прогон: `test/pty_driver.py` (реальный терминал 120×40, сценарий test/tui-scenario.txt,
живой LLM nvidia/nemotron-3-super-120b-a12b), проверка экрана/лога: `test/check_tui.py` (pyte).

### Итог: 10/11 контрольных точек (pty7)
Рамка «Диалог», промпт ввода, file_write, diff-блок, **вопрос разрешения**,
**/allow**, **Esc-прерывание**, /undo, /sessions, /toolcalls — ✅.
«Ошибка LLM» — N/A (в pty7 ошибок не было; отображение ошибок подтверждено в pty5/451).

### Найдено и исправлено ЖИВЬЁМ (юнит-тесты это ловить не могли)
1. **Дедлок старта TUI** (repl.rs): два `agent.lock().await` в одном выражении
   миграции session.json → TUI навсегда зависал до первого кадра.
2. **Undo уничтожал state.db и снапшоты** (snapshots.rs/bootstrap.rs):
   относительный HEPHAESTUS_HOME → `strip_prefix` давал Err → исключения
   info/exclude не писались → снапшот включал сам себя + state.db, undo их удалял.
   Фикс: `hephaestus_home()` абсолютизирует путь; обе стороны strip_prefix
   абсолютизируются. Регресс-тест `relative_home_inside_worktree_is_excluded`.
3. **`rm файл` проходил без подтверждения** (security.rs): rm-семейство было Low;
   теперь High → вопрос. Старый тест `ordinary_rm_is_not_high_risk` кодифицировал
   баг — заменён на `ordinary_rm_requires_confirmation`.
4. **Esc не прерывал ход** (repl.rs): для ходов через очередь busy=false →
   Esc чистил ввод. Теперь `busy || queued_pending > 0`; плюс LLM-вызов
   обёрнут в `tokio::select!` c `interrupt::wait_interrupt` — блокирующий
   вызов (custom-провайдеры) рвётся по Esc.
5. **Аудит разрешений не писался** (bash_tool/db_universal/file_tools):
   risk-based и rule-based asks теперь пишут granted/denied в permission_log.
6. **Двойное вложение логов** (logging/learning при HEPHAESTUS_HOME) —
   логи уезжали в `$HOME/.hephaestus/logs` вместо `$HEPHAESTUS_HOME/logs`.

### Проверено живьём ранее в этой серии
- nvidia API:间歇ный HTTP 451 (geo-флап) — агент переживает, ошибки отображаются.
- ollama: num_ctx 4096 мало для системного промпта → HTTP 400 (задокументировано).
- state.db: 12 таблиц, инкрементальная запись user/tool/assistant подтверждена.
