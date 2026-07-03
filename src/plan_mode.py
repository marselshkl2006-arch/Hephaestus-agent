"""
Plan Mode - режим планирования перед выполнением задач.

Аналог EnterPlanMode/ExitPlanMode из оригинального Claude Code.
Позволяет сначала спланировать задачу, получить одобрение пользователя,
и только потом начать выполнение.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any


@dataclass
class PlanConfig:
    """Конфигурация режима планирования."""
    plans_dir: Path
    current_plan_file: Path | None = None
    in_plan_mode: bool = False


class PlanMode:
    """Менеджер режима планирования."""

    def __init__(self, workspace_root: Path):
        """
        Инициализация Plan Mode.

        Args:
            workspace_root: Корневая директория workspace
        """
        self.workspace_root = workspace_root
        self.plans_dir = workspace_root / ".claude" / "plans"
        self.plans_dir.mkdir(parents=True, exist_ok=True)
        self.current_plan_file: Path | None = None
        self.in_plan_mode = False

    def enter(self) -> dict[str, Any]:
        """
        Войти в режим планирования.

        Returns:
            Информация о режиме планирования
        """
        if self.in_plan_mode:
            return {
                "success": False,
                "error": "Already in plan mode",
                "plan_file": str(self.current_plan_file),
            }

        # Создаем файл плана
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        plan_file = self.plans_dir / f"plan_{timestamp}.md"

        # Создаем шаблон плана
        template = """# Implementation Plan

**Created:** {timestamp}
**Status:** Draft

## Overview

[Describe the task and goals]

## Analysis

[Current state analysis, what needs to be changed]

## Approach

[High-level approach and strategy]

## Implementation Steps

1. [ ] Step 1
2. [ ] Step 2
3. [ ] Step 3

## Files to Modify

- `file1.py` - [what changes]
- `file2.py` - [what changes]

## Risks and Considerations

- [Risk 1]
- [Risk 2]

## Testing Plan

- [ ] Test 1
- [ ] Test 2

## Notes

[Additional notes]
""".format(timestamp=datetime.now().isoformat())

        plan_file.write_text(template, encoding='utf-8')

        self.current_plan_file = plan_file
        self.in_plan_mode = True

        return {
            "success": True,
            "plan_file": str(plan_file),
            "message": "Entered plan mode. Write your plan to the file.",
        }

    def exit(self, approved: bool = True) -> dict[str, Any]:
        """
        Выйти из режима планирования.

        Args:
            approved: План одобрен пользователем

        Returns:
            Результат выхода
        """
        if not self.in_plan_mode:
            return {
                "success": False,
                "error": "Not in plan mode",
            }

        if not self.current_plan_file or not self.current_plan_file.exists():
            return {
                "success": False,
                "error": "Plan file not found",
            }

        # Читаем план
        plan_content = self.current_plan_file.read_text(encoding='utf-8')

        # Обновляем статус в плане
        if approved:
            plan_content = plan_content.replace("**Status:** Draft", "**Status:** Approved")
        else:
            plan_content = plan_content.replace("**Status:** Draft", "**Status:** Rejected")

        self.current_plan_file.write_text(plan_content, encoding='utf-8')

        result = {
            "success": True,
            "approved": approved,
            "plan_file": str(self.current_plan_file),
            "plan_content": plan_content,
        }

        # Выходим из режима
        self.in_plan_mode = False
        self.current_plan_file = None

        return result

    def get_current_plan(self) -> dict[str, Any]:
        """
        Получить текущий план.

        Returns:
            Информация о текущем плане
        """
        if not self.in_plan_mode:
            return {
                "success": False,
                "error": "Not in plan mode",
            }

        if not self.current_plan_file or not self.current_plan_file.exists():
            return {
                "success": False,
                "error": "Plan file not found",
            }

        plan_content = self.current_plan_file.read_text(encoding='utf-8')

        return {
            "success": True,
            "plan_file": str(self.current_plan_file),
            "plan_content": plan_content,
            "in_plan_mode": self.in_plan_mode,
        }

    def update_plan(self, content: str) -> dict[str, Any]:
        """
        Обновить содержимое плана.

        Args:
            content: Новое содержимое плана

        Returns:
            Результат обновления
        """
        if not self.in_plan_mode:
            return {
                "success": False,
                "error": "Not in plan mode",
            }

        if not self.current_plan_file:
            return {
                "success": False,
                "error": "Plan file not found",
            }

        self.current_plan_file.write_text(content, encoding='utf-8')

        return {
            "success": True,
            "plan_file": str(self.current_plan_file),
            "message": "Plan updated",
        }

    def list_plans(self) -> list[dict[str, Any]]:
        """
        Список всех планов.

        Returns:
            Список планов
        """
        plans = []
        for plan_file in sorted(self.plans_dir.glob("plan_*.md"), reverse=True):
            content = plan_file.read_text(encoding='utf-8')

            # Извлекаем статус
            status = "Unknown"
            for line in content.split('\n'):
                if line.startswith("**Status:**"):
                    status = line.split("**Status:**")[1].strip()
                    break

            plans.append({
                "file": plan_file.name,
                "path": str(plan_file),
                "status": status,
                "created": datetime.fromtimestamp(plan_file.stat().st_mtime).isoformat(),
            })

        return plans


class EnterPlanModeTool:
    """Инструмент для входа в режим планирования."""

    def __init__(self, plan_mode: PlanMode):
        self.plan_mode = plan_mode

    def enter(self) -> dict[str, Any]:
        """Войти в режим планирования."""
        return self.plan_mode.enter()


class ExitPlanModeTool:
    """Инструмент для выхода из режима планирования."""

    def __init__(self, plan_mode: PlanMode):
        self.plan_mode = plan_mode

    def exit(self, approved: bool = True) -> dict[str, Any]:
        """
        Выйти из режима планирования.

        Args:
            approved: План одобрен

        Returns:
            Результат выхода
        """
        return self.plan_mode.exit(approved)


def create_plan_mode(workspace_root: Path) -> PlanMode:
    """
    Создать Plan Mode.

    Args:
        workspace_root: Корневая директория workspace

    Returns:
        Plan Mode
    """
    return PlanMode(workspace_root)
