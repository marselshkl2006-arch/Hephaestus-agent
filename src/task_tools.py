"""
Task Tools - инструменты для работы с задачами в Claude Code.

Предоставляет 4 основных инструмента:
- TaskCreateTool - создание задач
- TaskGetTool - получение деталей задачи
- TaskListTool - список всех задач
- TaskUpdateTool - обновление задачи
"""
from __future__ import annotations

from dataclasses import dataclass

from .task_manager import (
    TaskStatus,
    get_task_manager,
    task_create,
    task_get,
    task_list,
    task_update,
)


@dataclass
class TaskToolResult:
    """Результат выполнения task инструмента."""
    success: bool
    data: dict | list | None = None
    error: str | None = None


class TaskCreateTool:
    """Инструмент для создания задач."""

    def create(
        self,
        subject: str,
        description: str,
        active_form: str | None = None,
        metadata: dict | None = None,
    ) -> TaskToolResult:
        """
        Создать новую задачу.

        Args:
            subject: Краткое описание (императив, например "Fix bug in login")
            description: Детальное описание задачи
            active_form: Форма для отображения в процессе (например "Fixing bug")
            metadata: Дополнительные метаданные

        Returns:
            Результат с ID созданной задачи
        """
        try:
            result = task_create(
                subject=subject,
                description=description,
                active_form=active_form,
                metadata=metadata,
            )
            return TaskToolResult(success=True, data=result)
        except Exception as e:
            return TaskToolResult(success=False, error=str(e))


class TaskGetTool:
    """Инструмент для получения деталей задачи."""

    def get(self, task_id: str) -> TaskToolResult:
        """
        Получить детали задачи.

        Args:
            task_id: ID задачи

        Returns:
            Результат с полной информацией о задаче
        """
        try:
            result = task_get(task_id)
            if result is None:
                return TaskToolResult(success=False, error=f"Task {task_id} not found")
            return TaskToolResult(success=True, data=result)
        except Exception as e:
            return TaskToolResult(success=False, error=str(e))


class TaskListTool:
    """Инструмент для получения списка задач."""

    def list(self) -> TaskToolResult:
        """
        Получить список всех задач.

        Returns:
            Результат со списком задач (id, subject, status, owner, blocked_by)
        """
        try:
            result = task_list()
            return TaskToolResult(success=True, data=result)
        except Exception as e:
            return TaskToolResult(success=False, error=str(e))


class TaskUpdateTool:
    """Инструмент для обновления задач."""

    def update(
        self,
        task_id: str,
        status: str | None = None,
        subject: str | None = None,
        description: str | None = None,
        active_form: str | None = None,
        owner: str | None = None,
        add_blocks: list[str] | None = None,
        add_blocked_by: list[str] | None = None,
        metadata: dict | None = None,
    ) -> TaskToolResult:
        """
        Обновить задачу.

        Args:
            task_id: ID задачи
            status: Новый статус (pending, in_progress, completed, deleted)
            subject: Новый subject
            description: Новое описание
            active_form: Новая форма отображения
            owner: Новый владелец
            add_blocks: Добавить блокируемые задачи
            add_blocked_by: Добавить блокирующие задачи
            metadata: Обновить метаданные

        Returns:
            Результат с обновленной задачей
        """
        try:
            result = task_update(
                task_id=task_id,
                status=status,
                subject=subject,
                description=description,
                active_form=active_form,
                owner=owner,
                add_blocks=add_blocks,
                add_blocked_by=add_blocked_by,
                metadata=metadata,
            )
            if result is None:
                return TaskToolResult(success=False, error=f"Task {task_id} not found")
            return TaskToolResult(success=True, data=result)
        except Exception as e:
            return TaskToolResult(success=False, error=str(e))


def create_task_tools() -> dict:
    """
    Создать все task инструменты.

    Returns:
        Словарь с инструментами
    """
    return {
        'task_create': TaskCreateTool(),
        'task_get': TaskGetTool(),
        'task_list': TaskListTool(),
        'task_update': TaskUpdateTool(),
    }
