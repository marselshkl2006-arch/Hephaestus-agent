"""
Валидатор команд - проверяет доступность команд и предлагает альтернативы.
"""
from __future__ import annotations

import shutil
import subprocess
from dataclasses import dataclass
from typing import Optional


@dataclass
class CommandValidation:
    """Результат валидации команды."""
    is_valid: bool
    original_command: str
    fixed_command: Optional[str] = None
    warning: Optional[str] = None
    error: Optional[str] = None


class CommandValidator:
    """Валидатор и исправитель команд."""

    def __init__(self):
        self._command_cache = {}  # Кеш проверенных команд

    def validate_command(self, command: str) -> CommandValidation:
        """
        Проверить команду и предложить исправления если нужно.

        Args:
            command: Команда для проверки

        Returns:
            CommandValidation с результатом
        """
        # Извлекаем первое слово (саму команду)
        cmd_parts = command.strip().split()
        if not cmd_parts:
            return CommandValidation(
                is_valid=False,
                original_command=command,
                error="Пустая команда"
            )

        base_cmd = cmd_parts[0]

        # Проверяем docker-compose
        if base_cmd == "docker-compose":
            return self._validate_docker_compose(command)

        # Проверяем source
        if base_cmd == "source" or command.strip().startswith(". "):
            return self._validate_source(command)

        # Проверяем другие команды
        if base_cmd in ["docker", "git", "npm", "pip", "python3"]:
            if not self._is_command_available(base_cmd):
                return CommandValidation(
                    is_valid=False,
                    original_command=command,
                    error=f"Команда '{base_cmd}' не найдена в системе"
                )

        return CommandValidation(
            is_valid=True,
            original_command=command
        )

    def _validate_docker_compose(self, command: str) -> CommandValidation:
        """Проверить docker-compose и предложить альтернативу."""
        # Проверяем старую версию docker-compose
        if self._is_command_available("docker-compose"):
            return CommandValidation(
                is_valid=True,
                original_command=command
            )

        # Проверяем новую версию docker compose (без дефиса)
        if self._is_command_available("docker"):
            # Проверяем поддержку compose plugin
            try:
                result = subprocess.run(
                    ["docker", "compose", "version"],
                    capture_output=True,
                    timeout=5
                )
                if result.returncode == 0:
                    # Заменяем docker-compose на docker compose
                    fixed = command.replace("docker-compose", "docker compose")
                    return CommandValidation(
                        is_valid=True,
                        original_command=command,
                        fixed_command=fixed,
                        warning="docker-compose не найден, используем 'docker compose' (новая версия)"
                    )
            except:
                pass

        return CommandValidation(
            is_valid=False,
            original_command=command,
            error="docker-compose не найден. Установите Docker Compose: https://docs.docker.com/compose/install/"
        )

    def _validate_source(self, command: str) -> CommandValidation:
        """Проверить source команду и исправить для bash."""
        # source работает только в bash, не в sh
        fixed = f'bash -c {repr(command)}'
        return CommandValidation(
            is_valid=True,
            original_command=command,
            fixed_command=fixed,
            warning="'source' требует bash, команда будет выполнена через 'bash -c'"
        )

    def _is_command_available(self, command: str) -> bool:
        """Проверить доступность команды в системе."""
        if command in self._command_cache:
            return self._command_cache[command]

        result = shutil.which(command) is not None
        self._command_cache[command] = result
        return result
