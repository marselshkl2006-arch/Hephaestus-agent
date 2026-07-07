"""
Task Management System - система управления задачами.

Аналог TaskCreate, TaskUpdate, TaskList, TaskGet из оригинального Claude Code.
Позволяет создавать задачи, отслеживать прогресс, управлять зависимостями.
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field, asdict
from datetime import datetime
from enum import Enum
from pathlib import Path
from typing import Any
from uuid import uuid4


class TaskStatus(Enum):
    """Статусы задач."""
    PENDING = "pending"
    IN_PROGRESS = "in_progress"
    COMPLETED = "completed"
    DELETED = "deleted"


@dataclass
class Task:
    """Задача."""
    id: str
    subject: str
    description: str
    status: TaskStatus = TaskStatus.PENDING
    active_form: str | None = None  # Форма для отображения в процессе (e.g. "Running tests")
    owner: str | None = None
    created_at: str = field(default_factory=lambda: datetime.now().isoformat())
    updated_at: str = field(default_factory=lambda: datetime.now().isoformat())
    blocks: list[str] = field(default_factory=list)  # Задачи, которые блокирует эта
    blocked_by: list[str] = field(default_factory=list)  # Задачи, которые блокируют эту
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict:
        """Конвертировать в словарь."""
        data = asdict(self)
        data['status'] = self.status.value
        return data

    @classmethod
    def from_dict(cls, data: dict) -> Task:
        """Создать из словаря."""
        data = data.copy()
        data['status'] = TaskStatus(data['status'])
        return cls(**data)


class TaskManager:
    """Менеджер задач."""

    def __init__(self, storage_path: Path | None = None):
        """
        Инициализация менеджера задач.

        Args:
            storage_path: Путь к файлу хранения задач
        """
        self.storage_path = storage_path or Path.home() / ".claude" / "tasks.json"
        self.tasks: dict[str, Task] = {}
        self._load()

    def _load(self) -> None:
        """Загрузить задачи из файла."""
        if self.storage_path.exists():
            try:
                with open(self.storage_path, 'r', encoding='utf-8') as f:
                    data = json.load(f)
                    for task_data in data.get('tasks', []):
                        task = Task.from_dict(task_data)
                        self.tasks[task.id] = task
            except Exception as e:
                print(f"Warning: Failed to load tasks: {e}")

    def _save(self) -> None:
        """Сохранить задачи в файл."""
        self.storage_path.parent.mkdir(parents=True, exist_ok=True)
        data = {
            'tasks': [task.to_dict() for task in self.tasks.values() if task.status != TaskStatus.DELETED]
        }
        with open(self.storage_path, 'w', encoding='utf-8') as f:
            json.dump(data, f, indent=2, ensure_ascii=False)

    def create(
        self,
        subject: str,
        description: str,
        active_form: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> Task:
        """
        Создать новую задачу.

        Args:
            subject: Краткое описание задачи (императив)
            description: Детальное описание
            active_form: Форма для отображения в процессе
            metadata: Дополнительные метаданные

        Returns:
            Созданная задача
        """
        task_id = str(len(self.tasks) + 1)
        task = Task(
            id=task_id,
            subject=subject,
            description=description,
            active_form=active_form,
            metadata=metadata or {},
        )
        self.tasks[task_id] = task
        self._save()
        return task

    def get(self, task_id: str) -> Task | None:
        """
        Получить задачу по ID.

        Args:
            task_id: ID задачи

        Returns:
            Задача или None
        """
        return self.tasks.get(task_id)

    def list(self, include_deleted: bool = False) -> list[Task]:
        """
        Получить список всех задач.

        Args:
            include_deleted: Включить удаленные задачи

        Returns:
            Список задач
        """
        tasks = list(self.tasks.values())
        if not include_deleted:
            tasks = [t for t in tasks if t.status != TaskStatus.DELETED]
        return sorted(tasks, key=lambda t: int(t.id))

    def update(
        self,
        task_id: str,
        status: TaskStatus | None = None,
        subject: str | None = None,
        description: str | None = None,
        active_form: str | None = None,
        owner: str | None = None,
        add_blocks: list[str] | None = None,
        add_blocked_by: list[str] | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> Task | None:
        """
        Обновить задачу.

        Args:
            task_id: ID задачи
            status: Новый статус
            subject: Новый subject
            description: Новое описание
            active_form: Новая форма отображения
            owner: Новый владелец
            add_blocks: Добавить блокируемые задачи
            add_blocked_by: Добавить блокирующие задачи
            metadata: Обновить метаданные (merge)

        Returns:
            Обновленная задача или None
        """
        task = self.tasks.get(task_id)
        if not task:
            return None

        if status is not None:
            task.status = status
        if subject is not None:
            task.subject = subject
        if description is not None:
            task.description = description
        if active_form is not None:
            task.active_form = active_form
        if owner is not None:
            task.owner = owner
        if add_blocks:
            task.blocks.extend(add_blocks)
        if add_blocked_by:
            task.blocked_by.extend(add_blocked_by)
        if metadata:
            task.metadata.update(metadata)

        task.updated_at = datetime.now().isoformat()
        self._save()
        return task

    def delete(self, task_id: str) -> bool:
        """
        Удалить задачу (пометить как deleted).

        Args:
            task_id: ID задачи

        Returns:
            True если успешно
        """
        task = self.tasks.get(task_id)
        if not task:
            return False

        task.status = TaskStatus.DELETED
        task.updated_at = datetime.now().isoformat()
        self._save()
        return True

    def get_available_tasks(self) -> list[Task]:
        """
        Получить задачи доступные для выполнения.

        Returns:
            Список задач без блокировок и без владельца
        """
        available = []
        for task in self.tasks.values():
            if task.status == TaskStatus.PENDING and not task.owner:
                # Проверяем что нет блокирующих задач
                blocked = False
                for blocking_id in task.blocked_by:
                    blocking_task = self.tasks.get(blocking_id)
                    if blocking_task and blocking_task.status != TaskStatus.COMPLETED:
                        blocked = True
                        break
                if not blocked:
                    available.append(task)
        return sorted(available, key=lambda t: int(t.id))


# Глобальный экземпляр менеджера задач
_task_manager: TaskManager | None = None


def get_task_manager() -> TaskManager:
    """Получить глобальный экземпляр менеджера задач."""
    global _task_manager
    if _task_manager is None:
        _task_manager = TaskManager()
    return _task_manager


# Удобные функции для использования в инструментах
def task_create(subject: str, description: str, **kwargs) -> dict:
    """Создать задачу."""
    manager = get_task_manager()
    task = manager.create(subject, description, **kwargs)
    return {
        'task_id': task.id,
        'subject': task.subject,
        'status': task.status.value,
    }


def task_get(task_id: str) -> dict | None:
    """Получить задачу."""
    manager = get_task_manager()
    task = manager.get(task_id)
    if not task:
        return None
    return task.to_dict()


def task_list() -> list[dict]:
    """Список задач."""
    manager = get_task_manager()
    tasks = manager.list()
    return [
        {
            'id': t.id,
            'subject': t.subject,
            'status': t.status.value,
            'owner': t.owner,
            'blocked_by': t.blocked_by,
        }
        for t in tasks
    ]


def task_update(task_id: str, **kwargs) -> dict | None:
    """Обновить задачу."""
    manager = get_task_manager()

    # Конвертируем строковый статус в enum
    if 'status' in kwargs and isinstance(kwargs['status'], str):
        kwargs['status'] = TaskStatus(kwargs['status'])

    task = manager.update(task_id, **kwargs)
    if not task:
        return None
    return task.to_dict()
