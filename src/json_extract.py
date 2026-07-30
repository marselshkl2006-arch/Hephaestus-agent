"""
json_extract.py — надёжное извлечение JSON-объектов из текста модели.

Почему это нужно:
    Старый код искал JSON регулярками вида r'\\{.*?\\}' (non-greedy).
    Такой паттерн обрывается на ПЕРВОЙ встреченной закрывающей скобке,
    поэтому любой tool call с вложенным объектом в параметрах
    (например {"name": "file_write", "arguments": {"path": ..., "content": ...}})
    даёт обрезанную, невалидную JSON-строку → json.loads падает →
    вызов инструмента тихо теряется, хотя модель всё сформулировала верно.

    Здесь вместо регулярок используется посимвольный подсчёт баланса
    { } с учётом строк и экранирования — как это делает почти любой
    нормальный JSON-экстрактор (in-place, без внешних зависимостей).
"""
from __future__ import annotations

import json
import re
from typing import Any

_TOOL_CALL_TAG_RE = re.compile(r"<tool_call>(.*?)</tool_call>", re.DOTALL)


def find_json_objects(text: str) -> list[str]:
    """
    Находит все верхнеуровневые {...} блоки в тексте, корректно
    считая вложенность и игнорируя скобки внутри строк.

    Возвращает список подстрок-кандидатов (ещё не распарсенных).
    """
    objects: list[str] = []
    depth = 0
    start = -1
    in_string = False
    escape = False

    for i, ch in enumerate(text):
        if in_string:
            if escape:
                escape = False
            elif ch == "\\":
                escape = True
            elif ch == '"':
                in_string = False
            continue

        if ch == '"':
            in_string = True
            continue

        if ch == "{":
            if depth == 0:
                start = i
            depth += 1
        elif ch == "}":
            if depth > 0:
                depth -= 1
                if depth == 0 and start != -1:
                    objects.append(text[start:i + 1])
                    start = -1

    return objects


def extract_tool_calls(text: str) -> list[dict[str, Any]]:
    """
    Извлекает все валидные вызовы инструментов из текста модели,
    поддерживая форматы:
      1. {"tool": "name", "params": {...}}
      2. {"name": "name", "arguments": {...}}
      3. {"name": "name", "parameters": {...}}
    Работает как для JSON внутри ```json ... ```, так и для "голого" JSON.

    Returns:
        Список dict вида {"id": ..., "name": ..., "input": {...}}
    """
    results: list[dict[str, Any]] = []
    seen: set[str] = set()

    # 1. Приоритетный путь: явные <tool_call>...</tool_call> теги (Hermes-style).
    #    Это то, что описано в актуальном системном промпте — однозначно,
    #    без риска зацепить JSON-схему инструментов из того же сообщения.
    tagged = _TOOL_CALL_TAG_RE.findall(text)
    search_spaces = tagged if tagged else [text]

    for space in search_spaces:
        for raw in find_json_objects(space):
            try:
                data = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                continue

            if not isinstance(data, dict):
                continue

            name = None
            args: Any = {}

            if "name" in data and "arguments" in data:
                name = data["name"]
                args = data["arguments"]
            elif "name" in data and "parameters" in data:
                name = data["name"]
                args = data["parameters"]
            elif "name" in data and "params" in data:
                name = data["name"]
                args = data["params"]
            elif "tool" in data and "params" in data:
                name = data["tool"]
                args = data.get("params", {})
            elif "tool" in data and "arguments" in data:
                name = data["tool"]
                args = data["arguments"]

            if not name:
                continue

            if isinstance(args, str):
                try:
                    args = json.loads(args)
                except Exception:
                    args = {}

            if not isinstance(args, dict):
                continue

            sig = f"{name}:{json.dumps(args, sort_keys=True, ensure_ascii=False)}"
            if sig in seen:
                continue
            seen.add(sig)

            results.append({
                "id": f"call_{len(results)}",
                "name": name,
                "input": args,
            })

        if tagged and results:
            # Нашли валидные вызовы внутри тегов — не нужно дополнительно
            # сканировать весь текст (там может быть эхо схемы инструментов).
            break

    return results
