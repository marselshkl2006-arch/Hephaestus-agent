"""
Система автоматической проверки результатов выполнения команд.
Проверяет что команды действительно выполнились успешно.
"""
from __future__ import annotations

import subprocess
from dataclasses import dataclass
from typing import Optional


@dataclass
class VerificationResult:
    """Результат проверки."""
    verified: bool
    message: str
    details: Optional[str] = None


class ResultVerifier:
    """Проверяет результаты выполнения команд."""

    def verify_package_install(self, packages: str) -> VerificationResult:
        """
        Проверить что пакеты установлены.

        Args:
            packages: Список пакетов (через пробел)

        Returns:
            VerificationResult
        """
        if not packages:
            return VerificationResult(True, "Нет пакетов для проверки")

        # Разбиваем на отдельные пакеты
        package_list = packages.split()

        try:
            # Проверяем через pip list
            result = subprocess.run(
                ["pip", "list", "--format=freeze"],
                capture_output=True,
                text=True,
                timeout=10
            )

            if result.returncode != 0:
                return VerificationResult(
                    False,
                    "Не удалось проверить установленные пакеты",
                    result.stderr
                )

            installed = result.stdout.lower()
            missing = []

            for package in package_list:
                # Убираем версию если есть
                pkg_name = package.split('==')[0].split('>=')[0].split('<=')[0].lower()
                if pkg_name not in installed:
                    missing.append(pkg_name)

            if missing:
                return VerificationResult(
                    False,
                    f"Пакеты не установлены: {', '.join(missing)}",
                    "Запустите установку повторно"
                )

            return VerificationResult(
                True,
                f"✓ Все пакеты установлены: {', '.join(package_list)}"
            )

        except Exception as e:
            return VerificationResult(
                False,
                "Ошибка при проверке пакетов",
                str(e)
            )

    def verify_docker_container(self, container_name: Optional[str] = None) -> VerificationResult:
        """
        Проверить что Docker контейнер запущен.

        Args:
            container_name: Имя контейнера (опционально)

        Returns:
            VerificationResult
        """
        try:
            # Проверяем docker ps
            result = subprocess.run(
                ["docker", "ps", "--format", "{{.Names}}"],
                capture_output=True,
                text=True,
                timeout=10
            )

            if result.returncode != 0:
                return VerificationResult(
                    False,
                    "Docker не доступен или нет запущенных контейнеров",
                    result.stderr
                )

            running_containers = result.stdout.strip().split('\n')

            if container_name:
                if container_name in running_containers:
                    return VerificationResult(
                        True,
                        f"✓ Контейнер '{container_name}' запущен"
                    )
                else:
                    return VerificationResult(
                        False,
                        f"Контейнер '{container_name}' не найден среди запущенных",
                        f"Запущенные: {', '.join(running_containers)}"
                    )
            else:
                count = len([c for c in running_containers if c])
                return VerificationResult(
                    True,
                    f"✓ Запущено контейнеров: {count}"
                )

        except FileNotFoundError:
            return VerificationResult(
                False,
                "Docker не установлен",
                "Установите Docker: https://docs.docker.com/get-docker/"
            )
        except Exception as e:
            return VerificationResult(
                False,
                "Ошибка при проверке Docker",
                str(e)
            )

    def verify_api_health(self, url: str, timeout: int = 5) -> VerificationResult:
        """
        Проверить что API отвечает.

        Args:
            url: URL для проверки (обычно /health endpoint)
            timeout: Таймаут в секундах

        Returns:
            VerificationResult
        """
        try:
            import requests

            response = requests.get(url, timeout=timeout)

            if response.status_code == 200:
                return VerificationResult(
                    True,
                    f"✓ API доступен: {url}",
                    f"Статус: {response.status_code}"
                )
            else:
                return VerificationResult(
                    False,
                    f"API вернул ошибку: {response.status_code}",
                    response.text[:200]
                )

        except ImportError:
            return VerificationResult(
                False,
                "Модуль requests не установлен",
                "Установите: pip install requests"
            )
        except Exception as e:
            return VerificationResult(
                False,
                f"API недоступен: {url}",
                str(e)
            )

    def verify_file_exists(self, file_path: str) -> VerificationResult:
        """
        Проверить что файл существует.

        Args:
            file_path: Путь к файлу

        Returns:
            VerificationResult
        """
        from pathlib import Path

        path = Path(file_path)

        if path.exists():
            if path.is_file():
                size = path.stat().st_size
                return VerificationResult(
                    True,
                    f"✓ Файл существует: {file_path}",
                    f"Размер: {size} байт"
                )
            else:
                return VerificationResult(
                    False,
                    f"Путь существует, но это не файл: {file_path}",
                    "Это директория"
                )
        else:
            return VerificationResult(
                False,
                f"Файл не найден: {file_path}",
                "Проверьте путь"
            )

    def verify_command_available(self, command: str) -> VerificationResult:
        """
        Проверить что команда доступна в системе.

        Args:
            command: Имя команды

        Returns:
            VerificationResult
        """
        import shutil

        path = shutil.which(command)

        if path:
            return VerificationResult(
                True,
                f"✓ Команда доступна: {command}",
                f"Путь: {path}"
            )
        else:
            return VerificationResult(
                False,
                f"Команда не найдена: {command}",
                "Установите необходимое ПО"
            )


# Глобальный экземпляр
result_verifier = ResultVerifier()
