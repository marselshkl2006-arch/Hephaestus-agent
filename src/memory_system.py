"""
Memory System - долговременная память между сессиями.
Сохраняет важные факты о проекте, контекст, историю.
"""
from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any


@dataclass
class MemoryEntry:
    """Запись в памяти."""
    id: str
    type: str  # "fact", "context", "preference", "history"
    content: str
    metadata: dict[str, Any] = field(default_factory=dict)
    created_at: str = field(default_factory=lambda: datetime.now().isoformat())
    updated_at: str = field(default_factory=lambda: datetime.now().isoformat())
    tags: list[str] = field(default_factory=list)
    importance: int = 5  # 1-10, где 10 - очень важно


class MemorySystem:
    """Система долговременной памяти."""

    def __init__(self, storage_path: Path | None = None):
        """
        Инициализация системы памяти.

        Args:
            storage_path: Путь к файлу хранения (по умолчанию ~/.claude/memory.json)
        """
        if storage_path is None:
            storage_path = Path.home() / ".claude" / "memory.json"

        self.storage_path = Path(storage_path)
        self.storage_path.parent.mkdir(parents=True, exist_ok=True)

        self.memories: dict[str, MemoryEntry] = {}
        self._load()

    def _load(self) -> None:
        """Загрузить память из файла."""
        if not self.storage_path.exists():
            return

        try:
            with open(self.storage_path) as f:
                data = json.load(f)

            for mem_id, mem_data in data.items():
                # Приводим importance к int на случай если в JSON строка
                if "importance" in mem_data:
                    try:
                        mem_data["importance"] = int(mem_data["importance"])
                    except (TypeError, ValueError):
                        mem_data["importance"] = 5
                self.memories[mem_id] = MemoryEntry(**mem_data)

        except Exception:
            pass

    def _save(self) -> None:
        """Сохранить память в файл."""
        try:
            data = {
                mem_id: asdict(mem)
                for mem_id, mem in self.memories.items()
            }

            with open(self.storage_path, "w") as f:
                json.dump(data, f, indent=2, ensure_ascii=False)

        except Exception:
            pass

    def add(
        self,
        content: str,
        type: str = "fact",
        tags: list[str] | None = None,
        importance: int = 5,
        metadata: dict[str, Any] | None = None,
    ) -> MemoryEntry:
        """
        Добавить запись в память.

        Args:
            content: Содержимое записи
            type: Тип записи (fact, context, preference, history)
            tags: Теги для поиска
            importance: Важность (1-10)
            metadata: Дополнительные метаданные

        Returns:
            Созданная запись
        """
        mem_id = f"mem_{len(self.memories) + 1}_{int(datetime.now().timestamp())}"

        entry = MemoryEntry(
            id=mem_id,
            type=type,
            content=content,
            tags=tags or [],
            importance=importance,
            metadata=metadata or {},
        )

        self.memories[mem_id] = entry
        self._save()

        return entry

    def get(self, mem_id: str) -> MemoryEntry | None:
        """Получить запись по ID."""
        return self.memories.get(mem_id)

    def search(
        self,
        query: str | None = None,
        type: str | None = None,
        tags: list[str] | None = None,
        min_importance: int = 0,
    ) -> list[MemoryEntry]:
        """
        Поиск записей в памяти.

        Args:
            query: Текстовый поиск в содержимом
            type: Фильтр по типу
            tags: Фильтр по тегам
            min_importance: Минимальная важность

        Returns:
            Список найденных записей
        """
        results = []

        for mem in self.memories.values():
            # Фильтр по важности
            if mem.importance < int(min_importance):
                continue

            # Фильтр по типу
            if type and mem.type != type:
                continue

            # Фильтр по тегам
            if tags and not any(tag in mem.tags for tag in tags):
                continue

            # Текстовый поиск
            if query:
                query_lower = query.lower()
                if query_lower not in mem.content.lower():
                    continue

            results.append(mem)

        # Сортируем по важности и дате
        results.sort(key=lambda m: (int(m.importance), m.updated_at), reverse=True)

        return results

    def update(
        self,
        mem_id: str,
        content: str | None = None,
        tags: list[str] | None = None,
        importance: int | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> MemoryEntry | None:
        """
        Обновить запись в памяти.

        Args:
            mem_id: ID записи
            content: Новое содержимое
            tags: Новые теги
            importance: Новая важность
            metadata: Новые метаданные

        Returns:
            Обновленная запись или None
        """
        mem = self.memories.get(mem_id)
        if not mem:
            return None

        if content is not None:
            mem.content = content

        if tags is not None:
            mem.tags = tags

        if importance is not None:
            mem.importance = importance

        if metadata is not None:
            mem.metadata.update(metadata)

        mem.updated_at = datetime.now().isoformat()

        self._save()

        return mem

    def delete(self, mem_id: str) -> bool:
        """
        Удалить запись из памяти.

        Args:
            mem_id: ID записи

        Returns:
            True если удалено
        """
        if mem_id in self.memories:
            del self.memories[mem_id]
            self._save()
            return True

        return False

    def list_all(self, limit: int = 100) -> list[MemoryEntry]:
        """
        Получить все записи.

        Args:
            limit: Максимальное количество записей

        Returns:
            Список записей
        """
        memories = list(self.memories.values())
        memories.sort(key=lambda m: (int(m.importance), m.updated_at), reverse=True)
        return memories[:limit]

    def get_context(self, max_entries: int = 10) -> str:
        """
        Получить контекст для LLM из памяти.

        Args:
            max_entries: Максимальное количество записей

        Returns:
            Форматированный контекст
        """
        # Берем самые важные записи
        important = self.search(min_importance=7)[:max_entries]

        if not important:
            return ""

        lines = ["# Контекст из памяти:\n"]

        for mem in important:
            lines.append(f"- [{mem.type}] {mem.content}")
            if mem.tags:
                lines.append(f"  Теги: {', '.join(mem.tags)}")

        return "\n".join(lines)

    def clear(self) -> None:
        """Очистить всю память."""
        self.memories.clear()
        self._save()


def create_memory_system(storage_path: Path | None = None) -> MemorySystem:
    """
    Создать систему памяти.

    Args:
        storage_path: Путь к файлу хранения

    Returns:
        Система памяти
    """
    return MemorySystem(storage_path)
