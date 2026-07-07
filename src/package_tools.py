"""
Package Tools - инструменты для управления пакетами.
Поддержка: pip (Python), npm (Node.js), apt (Debian/Ubuntu).
"""
from __future__ import annotations

import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str = ""


class PackageInstallTool:
    """Установка пакетов через различные менеджеры."""

    def __init__(self, workspace_root: str | None = None):
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else Path.cwd()

    def install(
        self,
        package: str,
        manager: str = "auto",
        dev: bool = False,
        global_install: bool = False
    ) -> ToolResult:
        """Установить пакет.

        Args:
            package: Имя пакета для установки
            manager: Менеджер пакетов (auto, pip, npm, apt)
            dev: Установить как dev-зависимость (для npm)
            global_install: Глобальная установка

        Returns:
            ToolResult с результатом установки
        """
        # Автоопределение менеджера
        if manager == "auto":
            manager = self._detect_package_manager()

        if manager == "pip":
            return self._install_pip(package, global_install)
        elif manager == "npm":
            return self._install_npm(package, dev, global_install)
        elif manager == "apt":
            return self._install_apt(package)
        else:
            return ToolResult(
                success=False,
                output="",
                error=f"Unknown package manager: {manager}. Supported: pip, npm, apt"
            )

    def _detect_package_manager(self) -> str:
        """Автоматически определить менеджер пакетов."""
        # Проверяем Python проекты
        if (self.workspace_root / "requirements.txt").exists() or \
           (self.workspace_root / "pyproject.toml").exists() or \
           (self.workspace_root / "setup.py").exists():
            return "pip"

        # Проверяем Node.js проекты
        if (self.workspace_root / "package.json").exists():
            return "npm"

        # По умолчанию pip (т.к. мы в Python окружении)
        return "pip"

    def _install_pip(self, package: str, global_install: bool) -> ToolResult:
        """Установить Python пакет через pip."""
        try:
            cmd = [sys.executable, "-m", "pip", "install"]

            # Если не глобальная установка - используем --user
            if not global_install:
                cmd.append("--user")

            cmd.append(package)

            result = subprocess.run(
                cmd,
                cwd=str(self.workspace_root),
                capture_output=True,
                text=True,
                timeout=300  # 5 минут таймаут
            )

            success = result.returncode == 0

            output = result.stdout
            if result.stderr:
                output += f"\n{result.stderr}"

            return ToolResult(
                success=success,
                output=output,
                error="" if success else f"Failed to install {package}"
            )

        except FileNotFoundError:
            return ToolResult(
                success=False,
                output="",
                error="pip not found. Install Python first."
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
                error=f"Error installing package: {str(e)}"
            )

    def _install_npm(self, package: str, dev: bool, global_install: bool) -> ToolResult:
        """Установить Node.js пакет через npm."""
        try:
            cmd = ["npm", "install"]

            if global_install:
                cmd.append("-g")
            elif dev:
                cmd.append("--save-dev")

            cmd.append(package)

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
                output += f"\n{result.stderr}"

            return ToolResult(
                success=success,
                output=output,
                error="" if success else f"Failed to install {package}"
            )

        except FileNotFoundError:
            return ToolResult(
                success=False,
                output="",
                error="npm not found. Install Node.js first."
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
                error=f"Error installing package: {str(e)}"
            )

    def _install_apt(self, package: str) -> ToolResult:
        """Установить системный пакет через apt."""
        try:
            # apt требует sudo
            cmd = ["sudo", "apt-get", "install", "-y", package]

            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=600  # 10 минут для системных пакетов
            )

            success = result.returncode == 0

            output = result.stdout
            if result.stderr:
                output += f"\n{result.stderr}"

            return ToolResult(
                success=success,
                output=output,
                error="" if success else f"Failed to install {package}"
            )

        except FileNotFoundError:
            return ToolResult(
                success=False,
                output="",
                error="apt-get not found. Only works on Debian/Ubuntu."
            )
        except subprocess.TimeoutExpired:
            return ToolResult(
                success=False,
                output="",
                error="Installation timeout (10 minutes)"
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error installing package: {str(e)}"
            )


class PackageSearchTool:
    """Поиск пакетов в репозиториях."""

    def search(self, query: str, manager: str = "pip") -> ToolResult:
        """Поиск пакетов.

        Args:
            query: Поисковый запрос
            manager: Менеджер пакетов (pip, npm)

        Returns:
            ToolResult со списком найденных пакетов
        """
        if manager == "pip":
            return self._search_pip(query)
        elif manager == "npm":
            return self._search_npm(query)
        else:
            return ToolResult(
                success=False,
                output="",
                error=f"Search not supported for {manager}"
            )

    def _search_pip(self, query: str) -> ToolResult:
        """Поиск Python пакетов."""
        try:
            # pip search был отключён, используем PyPI API
            import requests
            response = requests.get(
                f"https://pypi.org/pypi/{query}/json",
                timeout=10
            )

            if response.status_code == 200:
                data = response.json()
                info = data.get("info", {})
                output = f"Package: {info.get('name')}\n"
                output += f"Version: {info.get('version')}\n"
                output += f"Summary: {info.get('summary')}\n"
                output += f"Author: {info.get('author')}\n"
                output += f"License: {info.get('license')}\n"
                output += f"URL: {info.get('package_url')}\n"

                return ToolResult(success=True, output=output)
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Package '{query}' not found on PyPI"
                )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error searching package: {str(e)}"
            )

    def _search_npm(self, query: str) -> ToolResult:
        """Поиск Node.js пакетов."""
        try:
            cmd = ["npm", "search", query, "--json"]

            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=30
            )

            if result.returncode == 0:
                packages = json.loads(result.stdout)
                if packages:
                    output = ""
                    for pkg in packages[:5]:  # Первые 5 результатов
                        output += f"Package: {pkg.get('name')}\n"
                        output += f"Version: {pkg.get('version')}\n"
                        output += f"Description: {pkg.get('description')}\n"
                        output += f"Author: {pkg.get('author', {}).get('name', 'N/A')}\n"
                        output += "\n"

                    return ToolResult(success=True, output=output)
                else:
                    return ToolResult(
                        success=False,
                        output="",
                        error=f"No packages found for '{query}'"
                    )
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=result.stderr
                )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error searching package: {str(e)}"
            )
