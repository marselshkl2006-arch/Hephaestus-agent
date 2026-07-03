"""
Инструмент для анализа структуры проекта.
Определяет тип проекта, зависимости, структуру файлов.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str = ""


@dataclass
class ProjectInfo:
    """Информация о проекте."""
    name: str
    type: str  # python, nodejs, go, rust, etc.
    language: str
    framework: str | None
    dependencies: List[str]
    dev_dependencies: List[str]
    test_framework: str | None
    structure: Dict[str, int]  # Количество файлов по типам
    entry_points: List[str]


class ProjectAnalyzerTool:
    """Анализ структуры и типа проекта."""

    def __init__(self, workspace_root: str | None = None):
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else Path.cwd()

    def analyze(self, detailed: bool = False) -> ToolResult:
        """Проанализировать проект.

        Args:
            detailed: Детальный анализ (включая все файлы)

        Returns:
            ToolResult с информацией о проекте
        """
        try:
            project_info = self._analyze_project()

            output = self._format_project_info(project_info, detailed)

            return ToolResult(
                success=True,
                output=output
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error analyzing project: {str(e)}"
            )

    def _analyze_project(self) -> ProjectInfo:
        """Выполнить анализ проекта."""
        # Определяем тип проекта
        project_type, language = self._detect_project_type()

        # Определяем имя проекта
        name = self._get_project_name(project_type)

        # Определяем фреймворк
        framework = self._detect_framework(project_type)

        # Получаем зависимости
        dependencies, dev_dependencies = self._get_dependencies(project_type)

        # Определяем тестовый фреймворк
        test_framework = self._detect_test_framework(project_type, dev_dependencies)

        # Анализируем структуру файлов
        structure = self._analyze_file_structure()

        # Находим точки входа
        entry_points = self._find_entry_points(project_type)

        return ProjectInfo(
            name=name,
            type=project_type,
            language=language,
            framework=framework,
            dependencies=dependencies,
            dev_dependencies=dev_dependencies,
            test_framework=test_framework,
            structure=structure,
            entry_points=entry_points
        )

    def _detect_project_type(self) -> tuple[str, str]:
        """Определить тип и язык проекта."""
        # Python проект
        if (self.workspace_root / "setup.py").exists() or \
           (self.workspace_root / "pyproject.toml").exists() or \
           (self.workspace_root / "requirements.txt").exists():
            return "python", "Python"

        # Node.js проект
        if (self.workspace_root / "package.json").exists():
            return "nodejs", "JavaScript/TypeScript"

        # Go проект
        if (self.workspace_root / "go.mod").exists():
            return "go", "Go"

        # Rust проект
        if (self.workspace_root / "Cargo.toml").exists():
            return "rust", "Rust"

        # Java проект
        if (self.workspace_root / "pom.xml").exists() or \
           (self.workspace_root / "build.gradle").exists():
            return "java", "Java"

        # По умолчанию - определяем по файлам
        py_files = list(self.workspace_root.rglob("*.py"))
        js_files = list(self.workspace_root.rglob("*.js")) + \
                   list(self.workspace_root.rglob("*.ts"))
        go_files = list(self.workspace_root.rglob("*.go"))

        if py_files:
            return "python", "Python"
        elif js_files:
            return "nodejs", "JavaScript/TypeScript"
        elif go_files:
            return "go", "Go"

        return "unknown", "Unknown"

    def _get_project_name(self, project_type: str) -> str:
        """Получить имя проекта."""
        # Из package.json
        if project_type == "nodejs":
            package_json = self.workspace_root / "package.json"
            if package_json.exists():
                try:
                    with open(package_json) as f:
                        data = json.load(f)
                        return data.get("name", self.workspace_root.name)
                except:
                    pass

        # Из pyproject.toml
        if project_type == "python":
            pyproject = self.workspace_root / "pyproject.toml"
            if pyproject.exists():
                try:
                    with open(pyproject) as f:
                        for line in f:
                            if line.startswith("name"):
                                return line.split("=")[1].strip().strip('"\'')
                except:
                    pass

        # По умолчанию - имя директории
        return self.workspace_root.name

    def _detect_framework(self, project_type: str) -> str | None:
        """Определить используемый фреймворк."""
        if project_type == "python":
            # Проверяем популярные Python фреймворки
            requirements = self.workspace_root / "requirements.txt"
            if requirements.exists():
                content = requirements.read_text().lower()
                if "django" in content:
                    return "Django"
                elif "flask" in content:
                    return "Flask"
                elif "fastapi" in content:
                    return "FastAPI"

        elif project_type == "nodejs":
            package_json = self.workspace_root / "package.json"
            if package_json.exists():
                try:
                    with open(package_json) as f:
                        data = json.load(f)
                        deps = {**data.get("dependencies", {}), **data.get("devDependencies", {})}

                        if "react" in deps:
                            return "React"
                        elif "vue" in deps:
                            return "Vue"
                        elif "angular" in deps or "@angular/core" in deps:
                            return "Angular"
                        elif "express" in deps:
                            return "Express"
                        elif "next" in deps:
                            return "Next.js"
                except:
                    pass

        return None

    def _get_dependencies(self, project_type: str) -> tuple[List[str], List[str]]:
        """Получить список зависимостей."""
        dependencies = []
        dev_dependencies = []

        if project_type == "python":
            requirements = self.workspace_root / "requirements.txt"
            if requirements.exists():
                dependencies = [
                    line.strip().split("==")[0].split(">=")[0].split("<=")[0]
                    for line in requirements.read_text().splitlines()
                    if line.strip() and not line.startswith("#")
                ]

        elif project_type == "nodejs":
            package_json = self.workspace_root / "package.json"
            if package_json.exists():
                try:
                    with open(package_json) as f:
                        data = json.load(f)
                        dependencies = list(data.get("dependencies", {}).keys())
                        dev_dependencies = list(data.get("devDependencies", {}).keys())
                except:
                    pass

        return dependencies, dev_dependencies

    def _detect_test_framework(self, project_type: str, dev_deps: List[str]) -> str | None:
        """Определить тестовый фреймворк."""
        if project_type == "python":
            if any("pytest" in dep for dep in dev_deps):
                return "pytest"
            # Проверяем наличие test файлов
            if list(self.workspace_root.rglob("test_*.py")):
                return "pytest/unittest"

        elif project_type == "nodejs":
            if "jest" in dev_deps:
                return "jest"
            elif "mocha" in dev_deps:
                return "mocha"
            elif "vitest" in dev_deps:
                return "vitest"

        return None

    def _analyze_file_structure(self) -> Dict[str, int]:
        """Проанализировать структуру файлов."""
        structure = {}

        extensions = [".py", ".js", ".ts", ".jsx", ".tsx", ".go", ".rs", ".java", ".c", ".cpp", ".h"]

        for ext in extensions:
            files = list(self.workspace_root.rglob(f"*{ext}"))
            if files:
                structure[ext] = len(files)

        # Добавляем конфигурационные файлы
        config_files = [
            "package.json", "requirements.txt", "pyproject.toml",
            "Cargo.toml", "go.mod", "pom.xml", "build.gradle"
        ]

        for config in config_files:
            if (self.workspace_root / config).exists():
                structure[config] = 1

        return structure

    def _find_entry_points(self, project_type: str) -> List[str]:
        """Найти точки входа в приложение."""
        entry_points = []

        if project_type == "python":
            # Ищем main.py, __main__.py, app.py
            for name in ["main.py", "__main__.py", "app.py", "run.py"]:
                if (self.workspace_root / name).exists():
                    entry_points.append(name)

        elif project_type == "nodejs":
            package_json = self.workspace_root / "package.json"
            if package_json.exists():
                try:
                    with open(package_json) as f:
                        data = json.load(f)
                        main = data.get("main")
                        if main:
                            entry_points.append(main)
                except:
                    pass

            # Проверяем стандартные файлы
            for name in ["index.js", "index.ts", "server.js", "app.js"]:
                if (self.workspace_root / name).exists():
                    entry_points.append(name)

        return entry_points

    def _format_project_info(self, info: ProjectInfo, detailed: bool) -> str:
        """Форматировать информацию о проекте."""
        output = f"""# Project Analysis: {info.name}

## Overview
- **Type:** {info.type}
- **Language:** {info.language}
- **Framework:** {info.framework or 'None detected'}
- **Test Framework:** {info.test_framework or 'None detected'}

## Dependencies
- **Production:** {len(info.dependencies)} packages
- **Development:** {len(info.dev_dependencies)} packages
"""

        if detailed and info.dependencies:
            output += "\n### Production Dependencies:\n"
            for dep in info.dependencies[:10]:  # Показываем первые 10
                output += f"  - {dep}\n"
            if len(info.dependencies) > 10:
                output += f"  ... and {len(info.dependencies) - 10} more\n"

        if detailed and info.dev_dependencies:
            output += "\n### Development Dependencies:\n"
            for dep in info.dev_dependencies[:10]:
                output += f"  - {dep}\n"
            if len(info.dev_dependencies) > 10:
                output += f"  ... and {len(info.dev_dependencies) - 10} more\n"

        output += "\n## File Structure\n"
        for ext, count in sorted(info.structure.items(), key=lambda x: x[1], reverse=True):
            output += f"  - {ext}: {count} file(s)\n"

        if info.entry_points:
            output += "\n## Entry Points\n"
            for entry in info.entry_points:
                output += f"  - {entry}\n"

        return output
