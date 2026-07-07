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

def _load_ocr_tools():
    from .ocr_tool import OCRTool
    return {"ocr": OCRTool()}

def _load_diagram_tools():
    from .diagram_generator import DiagramGenerator
    return {"diagram": DiagramGenerator()}

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
        "description": "Создать диаграмму классов для Python файла в формате Mermaid",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Путь к Python файлу"},
            "format": {"type": "string", "description": "Формат: mermaid или plantuml"}
        }, "required": ["file_path"]}
    },
    "diagram_flowchart": {
        "description": "Создать блок-схему для Python функции",
        "input_schema": {"type": "object", "properties": {
            "file_path": {"type": "string", "description": "Путь к файлу"},
            "function_name": {"type": "string", "description": "Имя функции"}
        }, "required": ["file_path"]}
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

SYSTEM_PROMPT = """Ты Гефест — AI агент для программирования. Бог кузнечного дела: создаёшь инструменты, куёшь код, строишь системы.

## ГЛАВНОЕ ПРАВИЛО — ОБЯЗАТЕЛЬНО
Если пользователь просит что-то СДЕЛАТЬ — ты ОБЯЗАН вызвать инструмент. НЕ описывай как это сделать. НЕ показывай код для ручного запуска. ДЕЛАЙ сам.

Примеры:
- "создай файл" → вызови file_write (НЕ пиши "используйте touch")
- "создай папку" → вызови bash с mkdir (НЕ пиши "выполните mkdir")
- "запусти тесты" → вызови bash с pytest
- "доработай файл" → сначала file_read, потом file_write или file_edit

## Принципы
- Читай файл перед редактированием (file_read → file_edit/file_write)
- Проверяй результат через bash или file_read после записи
- Пиши полный рабочий код, без заглушек и TODO
- Один инструмент за раз, жди результат

## Стиль
- Отвечай на русском если пользователь пишет на русском
- После выполнения — одна фраза что сделано
- Не извиняйся, не объясняй очевидное"""


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
        self.tools.update(_try_import(_load_ocr_tools))
        self.tools.update(_try_import(_load_diagram_tools))
        self.tools.update(_try_import(_load_doc_tools))
        self.tools.update(_try_import(_load_github_tools))

    def _get_tools_schema(self) -> list[dict]:
        """JSON Schema только для подключённых инструментов."""
        # Инструменты реализованные не напрямую через self.tools[name]
        ALWAYS_INCLUDE = {'docker_list', 'doc_readme', 'doc_generate', 'github_workflow', 'notebook_edit', 'docker_stop', 'db_schema', 'doc_docstrings', 'diagram_class', 'diagram_flowchart', 'git', 'docker_exec', 'ocr_extract', 'ocr_status', 'ocr_languages', 'docker_logs', 'db_query', 'docker_run'}

        schemas = []
        for name, schema in ALL_TOOL_SCHEMAS.items():
            if name in self.tools or name in ALWAYS_INCLUDE:
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
                return tool.read(kwargs.get("file_path", ""))
            elif tool_name == "file_write":
                return tool.write(kwargs.get("file_path", ""), kwargs.get("content", ""))
            elif tool_name == "file_edit":
                return tool.edit(kwargs.get("file_path", ""), kwargs.get("old_text", ""), kwargs.get("new_text", ""))
            elif tool_name == "file_delete":
                return tool.delete(kwargs.get("file_path", ""))
            elif tool_name == "file_move":
                return tool.move(kwargs.get("source", ""), kwargs.get("destination", ""))
            elif tool_name == "file_copy":
                return tool.copy(kwargs.get("source", ""), kwargs.get("destination", ""))
            elif tool_name == "file_exists":
                return tool.exists(kwargs.get("path", ""))
            elif tool_name == "glob":
                return tool.search(kwargs.get("pattern", ""))
            elif tool_name == "grep":
                return tool.search(kwargs.get("pattern", ""), file_pattern=kwargs.get("path", "**/*"))
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
                    return tool.execute(content=kwargs.get("content", ""), type=kwargs.get("type", "fact"),
                                        tags=kwargs.get("tags", ""), importance=kwargs.get("importance", 5))
                elif action == "search":
                    return tool.execute(query=kwargs.get("query", ""), type=kwargs.get("type", ""))
                elif action == "list":
                    return tool.execute(limit=kwargs.get("limit", 50))
                elif action == "delete":
                    return tool.execute(mem_id=kwargs.get("mem_id", ""))
            elif tool_name.startswith("task_"):
                action = tool_name.split("_", 1)[1]
                if action == "create":
                    r = tool.create(subject=kwargs.get("subject", ""), description=kwargs.get("description", ""))
                    return ToolResult(success=r.success, output=str(r.data or ""), error=r.error)
                elif action == "list":
                    r = tool.list()
                    return ToolResult(success=r.success, output=str(r.data or ""), error=r.error)
                elif action == "update":
                    r = tool.update(task_id=kwargs.get("task_id", ""), status=kwargs.get("status"),
                                    subject=kwargs.get("subject"))
                    return ToolResult(success=r.success, output=str(r.data or ""), error=r.error)
            elif tool_name.startswith("cron_"):
                action = tool_name.split("_", 1)[1]
                if action == "create":
                    return tool.execute(cron=kwargs.get("cron", kwargs.get("schedule", "")),
                                        prompt=kwargs.get("prompt", kwargs.get("command", kwargs.get("name", ""))),
                                        description=kwargs.get("description", ""))
                elif action == "list":
                    return tool.execute()
            elif tool_name.startswith("skill_"):
                action = tool_name.split("_", 1)[1]
                if action == "register":
                    return tool.execute(name=kwargs.get("name", ""),
                                        file_path=kwargs.get("file_path", kwargs.get("command", "")),
                                        description=kwargs.get("description", ""))
                elif action == "list":
                    return tool.execute()
                elif action == "execute":
                    return tool.execute(skill_id=kwargs.get("skill_id", ""))
            elif tool_name.startswith("notebook_"):
                action = tool_name.split("_", 1)[1]
                if action == "read":
                    return tool.execute(file_path=kwargs.get("file_path", ""))
                elif action == "edit":
                    return tool.execute(
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
                if t: return t.query(database=kwargs.get("database",""),
                    query=kwargs.get("query",""), db_type=kwargs.get("db_type","sqlite"))
            elif tool_name == "db_schema":
                t = self.tools.get("database")
                if t: return t.get_schema(database=kwargs.get("database",""),
                    db_type=kwargs.get("db_type","sqlite"))

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

            # === ДИАГРАММЫ ===
            elif tool_name == "diagram_class":
                t = self.tools.get("diagram")
                if t: return t.generate_class_diagram(file_path=kwargs.get("file_path",""),
                    format=kwargs.get("format","mermaid"))
            elif tool_name == "diagram_flowchart":
                t = self.tools.get("diagram")
                if t: return t.generate_flowchart(file_path=kwargs.get("file_path",""),
                    function_name=kwargs.get("function_name",""))

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

        tools_schema = self._get_tools_schema()
        learning_hint = self.learning.get_context_hint()
        context_section = ("\n\n## Контекст из прошлого опыта:\n" + learning_hint) if learning_hint else ""
        dynamic_system = SYSTEM_PROMPT + context_section
        max_iterations = 10
        last_tool_signatures: list[str] = []
        lazy_count = 0  # счётчик "ленивых" ответов подряд

        for iteration in range(max_iterations):
            try:
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

            # Детектор зацикливания — если 3 раза подряд те же инструменты
            if response.tool_use_blocks:
                sig = ",".join(f"{t['name']}:{json.dumps(t.get('input',{}), sort_keys=True)[:50]}" for t in response.tool_use_blocks)
                if last_tool_signatures.count(sig) >= 2:
                    self.messages.append(LLMMessage(role="assistant", content=""))
                    self.messages.append(LLMMessage(role="user", content="Tool results:\n\nСтоп — ты зациклился. Дай финальный ответ пользователю на его вопрос без вызова инструментов."))
                    last_tool_signatures = []
                    continue
                last_tool_signatures.append(sig)
                if len(last_tool_signatures) > 6:
                    last_tool_signatures.pop(0)

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
                # Контекстное обучение
                if result.success:
                    self.learning.record_success(name, params)
                else:
                    self.learning.record_failure(name, result.error or "", params)

                tool_results.append(
                    f"[{call_id}] {name}: {'OK' if result.success else 'ERROR'}\n"
                    f"{result.output if result.success else result.error}"
                )

            results_content = "\n\n---\n\n".join(tool_results)
            self.messages.append(LLMMessage(role="user", content=f"Tool results:\n\n{results_content}"))

        return "Достигнут лимит итераций."

    def reset(self):
        self.messages = []

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
