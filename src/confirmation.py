"""
Единая система подтверждений для Claude Code.

Простая логика:
- Всегда спрашиваем перед действием
- Варианты: y (yes), n (no), a (always), q (quit)
"""
from __future__ import annotations

from enum import Enum
from typing import Callable

from rich.console import Console
from rich.panel import Panel
from rich.syntax import Syntax

from .security import ActionRisk, SecurityCheck

console = Console()


class ConfirmationResult(Enum):
    """Результат подтверждения."""
    YES = "yes"           # Выполнить
    NO = "no"             # Не выполнять
    ALWAYS = "always"     # Выполнить и больше не спрашивать
    QUIT = "quit"         # Отменить и выйти


class ConfirmationManager:
    """Менеджер подтверждений действий."""

    def __init__(self):
        self.always_allow: set[str] = set()  # Типы действий, которые всегда разрешены
        self.auto_approve = False

    def set_auto_approve(self, enabled: bool):
        """Включить/выключить автоматическое подтверждение."""
        self.auto_approve = enabled

    def confirm_action(
        self,
        action: str,
        details: dict,
        security_check: SecurityCheck,
        preview: str | None = None
    ) -> ConfirmationResult:
        """
        Запросить подтверждение действия.

        Args:
            action: Тип действия (bash, file_write, file_delete, etc.)
            details: Детали действия (command, file_path, etc.)
            security_check: Результат проверки безопасности
            preview: Предпросмотр содержимого (для файлов)

        Returns:
            ConfirmationResult
        """
        # Если auto-approve включен, разрешаем всё кроме критических
        if self.auto_approve and security_check.risk != ActionRisk.CRITICAL:
            return ConfirmationResult.YES

        # Если действие в списке always_allow
        if action in self.always_allow:
            return ConfirmationResult.YES

        # Показываем информацию о действии
        self._show_action_info(action, details, security_check, preview)

        # Запрашиваем подтверждение
        while True:
            try:
                response = console.input(
                    "\n[bold cyan]Выполнить?[/bold cyan] "
                    "[green]\\[y][/green]es / "
                    "[red]\\[n][/red]o / "
                    "[yellow]\\[a][/yellow]lways / "
                    "[magenta]\\[q][/magenta]uit: "
                ).strip().lower()

                if response in ["y", "yes", "да"]:
                    return ConfirmationResult.YES
                elif response in ["n", "no", "нет"]:
                    return ConfirmationResult.NO
                elif response in ["a", "always", "всегда"]:
                    self.always_allow.add(action)
                    console.print(f"✓ Действие '{action}' больше не будет запрашивать подтверждение", style="green")
                    return ConfirmationResult.ALWAYS
                elif response in ["q", "quit", "выход"]:
                    return ConfirmationResult.QUIT
                else:
                    console.print("❌ Неверный ввод. Используйте: y/n/a/q", style="red")

            except (KeyboardInterrupt, EOFError):
                console.print("\n❌ Отменено пользователем", style="red")
                return ConfirmationResult.NO

    def _show_action_info(
        self,
        action: str,
        details: dict,
        security_check: SecurityCheck,
        preview: str | None
    ):
        """Показать информацию о действии."""
        # Определяем цвет по уровню риска
        risk_colors = {
            ActionRisk.SAFE: "green",
            ActionRisk.LOW: "cyan",
            ActionRisk.MEDIUM: "yellow",
            ActionRisk.HIGH: "orange1",
            ActionRisk.CRITICAL: "red",
        }
        color = risk_colors.get(security_check.risk, "white")

        # Формируем заголовок
        risk_emoji = {
            ActionRisk.SAFE: "✅",
            ActionRisk.LOW: "ℹ️",
            ActionRisk.MEDIUM: "⚠️",
            ActionRisk.HIGH: "🔥",
            ActionRisk.CRITICAL: "💀",
        }
        emoji = risk_emoji.get(security_check.risk, "❓")

        title = f"{emoji} {action.upper()}"

        # Формируем содержимое
        lines = []
        for key, value in details.items():
            lines.append(f"[bold]{key}:[/bold] {value}")

        if security_check.warning:
            lines.append("")
            lines.append(f"[{color} bold]{security_check.warning}[/{color} bold]")

        if security_check.reason:
            lines.append(f"[dim]{security_check.reason}[/dim]")

        content = "\n".join(lines)

        # Показываем панель
        console.print()
        console.print(Panel(
            content,
            title=title,
            border_style=color,
            padding=(1, 2)
        ))

        # Показываем предпросмотр если есть
        if preview:
            console.print()
            console.print(Panel(
                preview[:500] + ("..." if len(preview) > 500 else ""),
                title="📝 Предпросмотр",
                border_style="dim",
                padding=(1, 2)
            ))


# Глобальный экземпляр менеджера
_manager = ConfirmationManager()


def confirm_bash(command: str, security_check: SecurityCheck) -> ConfirmationResult:
    """Подтвердить выполнение bash команды."""
    return _manager.confirm_action(
        action="bash",
        details={"command": command},
        security_check=security_check
    )


def confirm_file_write(file_path: str, content: str, security_check: SecurityCheck) -> ConfirmationResult:
    """Подтвердить запись файла."""
    size = len(content.encode('utf-8'))
    return _manager.confirm_action(
        action="file_write",
        details={
            "file_path": file_path,
            "size": f"{size} bytes"
        },
        security_check=security_check,
        preview=content
    )


def confirm_file_read(file_path: str, security_check: SecurityCheck) -> ConfirmationResult:
    """Подтвердить чтение файла."""
    return _manager.confirm_action(
        action="file_read",
        details={"file_path": file_path},
        security_check=security_check
    )


def confirm_file_delete(file_path: str, security_check: SecurityCheck) -> ConfirmationResult:
    """Подтвердить удаление файла."""
    return _manager.confirm_action(
        action="file_delete",
        details={"file_path": file_path},
        security_check=security_check
    )


def set_auto_approve(enabled: bool):
    """Включить/выключить автоматическое подтверждение."""
    _manager.set_auto_approve(enabled)


def get_manager() -> ConfirmationManager:
    """Получить глобальный менеджер подтверждений."""
    return _manager
