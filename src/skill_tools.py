"""
Skill Tools - инструменты для работы с системой навыков.
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .real_tools import ToolResult
from .skill_system import SkillSystem, create_skill_system


@dataclass
class SkillRegisterTool:
    """Инструмент для регистрации навыка."""

    name: str = "skill_register"
    description: str = "Зарегистрировать новый навык/плагин"

    def __init__(self, skill_system: SkillSystem):
        self.skill_system = skill_system

    def execute(
        self,
        name: str,
        file_path: str,
        description: str = "",
    ) -> ToolResult:
        """
        Зарегистрировать навык.

        Args:
            name: Название навыка
            file_path: Путь к файлу с навыком
            description: Описание

        Returns:
            Результат выполнения
        """
        try:
            skill = self.skill_system.register(
                name=name,
                file_path=file_path,
                description=description,
            )

            return ToolResult(
                success=True,
                output=f"✅ Навык зарегистрирован: {skill.id}\n"
                       f"Название: {skill.name}\n"
                       f"Файл: {skill.file_path}\n"
                       f"Статус: {'Включен' if skill.enabled else 'Отключен'}",
            )

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class SkillListTool:
    """Инструмент для просмотра навыков."""

    name: str = "skill_list"
    description: str = "Показать все зарегистрированные навыки"

    def __init__(self, skill_system: SkillSystem):
        self.skill_system = skill_system

    def execute(self) -> ToolResult:
        """
        Показать все навыки.

        Returns:
            Результат выполнения
        """
        try:
            skills = self.skill_system.list_all()

            if not skills:
                return ToolResult(
                    success=True,
                    output="🔌 Нет зарегистрированных навыков",
                )

            lines = [f"🔌 Зарегистрированных навыков: {len(skills)}\n"]

            for skill in skills:
                status = "✅" if skill.enabled else "❌"
                lines.append(f"{status} {skill.id}")
                lines.append(f"   Название: {skill.name}")
                lines.append(f"   Описание: {skill.description or 'Нет'}")
                lines.append(f"   Файл: {skill.file_path}")
                lines.append("")

            return ToolResult(success=True, output="\n".join(lines))

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class SkillExecuteTool:
    """Инструмент для выполнения навыка."""

    name: str = "skill_execute"
    description: str = "Выполнить зарегистрированный навык"

    def __init__(self, skill_system: SkillSystem):
        self.skill_system = skill_system

    def execute(self, skill_id: str, **kwargs) -> ToolResult:
        """
        Выполнить навык.

        Args:
            skill_id: ID навыка
            **kwargs: Аргументы для навыка

        Returns:
            Результат выполнения
        """
        try:
            result = self.skill_system.execute(skill_id, **kwargs)

            return ToolResult(
                success=True,
                output=f"✅ Навык {skill_id} выполнен\n"
                       f"Результат: {result}",
            )

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class SkillUnregisterTool:
    """Инструмент для удаления навыка."""

    name: str = "skill_unregister"
    description: str = "Удалить навык из реестра"

    def __init__(self, skill_system: SkillSystem):
        self.skill_system = skill_system

    def execute(self, skill_id: str) -> ToolResult:
        """
        Удалить навык.

        Args:
            skill_id: ID навыка

        Returns:
            Результат выполнения
        """
        try:
            success = self.skill_system.unregister(skill_id)

            if success:
                return ToolResult(
                    success=True,
                    output=f"✅ Навык {skill_id} удален",
                )
            else:
                return ToolResult(
                    success=False,
                    error=f"Навык {skill_id} не найден",
                )

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


def create_skill_tools(skills_dir: Path | None = None) -> dict[str, any]:
    """
    Создать инструменты для работы с навыками.

    Args:
        skills_dir: Директория с навыками

    Returns:
        Словарь инструментов
    """
    skill_system = create_skill_system(skills_dir)

    return {
        "skill_register": SkillRegisterTool(skill_system),
        "skill_list": SkillListTool(skill_system),
        "skill_execute": SkillExecuteTool(skill_system),
        "skill_unregister": SkillUnregisterTool(skill_system),
    }
