"""
Новые критичные инструменты для автоматизации разработки.
"""
from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .real_tools import ToolResult


@dataclass
class ProjectInfo:
    """Информация о проекте."""
    root: Path
    language: str | None = None
    framework: str | None = None
    dependencies: list[str] | None = None
    test_framework: str | None = None
    package_manager: str | None = None


class ProjectAnalyzerTool:
    """Анализ структуры проекта и определение технологий."""

    def analyze(self, path: str = ".") -> ToolResult:
        """
        Проанализировать проект и определить его характеристики.

        Args:
            path: Путь к проекту

        Returns:
            ToolResult с информацией о проекте
        """
        try:
            project_path = Path(path).resolve()
            if not project_path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Path not found: {path}"
                )

            info = ProjectInfo(root=project_path)

            # Определяем язык и фреймворк по файлам
            files = list(project_path.rglob("*"))
            file_names = [f.name for f in files if f.is_file()]

            # Python
            if "requirements.txt" in file_names or "setup.py" in file_names or "pyproject.toml" in file_names:
                info.language = "Python"
                info.package_manager = "pip"

                # Определяем фреймворк
                if "manage.py" in file_names:
                    info.framework = "Django"
                elif any("fastapi" in f.lower() for f in file_names):
                    info.framework = "FastAPI"
                elif any("flask" in f.lower() for f in file_names):
                    info.framework = "Flask"

                # Определяем тест-фреймворк
                if "pytest.ini" in file_names or any("pytest" in f for f in file_names):
                    info.test_framework = "pytest"
                elif any("unittest" in f for f in file_names):
                    info.test_framework = "unittest"

            # JavaScript/TypeScript
            elif "package.json" in file_names:
                info.language = "JavaScript/TypeScript"
                info.package_manager = "npm"

                # Читаем package.json для определения фреймворка
                package_json_path = project_path / "package.json"
                if package_json_path.exists():
                    try:
                        with open(package_json_path) as f:
                            package_data = json.load(f)
                            deps = {**package_data.get("dependencies", {}), **package_data.get("devDependencies", {})}

                            if "react" in deps:
                                info.framework = "React"
                            elif "vue" in deps:
                                info.framework = "Vue"
                            elif "next" in deps:
                                info.framework = "Next.js"
                            elif "express" in deps:
                                info.framework = "Express"

                            if "jest" in deps:
                                info.test_framework = "Jest"
                            elif "mocha" in deps:
                                info.test_framework = "Mocha"
                    except:
                        pass

            # Go
            elif "go.mod" in file_names:
                info.language = "Go"
                info.package_manager = "go"
                info.test_framework = "go test"

            # Rust
            elif "Cargo.toml" in file_names:
                info.language = "Rust"
                info.package_manager = "cargo"
                info.test_framework = "cargo test"

            # Ruby
            elif "Gemfile" in file_names:
                info.language = "Ruby"
                info.package_manager = "bundler"
                if "config.ru" in file_names:
                    info.framework = "Rails/Rack"

            # Java
            elif "pom.xml" in file_names:
                info.language = "Java"
                info.package_manager = "maven"
            elif "build.gradle" in file_names:
                info.language = "Java/Kotlin"
                info.package_manager = "gradle"

            # Формируем отчет
            report = f"""Project Analysis:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Root: {info.root}
Language: {info.language or 'Unknown'}
Framework: {info.framework or 'None detected'}
Package Manager: {info.package_manager or 'Unknown'}
Test Framework: {info.test_framework or 'None detected'}
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Files found: {len(files)}
"""

            # Добавляем структуру директорий
            dirs = [f for f in files if f.is_dir() and not any(part.startswith('.') for part in f.parts)]
            if dirs:
                report += "\nKey directories:\n"
                for d in sorted(dirs)[:10]:
                    rel_path = d.relative_to(project_path)
                    report += f"  - {rel_path}/\n"

            return ToolResult(
                success=True,
                output=report,
                error=None
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Analysis error: {str(e)}"
            )


