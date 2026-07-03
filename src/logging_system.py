"""
Logging System - структурированная система логирования.
Уровни: DEBUG, INFO, WARN, ERROR, CRITICAL
Ротация файлов, форматирование, фильтрация.
"""
from __future__ import annotations

import json
import logging
import sys
from datetime import datetime
from logging.handlers import RotatingFileHandler
from pathlib import Path
from typing import Any


class ClaudeLogger:
    """Централизованная система логирования для Claude Code."""

    _instance = None
    _initialized = False

    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __init__(self):
        if self._initialized:
            return
        
        self._initialized = True
        self.log_dir = Path.home() / ".claude_code" / "logs"
        self.log_dir.mkdir(parents=True, exist_ok=True)
        
        # Создаём логгеры
        self.logger = logging.getLogger("claude_code")
        self.logger.setLevel(logging.DEBUG)
        
        # Очищаем существующие handlers
        self.logger.handlers.clear()
        
        # Добавляем handlers
        self._setup_handlers()

    def _setup_handlers(self):
        """Настроить handlers для логирования."""
        
        # 1. File handler - все логи
        all_logs = self.log_dir / "claude_code.log"
        file_handler = RotatingFileHandler(
            all_logs,
            maxBytes=10 * 1024 * 1024,  # 10MB
            backupCount=5,
            encoding='utf-8'
        )
        file_handler.setLevel(logging.DEBUG)
        file_handler.setFormatter(self._get_formatter())
        self.logger.addHandler(file_handler)
        
        # 2. Error file handler - только ошибки
        error_logs = self.log_dir / "errors.log"
        error_handler = RotatingFileHandler(
            error_logs,
            maxBytes=10 * 1024 * 1024,  # 10MB
            backupCount=5,
            encoding='utf-8'
        )
        error_handler.setLevel(logging.ERROR)
        error_handler.setFormatter(self._get_formatter())
        self.logger.addHandler(error_handler)
        
        # 3. Console handler - только WARNING и выше
        console_handler = logging.StreamHandler(sys.stderr)
        console_handler.setLevel(logging.WARNING)
        console_handler.setFormatter(self._get_console_formatter())
        self.logger.addHandler(console_handler)

    def _get_formatter(self) -> logging.Formatter:
        """Получить форматтер для файлов."""
        return logging.Formatter(
            fmt='%(asctime)s | %(levelname)-8s | %(name)s | %(message)s',
            datefmt='%Y-%m-%d %H:%M:%S'
        )

    def _get_console_formatter(self) -> logging.Formatter:
        """Получить форматтер для консоли."""
        return logging.Formatter(
            fmt='%(levelname)s: %(message)s'
        )

    def debug(self, message: str, **kwargs):
        """Логировать DEBUG сообщение."""
        extra_info = self._format_extra(kwargs)
        self.logger.debug(f"{message} {extra_info}".strip())

    def info(self, message: str, **kwargs):
        """Логировать INFO сообщение."""
        extra_info = self._format_extra(kwargs)
        self.logger.info(f"{message} {extra_info}".strip())

    def warning(self, message: str, **kwargs):
        """Логировать WARNING сообщение."""
        extra_info = self._format_extra(kwargs)
        self.logger.warning(f"{message} {extra_info}".strip())

    def error(self, message: str, exception: Exception | None = None, **kwargs):
        """Логировать ERROR сообщение."""
        extra_info = self._format_extra(kwargs)
        
        if exception:
            self.logger.error(
                f"{message} {extra_info}".strip(),
                exc_info=exception
            )
        else:
            self.logger.error(f"{message} {extra_info}".strip())

    def critical(self, message: str, exception: Exception | None = None, **kwargs):
        """Логировать CRITICAL сообщение."""
        extra_info = self._format_extra(kwargs)
        
        if exception:
            self.logger.critical(
                f"{message} {extra_info}".strip(),
                exc_info=exception
            )
        else:
            self.logger.critical(f"{message} {extra_info}".strip())

    def _format_extra(self, kwargs: dict[str, Any]) -> str:
        """Форматировать дополнительную информацию."""
        if not kwargs:
            return ""
        
        parts = []
        for key, value in kwargs.items():
            if isinstance(value, (dict, list)):
                value = json.dumps(value)
            parts.append(f"{key}={value}")
        
        return f"[{', '.join(parts)}]"

    def log_tool_execution(
        self,
        tool_name: str,
        success: bool,
        duration_ms: float | None = None,
        error: str | None = None
    ):
        """Логировать выполнение инструмента."""
        if success:
            self.info(
                f"Tool executed: {tool_name}",
                duration_ms=duration_ms
            )
        else:
            self.error(
                f"Tool failed: {tool_name}",
                error=error,
                duration_ms=duration_ms
            )

    def log_llm_request(
        self,
        provider: str,
        model: str,
        tokens: int | None = None,
        duration_ms: float | None = None
    ):
        """Логировать запрос к LLM."""
        self.info(
            f"LLM request: {provider}/{model}",
            tokens=tokens,
            duration_ms=duration_ms
        )

    def log_llm_error(
        self,
        provider: str,
        model: str,
        error: str,
        exception: Exception | None = None
    ):
        """Логировать ошибку LLM."""
        self.error(
            f"LLM error: {provider}/{model} - {error}",
            exception=exception
        )

    def log_session_event(self, event: str, session_id: str | None = None):
        """Логировать событие сессии."""
        self.info(f"Session event: {event}", session_id=session_id)

    def get_log_file_path(self, log_type: str = "all") -> Path:
        """Получить путь к файлу логов."""
        if log_type == "errors":
            return self.log_dir / "errors.log"
        else:
            return self.log_dir / "claude_code.log"


# Глобальный экземпляр логгера
_logger_instance = None


def get_logger() -> ClaudeLogger:
    """Получить глобальный экземпляр логгера."""
    global _logger_instance
    if _logger_instance is None:
        _logger_instance = ClaudeLogger()
    return _logger_instance


# Удобные функции для быстрого доступа
def debug(message: str, **kwargs):
    """Логировать DEBUG."""
    get_logger().debug(message, **kwargs)


def info(message: str, **kwargs):
    """Логировать INFO."""
    get_logger().info(message, **kwargs)


def warning(message: str, **kwargs):
    """Логировать WARNING."""
    get_logger().warning(message, **kwargs)


def error(message: str, exception: Exception | None = None, **kwargs):
    """Логировать ERROR."""
    get_logger().error(message, exception=exception, **kwargs)


def critical(message: str, exception: Exception | None = None, **kwargs):
    """Логировать CRITICAL."""
    get_logger().critical(message, exception=exception, **kwargs)
