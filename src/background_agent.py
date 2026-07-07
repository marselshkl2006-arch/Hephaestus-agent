"""
Background Agent - запуск агентов в фоновом режиме.
Позволяет выполнять длительные задачи асинхронно.
"""
from __future__ import annotations

import json
import threading
import time
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path
from typing import Callable, Any
from uuid import uuid4

from .real_tools import ToolResult


@dataclass
class BackgroundTask:
    """Фоновая задача."""
    task_id: str
    description: str
    status: str  # pending, running, completed, failed
    created_at: str
    started_at: str | None = None
    completed_at: str | None = None
    result: str | None = None
    error: str | None = None
    progress: int = 0  # 0-100


class BackgroundAgentTool:
    """Инструмент для запуска фоновых агентов."""

    def __init__(self, workspace_root: str = "."):
        self.workspace_root = Path(workspace_root)
        self.tasks_dir = Path.home() / ".claude_code" / "background_tasks"
        self.tasks_dir.mkdir(parents=True, exist_ok=True)
        self.tasks: dict[str, BackgroundTask] = {}
        self.threads: dict[str, threading.Thread] = {}
        self._load_tasks()

    def _load_tasks(self) -> None:
        """Загрузить сохраненные задачи."""
        for task_file in self.tasks_dir.glob("*.json"):
            try:
                with open(task_file, 'r', encoding='utf-8') as f:
                    data = json.load(f)
                    task = BackgroundTask(**data)
                    self.tasks[task.task_id] = task
            except Exception:
                pass

    def _save_task(self, task: BackgroundTask) -> None:
        """Сохранить задачу на диск."""
        task_file = self.tasks_dir / f"{task.task_id}.json"
        with open(task_file, 'w', encoding='utf-8') as f:
            json.dump(asdict(task), f, indent=2, ensure_ascii=False)

    def start_task(
        self,
        description: str,
        task_function: Callable[[], Any],
        on_progress: Callable[[int], None] | None = None
    ) -> ToolResult:
        """
        Запустить фоновую задачу.

        Args:
            description: Описание задачи
            task_function: Функция для выполнения
            on_progress: Callback для обновления прогресса

        Returns:
            ToolResult с task_id
        """
        task_id = uuid4().hex[:8]
        task = BackgroundTask(
            task_id=task_id,
            description=description,
            status="pending",
            created_at=datetime.now().isoformat()
        )

        self.tasks[task_id] = task
        self._save_task(task)

        def run_task():
            try:
                task.status = "running"
                task.started_at = datetime.now().isoformat()
                self._save_task(task)

                # Выполняем задачу
                result = task_function()

                task.status = "completed"
                task.completed_at = datetime.now().isoformat()
                task.result = str(result)
                task.progress = 100
                self._save_task(task)

            except Exception as e:
                task.status = "failed"
                task.completed_at = datetime.now().isoformat()
                task.error = str(e)
                self._save_task(task)

        # Запускаем в отдельном потоке
        thread = threading.Thread(target=run_task, daemon=True)
        self.threads[task_id] = thread
        thread.start()

        return ToolResult(
            success=True,
            output=f"Background task started: {task_id}\nDescription: {description}",
            error=None
        )

    def get_status(self, task_id: str) -> ToolResult:
        """
        Получить статус задачи.

        Args:
            task_id: ID задачи

        Returns:
            ToolResult со статусом
        """
        task = self.tasks.get(task_id)
        if not task:
            return ToolResult(
                success=False,
                output="",
                error=f"Task not found: {task_id}"
            )

        output = f"""Task: {task.task_id}
Description: {task.description}
Status: {task.status}
Progress: {task.progress}%
Created: {task.created_at}
Started: {task.started_at or 'N/A'}
Completed: {task.completed_at or 'N/A'}
"""

        if task.result:
            output += f"\nResult:\n{task.result}"

        if task.error:
            output += f"\nError:\n{task.error}"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def list_tasks(self, status_filter: str | None = None) -> ToolResult:
        """
        Список всех задач.

        Args:
            status_filter: Фильтр по статусу (pending, running, completed, failed)

        Returns:
            ToolResult со списком задач
        """
        tasks = list(self.tasks.values())

        if status_filter:
            tasks = [t for t in tasks if t.status == status_filter]

        if not tasks:
            return ToolResult(
                success=True,
                output="No background tasks found",
                error=None
            )

        output = "Background Tasks:\n\n"
        for task in sorted(tasks, key=lambda t: t.created_at, reverse=True):
            output += f"[{task.task_id}] {task.description}\n"
            output += f"  Status: {task.status} ({task.progress}%)\n"
            output += f"  Created: {task.created_at}\n"
            if task.error:
                output += f"  Error: {task.error}\n"
            output += "\n"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def cancel_task(self, task_id: str) -> ToolResult:
        """
        Отменить задачу (если возможно).

        Args:
            task_id: ID задачи

        Returns:
            ToolResult
        """
        task = self.tasks.get(task_id)
        if not task:
            return ToolResult(
                success=False,
                output="",
                error=f"Task not found: {task_id}"
            )

        if task.status in ["completed", "failed"]:
            return ToolResult(
                success=False,
                output="",
                error=f"Cannot cancel task in status: {task.status}"
            )

        # Помечаем как отмененную
        task.status = "failed"
        task.error = "Cancelled by user"
        task.completed_at = datetime.now().isoformat()
        self._save_task(task)

        return ToolResult(
            success=True,
            output=f"Task cancelled: {task_id}",
            error=None
        )

    def wait_for_task(self, task_id: str, timeout: int = 300) -> ToolResult:
        """
        Ждать завершения задачи.

        Args:
            task_id: ID задачи
            timeout: Таймаут в секундах

        Returns:
            ToolResult с результатом задачи
        """
        task = self.tasks.get(task_id)
        if not task:
            return ToolResult(
                success=False,
                output="",
                error=f"Task not found: {task_id}"
            )

        thread = self.threads.get(task_id)
        if thread and thread.is_alive():
            thread.join(timeout=timeout)

        # Перезагружаем задачу с диска
        task_file = self.tasks_dir / f"{task_id}.json"
        if task_file.exists():
            with open(task_file, 'r', encoding='utf-8') as f:
                data = json.load(f)
                task = BackgroundTask(**data)
                self.tasks[task_id] = task

        if task.status == "running":
            return ToolResult(
                success=False,
                output="",
                error=f"Task still running after {timeout}s timeout"
            )

        return self.get_status(task_id)


# Пример использования
if __name__ == "__main__":
    import time

    tool = BackgroundAgentTool()

    # Тестовая задача
    def long_task():
        time.sleep(3)
        return "Task completed successfully!"

    # Запускаем
    result = tool.start_task("Test long task", long_task)
    print(result.output)

    # Проверяем статус
    task_id = result.output.split(":")[1].split("\n")[0].strip()
    time.sleep(1)
    print("\n" + tool.get_status(task_id).output)

    # Ждем завершения
    print("\nWaiting for completion...")
    final = tool.wait_for_task(task_id)
    print(final.output)
