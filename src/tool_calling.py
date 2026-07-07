"""
Tool Calling - парсинг и форматирование вызовов инструментов.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass
class ToolCall:
    """Вызов инструмента."""
    tool_name: str
    arguments: dict[str, Any]
    id: str | None = None


class ToolCallParser:
    """Парсер для извлечения вызовов инструментов из ответа LLM."""

    def __init__(self):
        """Инициализация парсера."""
        pass

    def parse(self, text: str) -> list[ToolCall]:
        """
        Парсинг текста для извлечения вызовов инструментов.

        Args:
            text: Текст ответа LLM

        Returns:
            Список найденных вызовов инструментов
        """
        # Базовая реализация - возвращаем пустой список
        # В реальности здесь должен быть парсинг tool_use блоков
        return []


def format_tool_result(tool_call: ToolCall, result: Any) -> str:
    """
    Форматирование результата выполнения инструмента.

    Args:
        tool_call: Вызов инструмента
        result: Результат выполнения

    Returns:
        Отформатированная строка с результатом
    """
    tool_name = tool_call.tool_name

    if isinstance(result, dict):
        if result.get("success"):
            output = result.get("output", result.get("content", ""))
            return f"✓ {tool_name}: {output}"
        else:
            error = result.get("error", "Unknown error")
            return f"✗ {tool_name}: {error}"

    return f"✓ {tool_name}: {result}"
