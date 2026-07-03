"""
Worktree Tools - инструменты для работы с git worktree.
"""
from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path

from .real_tools import ToolResult


@dataclass
class EnterWorktreeTool:
    """Инструмент для создания git worktree."""

    name: str = "enter_worktree"
    description: str = "Создать изолированный git worktree"

    def __init__(self, workspace_root: Path):
        self.workspace_root = workspace_root

    def execute(
        self,
        name: str = "",
        branch: str = "",
    ) -> ToolResult:
        """
        Создать worktree.

        Args:
            name: Название worktree (по умолчанию генерируется)
            branch: Ветка для worktree (по умолчанию текущая)

        Returns:
            Результат выполнения
        """
        try:
            # Генерируем имя если не указано
            if not name:
                import time
                name = f"worktree_{int(time.time())}"

            # Путь к worktree
            worktree_path = self.workspace_root / ".worktrees" / name

            # Создаем директорию
            worktree_path.parent.mkdir(parents=True, exist_ok=True)

            # Создаем worktree
            cmd = ["git", "worktree", "add"]

            if branch:
                cmd.extend(["-b", branch])

            cmd.append(str(worktree_path))

            result = subprocess.run(
                cmd,
                cwd=self.workspace_root,
                capture_output=True,
                text=True,
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    error=f"Ошибка создания worktree: {result.stderr}",
                )

            return ToolResult(
                success=True,
                output=f"✅ Worktree создан: {worktree_path}\n"
                       f"Название: {name}\n"
                       f"Ветка: {branch or 'текущая'}",
            )

        except Exception as e:
            return ToolResult(success=False, error=str(e))


@dataclass
class ExitWorktreeTool:
    """Инструмент для удаления git worktree."""

    name: str = "exit_worktree"
    description: str = "Удалить git worktree"

    def __init__(self, workspace_root: Path):
        self.workspace_root = workspace_root

    def execute(
        self,
        name: str,
        force: bool = False,
    ) -> ToolResult:
        """
        Удалить worktree.

        Args:
            name: Название worktree
            force: Принудительное удаление

        Returns:
            Результат выполнения
        """
        try:
            worktree_path = self.workspace_root / ".worktrees" / name

            if not worktree_path.exists():
                return ToolResult(
                    success=False,
                    error=f"Worktree не найден: {name}",
                )

            # Удаляем worktree
            cmd = ["git", "worktree", "remove"]

            if force:
                cmd.append("--force")

            cmd.append(str(worktree_path))

            result = subprocess.run(
                cmd,
                cwd=self.workspace_root,
                capture_output=True,
                text=True,
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    error=f"Ошибка удаления worktree: {result.stderr}",
                )

            return ToolResult(
                success=True,
                output=f"✅ Worktree удален: {name}",
            )

        except Exception as e:
            return ToolResult(success=False, error=str(e))


@dataclass
class ListWorktreesTool:
    """Инструмент для просмотра git worktrees."""

    name: str = "list_worktrees"
    description: str = "Показать все git worktrees"

    def __init__(self, workspace_root: Path):
        self.workspace_root = workspace_root

    def execute(self) -> ToolResult:
        """
        Показать все worktrees.

        Returns:
            Результат выполнения
        """
        try:
            result = subprocess.run(
                ["git", "worktree", "list"],
                cwd=self.workspace_root,
                capture_output=True,
                text=True,
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    error=f"Ошибка получения списка: {result.stderr}",
                )

            return ToolResult(
                success=True,
                output=f"🌳 Git Worktrees:\n\n{result.stdout}",
            )

        except Exception as e:
            return ToolResult(success=False, error=str(e))


def create_worktree_tools(workspace_root: Path) -> dict[str, any]:
    """
    Создать инструменты для работы с worktree.

    Args:
        workspace_root: Корневая директория workspace

    Returns:
        Словарь инструментов
    """
    return {
        "enter_worktree": EnterWorktreeTool(workspace_root),
        "exit_worktree": ExitWorktreeTool(workspace_root),
        "list_worktrees": ListWorktreesTool(workspace_root),
    }
