"""
Skill System - система навыков/плагинов для расширения функционала.
"""
from __future__ import annotations

import importlib.util
import json
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Callable


@dataclass
class Skill:
    """Навык/плагин."""
    id: str
    name: str
    description: str
    file_path: str
    enabled: bool = True
    created_at: str = field(default_factory=lambda: datetime.now().isoformat())
    metadata: dict[str, Any] = field(default_factory=dict)
    handler: Callable | None = None  # Функция-обработчик


class SkillSystem:
    """Система навыков."""

    def __init__(self, skills_dir: Path | None = None):
        """
        Инициализация системы навыков.

        Args:
            skills_dir: Директория с навыками (по умолчанию ~/.claude/skills/)
        """
        if skills_dir is None:
            skills_dir = Path.home() / ".claude" / "skills"

        self.skills_dir = Path(skills_dir)
        self.skills_dir.mkdir(parents=True, exist_ok=True)

        self.registry_path = self.skills_dir / "registry.json"

        self.skills: dict[str, Skill] = {}
        self._load_registry()
        self._load_skills()

    def _load_registry(self) -> None:
        """Загрузить реестр навыков."""
        if not self.registry_path.exists():
            return

        try:
            with open(self.registry_path) as f:
                data = json.load(f)

            for skill_id, skill_data in data.items():
                # Не загружаем handler из JSON
                skill_data.pop("handler", None)
                self.skills[skill_id] = Skill(**skill_data)

        except Exception:
            pass

    def _save_registry(self) -> None:
        """Сохранить реестр навыков."""
        try:
            data = {}
            for skill_id, skill in self.skills.items():
                skill_dict = asdict(skill)
                # Не сохраняем handler в JSON
                skill_dict.pop("handler", None)
                data[skill_id] = skill_dict

            with open(self.registry_path, "w") as f:
                json.dump(data, f, indent=2, ensure_ascii=False)

        except Exception:
            pass

    def _load_skills(self) -> None:
        """Загрузить навыки из файлов."""
        for skill in self.skills.values():
            if skill.enabled:
                self._load_skill_handler(skill)

    def _load_skill_handler(self, skill: Skill) -> bool:
        """
        Загрузить обработчик навыка из файла.

        Args:
            skill: Навык

        Returns:
            True если загружено успешно
        """
        try:
            file_path = Path(skill.file_path)
            if not file_path.exists():
                return False

            # Загружаем модуль
            spec = importlib.util.spec_from_file_location(skill.id, file_path)
            if not spec or not spec.loader:
                return False

            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)

            # Ищем функцию execute
            if hasattr(module, "execute"):
                skill.handler = module.execute
                return True

            return False

        except Exception:
            return False

    def register(
        self,
        name: str,
        file_path: str | Path,
        description: str = "",
        metadata: dict[str, Any] | None = None,
    ) -> Skill:
        """
        Зарегистрировать новый навык.

        Args:
            name: Название навыка
            file_path: Путь к файлу с навыком
            description: Описание
            metadata: Метаданные

        Returns:
            Зарегистрированный навык
        """
        skill_id = f"skill_{name.lower().replace(' ', '_')}"

        skill = Skill(
            id=skill_id,
            name=name,
            description=description,
            file_path=str(file_path),
            metadata=metadata or {},
        )

        # Загружаем обработчик
        self._load_skill_handler(skill)

        self.skills[skill_id] = skill
        self._save_registry()

        return skill

    def get(self, skill_id: str) -> Skill | None:
        """Получить навык по ID."""
        return self.skills.get(skill_id)

    def list_all(self) -> list[Skill]:
        """Получить все навыки."""
        return list(self.skills.values())

    def execute(self, skill_id: str, **kwargs) -> Any:
        """
        Выполнить навык.

        Args:
            skill_id: ID навыка
            **kwargs: Аргументы для навыка

        Returns:
            Результат выполнения
        """
        skill = self.skills.get(skill_id)
        if not skill:
            raise ValueError(f"Навык {skill_id} не найден")

        if not skill.enabled:
            raise ValueError(f"Навык {skill_id} отключен")

        if not skill.handler:
            raise ValueError(f"Навык {skill_id} не имеет обработчика")

        return skill.handler(**kwargs)

    def enable(self, skill_id: str) -> bool:
        """
        Включить навык.

        Args:
            skill_id: ID навыка

        Returns:
            True если успешно
        """
        skill = self.skills.get(skill_id)
        if not skill:
            return False

        skill.enabled = True
        self._load_skill_handler(skill)
        self._save_registry()

        return True

    def disable(self, skill_id: str) -> bool:
        """
        Отключить навык.

        Args:
            skill_id: ID навыка

        Returns:
            True если успешно
        """
        skill = self.skills.get(skill_id)
        if not skill:
            return False

        skill.enabled = False
        skill.handler = None
        self._save_registry()

        return True

    def unregister(self, skill_id: str) -> bool:
        """
        Удалить навык из реестра.

        Args:
            skill_id: ID навыка

        Returns:
            True если удалено
        """
        if skill_id in self.skills:
            del self.skills[skill_id]
            self._save_registry()
            return True

        return False


def create_skill_system(skills_dir: Path | None = None) -> SkillSystem:
    """
    Создать систему навыков.

    Args:
        skills_dir: Директория с навыками

    Returns:
        Система навыков
    """
    return SkillSystem(skills_dir)
