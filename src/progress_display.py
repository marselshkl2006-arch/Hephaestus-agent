"""
Система отображения прогресса выполнения команд.
Показывает пользователю что именно делает агент в реальном времени.
"""
from __future__ import annotations

from enum import Enum
from typing import Optional

from rich.console import Console
from rich.live import Live
from rich.panel import Panel
from rich.progress import Progress, SpinnerColumn, TextColumn, BarColumn, TaskProgressColumn
from rich.table import Table
from rich.text import Text

console = Console()


class ActionType(Enum):
    """Типы действий агента."""
    SEARCHING = "🔍 Поиск"
    READING = "📖 Чтение"
    WRITING = "✍️  Запись"
    EDITING = "✏️  Редактирование"
    EXECUTING = "⚙️  Выполнение"
    INSTALLING = "📦 Установка"
    CONFIGURING = "🔧 Настройка"
    TESTING = "🧪 Тестирование"
    BUILDING = "🏗️  Сборка"
    DEPLOYING = "🚀 Деплой"
    ANALYZING = "🔬 Анализ"
    MONITORING = "📊 Мониторинг"


class ActionStatus(Enum):
    """Статус выполнения действия."""
    PENDING = "⏳"
    IN_PROGRESS = "🔄"
    SUCCESS = "✅"
    WARNING = "⚠️"
    ERROR = "❌"


class ProgressDisplay:
    """Отображение прогресса выполнения команд."""

    def __init__(self):
        self.current_action: Optional[str] = None
        self.current_status = ActionStatus.PENDING
        self.details: list[str] = []

    def start_action(self, action_type: ActionType, description: str):
        """Начать новое действие."""
        self.current_action = f"{action_type.value} {description}"
        self.current_status = ActionStatus.IN_PROGRESS
        self.details = []

        console.print(f"\n{ActionStatus.IN_PROGRESS.value} {self.current_action}", style="bold cyan")

    def add_detail(self, detail: str):
        """Добавить деталь о текущем действии."""
        self.details.append(detail)
        console.print(f"  → {detail}", style="dim")

    def update_status(self, status: ActionStatus, message: Optional[str] = None):
        """Обновить статус текущего действия."""
        self.current_status = status

        if message:
            style = {
                ActionStatus.SUCCESS: "green",
                ActionStatus.WARNING: "yellow",
                ActionStatus.ERROR: "red",
            }.get(status, "white")

            console.print(f"{status.value} {message}", style=style)

    def finish_action(self, success: bool, message: Optional[str] = None):
        """Завершить текущее действие."""
        status = ActionStatus.SUCCESS if success else ActionStatus.ERROR
        self.update_status(status, message or ("Выполнено" if success else "Ошибка"))
        self.current_action = None

    def show_command_execution(self, command: str, output: str, success: bool):
        """Показать выполнение команды с выводом."""
        # Показываем команду
        console.print(Panel(
            f"[bold cyan]$ {command}[/bold cyan]",
            title="Команда",
            border_style="cyan"
        ))

        # Показываем вывод если есть
        if output.strip():
            # Ограничиваем вывод для читаемости
            lines = output.split('\n')
            if len(lines) > 20:
                preview = '\n'.join(lines[:10]) + '\n...\n' + '\n'.join(lines[-10:])
            else:
                preview = output

            console.print(Panel(
                preview,
                title="Вывод",
                border_style="green" if success else "red"
            ))

    def show_tool_summary(self, tool_name: str, params: dict, result: str):
        """Показать сводку выполнения инструмента."""
        table = Table(title=f"Инструмент: {tool_name}", show_header=False)
        table.add_column("Параметр", style="cyan")
        table.add_column("Значение", style="white")

        for key, value in params.items():
            # Ограничиваем длину значений
            value_str = str(value)
            if len(value_str) > 100:
                value_str = value_str[:100] + "..."
            table.add_row(key, value_str)

        console.print(table)
        console.print(f"Результат: {result}\n")


# Глобальный экземпляр для использования во всём приложении
progress_display = ProgressDisplay()
