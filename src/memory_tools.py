"""
Memory Tools - инструменты для работы с долговременной памятью.
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .memory_system import MemorySystem, create_memory_system
from .real_tools import ToolResult


@dataclass
class MemoryAddTool:
    """Инструмент для добавления записи в память."""

    name: str = "memory_add"
    description: str = "Добавить запись в долговременную память"

    def __init__(self, memory_system: MemorySystem):
        self.memory_system = memory_system

    def execute(
        self,
        content: str,
        type: str = "fact",
        tags: str = "",
        importance: int = 5,
    ) -> ToolResult:
        """
        Добавить запись в память.

        Args:
            content: Содержимое записи
            type: Тип (fact, context, preference, history)
            tags: Теги через запятую
            importance: Важность 1-10

        Returns:
            Результат выполнения
        """
        try:
            # Приводим типы — модель может прислать строки
            try:
                importance = int(importance)
            except (TypeError, ValueError):
                importance = 5
            importance = max(1, min(10, importance))

            if isinstance(tags, list):
                tag_list = tags
            else:
                tag_list = [t.strip() for t in str(tags).split(",") if t.strip()]

            entry = self.memory_system.add(
                content=content,
                type=str(type),
                tags=tag_list,
                importance=importance,
            )

            return ToolResult(
                success=True,
                output=f"✅ Запись добавлена в память: {entry.id}\n"
                       f"Тип: {entry.type}\n"
                       f"Важность: {entry.importance}/10",
            )

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class MemorySearchTool:
    """Инструмент для поиска в памяти."""

    name: str = "memory_search"
    description: str = "Поиск записей в долговременной памяти"

    def __init__(self, memory_system: MemorySystem):
        self.memory_system = memory_system

    def execute(
        self,
        query: str = "",
        type: str = "",
        tags: str = "",
        min_importance: int = 0,
    ) -> ToolResult:
        """
        Поиск записей в памяти.

        Args:
            query: Текстовый поиск
            type: Фильтр по типу
            tags: Фильтр по тегам (через запятую)
            min_importance: Минимальная важность

        Returns:
            Результат выполнения
        """
        try:
            # Приводим к int — модель может прислать строку "0"
            try:
                min_importance = int(min_importance)
            except (TypeError, ValueError):
                min_importance = 0

            tag_list = [t.strip() for t in str(tags).split(",") if t.strip()] if tags else None

            results = self.memory_system.search(
                query=query or None,
                type=type or None,
                tags=tag_list,
                min_importance=min_importance,
            )

            if not results:
                return ToolResult(
                    success=True,
                    output="❌ Записи не найдены",
                )

            lines = [f"✅ Найдено записей: {len(results)}\n"]

            for mem in results[:20]:  # Показываем первые 20
                lines.append(f"📝 {mem.id}")
                lines.append(f"   Тип: {mem.type} | Важность: {mem.importance}/10")
                lines.append(f"   {mem.content}")
                if mem.tags:
                    lines.append(f"   Теги: {', '.join(mem.tags)}")
                lines.append("")

            return ToolResult(success=True, output="\n".join(lines))

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class MemoryUpdateTool:
    """Инструмент для обновления записи в памяти."""

    name: str = "memory_update"
    description: str = "Обновить запись в долговременной памяти"

    def __init__(self, memory_system: MemorySystem):
        self.memory_system = memory_system

    def execute(
        self,
        mem_id: str,
        content: str = "",
        tags: str = "",
        importance: int = -1,
    ) -> ToolResult:
        """
        Обновить запись в памяти.

        Args:
            mem_id: ID записи
            content: Новое содержимое
            tags: Новые теги (через запятую)
            importance: Новая важность

        Returns:
            Результат выполнения
        """
        try:
            try:
                importance = int(importance)
            except (TypeError, ValueError):
                importance = -1
            tag_list = [t.strip() for t in str(tags).split(",") if t.strip()] if tags else None

            entry = self.memory_system.update(
                mem_id=mem_id,
                content=content or None,
                tags=tag_list,
                importance=int(importance) if str(importance).lstrip('-').isdigit() and int(importance) >= 0 else None,
            )

            if not entry:
                return ToolResult(
                    success=False,
                    error=f"Запись {mem_id} не найдена",
                )

            return ToolResult(
                success=True,
                output=f"✅ Запись обновлена: {entry.id}\n"
                       f"Содержимое: {entry.content[:100]}...",
            )

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class MemoryDeleteTool:
    """Инструмент для удаления записи из памяти."""

    name: str = "memory_delete"
    description: str = "Удалить запись из долговременной памяти"

    def __init__(self, memory_system: MemorySystem):
        self.memory_system = memory_system

    def execute(self, mem_id: str) -> ToolResult:
        """
        Удалить запись из памяти.

        Args:
            mem_id: ID записи

        Returns:
            Результат выполнения
        """
        try:
            success = self.memory_system.delete(mem_id)

            if success:
                return ToolResult(
                    success=True,
                    output=f"✅ Запись {mem_id} удалена из памяти",
                )
            else:
                return ToolResult(
                    success=False,
                    error=f"Запись {mem_id} не найдена",
                )

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class MemoryListTool:
    """Инструмент для просмотра всех записей в памяти."""

    name: str = "memory_list"
    description: str = "Показать все записи в долговременной памяти"

    def __init__(self, memory_system: MemorySystem):
        self.memory_system = memory_system

    def execute(self, limit: int = 50) -> ToolResult:
        """
        Показать все записи в памяти.

        Args:
            limit: Максимальное количество записей

        Returns:
            Результат выполнения
        """
        try:
            memories = self.memory_system.list_all(limit=limit)

            if not memories:
                return ToolResult(
                    success=True,
                    output="📝 Память пуста",
                )

            lines = [f"📝 Всего записей: {len(memories)}\n"]

            for mem in memories:
                lines.append(f"• {mem.id}")
                lines.append(f"  [{mem.type}] {mem.content[:80]}...")
                lines.append(f"  Важность: {mem.importance}/10")
                if mem.tags:
                    lines.append(f"  Теги: {', '.join(mem.tags)}")
                lines.append("")

            return ToolResult(success=True, output="\n".join(lines))

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


def create_memory_tools(
    storage_path: Path | None = None
) -> dict[str, any]:
    """
    Создать инструменты для работы с памятью.

    Args:
        storage_path: Путь к файлу хранения памяти

    Returns:
        Словарь инструментов
    """
    memory_system = create_memory_system(storage_path)

    return {
        "memory_add": MemoryAddTool(memory_system),
        "memory_search": MemorySearchTool(memory_system),
        "memory_update": MemoryUpdateTool(memory_system),
        "memory_delete": MemoryDeleteTool(memory_system),
        "memory_list": MemoryListTool(memory_system),
    }
