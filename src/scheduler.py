"""
Scheduler - планировщик задач с поддержкой cron.
"""
from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any


@dataclass
class ScheduledTask:
    """Запланированная задача."""
    id: str
    cron: str  # Cron выражение (например: "0 9 * * *")
    prompt: str  # Что выполнить
    description: str = ""
    enabled: bool = True
    created_at: str = field(default_factory=lambda: datetime.now().isoformat())
    last_run: str | None = None
    next_run: str | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


class Scheduler:
    """Планировщик задач."""

    def __init__(self, storage_path: Path | None = None):
        """
        Инициализация планировщика.

        Args:
            storage_path: Путь к файлу хранения задач
        """
        if storage_path is None:
            storage_path = Path.home() / ".claude" / "scheduled_tasks.json"

        self.storage_path = Path(storage_path)
        self.storage_path.parent.mkdir(parents=True, exist_ok=True)

        self.tasks: dict[str, ScheduledTask] = {}
        self._load()

    def _load(self) -> None:
        """Загрузить задачи из файла."""
        if not self.storage_path.exists():
            return

        try:
            with open(self.storage_path) as f:
                data = json.load(f)

            for task_id, task_data in data.items():
                self.tasks[task_id] = ScheduledTask(**task_data)

        except Exception:
            pass

    def _save(self) -> None:
        """Сохранить задачи в файл."""
        try:
            data = {
                task_id: asdict(task)
                for task_id, task in self.tasks.items()
            }

            with open(self.storage_path, "w") as f:
                json.dump(data, f, indent=2, ensure_ascii=False)

        except Exception:
            pass

    def create(
        self,
        cron: str,
        prompt: str,
        description: str = "",
        metadata: dict[str, Any] | None = None,
    ) -> ScheduledTask:
        """
        Создать запланированную задачу.

        Args:
            cron: Cron выражение (например: "0 9 * * *" для 9:00 каждый день)
            prompt: Что выполнить
            description: Описание задачи
            metadata: Дополнительные метаданные

        Returns:
            Созданная задача
        """
        task_id = f"task_{len(self.tasks) + 1}_{int(datetime.now().timestamp())}"

        task = ScheduledTask(
            id=task_id,
            cron=cron,
            prompt=prompt,
            description=description,
            metadata=metadata or {},
        )

        self.tasks[task_id] = task
        self._save()

        return task

    def get(self, task_id: str) -> ScheduledTask | None:
        """Получить задачу по ID."""
        return self.tasks.get(task_id)

    def list_all(self) -> list[ScheduledTask]:
        """
        Получить все задачи.

        Returns:
            Список задач
        """
        return list(self.tasks.values())

    def update(
        self,
        task_id: str,
        cron: str | None = None,
        prompt: str | None = None,
        description: str | None = None,
        enabled: bool | None = None,
    ) -> ScheduledTask | None:
        """
        Обновить задачу.

        Args:
            task_id: ID задачи
            cron: Новое cron выражение
            prompt: Новый prompt
            description: Новое описание
            enabled: Включена ли задача

        Returns:
            Обновленная задача или None
        """
        task = self.tasks.get(task_id)
        if not task:
            return None

        if cron is not None:
            task.cron = cron

        if prompt is not None:
            task.prompt = prompt

        if description is not None:
            task.description = description

        if enabled is not None:
            task.enabled = enabled

        self._save()

        return task

    def delete(self, task_id: str) -> bool:
        """
        Удалить задачу.

        Args:
            task_id: ID задачи

        Returns:
            True если удалено
        """
        if task_id in self.tasks:
            del self.tasks[task_id]
            self._save()
            return True

        return False

    def mark_run(self, task_id: str) -> None:
        """
        Отметить что задача выполнена.

        Args:
            task_id: ID задачи
        """
        task = self.tasks.get(task_id)
        if task:
            task.last_run = datetime.now().isoformat()
            self._save()


def create_scheduler(storage_path: Path | None = None) -> Scheduler:
    """
    Создать планировщик.

    Args:
        storage_path: Путь к файлу хранения

    Returns:
        Планировщик
    """
    return Scheduler(storage_path)
