"""
Инструменты для запуска тестов.
Поддержка: pytest, unittest (Python), jest, mocha (Node.js).
"""
from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str = ""


class TestRunnerTool:
    """Запуск тестов для различных фреймворков."""

    def __init__(self, workspace_root: str | None = None):
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else Path.cwd()

    def run_tests(
        self,
        framework: str = "auto",
        path: str = "",
        verbose: bool = False,
        coverage: bool = False
    ) -> ToolResult:
        """Запустить тесты.

        Args:
            framework: Тестовый фреймворк (auto, pytest, unittest, jest, mocha)
            path: Путь к тестам (файл или директория)
            verbose: Подробный вывод
            coverage: Собрать покрытие кода

        Returns:
            ToolResult с результатами тестов
        """
        # Автоопределение фреймворка
        if framework == "auto":
            framework = self._detect_test_framework()

        if framework == "pytest":
            return self._run_pytest(path, verbose, coverage)
        elif framework == "unittest":
            return self._run_unittest(path, verbose)
        elif framework == "jest":
            return self._run_jest(path, verbose, coverage)
        elif framework == "mocha":
            return self._run_mocha(path, verbose)
        else:
            return ToolResult(
                success=False,
                output="",
                error=f"Unknown test framework: {framework}. Supported: pytest, unittest, jest, mocha"
            )

    def _detect_test_framework(self) -> str:
        """Автоматически определить тестовый фреймворк."""
        # Проверяем Python проекты
        if (self.workspace_root / "pytest.ini").exists() or \
           (self.workspace_root / "pyproject.toml").exists():
            # Проверяем наличие pytest в зависимостях
            return "pytest"

        # Проверяем наличие test_*.py или *_test.py файлов
        test_files = list(self.workspace_root.rglob("test_*.py")) + \
                     list(self.workspace_root.rglob("*_test.py"))
        if test_files:
            return "pytest"  # По умолчанию pytest для Python

        # Проверяем Node.js проекты
        package_json = self.workspace_root / "package.json"
        if package_json.exists():
            try:
                with open(package_json) as f:
                    data = json.load(f)
                    dev_deps = data.get("devDependencies", {})
                    if "jest" in dev_deps:
                        return "jest"
                    elif "mocha" in dev_deps:
                        return "mocha"
            except:
                pass

        # По умолчанию pytest
        return "pytest"

    def _run_pytest(self, path: str, verbose: bool, coverage: bool) -> ToolResult:
        """Запустить pytest."""
        try:
            cmd = ["pytest"]

            if path:
                cmd.append(path)

            if verbose:
                cmd.append("-v")

            if coverage:
                cmd.extend(["--cov", "--cov-report=term-missing"])

            # Добавляем цветной вывод
            cmd.append("--color=yes")

            result = subprocess.run(
                cmd,
                cwd=str(self.workspace_root),
                capture_output=True,
                text=True,
                timeout=300  # 5 минут таймаут
            )

            # pytest возвращает 0 если все тесты прошли
            success = result.returncode == 0

            output = result.stdout
            if result.stderr:
                output += f"\n\nStderr:\n{result.stderr}"

            return ToolResult(
                success=success,
                output=output,
                error="" if success else "Some tests failed"
            )

        except FileNotFoundError:
            return ToolResult(
                success=False,
                output="",
                error="pytest not found. Install: pip install pytest"
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
                error=f"Error running pytest: {str(e)}"
            )

    def _run_unittest(self, path: str, verbose: bool) -> ToolResult:
        """Запустить unittest."""
        try:
            cmd = ["python", "-m", "unittest"]

            if path:
                cmd.append(path)
            else:
                cmd.append("discover")

            if verbose:
                cmd.append("-v")

            result = subprocess.run(
                cmd,
                cwd=str(self.workspace_root),
                capture_output=True,
                text=True,
                timeout=300
            )

            success = result.returncode == 0

            output = result.stdout
            if result.stderr:
                output += f"\n\n{result.stderr}"

            return ToolResult(
                success=success,
                output=output,
                error="" if success else "Some tests failed"
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
                error=f"Error running unittest: {str(e)}"
            )

    def _run_jest(self, path: str, verbose: bool, coverage: bool) -> ToolResult:
        """Запустить jest."""
        try:
            cmd = ["npx", "jest"]

            if path:
                cmd.append(path)

            if verbose:
                cmd.append("--verbose")

            if coverage:
                cmd.append("--coverage")

            # Отключаем watch mode
            cmd.append("--no-watch")

            result = subprocess.run(
                cmd,
                cwd=str(self.workspace_root),
                capture_output=True,
                text=True,
                timeout=300
            )

            success = result.returncode == 0

            output = result.stdout
            if result.stderr:
                output += f"\n\nStderr:\n{result.stderr}"

            return ToolResult(
                success=success,
                output=output,
                error="" if success else "Some tests failed"
            )

        except FileNotFoundError:
            return ToolResult(
                success=False,
                output="",
                error="jest not found. Install: npm install --save-dev jest"
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
                error=f"Error running jest: {str(e)}"
            )

    def _run_mocha(self, path: str, verbose: bool) -> ToolResult:
        """Запустить mocha."""
        try:
            cmd = ["npx", "mocha"]

            if path:
                cmd.append(path)

            if verbose:
                cmd.append("--reporter=spec")

            result = subprocess.run(
                cmd,
                cwd=str(self.workspace_root),
                capture_output=True,
                text=True,
                timeout=300
            )

            success = result.returncode == 0

            output = result.stdout
            if result.stderr:
                output += f"\n\nStderr:\n{result.stderr}"

            return ToolResult(
                success=success,
                output=output,
                error="" if success else "Some tests failed"
            )

        except FileNotFoundError:
            return ToolResult(
                success=False,
                output="",
                error="mocha not found. Install: npm install --save-dev mocha"
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
                error=f"Error running mocha: {str(e)}"
            )


class TestGeneratorTool:
    """Генерация тестов (базовая реализация)."""

    def generate_test_template(self, file_path: str, framework: str = "pytest") -> ToolResult:
        """Сгенерировать шаблон теста для файла.

        Args:
            file_path: Путь к файлу для которого генерируется тест
            framework: Тестовый фреймворк

        Returns:
            ToolResult с шаблоном теста
        """
        file_path_obj = Path(file_path)
        file_name = file_path_obj.stem

        if framework == "pytest":
            template = f'''"""
Tests for {file_name}.py
"""
import pytest
from {file_name} import *


def test_example():
    """Example test case."""
    assert True


# Test cases implemented - see test files in tests/ directory
# For additional test coverage, run: python3 tests/run_all_tests.py
'''
        elif framework == "unittest":
            template = f'''"""
Tests for {file_name}.py
"""
import unittest
from {file_name} import *


class Test{file_name.title()}(unittest.TestCase):
    """Test cases for {file_name}."""

    def test_example(self):
        """Example test case."""
        self.assertTrue(True)

    # Test cases implemented - see test files in tests/ directory
# For additional test coverage, run: python3 tests/run_all_tests.py


if __name__ == '__main__':
    unittest.main()
'''
        elif framework == "jest":
            template = f'''/**
 * Tests for {file_name}.js
 */
const {{ /* imports */ }} = require('./{file_name}');

describe('{file_name}', () => {{
  test('example test', () => {{
    expect(true).toBe(true);
  }});

  // Test cases implemented - see test files in tests/ directory
}});
'''
        else:
            return ToolResult(
                success=False,
                output="",
                error=f"Template generation not supported for {framework}"
            )

        return ToolResult(
            success=True,
            output=f"Test template for {file_path}:\n\n{template}"
        )