class PackageInstallTool:
    """Установка пакетов и зависимостей."""

    def install(self, package: str, manager: str = "auto", dev: bool = False, global_install: bool = False) -> ToolResult:
        """
        Установить пакет.

        Args:
            package: Имя пакета для установки
            manager: Менеджер пакетов (auto, pip, npm, cargo, etc.)
            dev: Установить как dev-зависимость (для npm)
            global_install: Глобальная установка

        Returns:
            ToolResult с результатом установки
        """
        try:
            # Автоопределение менеджера пакетов если auto
            if manager == "auto":
                if Path("requirements.txt").exists() or Path("setup.py").exists():
                    manager = "pip"
                elif Path("package.json").exists():
                    manager = "npm"
                elif Path("Cargo.toml").exists():
                    manager = "cargo"
                elif Path("go.mod").exists():
                    manager = "go"
                else:
                    return ToolResult(
                        success=False,
                        output="",
                        error="Cannot detect package manager. Please specify explicitly."
                    )

            # Формируем команду
            if manager == "pip":
                cmd = ["pip", "install", package]
            elif manager == "npm":
                cmd = ["npm", "install", package]
            elif manager == "yarn":
                cmd = ["yarn", "add", package]
            elif manager == "cargo":
                cmd = ["cargo", "add", package]
            elif manager == "go":
                cmd = ["go", "get", package]
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Unsupported package manager: {manager}"
                )

            # Выполняем установку
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=300  # 5 минут
            )

            if result.returncode == 0:
                return ToolResult(
                    success=True,
                    output=f"✓ Installed: {package}\n\n{result.stdout}",
                    error=None
                )
            else:
                return ToolResult(
                    success=False,
                    output=result.stdout,
                    error=result.stderr
                )

        except subprocess.TimeoutExpired:
            return ToolResult(
                success=False,
                output="",
                error="Installation timeout (5 minutes)"
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Installation error: {str(e)}"
            )


class TestRunnerTool:
    """Запуск тестов."""

    def run(self, test_path: str | None = None, framework: str | None = None) -> ToolResult:
        """
        Запустить тесты.

        Args:
            test_path: Путь к тестам (опционально)
            framework: Тест-фреймворк (pytest, jest, go test, etc.)

        Returns:
            ToolResult с результатами тестов
        """
        try:
            # Автоопределение фреймворка
            if not framework:
                if Path("pytest.ini").exists() or any(Path(".").glob("**/test_*.py")):
                    framework = "pytest"
                elif Path("package.json").exists():
                    framework = "npm"
                elif Path("go.mod").exists():
                    framework = "go"
                elif Path("Cargo.toml").exists():
                    framework = "cargo"
                else:
                    return ToolResult(
                        success=False,
                        output="",
                        error="Cannot detect test framework. Please specify explicitly."
                    )

            # Формируем команду
            if framework == "pytest":
                cmd = ["pytest", "-v"]
                if test_path:
                    cmd.append(test_path)
            elif framework == "npm" or framework == "jest":
                cmd = ["npm", "test"]
            elif framework == "go":
                cmd = ["go", "test", "./..."]
                if test_path:
                    cmd = ["go", "test", test_path]
            elif framework == "cargo":
                cmd = ["cargo", "test"]
            elif framework == "unittest":
                cmd = ["python", "-m", "unittest", "discover"]
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Unsupported test framework: {framework}"
                )

            # Запускаем тесты
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=300  # 5 минут
            )

            output = f"Test Results:\n{'='*50}\n{result.stdout}\n"
            if result.stderr:
                output += f"\nErrors:\n{result.stderr}\n"

            return ToolResult(
                success=result.returncode == 0,
                output=output,
                error=None if result.returncode == 0 else "Some tests failed"
            )

        except subprocess.TimeoutExpired:
            return ToolResult(
                success=False,
                output="",
                error="Tests timeout (5 minutes)"
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Test execution error: {str(e)}"
            )


class HttpRequestTool:
    """HTTP запросы к API."""

    def request(
        self,
        url: str,
        method: str = "GET",
        headers: dict | None = None,
        data: dict | None = None,
        json_data: dict | None = None,
    ) -> ToolResult:
        """
        Выполнить HTTP запрос.

        Args:
            url: URL для запроса
            method: HTTP метод (GET, POST, PUT, DELETE, etc.)
            headers: HTTP заголовки
            data: Данные формы
            json_data: JSON данные

        Returns:
            ToolResult с ответом
        """
        try:
            import requests

            method = method.upper()
            kwargs = {}

            if headers:
                kwargs["headers"] = headers
            if data:
                kwargs["data"] = data
            if json_data:
                kwargs["json"] = json_data

            response = requests.request(method, url, **kwargs, timeout=30)

            output = f"""HTTP {method} {url}
Status: {response.status_code} {response.reason}
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Headers:
{json.dumps(dict(response.headers), indent=2)}

Body:
{response.text[:1000]}{'...' if len(response.text) > 1000 else ''}
"""

            return ToolResult(
                success=response.ok,
                output=output,
                error=None if response.ok else f"HTTP {response.status_code}"
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"HTTP request error: {str(e)}"
            )


def create_advanced_tools() -> dict[str, Any]:
    """Создать продвинутые инструменты."""
    return {
        "project_analyze": ProjectAnalyzerTool(),
        "package_install": PackageInstallTool(),
        "test_run": TestRunnerTool(),
        "http_request": HttpRequestTool(),
    }
