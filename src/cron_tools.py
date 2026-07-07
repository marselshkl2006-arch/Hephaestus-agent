"""
Cron Tools - инструменты для работы с планировщиком задач.
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .real_tools import ToolResult
from .scheduler import Scheduler, create_scheduler


@dataclass
class CronCreateTool:
    """Инструмент для создания запланированной задачи."""

    name: str = "cron_create"
    description: str = "Создать запланированную задачу (cron)"

    def __init__(self, scheduler: Scheduler):
        self.scheduler = scheduler

    def execute(
        self,
        cron: str,
        prompt: str,
        description: str = "",
    ) -> ToolResult:
        """
        Создать запланированную задачу.

        Args:
            cron: Cron выражение (например: "0 9 * * *")
            prompt: Что выполнить
            description: Описание задачи

        Returns:
            Результат выполнения
        """
        try:
            task = self.scheduler.create(
                cron=cron,
                prompt=prompt,
                description=description,
            )

            return ToolResult(
                success=True,
                output=f"✅ Задача создана: {task.id}\n"
                       f"Расписание: {task.cron}\n"
                       f"Команда: {task.prompt}\n"
                       f"Описание: {task.description or 'Нет'}",
            )

        except Exception as e:
            return ToolResult(success=False, error=str(e))


@dataclass
class CronListTool:
    """Инструмент для просмотра запланированных задач."""

    name: str = "cron_list"
    description: str = "Показать все запланированные задачи"

    def __init__(self, scheduler: Scheduler):
        self.scheduler = scheduler

    def execute(self) -> ToolResult:
        """
        Показать все запланированные задачи.

        Returns:
            Результат выполнения
        """
        try:
            tasks = self.scheduler.list_all()

            if not tasks:
                return ToolResult(
                    success=True,
                    output="📅 Нет запланированных задач",
                )

            lines = [f"📅 Запланированных задач: {len(tasks)}\n"]

            for task in tasks:
                status = "✅" if task.enabled else "❌"
                lines.append(f"{status} {task.id}")
                lines.append(f"   Расписание: {task.cron}")
                lines.append(f"   Команда: {task.prompt}")
                if task.description:
                    lines.append(f"   Описание: {task.description}")
                if task.last_run:
                    lines.append(f"   Последний запуск: {task.last_run}")
                lines.append("")

            return ToolResult(success=True, output="\n".join(lines))

        except Exception as e:
            return ToolResult(success=False, error=str(e))


@dataclass
class CronDeleteTool:
    """Инструмент для удаления запланированной задачи."""

    name: str = "cron_delete"
    description: str = "Удалить запланированную задачу"

    def __init__(self, scheduler: Scheduler):
        self.scheduler = scheduler

    def execute(self, task_id: str) -> ToolResult:
        """
        Удалить запланированную задачу.

        Args:
            task_id: ID задачи

        Returns:
            Результат выполнения
        """
        try:
            success = self.scheduler.delete(task_id)

            if success:
                return ToolResult(
                    success=True,
                    output=f"✅ Задача {task_id} удалена",
                )
            else:
                return ToolResult(
                    success=False,
                    error=f"Задача {task_id} не найдена",
                )

        except Exception as e:
            return ToolResult(success=False, error=str(e))


def create_cron_tools(storage_path: Path | None = None) -> dict[str, any]:
    """
    Создать инструменты для работы с планировщиком.

    Args:
        storage_path: Путь к файлу хранения задач

    Returns:
        Словарь инструментов
    """
    scheduler = create_scheduler(storage_path)

    return {
        "cron_create": CronCreateTool(scheduler),
        "cron_list": CronListTool(scheduler),
        "cron_delete": CronDeleteTool(scheduler),
    }
