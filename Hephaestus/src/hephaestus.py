#!/usr/bin/env python3
"""
⚡ ГЕФЕСТ — AI Coding Agent
Бог кузнечного дела и инструментов.

Использование:
    python -m src.hephaestus                          # Интерактивный режим
    python -m src.hephaestus "напиши hello world"     # Одиночный запрос
    python -m src.hephaestus --provider ollama        # Ollama
    python -m src.hephaestus --provider openrouter    # OpenRouter
    python -m src.hephaestus --provider anthropic     # Claude API
    python -m src.hephaestus --model llama3.2:3b      # Выбор модели
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

from .llm_client import (
    LLMConfig, LLMMessage, LLMProvider, LLMResponse,
    auto_detect_provider, create_llm_client,
)
from .real_tools import (
    BashTool, FileEditTool, FileReadTool, FileWriteTool,
    GitTool, GlobTool, GrepTool, ToolResult, create_tools,
)
from .session_manager import SessionManager
from .context_learning import ContextLearning
from .hephaestus_repl import (
    HephaestusREPL,
    ForgeSpinner,
    show_success, show_error, show_warning, show_info,
    show_tool_call, show_tool_result, show_thinking, show_debug, print_response,
    console,
)

# Опциональные модули — не падаем если нет
def _try_import(fn):
    try:
        return fn()
    except Exception:
        return {}

def _load_web_tools():
    from .web_tools import create_web_tools
    return create_web_tools()

def _load_memory_tools():
    from .memory_tools import create_memory_tools
    return create_memory_tools()

def _load_task_tools():
    from .task_tools import create_task_tools
    return create_task_tools()

def _load_cron_tools():
    from .cron_tools import create_cron_tools
    return create_cron_tools()

def _load_skill_tools():
    from .skill_tools import create_skill_tools
    return create_skill_tools()

def _load_notebook_tools():
    from .notebook_tools import create_notebook_tools
    return create_notebook_tools()

def _load_docker_tools():
    from .docker_tools import DockerTool
    return {"docker": DockerTool()}

def _load_database_tools():
    from .database_tools import DatabaseQueryTool
    return {"database": DatabaseQueryTool()}

def _load_web_surfer():
    from .web_surfer import create_web_surfer
    return create_web_surfer()

def _load_ocr_tools():
    from .ocr_tool import OCRTool
    return {"ocr": OCRTool()}

def _load_diagram_tools():
    from .rich_diagrams import create_diagram_tools
    return create_diagram_tools()

def _load_doc_tools():
    from .doc_generator import DocGenerator
    return {"doc_generator": DocGenerator()}

def _load_github_tools():
    from .github_actions_tool import GithubActionsTool
    return {"github": GithubActionsTool()}


# ─────────────────────────────────────────────────────────────────────────────
# JSON Schema всех инструментов
# ─────────────────────────────────────────────────────────────────────────────

ALL_TOOL_SCHEMAS: dict[str, dict] = {
    "bash": {
        "description": "Выполнить bash команду в рабочей папке",
        "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}
    },
    "file_read": {
        "description": "Прочитать содержимое файла",
        "input_schema": {"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]}
    },
    "file_write": {
        "description": "Записать содержимое в файл (перезаписывает полностью)",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string"},
            "content": {"type": "string"}
        }, "required": ["file_path", "content"]}
    },
    "file_edit": {
        "description": "Заменить точный фрагмент текста в файле. old_text должен существовать дословно.",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string"},
            "old_text": {"type": "string"},
            "new_text": {"type": "string"}
        }, "required": ["file_path", "old_text", "new_text"]}
    },
    "file_delete": {
        "description": "Удалить файл",
        "input_schema": {"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]}
    },
    "file_move": {
        "description": "Переместить или переименовать файл",
        "input_schema": {"type": "object", "properties": {
            "source": {"type": "string"}, "destination": {"type": "string"}
        }, "required": ["source", "destination"]}
    },
    "file_copy": {
        "description": "Скопировать файл",
        "input_schema": {"type": "object", "properties": {
            "source": {"type": "string"}, "destination": {"type": "string"}
        }, "required": ["source", "destination"]}
    },
    "file_exists": {
        "description": "Проверить существование файла или папки",
        "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
    },
    "glob": {
        "description": "Найти файлы по паттерну (например **/*.py)",
        "input_schema": {"type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}
    },
    "grep": {
        "description": "Поиск текста в файлах по regex",
        "input_schema": {"type": "object", "properties": {
            "pattern": {"type": "string"},
            "path": {"type": "string", "description": "Папка или файл для поиска"}
        }, "required": ["pattern"]}
    },
    "git": {
        "description": "Выполнить git команду. Примеры: 'status', 'diff', 'log', 'add .', 'commit -m \"fix\"'",
        "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}
    },
    "web_search_deep": {
        "description": "Глубокий поиск — ищет и читает несколько страниц, собирает информацию",
        "input_schema": {"type": "object", "properties": {
            "query": {"type": "string", "description": "Поисковый запрос"},
            "depth": {"type": "integer", "description": "Сколько страниц прочитать (1-5, по умолчанию 3)"},
            "save_to": {"type": "string", "description": "Путь для сохранения результата"}
        }, "required": ["query"]}
    },
    "web_page": {
        "description": "Загрузить страницу и извлечь текст, ссылки или всё сразу",
        "input_schema": {"type": "object", "properties": {
            "url": {"type": "string", "description": "URL страницы"},
            "extract": {"type": "string", "description": "Что извлечь: text, links, both (по умолчанию text)"},
            "save_to": {"type": "string", "description": "Путь для сохранения"}
        }, "required": ["url"]}
    },
    "web_fetch": {
        "description": "Загрузить URL и вернуть содержимое",
        "input_schema": {"type": "object", "properties": {
            "url": {"type": "string"},
            "prompt": {"type": "string", "description": "Что извлечь"}
        }, "required": ["url"]}
    },
    "web_search": {
        "description": "Поиск в интернете",
        "input_schema": {"type": "object", "properties": {
            "query": {"type": "string"},
            "max_results": {"type": "integer"}
        }, "required": ["query"]}
    },
    "memory_add": {
        "description": "Сохранить факт или заметку в память",
        "input_schema": {"type": "object", "properties": {
            "content": {"type": "string"},
            "type": {"type": "string"},
            "tags": {"type": "string"},
            "importance": {"type": "integer"}
        }, "required": ["content"]}
    },
    "memory_search": {
        "description": "Поиск в памяти",
        "input_schema": {"type": "object", "properties": {
            "query": {"type": "string"},
            "type": {"type": "string"}
        }, "required": []}
    },
    "memory_list": {
        "description": "Список всех записей в памяти",
        "input_schema": {"type": "object", "properties": {"limit": {"type": "integer"}}, "required": []}
    },
    "memory_delete": {
        "description": "Удалить запись из памяти по ID",
        "input_schema": {"type": "object", "properties": {"mem_id": {"type": "string"}}, "required": ["mem_id"]}
    },
    "task_create": {
        "description": "Создать задачу",
        "input_schema": {"type": "object", "properties": {
            "subject": {"type": "string"},
            "description": {"type": "string"}
        }, "required": ["subject"]}
    },
    "task_list": {
        "description": "Список задач",
        "input_schema": {"type": "object", "properties": {}, "required": []}
    },
    "task_update": {
        "description": "Обновить задачу",
        "input_schema": {"type": "object", "properties": {
            "task_id": {"type": "string"},
            "status": {"type": "string"},
            "subject": {"type": "string"}
        }, "required": ["task_id"]}
    },
    "notebook_create": {
        "description": "Создать новый пустой Jupyter notebook",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Путь к новому .ipynb файлу"}
        }, "required": ["file_path"]}
    },
    "notebook_read": {
        "description": "Прочитать Jupyter notebook",
        "input_schema": {"type": "object", "properties": {"file_path": {"type": "string"}}, "required": ["file_path"]}
    },
    "notebook_edit": {
        "description": "Редактировать ячейку в Jupyter notebook",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string"},
            "cell_index": {"type": "integer"},
            "new_source": {"type": "string"}
        }, "required": ["file_path", "cell_index", "new_source"]}
    },
    "cron_create": {
        "description": "Создать запланированную cron задачу. schedule — cron выражение например '0 9 * * *'",
        "input_schema": {"type": "object", "properties": {
            "cron": {"type": "string", "description": "Cron выражение, например '0 9 * * *'"},
            "prompt": {"type": "string", "description": "Что должна делать задача"},
            "description": {"type": "string", "description": "Описание задачи"}
        }, "required": ["cron", "prompt"]}
    },
    "cron_list": {
        "description": "Список cron задач",
        "input_schema": {"type": "object", "properties": {}, "required": []}
    },
    "cron_delete": {
        "description": "Удалить cron задачу по ID",
        "input_schema": {"type": "object", "properties": {
            "task_id": {"type": "string", "description": "ID cron задачи (например task_1_123456)"}
        }, "required": ["task_id"]}
    },
    "skill_register": {
        "description": "Зарегистрировать новый навык — скрипт или файл",
        "input_schema": {"type": "object", "properties": {
            "name": {"type": "string", "description": "Название навыка"},
            "file_path": {"type": "string", "description": "Путь к файлу скрипта"},
            "description": {"type": "string", "description": "Описание что делает навык"}
        }, "required": ["name", "file_path"]}
    },
    "skill_list": {
        "description": "Список навыков",
        "input_schema": {"type": "object", "properties": {}, "required": []}
    },
    "skill_execute": {
        "description": "Выполнить навык по ID",
        "input_schema": {"type": "object", "properties": {"skill_id": {"type": "string"}}, "required": ["skill_id"]}
    },

    # === DOCKER ===
    "docker_list": {
        "description": "Список Docker контейнеров",
        "input_schema": {"type": "object", "properties": {
            "all": {"type": "boolean", "description": "Показать все включая остановленные"}
        }, "required": []}
    },
    "docker_run": {
        "description": "Запустить Docker контейнер",
        "input_schema": {"type": "object", "properties": {
            "image": {"type": "string", "description": "Docker образ"},
            "command": {"type": "string", "description": "Команда внутри контейнера"},
            "name": {"type": "string", "description": "Имя контейнера"},
            "ports": {"type": "string", "description": "Маппинг портов, например '8080:80'"},
            "detach": {"type": "boolean", "description": "Запустить в фоне"}
        }, "required": ["image"]}
    },
    "docker_stop": {
        "description": "Остановить Docker контейнер",
        "input_schema": {"type": "object", "properties": {
            "container": {"type": "string", "description": "Имя или ID контейнера"}
        }, "required": ["container"]}
    },
    "docker_logs": {
        "description": "Показать логи Docker контейнера",
        "input_schema": {"type": "object", "properties": {
            "container": {"type": "string", "description": "Имя или ID контейнера"},
            "tail": {"type": "integer", "description": "Последние N строк (по умолчанию 100)"}
        }, "required": ["container"]}
    },
    "docker_exec": {
        "description": "Выполнить команду внутри Docker контейнера",
        "input_schema": {"type": "object", "properties": {
            "container": {"type": "string", "description": "Имя или ID контейнера"},
            "command": {"type": "string", "description": "Команда для выполнения"}
        }, "required": ["container", "command"]}
    },

    # === БАЗА ДАННЫХ ===
    "db_query": {
        "description": "Выполнить SQL запрос к базе данных (SQLite, PostgreSQL, MySQL)",
        "input_schema": {"type": "object", "properties": {
            "database": {"type": "string", "description": "Путь к SQLite файлу или connection string"},
            "query": {"type": "string", "description": "SQL запрос"},
            "db_type": {"type": "string", "description": "Тип БД: sqlite, postgresql, mysql (по умолчанию sqlite)"}
        }, "required": ["database", "query"]}
    },
    "db_schema": {
        "description": "Показать схему таблиц базы данных",
        "input_schema": {"type": "object", "properties": {
            "database": {"type": "string", "description": "Путь к БД или connection string"},
            "db_type": {"type": "string", "description": "Тип БД: sqlite, postgresql, mysql"}
        }, "required": ["database"]}
    },

    # === СИСТЕМА ===
    "ocr_extract": {
        "description": "Распознать текст с изображения (OCR). Поддерживает png, jpg, bmp, tiff",
        "input_schema": {"type": "object", "properties": {
            "image_path": {"type": "string", "description": "Путь к изображению"},
            "lang": {"type": "string", "description": "Язык: rus, eng, rus+eng (по умолчанию rus+eng)"},
            "psm": {"type": "integer", "description": "Режим сегментации: 3=авто, 6=блок, 11=разреженный"}
        }, "required": ["image_path"]}
    },
    "ocr_status": {
        "description": "Проверить статус OCR — установлен ли Tesseract",
        "input_schema": {"type": "object", "properties": {}, "required": []}
    },
    "ocr_languages": {
        "description": "Список доступных языков для OCR",
        "input_schema": {"type": "object", "properties": {}, "required": []}
    },

    # === ДИАГРАММЫ ===
    "diagram_class": {
        "description": "Диаграмма классов Python файла прямо в терминале (Rich ASCII)",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Путь к Python файлу"},
            "save_to": {"type": "string", "description": "Путь для сохранения результата (опционально)"}
        }, "required": ["file_path"]}
    },
    "diagram_tree": {
        "description": "Дерево файлов и папок в терминале",
        "input_schema": {"type": "object", "properties": {
            "directory": {"type": "string", "description": "Папка (по умолчанию текущая)"},
            "max_depth": {"type": "integer", "description": "Глубина вложенности (по умолчанию 3)"},
            "save_to": {"type": "string", "description": "Путь для сохранения"}
        }, "required": []}
    },
    "diagram_deps": {
        "description": "Граф импортов и зависимостей Python файла",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Путь к Python файлу"},
            "save_to": {"type": "string", "description": "Путь для сохранения"}
        }, "required": ["file_path"]}
    },
    "diagram_mermaid": {
        "description": "Конвертировать Mermaid код в ASCII диаграмму для терминала",
        "input_schema": {"type": "object", "properties": {
            "mermaid_code": {"type": "string", "description": "Mermaid код диаграммы"},
            "save_to": {"type": "string", "description": "Путь для сохранения"}
        }, "required": ["mermaid_code"]}
    },

    # === ДОКУМЕНТАЦИЯ ===
    "doc_generate": {
        "description": "Сгенерировать API документацию для Python файла",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Путь к Python файлу"}
        }, "required": ["file_path"]}
    },
    "doc_readme": {
        "description": "Сгенерировать README.md для проекта",
        "input_schema": {"type": "object", "properties": {
            "project_path": {"type": "string", "description": "Путь к папке проекта"}
        }, "required": []}
    },
    "doc_docstrings": {
        "description": "Добавить docstrings к функциям в Python файле",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Путь к Python файлу"}
        }, "required": ["file_path"]}
    },

    # === GITHUB ACTIONS ===
    "github_workflow": {
        "description": "Создать GitHub Actions workflow файл (.github/workflows/)",
        "input_schema": {"type": "object", "properties": {
            "name": {"type": "string", "description": "Название workflow"},
            "trigger": {"type": "string", "description": "Триггер: push, pull_request, schedule"},
            "jobs": {"type": "string", "description": "Описание что должен делать workflow"}
        }, "required": ["name"]}
    },
}


# ─────────────────────────────────────────────────────────────────────────────
# Системный промпт
# ─────────────────────────────────────────────────────────────────────────────

SYSTEM_PROMPT = """Ты Гефест — AI агент. Отвечаешь на русском.

