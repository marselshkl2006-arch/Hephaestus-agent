"""
Config Tools - инструменты для управления конфигурацией.
Поддержка: YAML, JSON, TOML, .env файлов.
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str = ""


class ConfigManagerTool:
    """Управление конфигурационными файлами."""

    def __init__(self, workspace_root: str | None = None):
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else Path.cwd()

    def read_config(self, file_path: str, format: str = "auto") -> ToolResult:
        """Прочитать конфигурационный файл.

        Args:
            file_path: Путь к файлу конфигурации
            format: Формат файла (auto, json, yaml, toml, env)

        Returns:
            ToolResult с содержимым конфигурации
        """
        try:
            path = Path(file_path)
            if not path.is_absolute():
                path = self.workspace_root / path

            if not path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Config file not found: {file_path}"
                )

            # Автоопределение формата
            if format == "auto":
                format = self._detect_format(path)

            # Читаем файл
            if format == "json":
                return self._read_json(path)
            elif format == "yaml":
                return self._read_yaml(path)
            elif format == "toml":
                return self._read_toml(path)
            elif format == "env":
                return self._read_env(path)
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Unsupported format: {format}. Use: json, yaml, toml, env"
                )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error reading config: {str(e)}"
            )

    def write_config(
        self,
        file_path: str,
        data: dict[str, Any],
        format: str = "auto"
    ) -> ToolResult:
        """Записать конфигурационный файл.

        Args:
            file_path: Путь к файлу
            data: Данные для записи
            format: Формат файла (auto, json, yaml, toml)

        Returns:
            ToolResult с результатом записи
        """
        try:
            path = Path(file_path)
            if not path.is_absolute():
                path = self.workspace_root / path

            # Автоопределение формата
            if format == "auto":
                format = self._detect_format(path)

            # Создаём директорию если нужно
            path.parent.mkdir(parents=True, exist_ok=True)

            # Записываем файл
            if format == "json":
                return self._write_json(path, data)
            elif format == "yaml":
                return self._write_yaml(path, data)
            elif format == "toml":
                return self._write_toml(path, data)
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Unsupported format for writing: {format}"
                )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error writing config: {str(e)}"
            )

    def get_env_var(self, var_name: str, default: str | None = None) -> ToolResult:
        """Получить переменную окружения.

        Args:
            var_name: Имя переменной
            default: Значение по умолчанию

        Returns:
            ToolResult со значением переменной
        """
        value = os.getenv(var_name, default)
        
        if value is None:
            return ToolResult(
                success=False,
                output="",
                error=f"Environment variable not found: {var_name}"
            )

        return ToolResult(
            success=True,
            output=f"{var_name}={value}"
        )

    def set_env_var(self, var_name: str, value: str) -> ToolResult:
        """Установить переменную окружения (только для текущего процесса).

        Args:
            var_name: Имя переменной
            value: Значение

        Returns:
            ToolResult с результатом
        """
        try:
            os.environ[var_name] = value
            return ToolResult(
                success=True,
                output=f"Set {var_name}={value}"
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error setting env var: {str(e)}"
            )

    def _detect_format(self, path: Path) -> str:
        """Определить формат файла по расширению."""
        suffix = path.suffix.lower()
        
        if suffix == ".json":
            return "json"
        elif suffix in [".yaml", ".yml"]:
            return "yaml"
        elif suffix == ".toml":
            return "toml"
        elif suffix == ".env":
            return "env"
        else:
            return "json"  # По умолчанию

    def _read_json(self, path: Path) -> ToolResult:
        """Прочитать JSON файл."""
        try:
            with open(path, 'r', encoding='utf-8') as f:
                data = json.load(f)
            
            output = json.dumps(data, indent=2, ensure_ascii=False)
            return ToolResult(success=True, output=output)

        except json.JSONDecodeError as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Invalid JSON: {str(e)}"
            )

    def _read_yaml(self, path: Path) -> ToolResult:
        """Прочитать YAML файл."""
        try:
            import yaml
        except ImportError:
            return ToolResult(
                success=False,
                output="",
                error="PyYAML not installed. Install: pip install pyyaml"
            )

        try:
            with open(path, 'r', encoding='utf-8') as f:
                data = yaml.safe_load(f)
            
            output = yaml.dump(data, default_flow_style=False, allow_unicode=True)
            return ToolResult(success=True, output=output)

        except yaml.YAMLError as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Invalid YAML: {str(e)}"
            )

    def _read_toml(self, path: Path) -> ToolResult:
        """Прочитать TOML файл."""
        try:
            import tomli
        except ImportError:
            return ToolResult(
                success=False,
                output="",
                error="tomli not installed. Install: pip install tomli"
            )

        try:
            with open(path, 'rb') as f:
                data = tomli.load(f)
            
            output = json.dumps(data, indent=2, ensure_ascii=False)
            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Invalid TOML: {str(e)}"
            )

    def _read_env(self, path: Path) -> ToolResult:
        """Прочитать .env файл."""
        try:
            env_vars = {}
            
            with open(path, 'r', encoding='utf-8') as f:
                for line in f:
                    line = line.strip()
                    
                    # Пропускаем пустые строки и комментарии
                    if not line or line.startswith('#'):
                        continue
                    
                    # Парсим KEY=VALUE
                    if '=' in line:
                        key, value = line.split('=', 1)
                        key = key.strip()
                        value = value.strip()
                        
                        # Убираем кавычки
                        if value.startswith('"') and value.endswith('"'):
                            value = value[1:-1]
                        elif value.startswith("'") and value.endswith("'"):
                            value = value[1:-1]
                        
                        env_vars[key] = value
            
            output = "\n".join(f"{k}={v}" for k, v in env_vars.items())
            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error reading .env: {str(e)}"
            )

    def _write_json(self, path: Path, data: dict) -> ToolResult:
        """Записать JSON файл."""
        try:
            with open(path, 'w', encoding='utf-8') as f:
                json.dump(data, f, indent=2, ensure_ascii=False)
            
            return ToolResult(
                success=True,
                output=f"Config written to: {path}"
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error writing JSON: {str(e)}"
            )

    def _write_yaml(self, path: Path, data: dict) -> ToolResult:
        """Записать YAML файл."""
        try:
            import yaml
        except ImportError:
            return ToolResult(
                success=False,
                output="",
                error="PyYAML not installed. Install: pip install pyyaml"
            )

        try:
            with open(path, 'w', encoding='utf-8') as f:
                yaml.dump(data, f, default_flow_style=False, allow_unicode=True)
            
            return ToolResult(
                success=True,
                output=f"Config written to: {path}"
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error writing YAML: {str(e)}"
            )

    def _write_toml(self, path: Path, data: dict) -> ToolResult:
        """Записать TOML файл."""
        try:
            import tomli_w
        except ImportError:
            return ToolResult(
                success=False,
                output="",
                error="tomli-w not installed. Install: pip install tomli-w"
            )

        try:
            with open(path, 'wb') as f:
                tomli_w.dump(data, f)
            
            return ToolResult(
                success=True,
                output=f"Config written to: {path}"
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error writing TOML: {str(e)}"
            )
