"""
Smart Context Compression - умная компрессия контекста диалога.

Улучшенная версия компрессии с:
- Сохранением важных сообщений (tool results, ошибки)
- Суммаризацией старых сообщений через LLM
- Приоритизацией по важности
- Адаптивными порогами компрессии
"""
from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Any

from .context_manager import Message, Session
from .llm_client import LLMClient, LLMMessage


class MessagePriority(Enum):
    """Приоритет сообщения."""
    CRITICAL = 3  # Никогда не удалять (системные, последние N)
    HIGH = 2      # Сохранять при компрессии (tool results, ошибки)
    NORMAL = 1    # Можно суммаризировать
    LOW = 0       # Можно удалить


@dataclass
class MessageMetadata:
    """Метаданные сообщения для компрессии."""
    priority: MessagePriority
    tokens: int
    has_tool_result: bool = False
    has_error: bool = False
    is_recent: bool = False


class SmartContextCompressor:
    """Умный компрессор контекста."""

    def __init__(
        self,
        llm_client: LLMClient,
        max_messages: int = 50,
        keep_recent: int = 10,
        target_compression_ratio: float = 0.5,
    ):
        """
        Инициализация компрессора.

        Args:
            llm_client: LLM клиент для суммаризации
            max_messages: Максимальное количество сообщений
            keep_recent: Сколько последних сообщений всегда сохранять
            target_compression_ratio: Целевой коэффициент сжатия (0.5 = 50%)
        """
        self.llm_client = llm_client
        self.max_messages = max_messages
        self.keep_recent = keep_recent
        self.target_compression_ratio = target_compression_ratio

    def _analyze_message(self, message: Message, index: int, total: int) -> MessageMetadata:
        """
        Проанализировать сообщение и определить его приоритет.

        Args:
            message: Сообщение
            index: Индекс в списке
            total: Общее количество сообщений

        Returns:
            Метаданные сообщения
        """
        # Определяем, является ли сообщение недавним
        is_recent = index >= total - self.keep_recent

        # Проверяем на tool results и ошибки
        has_tool_result = "```" in message.content or "✅" in message.content
        has_error = "error" in message.content.lower() or "❌" in message.content

        # Определяем приоритет
        if is_recent:
            priority = MessagePriority.CRITICAL
        elif has_error:
            priority = MessagePriority.HIGH
        elif has_tool_result:
            priority = MessagePriority.HIGH
        elif message.role == "system":
            priority = MessagePriority.CRITICAL
        else:
            priority = MessagePriority.NORMAL

        return MessageMetadata(
            priority=priority,
            tokens=message.tokens,
            has_tool_result=has_tool_result,
            has_error=has_error,
            is_recent=is_recent,
        )

    def _summarize_messages(self, messages: list[Message]) -> str:
        """
        Суммаризировать группу сообщений через LLM.

        Args:
            messages: Сообщения для суммаризации

        Returns:
            Суммаризированный текст
        """
        # Формируем промпт для суммаризации
        conversation = "\n\n".join([
            f"{msg.role}: {msg.content[:500]}"  # Ограничиваем длину
            for msg in messages
        ])

        prompt = f"""Summarize the following conversation concisely, preserving key information:

{conversation}

Provide a brief summary (2-3 sentences) that captures the main points and any important decisions or results."""

        try:
            response = self.llm_client.complete(
                messages=[LLMMessage(role="user", content=prompt)],
                system="You are a helpful assistant that summarizes conversations concisely.",
            )
            return response.content
        except Exception as e:
            # Fallback: простая конкатенация
            return f"[Summary of {len(messages)} messages]"

    def compress(self, session: Session) -> tuple[Session, dict[str, Any]]:
        """
        Сжать контекст сессии.

        Args:
            session: Сессия для сжатия

        Returns:
            (compressed_session, stats) - сжатая сессия и статистика
        """
        messages = session.messages
        total = len(messages)

        if total <= self.max_messages:
            return session, {"compressed": False, "reason": "Below threshold"}

        # Анализируем все сообщения
        metadata = [
            self._analyze_message(msg, i, total)
            for i, msg in enumerate(messages)
        ]

        # Разделяем на группы по приоритету
        critical = []
        high = []
        normal = []
        low = []

        for i, (msg, meta) in enumerate(zip(messages, metadata)):
            if meta.priority == MessagePriority.CRITICAL:
                critical.append((i, msg, meta))
            elif meta.priority == MessagePriority.HIGH:
                high.append((i, msg, meta))
            elif meta.priority == MessagePriority.NORMAL:
                normal.append((i, msg, meta))
            else:
                low.append((i, msg, meta))

        # Стратегия компрессии:
        # 1. Всегда сохраняем CRITICAL
        # 2. Сохраняем HIGH
        # 3. Суммаризируем NORMAL в группы
        # 4. Удаляем LOW

        new_messages = []

        # Добавляем критические (последние N)
        for i, msg, meta in critical:
            new_messages.append((i, msg))

        # Добавляем важные (tool results, ошибки)
        for i, msg, meta in high:
            if i not in [idx for idx, _ in new_messages]:
                new_messages.append((i, msg))

        # Суммаризируем обычные сообщения
        if normal:
            # Группируем по 5 сообщений
            normal_msgs = [msg for _, msg, _ in normal]
            chunk_size = 5
            for i in range(0, len(normal_msgs), chunk_size):
                chunk = normal_msgs[i:i + chunk_size]
                if len(chunk) > 1:
                    summary = self._summarize_messages(chunk)
                    summary_msg = Message(
                        role="system",
                        content=f"[Compressed summary]: {summary}",
                        tokens=len(summary.split()),
                    )
                    new_messages.append((normal[i][0], summary_msg))
                else:
                    new_messages.append((normal[i][0], chunk[0]))

        # Сортируем по исходному порядку
        new_messages.sort(key=lambda x: x[0])

        # Создаем новую сессию
        compressed_session = Session(
            session_id=session.session_id,
            llm_config=session.llm_config,
            compact_threshold=session.compact_threshold,
        )
        compressed_session.messages = [msg for _, msg in new_messages]

        # Статистика
        stats = {
            "compressed": True,
            "original_count": total,
            "compressed_count": len(compressed_session.messages),
            "removed_count": total - len(compressed_session.messages),
            "compression_ratio": len(compressed_session.messages) / total,
            "critical_kept": len(critical),
            "high_kept": len(high),
            "normal_summarized": len(normal),
            "low_removed": len(low),
        }

        return compressed_session, stats


def create_smart_compressor(
    llm_client: LLMClient,
    max_messages: int = 50,
    keep_recent: int = 10,
) -> SmartContextCompressor:
    """
    Создать умный компрессор контекста.

    Args:
        llm_client: LLM клиент
        max_messages: Максимальное количество сообщений
        keep_recent: Сколько последних сообщений сохранять

    Returns:
        Компрессор
    """
    return SmartContextCompressor(
        llm_client=llm_client,
        max_messages=max_messages,
        keep_recent=keep_recent,
    )