ПРАВИЛО №1 — ЕДИНСТВЕННОЕ ВАЖНОЕ:
Когда нужно что-то СДЕЛАТЬ — отвечай ТОЛЬКО JSON в таком формате:
{"name": "tool_name", "arguments": {"param": "value"}}

НЕЛЬЗЯ писать текст ДО или ПОСЛЕ JSON при вызове инструмента.
НЕЛЬЗЯ объяснять как выполнить команду — ВЫПОЛНЯЙ сам.
НЕЛЬЗЯ выдумывать инструменты — используй только те что в списке TOOLS.

Примеры правильных ответов:
Запрос: "создай файл test.txt"
Ответ: {"name": "file_write", "arguments": {"file_path": "test.txt", "content": ""}}

Запрос: "выполни ls"  
Ответ: {"name": "bash", "arguments": {"command": "ls"}}

Запрос: "создай notebook test.ipynb"
Ответ: {"name": "notebook_create", "arguments": {"file_path": "test.ipynb"}}

Запрос: "удали cron задачу task_1"
Ответ: {"name": "cron_delete", "arguments": {"task_id": "task_1"}}

После получения результата инструмента — напиши ОДНУ строку что сделано."""


# ─────────────────────────────────────────────────────────────────────────────
# Парсинг ответа от модели (JSON fallback для больших моделей)
# ─────────────────────────────────────────────────────────────────────────────

def _parse_any_json_tool(text: str) -> list[dict]:
    """
    Парсим tool call в любом JSON формате который могут выдавать модели:
    1. {"tool": "name", "params": {...}}          ← наш формат
    2. {"name": "name", "arguments": {...}}        ← OpenAI-подобный
    3. ```json {...} ```                           ← в markdown блоке
    """
    results = []

    # Ищем JSON блоки — в markdown или голые
    patterns = [
        r'```json\s*(\{.*?\})\s*```',
        r'```\s*(\{.*?\})\s*```',
        r'(\{\s*"(?:tool|name)"\s*:.*?\})',
    ]

    for pattern in patterns:
        for match in re.findall(pattern, text, re.DOTALL):
            try:
                data = json.loads(match.strip())
                # Формат 1: {"tool": ..., "params": ...}
                if "tool" in data and "params" in data:
                    results.append({
                        "id": f"call_{len(results)}",
                        "name": data["tool"],
                        "input": data.get("params", {}),
                    })
                # Формат 2: {"name": ..., "arguments": ...}
                elif "name" in data and "arguments" in data:
                    args = data["arguments"]
                    if isinstance(args, str):
                        try:
                            args = json.loads(args)
                        except Exception:
                            args = {}
                    results.append({
                        "id": f"call_{len(results)}",
                        "name": data["name"],
                        "input": args,
                    })
            except json.JSONDecodeError:
                continue
        if results:
            break

    return results


# ─────────────────────────────────────────────────────────────────────────────
# Агент Гефест
# ─────────────────────────────────────────────────────────────────────────────

class HephaestusAgent:
    """Агент Гефест — бог кузнечного дела и инструментов."""

    def __init__(
        self,
        llm_config: LLMConfig,
        workspace_root: str | None = None,
        auto_approve: bool = False,
    ):
        self.llm_config = llm_config
        self.llm_client = create_llm_client(llm_config)
        self.workspace_root = Path(workspace_root or Path.cwd()).resolve()
        self.auto_approve = auto_approve
        self.debug = False  # включается через --debug
        self.auto_commit = False  # git автокоммиты
        self.messages: list[LLMMessage] = []

        self._init_tools()
        self.session_manager = SessionManager()
        self.learning = ContextLearning()
        self.session_id: str | None = None

    def _init_tools(self):
        """Инициализация всех инструментов."""
        ws = str(self.workspace_root)
        self.tools = create_tools(ws)

        # Подгружаем опциональные модули
        self.tools.update(_try_import(_load_web_tools))
        self.tools.update(_try_import(_load_memory_tools))
        self.tools.update(_try_import(_load_task_tools))
        self.tools.update(_try_import(_load_cron_tools))
        self.tools.update(_try_import(_load_skill_tools))
        self.tools.update(_try_import(_load_notebook_tools))
        self.tools.update(_try_import(_load_docker_tools))
        self.tools.update(_try_import(_load_database_tools))
        self.tools.update(_try_import(_load_web_surfer))
        self.tools.update(_try_import(_load_ocr_tools))
        self.tools.update(_try_import(_load_diagram_tools))
        self.tools.update(_try_import(_load_doc_tools))
        self.tools.update(_try_import(_load_github_tools))

    def _get_tools_schema(self, query: str = "") -> list[dict]:
        """JSON Schema — умный выбор релевантных инструментов."""
        ALWAYS_INCLUDE = {
            "git", "ocr_extract", "ocr_status", "ocr_languages",
            "web_search_deep", "web_page", "notebook_edit",
            "notebook_create", "notebook_read", "cron_delete",
            "diagram_class", "diagram_tree", "diagram_deps", "diagram_mermaid",
        }

        # Ключевые слова → группы инструментов
        KEYWORD_MAP = {
            ("файл", "file", "создай", "прочитай", "удали", "измени",
             "скопируй", "перемести", "напиши", "сохрани", "открой"):
                {"bash", "file_read", "file_write", "file_edit", "file_delete",
                 "file_move", "file_copy", "file_exists", "glob", "grep", "git"},
            ("память", "запомни", "помни", "memory", "найди в памяти",
             "забудь", "вспомни"):
                {"memory_add", "memory_search", "memory_list", "memory_delete"},
            ("задач", "task", "todo", "сделать", "выполнить", "список дел"):
                {"task_create", "task_list", "task_update"},
            ("cron", "расписани", "schedule", "запуск", "автоматическ"):
                {"cron_create", "cron_list", "cron_delete"},
            ("навык", "skill", "скрипт", "зарегистрир"):
                {"skill_register", "skill_list", "skill_execute"},
            ("notebook", "jupyter", "ipynb", "ячейк"):
                {"notebook_create", "notebook_read", "notebook_edit"},
            ("база", "бд", "db", "sql", "таблиц", "запрос", "select",
             "insert", "create table"):
                {"db_query", "db_schema"},
            ("docker", "контейнер", "образ", "запусти контейнер"):
                {"docker_list", "docker_run", "docker_stop",
                 "docker_logs", "docker_exec"},
            ("веб", "web", "сайт", "url", "http", "загрузи", "поиск",
             "найди в интернете", "search", "google"):
                {"web_search", "web_fetch", "web_search_deep", "web_page"},
            ("ocr", "распознай", "изображени", "фото", "скриншот"):
                {"ocr_extract", "ocr_status", "ocr_languages"},
            ("диаграмм", "дерево", "граф", "класс", "структур", "mermaid"):
                {"diagram_class", "diagram_tree", "diagram_deps", "diagram_mermaid"},
            ("bash", "команд", "терминал", "shell", "выполни", "запусти"):
                {"bash"},
            ("документац", "readme", "docstring", "doc"):
                {"doc_generate", "doc_readme", "doc_docstrings"},
            ("github", "workflow", "ci", "action"):
                {"github_workflow"},
        }

        # Базовые инструменты — всегда
        BASE = {"bash", "file_read", "file_write", "file_edit",
                "file_delete", "glob", "git"}

        if query:
            q = query.lower()
            selected = set(BASE) | ALWAYS_INCLUDE
            for keywords, tools in KEYWORD_MAP.items():
                if any(kw in q for kw in keywords):
                    selected |= tools
            # Если запрос длинный (много задач) — берём все
            if len(query) > 200 or query.count("\n") > 3:
                selected = None  # все инструменты
        else:
            selected = None  # все инструменты

        schemas = []
        for name, schema in ALL_TOOL_SCHEMAS.items():
            in_tools = name in self.tools or name in ALWAYS_INCLUDE or name == "git"
            if not in_tools:
                continue
            if selected is None or name in selected:
                schemas.append({
                    "name": name,
                    "description": schema["description"],
                    "input_schema": schema["input_schema"],
                })
        return schemas


    def execute_tool(self, tool_name: str, **kwargs) -> ToolResult:
        """Выполнить инструмент."""
        tool = self.tools.get(tool_name)

        try:
            if tool_name == "bash":
                return tool.execute(kwargs.get("command", ""))
            elif tool_name == "file_read":
                fp = kwargs.get("file_path") or kwargs.get("filename") or ""
                # Если path — папка, а filename отдельно
                path_arg = kwargs.get("path", "")
                if not fp and path_arg:
                    fname = kwargs.get("filename", "")
                    if fname and not path_arg.endswith(fname):
                        import os as _os
                        fp = _os.path.join(path_arg.rstrip("/"), fname)
                    else:
                        fp = path_arg
                return tool.read(fp)
            elif tool_name == "file_write":
                fp = kwargs.get("file_path") or kwargs.get("filename") or ""
                path_arg = kwargs.get("path", "")
                if not fp and path_arg:
                    fname = kwargs.get("filename", "")
                    if fname and not path_arg.endswith(fname):
                        import os as _os
                        fp = _os.path.join(path_arg.rstrip("/"), fname)
                    else:
                        fp = path_arg
                content = kwargs.get("content") or kwargs.get("text") or kwargs.get("data") or ""
                if fp:
                    from pathlib import Path as _P
                    _P(fp).parent.mkdir(parents=True, exist_ok=True)
                return tool.write(fp, content)
            elif tool_name == "file_edit":
                fp = kwargs.get("file_path") or kwargs.get("path") or kwargs.get("filename") or ""
                return tool.edit(fp,
                    kwargs.get("old_text") or kwargs.get("old") or kwargs.get("search") or "",
                    kwargs.get("new_text") or kwargs.get("new") or kwargs.get("replace") or "")
            elif tool_name == "file_delete":
                fp = kwargs.get("file_path") or kwargs.get("path") or kwargs.get("filename") or ""
                return tool.delete(fp)
            elif tool_name in ("file_move", "file_rename"):
                src = (kwargs.get("source") or kwargs.get("source_path") or
                       kwargs.get("src") or kwargs.get("src_path") or
                       kwargs.get("from") or kwargs.get("file_path") or "")
                dst = (kwargs.get("destination") or kwargs.get("destination_path") or
                       kwargs.get("dst") or kwargs.get("dst_path") or
                       kwargs.get("to") or kwargs.get("new_path") or "")
                return tool.move(src, dst)
            elif tool_name == "file_copy":
                src = (kwargs.get("source") or kwargs.get("source_path") or
                       kwargs.get("src") or kwargs.get("src_path") or
                       kwargs.get("from") or kwargs.get("file_path") or "")
                dst = (kwargs.get("destination") or kwargs.get("destination_path") or
                       kwargs.get("dst") or kwargs.get("dst_path") or
                       kwargs.get("to") or kwargs.get("new_path") or "")
                return tool.copy(src, dst)
            elif tool_name == "file_exists":
                fp = kwargs.get("path") or kwargs.get("file_path") or kwargs.get("filename") or ""
                return tool.exists(fp)
            elif tool_name == "glob":
                pattern = kwargs.get("pattern") or kwargs.get("path") or "**/*"
                # Если абсолютный путь — конвертируем в относительный паттерн
                if pattern.startswith("/"):
                    import os as _os
                    try:
                        ws = str(self.workspace_root)
                        if pattern.startswith(ws):
                            pattern = pattern[len(ws):].lstrip("/")
                        else:
                            bash = self.tools.get("bash")
                            if bash:
                                return bash.execute(
                                    f"find {pattern} -not -path '*/venv/*' "
                                    f"-not -path '*/__pycache__/*' 2>/dev/null | head -50"
                                )
                    except Exception:
                        pass
                # Фильтруем мусор в результатах
                result = tool.search(pattern)
                if result.success and result.output:
                    lines = [
                        l for l in result.output.split("\n")
                        if l and "venv/" not in l and "__pycache__/" not in l
                        and ".pyc" not in l
                    ]
                    result.output = "\n".join(lines)
                return result
            elif tool_name == "grep":
                pattern = kwargs.get("pattern") or kwargs.get("query") or ""
                path = kwargs.get("path") or kwargs.get("directory") or "**/*"
                # Абсолютные пути через bash grep
                if path.startswith("/"):
                    bash = self.tools.get("bash")
                    if bash:
                        return bash.execute(f"grep -r '{pattern}' {path} 2>/dev/null | head -50")
                return tool.search(pattern, file_pattern=path)
            elif tool_name == "git":
                bash = self.tools.get("bash")
                return bash.execute(f"git {kwargs.get('command', 'status')}")
            elif tool_name == "web_fetch":
                r = tool.fetch(url=kwargs.get("url", ""), prompt=kwargs.get("prompt"))
                return ToolResult(success=r.success, output=r.markdown or r.content or "", error=r.error)
            elif tool_name == "web_search":
                results = tool.search(query=kwargs.get("query", ""), max_results=kwargs.get("max_results", 5))
                output = "\n\n".join(f"**{r.title}**\n{r.url}\n{r.snippet}" for r in results)
                return ToolResult(success=True, output=output)
            elif tool_name.startswith("memory_"):
                action = tool_name.split("_", 1)[1]
                if action == "add":
                    content = (kwargs.get("content") or kwargs.get("text") or
                               kwargs.get("message") or kwargs.get("note") or "")
                    return tool.execute(content=content, type=kwargs.get("type", "fact"),
                                        tags=kwargs.get("tags", ""), importance=kwargs.get("importance", 5))
                elif action == "search":
                    return tool.execute(query=kwargs.get("query", ""), type=kwargs.get("type", ""))
                elif action == "list":
                    return tool.execute(limit=kwargs.get("limit", 50))
                elif action == "delete":
                    mem_id = (kwargs.get("mem_id") or kwargs.get("id") or
                              kwargs.get("memory_id") or "")
                    # Если передали текст вместо ID — ищем по содержимому
                    if not mem_id or not mem_id.startswith("mem_"):
                        search_query = (mem_id or kwargs.get("text") or
                                        kwargs.get("content") or kwargs.get("query") or "")
                        import re as _re
                        if search_query:
                            # Ищем по содержимому
                            search_tool = self.tools.get("memory_search")
                            if search_tool:
                                sr = search_tool.execute(query=search_query)
                                if sr.success and "mem_" in (sr.output or ""):
                                    found = _re.findall("mem_[0-9]+_[0-9]+", sr.output)
                                    if found:
                                        mem_id = found[0]  # берём первый найденный
                        if not mem_id or not mem_id.startswith("mem_"):
                            # Ничего не нашли — показываем список
                            list_tool = self.tools.get("memory_list")
                            if list_tool:
                                lr = list_tool.execute(limit=20)
                                ids = _re.findall("mem_[0-9]+_[0-9]+", lr.output or "")
                                return ToolResult(
                                    success=False, output="",
                                    error=f"Укажи реальный ID записи. "
                                          f"Доступные ID: {', '.join(ids[:5]) if ids else 'нет записей'}"
                                )
                    return tool.execute(mem_id=mem_id)
            elif tool_name.startswith("task_"):
                action = tool_name.split("_", 1)[1]
                if action == "create":
                    subject = (kwargs.get("subject") or kwargs.get("title") or
                               kwargs.get("name") or kwargs.get("task") or "")
                    r = tool.create(subject=subject, description=kwargs.get("description", ""))
                    return ToolResult(success=r.success, output=str(r.data or ""), error=r.error)
                elif action == "list":
                    r = tool.list()
                    return ToolResult(success=r.success, output=str(r.data or ""), error=r.error)
                elif action == "update":
                    task_id = (kwargs.get("task_id") or kwargs.get("id") or
                               kwargs.get("task") or "")
                    r = tool.update(task_id=task_id, status=kwargs.get("status"),
                                    subject=kwargs.get("subject") or kwargs.get("title"))
                    return ToolResult(success=r.success, output=str(r.data or ""), error=r.error)
            elif tool_name.startswith("cron_"):
                action = tool_name.split("_", 1)[1]
                if action == "create":
                    t = self.tools.get("cron_create") or tool
                    if t: return t.execute(
                        cron=kwargs.get("cron", kwargs.get("schedule", "")),
                        prompt=kwargs.get("prompt", kwargs.get("command", kwargs.get("name", ""))),
                        description=kwargs.get("description", ""))
                elif action == "list":
                    t = self.tools.get("cron_list") or tool
                    if t: return t.execute()
                elif action == "delete":
                    t = self.tools.get("cron_delete")
                    if t:
                        task_id = (kwargs.get("task_id") or kwargs.get("id") or
                                   kwargs.get("cron_id") or kwargs.get("name") or "")
                        return t.execute(task_id=task_id)
            elif tool_name.startswith("skill_"):
                action = tool_name.split("_", 1)[1]
                if action == "register":
                    return tool.execute(name=kwargs.get("name", ""),
                                        file_path=kwargs.get("file_path", kwargs.get("command", "")),
                                        description=kwargs.get("description", ""))
                elif action == "list":
                    return tool.execute()
                elif action == "execute":
                    skill_id = (kwargs.get("skill_id") or kwargs.get("id") or
                                kwargs.get("name") or "")
                    # Конвертируем имя в ID
                    if skill_id:
                        list_tool = self.tools.get("skill_list")
                        if list_tool:
                            list_res = list_tool.execute() or type("R", (), {"output": ""})()
                            out = list_res.output or ""
                            # Пробуем разные варианты ID
                            candidates = [
                                skill_id,
                                f"skill_{skill_id}",
                                f"skill_{skill_id.lower().replace(' ', '_')}",
                                f"skill_{skill_id.lower().replace('-', '_')}",
                            ]
                            for c in candidates:
                                if c in out:
                                    skill_id = c
                                    break
                    return tool.execute(skill_id=skill_id)
            elif tool_name.startswith("notebook_"):
                action = tool_name.split("_", 1)[1]
                if action == "create":
                    t = self.tools.get("notebook_create")
                    if t:
                        fp = (kwargs.get("file_path") or kwargs.get("path") or
                              kwargs.get("filename") or "")
                        if fp:
                            from pathlib import Path as _P
                            _P(fp).parent.mkdir(parents=True, exist_ok=True)
                        return t.execute(file_path=fp)
                elif action == "read":
                    t = self.tools.get("notebook_read") or tool
                    if t:
                        fp = (kwargs.get("file_path") or kwargs.get("path") or
                              kwargs.get("filename") or "")
                        return t.execute(file_path=fp)
                elif action == "edit":
                    t = self.tools.get("notebook_edit") or tool
                    if t: return t.execute(
                        file_path=kwargs.get("file_path", ""),
                        cell_index=int(kwargs.get("cell_index", 0)),
                        new_content=kwargs.get("new_content", kwargs.get("new_source", "")),
                    )

            # === DOCKER ===
            elif tool_name == "docker_list":
                t = self.tools.get("docker")
                if t: return t.list_containers(all_containers=kwargs.get("all", False))
            elif tool_name == "docker_run":
                t = self.tools.get("docker")
                if t:
                    ports = {}
                    raw = kwargs.get("ports", "")
                    if raw:
                        try:
                            h_port, c_port = str(raw).split(":")
                            ports = {f"{c_port}/tcp": int(h_port)}
                        except Exception:
                            pass
                    return t.run_container(image=kwargs.get("image",""), command=kwargs.get("command"),
                        name=kwargs.get("name"), ports=ports, detach=kwargs.get("detach", True))
            elif tool_name == "docker_stop":
                t = self.tools.get("docker")
                if t: return t.stop_container(kwargs.get("container",""))
            elif tool_name == "docker_logs":
                t = self.tools.get("docker")
                if t: return t.logs(kwargs.get("container",""), tail=kwargs.get("tail", 100))
            elif tool_name == "docker_exec":
                t = self.tools.get("docker")
                if t: return t.exec_command(kwargs.get("container",""), kwargs.get("command",""))

            # === БАЗА ДАННЫХ ===
            elif tool_name == "db_query":
                t = self.tools.get("database")
                if t:
                    db = (kwargs.get("database") or kwargs.get("database_path") or
                          kwargs.get("db_path") or kwargs.get("path") or "")
                    if db:
                        from pathlib import Path as _P
                        _P(db).parent.mkdir(parents=True, exist_ok=True)
                    return t.query(database=db,
                        query=kwargs.get("query",""), db_type=kwargs.get("db_type","sqlite"))
            elif tool_name == "db_schema":
                t = self.tools.get("database")
                if t:
                    db = (kwargs.get("database") or kwargs.get("database_path") or
                          kwargs.get("db_path") or "")
                    return t.get_schema(database=db, db_type=kwargs.get("db_type","sqlite"))

            # === ВЕБ СЁРФИНГ ===
            elif tool_name == "web_search_deep":
                t = self.tools.get("web_surfer")
                if t: return t.research(
                    query=kwargs.get("query", ""),
                    depth=int(kwargs.get("depth", 3)),
                    save_to=kwargs.get("save_to", ""),
                )
            elif tool_name == "web_page":
                t = self.tools.get("web_surfer")
                if t: return t.fetch_page(
                    url=kwargs.get("url", ""),
                    extract=kwargs.get("extract", "text"),
                    save_to=kwargs.get("save_to", ""),
                )
            elif tool_name == "web_search":
                # Пробуем сначала web_surfer, потом старый web_tools
                t = self.tools.get("web_surfer")
                if t:
                    return t.search(
                        query=kwargs.get("query", ""),
                        max_results=int(kwargs.get("max_results", 8)),
                    )

            # === OCR ===
            elif tool_name == "ocr_extract":
                t = self.tools.get("ocr")
                if t: return t.extract_text(
                    image_path=kwargs.get("image_path", ""),
                    lang=kwargs.get("lang", "rus+eng"),
                    psm=int(kwargs.get("psm", 3)),
                )
            elif tool_name == "ocr_status":
                t = self.tools.get("ocr")
                if t: return t.status()
            elif tool_name == "ocr_languages":
                t = self.tools.get("ocr")
                if t: return t.get_languages()

            # === ДИАГРАММЫ (Rich — терминальные) ===
            elif tool_name == "diagram_class":
                t = self.tools.get("diagram")
                if t: return t.class_diagram(
                    file_path=kwargs.get("file_path",""),
                    save_to=kwargs.get("save_to",""))
            elif tool_name == "diagram_tree":
                t = self.tools.get("diagram")
                if t: return t.file_tree(
                    directory=kwargs.get("directory", "."),
                    max_depth=int(kwargs.get("max_depth", 3)),
                    save_to=kwargs.get("save_to",""))
            elif tool_name == "diagram_deps":
                t = self.tools.get("diagram")
                if t: return t.dependency_graph(
                    file_path=kwargs.get("file_path",""),
                    save_to=kwargs.get("save_to",""))
            elif tool_name == "diagram_mermaid":
                t = self.tools.get("diagram")
                if t: return t.mermaid_to_ascii(
                    mermaid_code=kwargs.get("mermaid_code",""),
                    save_to=kwargs.get("save_to",""))
            elif tool_name == "diagram_flowchart":  # алиас для старого
                t = self.tools.get("diagram")
                if t: return t.class_diagram(file_path=kwargs.get("file_path",""))

            # === ДОКУМЕНТАЦИЯ ===
            elif tool_name == "doc_generate":
                t = self.tools.get("doc_generator")
                if t: return t.generate_api_docs(file_path=kwargs.get("file_path",""))
            elif tool_name == "doc_readme":
                t = self.tools.get("doc_generator")
                if t: return t.generate_readme(project_path=kwargs.get("project_path","."))
            elif tool_name == "doc_docstrings":
                t = self.tools.get("doc_generator")
                if t: return t.generate_docstrings(file_path=kwargs.get("file_path",""))

            # === GITHUB ACTIONS ===
            elif tool_name == "github_workflow":
                t = self.tools.get("github")
                if t: return t.create_workflow_file(name=kwargs.get("name",""),
                    trigger=kwargs.get("trigger","push"), jobs=kwargs.get("jobs",""))

            # Generic fallback
            elif tool:
                try:
                    if hasattr(tool, "execute"):
                        return tool.execute(**kwargs)
                    elif hasattr(tool, "run"):
                        return tool.run(**kwargs)
                except Exception as e:
                    return ToolResult(success=False, output="", error=str(e))

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

        return ToolResult(success=False, output="", error=f"Инструмент не найден: {tool_name}")

    def chat(self, user_message: str) -> str:
        """Отправить сообщение и получить ответ."""
        self.messages.append(LLMMessage(role="user", content=user_message))

        tools_schema = self._get_tools_schema(query=user_message)
        learning_hint = self.learning.get_context_hint()
        context_section = ("\n\n## Контекст из прошлого опыта:\n" + learning_hint) if learning_hint else ""
        dynamic_system = SYSTEM_PROMPT + context_section
        MAX_ITERATIONS = 50          # защита от бесконечного цикла
        STUCK_THRESHOLD = 3          # одинаковых вызовов подряд = зациклились
        last_tool_signatures: list[str] = []
        lazy_count = 0
        stuck_count = 0              # счётчик одинаковых вызовов

        for iteration in range(MAX_ITERATIONS):
            try:
                # Streaming — показываем токены в реальном времени
                import sys as _sys
                streamed_tokens = []

                def _on_token(token: str):
                    streamed_tokens.append(token)
                    # Печатаем только если нет tool calls ещё
                    if len(streamed_tokens) == 1:
                        _sys.stdout.write("\n")
                    _sys.stdout.write(token)
                    _sys.stdout.flush()

                # Пробуем streaming если поддерживается
                stream_fn = getattr(self.llm_client, "stream_complete_with_tools", None)
                if stream_fn and self.llm_client.__class__.__name__ == "OllamaClient":
                    response = stream_fn(
                        messages=self.messages,
                        tools=tools_schema,
                        system=dynamic_system,
                        on_token=_on_token,
                    )
                    if streamed_tokens:
                        _sys.stdout.write("\n")
                        _sys.stdout.flush()
                else:
                    response = self.llm_client.complete_with_tools(
                        messages=self.messages,
                        tools=tools_schema,
                        system=dynamic_system,
                    )
            except Exception as e:
                return f"❌ Ошибка LLM: {e}"

            # Если нет нативных tool calls — пробуем JSON парсинг из текста
            if not response.tool_use_blocks:
                parsed = _parse_any_json_tool(response.content or "")
                if parsed:
                    response.tool_use_blocks = parsed
                    response.stop_reason = "tool_use"

            # Умный детектор зацикливания
            if response.tool_use_blocks:
                sig = ",".join(
                    f"{t['name']}:{json.dumps(t.get('input',{}), sort_keys=True)[:80]}"
                    for t in response.tool_use_blocks
                )
                if last_tool_signatures and last_tool_signatures[-1] == sig:
                    stuck_count += 1
                else:
                    stuck_count = 0
                last_tool_signatures.append(sig)
                if len(last_tool_signatures) > 10:
                    last_tool_signatures.pop(0)

                if stuck_count >= STUCK_THRESHOLD:
                    tool_name_stuck = response.tool_use_blocks[0]["name"]
                    # Readonly инструменты (diagram_tree, file_read и т.д.) — просто останавливаем
                    readonly = {"diagram_tree", "diagram_class", "diagram_deps", "diagram_mermaid",
                                "file_read", "file_exists", "glob", "grep", "task_list",
                                "memory_list", "memory_search", "cron_list", "skill_list"}
                    if tool_name_stuck in readonly:
                        # Не зацикливание — просто повтор read-only. Возвращаем последний результат.
                        self.messages.append(LLMMessage(role="assistant", content=""))
                        self.messages.append(LLMMessage(
                            role="user",
                            content=f"Результат уже получен. Переходи к следующему шагу."
                        ))
                    else:
                        self.messages.append(LLMMessage(role="assistant", content=""))
                        self.messages.append(LLMMessage(
                            role="user",
                            content=f"Инструмент '{tool_name_stuck}' вызван {stuck_count+1} раз с тем же результатом. "
                                    "Объясни пользователю что произошло и переходи к следующему шагу."
                        ))
                    stuck_count = 0
                    continue

            # Нет инструментов
            if not response.tool_use_blocks:
                text = response.content if isinstance(response.content, str) else str(response.content)
                # Если первая итерация и есть глаголы действия — принуждаем
                action_words = ["создай", "сделай", "запусти", "выполни", "покажи", "найди",
                                "удали", "скопируй", "переименуй", "добавь", "запомни",
                                "create", "run", "execute", "show", "find", "delete", "search"]
                is_action = any(w in self.messages[-1].content.lower() for w in action_words) if self.messages else False
                if is_action and iteration == 0 and lazy_count < 2:
                    lazy_count += 1
                    self.messages.append(LLMMessage(role="assistant", content=text))
                    self.messages.append(LLMMessage(
                        role="user",
                        content="ВНИМАНИЕ: ты должен использовать инструмент для выполнения этой задачи. "
                                "Вызови нужный инструмент через JSON, не описывай как это сделать вручную."
                    ))
                    continue
                self.messages.append(LLMMessage(role="assistant", content=text))
                return text

            # Показываем мысли если есть
            assistant_text = response.content if isinstance(response.content, str) else ""
            if assistant_text.strip():
                show_thinking(assistant_text)

            self.messages.append(LLMMessage(role="assistant", content=assistant_text))

            # Проверяем что инструменты реально существуют
            valid_names = {s["name"] for s in tools_schema}
            valid_blocks = [t for t in response.tool_use_blocks if t["name"] in valid_names]
            invalid_blocks = [t for t in response.tool_use_blocks if t["name"] not in valid_names]

            if invalid_blocks and not valid_blocks:
                # Модель выдала несуществующий инструмент — принуждаем заново
                bad_names = [t["name"] for t in invalid_blocks]
                self.messages.append(LLMMessage(role="assistant", content=""))
                self.messages.append(LLMMessage(
                    role="user",
                    content=f"Инструменты {bad_names} не существуют. "
                            f"Используй ТОЛЬКО инструменты из списка TOOLS. "
                            f"Ответь JSON с правильным именем инструмента."
                ))
                continue
            response.tool_use_blocks = valid_blocks or response.tool_use_blocks

            # Выполняем инструменты
            tool_results = []
            for tool_call in response.tool_use_blocks:
                name = tool_call["name"]
                params = tool_call.get("input", {})
                call_id = tool_call.get("id", f"call_{iteration}")

                import time as _time
                show_tool_call(name, json.dumps(params, ensure_ascii=False)[:100])
                _t0 = _time.time()
                result = self.execute_tool(name, **params)
                _elapsed = (_time.time() - _t0) * 1000
                show_tool_result(result.output or result.error or "", result.success)
                if self.debug:
                    show_debug(name, params, result, _elapsed)

                # Диаграммы — добавляем полный вывод в контекст явно
                DIAGRAM_TOOLS = {"diagram_class", "diagram_tree", "diagram_deps", "diagram_mermaid"}
                if name in DIAGRAM_TOOLS and result.success and result.output:
                    from .hephaestus_repl import console as _console
                    from rich.panel import Panel as _Panel
                    _console.print(_Panel(
                        result.output,
                        title=f"[bold red]{name}[/bold red]",
                        border_style="red",
                        padding=(0, 1),
                    ))
                # Контекстное обучение
                if result.success:
                    self.learning.record_success(name, params)
                    # Автокоммит в git после изменения файлов
                    if self.auto_commit and name in ("file_write", "file_edit", "file_delete"):
                        fp = params.get("file_path") or params.get("path") or params.get("filename", "")
                        if fp:
                            bash = self.tools.get("bash")
                            if bash:
                                bash.execute(f"git add -A && git commit -m 'feat: {name} {fp[:50]}' 2>/dev/null || true")
                else:
                    self.learning.record_failure(name, result.error or "", params)

                tool_results.append(
                    f"[{call_id}] {name}: {'OK' if result.success else 'ERROR'}\n"
                    f"{result.output if result.success else result.error}"
                )

            results_content = "\n\n---\n\n".join(tool_results)
            self.messages.append(LLMMessage(role="user", content=f"Tool results:\n\n{results_content}"))

        # Если дошли до MAX_ITERATIONS — значит что-то пошло не так
        return "Задача выполнена или требует уточнения."

    def reset(self):
        self.messages = []

    def pursue_goal(self, goal: str, save_report_to: str = "") -> str:
        """Goal mode — планирует и выполняет цель до конца."""
        from .goal_mode import GoalModeAgent
        from .hephaestus_repl import show_info
        gm = GoalModeAgent(agent=self, on_progress=lambda m: show_info(m))
        return gm.pursue(goal, save_report_to=save_report_to)

    def save_session(self) -> str:
        """Сохранить текущую сессию."""
        sid = self.session_manager.save(self.messages)
        self.session_id = sid
        return sid

    def load_session(self, session_id: str) -> bool:
        """Загрузить сессию по ID."""
        from .llm_client import LLMMessage
        messages_data = self.session_manager.load(session_id)
        if messages_data is None:
            return False
        self.messages = [LLMMessage(role=m["role"], content=m["content"]) for m in messages_data]
        self.session_id = session_id
        return True


# ─────────────────────────────────────────────────────────────────────────────
# Entry point
# ─────────────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="⚡ Гефест — AI Coding Agent",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Примеры:
  python -m src.hephaestus
  python -m src.hephaestus "создай файл hello.py"
  python -m src.hephaestus --provider ollama --model llama3.2:3b
  python -m src.hephaestus --provider anthropic
        """
    )
    parser.add_argument("query", nargs="?", help="Одиночный запрос")
    parser.add_argument("--provider", choices=["ollama", "openai", "anthropic", "openrouter", "koboldcpp"])
    parser.add_argument("--model", help="Название модели")
    parser.add_argument("--base-url", help="Базовый URL (для Ollama/KoboldCPP)")
    parser.add_argument("--api-key", help="API ключ")
    parser.add_argument("--workspace", help="Рабочая папка")
    parser.add_argument("--auto-approve", action="store_true")
    parser.add_argument("--temperature", type=float, default=0.1)
    parser.add_argument("--no-logo", action="store_true", help="Без анимации логотипа")
    parser.add_argument("--auto-commit", action="store_true", help="Автокоммит в git после изменений")
    parser.add_argument("--debug", action="store_true", help="Отладочный режим — показывает детали вызовов инструментов")
    args = parser.parse_args()

    # Провайдер
    if args.provider:
        provider = LLMProvider(args.provider)
        default_models = {
            "ollama": os.getenv("OLLAMA_MODEL", "llama3.2:3b"),
            "openai": "gpt-4o",
            "anthropic": "claude-sonnet-4-6",
            "openrouter": os.getenv("OPENROUTER_MODEL", "qwen/qwen-2.5-coder-32b-instruct"),
            "koboldcpp": "local-model",
        }
        model = args.model or default_models[args.provider]
    else:
        provider, model = auto_detect_provider()
        if args.model:
            model = args.model

    config = LLMConfig(
        provider=provider,
        model=model,
        api_key=args.api_key,
        base_url=args.base_url,
        temperature=args.temperature,
    )

    agent = HephaestusAgent(
        llm_config=config,
        workspace_root=args.workspace,
        auto_approve=args.auto_approve,
    )
    agent.debug = getattr(args, "debug", False)
    agent.auto_commit = getattr(args, "auto_commit", False)

    if args.query:
        # Одиночный запрос
        response = agent.chat(args.query)
        print_response(response)
    else:
        # Интерактивный REPL
        repl = HephaestusREPL()

        def on_message(msg: str) -> str:
            with ForgeSpinner("Кую...") as spinner:
                # Запускаем в отдельном потоке чтобы спиннер работал
                import threading
                result = [None]
                error = [None]

                def run():
                    try:
                        result[0] = agent.chat(msg)
                    except Exception as e:
                        error[0] = str(e)

                t = threading.Thread(target=run)
                t.start()

                import time
                while t.is_alive():
                    spinner.update()
                    time.sleep(0.4)
                t.join()

            if error[0]:
                return f"❌ {error[0]}"
            return result[0] or ""

        repl.run(agent=agent, on_message=on_message)


if __name__ == "__main__":
    main()
