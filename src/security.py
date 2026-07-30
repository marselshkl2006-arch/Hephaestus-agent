"""
Простая и понятная система безопасности для Claude Code.

Принципы:
1. Агент может работать везде (нет workspace boundary)
2. Предупреждаем о системных путях и деструктивных командах
3. Всегда спрашиваем подтверждение перед действием
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from enum import Enum
from pathlib import Path


class ActionRisk(Enum):
    """Уровень риска действия."""
    SAFE = "safe"           # Безопасные действия (чтение, ls)
    LOW = "low"             # Низкий риск (создание файлов)
    MEDIUM = "medium"       # Средний риск (редактирование, перемещение)
    HIGH = "high"           # Высокий риск (удаление, системные пути)
    CRITICAL = "critical"   # Критический риск (rm -rf, форматирование)


@dataclass
class SecurityCheck:
    """Результат проверки безопасности."""
    allowed: bool
    risk: ActionRisk
    warning: str | None = None
    reason: str | None = None


class SecurityValidator:
    """Валидатор безопасности действий."""

    # Системные пути (высокий риск)
    SYSTEM_PATHS = [
        "/etc/", "/sys/", "/proc/", "/dev/", "/boot/",
        "/bin/", "/sbin/", "/usr/bin/", "/usr/sbin/"
    ]

    # Деструктивные команды (критический риск)
    DESTRUCTIVE_COMMANDS = [
        r"\brm\s+-rf\b",
        r"\bdd\b.*if=",
        r"\bmkfs\b",
        r"\bformat\b",
        r"\bfdisk\b",
        r"\bparted\b",
        r"\b:\(\)\{.*\};\s*:\b",  # Fork bomb
    ]

    # Опасные команды (высокий риск)
    DANGEROUS_COMMANDS = [
        r"\brm\b",
        r"\bmv\b.*\s+/",
        r"\bchmod\b.*777",
        r"\bchown\b.*root",
        r"\bsudo\b",
        r"\bsu\b",
    ]

    # Катастрофические команды — БЕЗУСЛОВНЫЙ блок, без вариантов.
    # В отличие от DESTRUCTIVE_COMMANDS выше (которые только предупреждают,
    # allowed=True), эти паттерны нарочно узкие и целятся именно в
    # необратимое уничтожение системы/диска целиком — а не в обычный
    # "rm -rf ./build", который агенту нужен постоянно и который блокировать
    # нельзя. Это последний рубеж на случай опечатки, галлюцинации модели
    # или неправильно распознанной голосовой команды — срабатывает всегда,
    # независимо от auto_approve и от того, есть ли рядом живой человек за
    # терминалом, чтобы нажать 'y'.
    HARD_BLOCK_PATTERNS = [
        (r"\brm\s+.*-[a-z]*r[a-z]*f[a-z]*\s+/(\s|$)", "rm -rf на корень файловой системы"),
        (r"\brm\s+.*-[a-z]*r[a-z]*f[a-z]*\s+/\*", "rm -rf на всё содержимое корня"),
        (r"\brm\s+.*-[a-z]*r[a-z]*f[a-z]*\s+(~|\$HOME)(\s|/|$)", "rm -rf на домашнюю папку целиком"),
        (r"--no-preserve-root", "обход защиты rm от удаления корня"),
        (r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:", "fork bomb"),
        (r"\bdd\b[^\n|]*\bof=/dev/(sd|nvme|hd|vd|xvd)", "прямая запись поверх диска целиком"),
        (r"\bmkfs(\.\w+)?\b", "форматирование раздела/диска"),
        (r"\bwipefs\b", "стирание разметки диска"),
        (r">\s*/dev/(sd|nvme|hd|vd|xvd)[a-z0-9]*(\s|$)", "запись напрямую в блочное устройство"),
        (r"\bchmod\s+-R\s+777\s+/(\s|$)", "рекурсивный chmod 777 на корень"),
    ]

    def check_catastrophic(self, command: str) -> str | None:
        """Проверить, не является ли команда безусловно недопустимой.

        Возвращает причину блокировки или None, если команда не входит
        в список необратимых системных катастроф (сама по себе НЕ
        заменяет обычную оценку риска ниже — это отдельный, более узкий
        и более жёсткий барьер).
        """
        for pattern, reason in self.HARD_BLOCK_PATTERNS:
            if re.search(pattern, command, re.IGNORECASE):
                return reason
        return None

    def check_bash_command(self, command: str) -> SecurityCheck:
        """
        Проверить bash команду.

        Args:
            command: Команда для проверки

        Returns:
            SecurityCheck с результатом
        """
        # Проверка на деструктивные команды
        for pattern in self.DESTRUCTIVE_COMMANDS:
            if re.search(pattern, command, re.IGNORECASE):
                return SecurityCheck(
                    allowed=True,  # Разрешаем, но с предупреждением
                    risk=ActionRisk.CRITICAL,
                    warning="⚠️  КРИТИЧЕСКАЯ ОПАСНОСТЬ: Деструктивная команда!",
                    reason=f"Команда может уничтожить данные: {command}"
                )

        # Проверка на опасные команды
        for pattern in self.DANGEROUS_COMMANDS:
            if re.search(pattern, command, re.IGNORECASE):
                return SecurityCheck(
                    allowed=True,
                    risk=ActionRisk.HIGH,
                    warning="⚠️  ВЫСОКИЙ РИСК: Опасная команда!",
                    reason=f"Команда может изменить систему: {command}"
                )

        # Безопасные команды для чтения
        safe_commands = ["ls", "cat", "head", "tail", "grep", "find", "pwd", "echo", "date"]
        if any(command.strip().startswith(cmd) for cmd in safe_commands):
            return SecurityCheck(
                allowed=True,
                risk=ActionRisk.SAFE,
            )

        # По умолчанию - низкий риск
        return SecurityCheck(
            allowed=True,
            risk=ActionRisk.LOW,
        )

    def check_file_path(self, file_path: str, action: str = "access") -> SecurityCheck:
        """
        Проверить путь к файлу.

        Args:
            file_path: Путь к файлу
            action: Тип действия (read, write, delete)

        Returns:
            SecurityCheck с результатом
        """
        # Проверка на системные пути
        for sys_path in self.SYSTEM_PATHS:
            if file_path.startswith(sys_path):
                return SecurityCheck(
                    allowed=True,
                    risk=ActionRisk.HIGH,
                    warning=f"⚠️  СИСТЕМНЫЙ ПУТЬ: {sys_path}",
                    reason=f"Изменение системных файлов может сломать систему"
                )

        # Проверка на чувствительные файлы
        sensitive_patterns = [
            r"/\.ssh/",
            r"/\.aws/",
            r"/\.env$",
            r"password",
            r"secret",
            r"token",
            r"credentials",
            r"id_rsa",
            r"id_ed25519",
        ]

        for pattern in sensitive_patterns:
            if re.search(pattern, file_path, re.IGNORECASE):
                return SecurityCheck(
                    allowed=True,
                    risk=ActionRisk.HIGH,
                    warning="⚠️  ЧУВСТВИТЕЛЬНЫЙ ФАЙЛ!",
                    reason=f"Файл может содержать секреты: {file_path}"
                )

        # Определяем риск по типу действия
        if action == "read":
            risk = ActionRisk.SAFE
        elif action == "write":
            risk = ActionRisk.LOW
        elif action == "delete":
            risk = ActionRisk.MEDIUM
        else:
            risk = ActionRisk.LOW

        return SecurityCheck(
            allowed=True,
            risk=risk,
        )

    def check_file_write(self, file_path: str, content: str) -> SecurityCheck:
        """Проверить запись файла."""
        return self.check_file_path(file_path, action="write")

    def check_file_read(self, file_path: str) -> SecurityCheck:
        """Проверить чтение файла."""
        return self.check_file_path(file_path, action="read")

    def check_file_delete(self, file_path: str) -> SecurityCheck:
        """Проверить удаление файла."""
        check = self.check_file_path(file_path, action="delete")

        # Удаление всегда как минимум средний риск
        if check.risk == ActionRisk.SAFE or check.risk == ActionRisk.LOW:
            check.risk = ActionRisk.MEDIUM
            check.warning = "⚠️  Удаление файла"

        return check


# Глобальный экземпляр валидатора
_validator = SecurityValidator()


def check_bash_command(command: str) -> SecurityCheck:
    """Проверить bash команду."""
    return _validator.check_bash_command(command)


def check_file_write(file_path: str, content: str = "") -> SecurityCheck:
    """Проверить запись файла."""
    return _validator.check_file_write(file_path, content)


def check_file_read(file_path: str) -> SecurityCheck:
    """Проверить чтение файла."""
    return _validator.check_file_read(file_path)


def check_file_delete(file_path: str) -> SecurityCheck:
    """Проверить удаление файла."""
    return _validator.check_file_delete(file_path)
