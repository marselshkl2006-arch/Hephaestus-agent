"""
Улучшенное управление контекстом и сессиями с поддержкой сжатия и системных промптов.
"""
from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

from .llm_client import LLMClient, LLMConfig, LLMMessage, create_llm_client


@dataclass
class Message:
    """Сообщение в диалоге."""
    role: str  # "user", "assistant", "system"
    content: str
    timestamp: str = field(default_factory=lambda: datetime.now().isoformat())
    tokens: int = 0


@dataclass
class SessionMetadata:
    """Метаданные сессии."""
    session_id: str
    created_at: str
    updated_at: str
    total_messages: int
    total_tokens: int
    model: str
    provider: str


@dataclass
class Session:
    """Сессия диалога с управлением контекстом."""
    session_id: str
    messages: list[Message] = field(default_factory=list)
    metadata: SessionMetadata | None = None
    max_context_tokens: int = 100000
    compact_threshold: int = 50  # Сжимать после N сообщений

    def add_message(self, role: str, content: str | list, tokens: int = 0) -> None:
        """Добавить сообщение в сессию."""
        # Если content - список, оставляем как есть (для tool_results)
        # Если строка - считаем токены
        if isinstance(content, str):
            estimated_tokens = tokens or len(content.split())
        else:
            # Для списков (tool_results) примерная оценка
            estimated_tokens = tokens or 100

        message = Message(
            role=role,
            content=content,
            tokens=estimated_tokens,
        )
        self.messages.append(message)

        # Автоматическое сжатие при превышении порога
        if len(self.messages) > self.compact_threshold:
            self._auto_compact()

    def get_messages(self, limit: int | None = None) -> list[Message]:
        """Получить сообщения (последние N)."""
        if limit:
            return self.messages[-limit:]
        return self.messages

    def get_total_tokens(self) -> int:
        """Получить общее количество токенов."""
        return sum(msg.tokens for msg in self.messages)

    def _auto_compact(self, use_smart_compression: bool = True) -> None:
        """
        Автоматически сжать контекст при превышении порога.

        Args:
            use_smart_compression: Использовать умную компрессию (требует LLM)
        """
        total_tokens = self.get_total_tokens()

        if total_tokens > self.max_context_tokens or len(self.messages) > self.compact_threshold:
            if use_smart_compression:
                # Используем умную компрессию (будет реализовано в агенте)
                # Здесь просто помечаем что нужна компрессия
                pass
            else:
                # Простая компрессия - оставляем только последние сообщения
                keep_count = self.compact_threshold // 2
                self.messages = self.messages[-keep_count:]

    def compact_with_summary(self, llm_client: LLMClient) -> None:
        """
        Сжать контекст через суммаризацию старых сообщений.
        Оставляет последние N сообщений, а старые заменяет на краткое резюме.
        """
        if len(self.messages) <= self.compact_threshold:
            return

        # Разделяем на старые и новые сообщения
        keep_count = self.compact_threshold // 2
        old_messages = self.messages[:-keep_count]
        new_messages = self.messages[-keep_count:]

        # Создаём резюме старых сообщений
        summary_prompt = self._create_summary_prompt(old_messages)

        try:
            summary_response = llm_client.complete(
                messages=[LLMMessage(role="user", content=summary_prompt)],
                system="You are a helpful assistant that summarizes conversations concisely.",
            )

            # Заменяем старые сообщения на резюме
            summary_message = Message(
                role="system",
                content=f"[Previous conversation summary]\n{summary_response.content}",
                tokens=summary_response.usage.get("completion_tokens", 0),
            )

            self.messages = [summary_message] + new_messages

        except Exception as e:
            # Если суммаризация не удалась, просто обрезаем
            print(f"Warning: Failed to summarize context: {e}")
            self.messages = new_messages

    def _create_summary_prompt(self, messages: list[Message]) -> str:
        """Создать промпт для суммаризации."""
        conversation = "\n\n".join(
            f"{msg.role.upper()}: {msg.content}"
            for msg in messages
        )

        return f"""Summarize the following conversation in 3-5 bullet points, focusing on:
- Key decisions made
- Important context established
- Current state of the task

Conversation:
{conversation}

Summary:"""

    def to_dict(self) -> dict[str, Any]:
        """Сериализовать сессию в словарь."""
        return {
            "session_id": self.session_id,
            "messages": [asdict(msg) for msg in self.messages],
            "metadata": asdict(self.metadata) if self.metadata else None,
            "max_context_tokens": self.max_context_tokens,
            "compact_threshold": self.compact_threshold,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> Session:
        """Десериализовать сессию из словаря."""
        messages = [Message(**msg) for msg in data.get("messages", [])]
        metadata_data = data.get("metadata")
        metadata = SessionMetadata(**metadata_data) if metadata_data else None

        return cls(
            session_id=data["session_id"],
            messages=messages,
            metadata=metadata,
            max_context_tokens=data.get("max_context_tokens", 100000),
            compact_threshold=data.get("compact_threshold", 50),
        )


class SessionStore:
    """Хранилище сессий с поддержкой сохранения/загрузки."""

    def __init__(self, storage_dir: Path | None = None):
        self.storage_dir = storage_dir or Path.home() / ".claude_code" / "sessions"
        self.storage_dir.mkdir(parents=True, exist_ok=True)

    def save(self, session: Session) -> Path:
        """Сохранить сессию."""
        # Обновляем метаданные
        if session.metadata:
            session.metadata = SessionMetadata(
                session_id=session.session_id,
                created_at=session.metadata.created_at,
                updated_at=datetime.now().isoformat(),
                total_messages=len(session.messages),
                total_tokens=session.get_total_tokens(),
                model=session.metadata.model,
                provider=session.metadata.provider,
            )

        path = self.storage_dir / f"{session.session_id}.json"
        path.write_text(json.dumps(session.to_dict(), indent=2))
        return path

    def load(self, session_id: str) -> Session:
        """Загрузить сессию."""
        path = self.storage_dir / f"{session_id}.json"

        if not path.exists():
            raise FileNotFoundError(f"Session not found: {session_id}")

        data = json.loads(path.read_text())
        return Session.from_dict(data)

    def list_sessions(self) -> list[SessionMetadata]:
        """Получить список всех сессий."""
        sessions = []

        for path in self.storage_dir.glob("*.json"):
            try:
                data = json.loads(path.read_text())
                if data.get("metadata"):
                    sessions.append(SessionMetadata(**data["metadata"]))
            except Exception:
                continue

        return sorted(sessions, key=lambda s: s.updated_at, reverse=True)

    def delete(self, session_id: str) -> bool:
        """Удалить сессию."""
        path = self.storage_dir / f"{session_id}.json"

        if path.exists():
            path.unlink()
            return True

        return False


class SystemPromptBuilder:
    """Построитель динамических системных промптов."""

    def __init__(self):
        self.base_prompt = """You are Claude Code, an AI coding assistant.

Your capabilities:
- Read and write files
- Execute shell commands
- Search code with grep and glob
- Work with git repositories
- Explain and debug code

Guidelines:
- Always explain your reasoning
- Ask for clarification when needed
- Be concise but thorough
- Follow security best practices
- Respect file permissions and workspace boundaries
"""

    def build(
        self,
        task_context: str | None = None,
        workspace_info: dict[str, Any] | None = None,
        security_rules: list[str] | None = None,
    ) -> str:
        """
        Построить системный промпт с учётом контекста.

        Args:
            task_context: Контекст текущей задачи
            workspace_info: Информация о workspace (язык, фреймворк и т.д.)
            security_rules: Дополнительные правила безопасности
        """
        prompt_parts = [self.base_prompt]

        if workspace_info:
            prompt_parts.append("\nWorkspace context:")
            for key, value in workspace_info.items():
                prompt_parts.append(f"- {key}: {value}")

        if task_context:
            prompt_parts.append(f"\nCurrent task: {task_context}")

        if security_rules:
            prompt_parts.append("\nSecurity rules:")
            for rule in security_rules:
                prompt_parts.append(f"- {rule}")

        return "\n".join(prompt_parts)


def create_session(
    session_id: str,
    llm_config: LLMConfig,
    max_context_tokens: int = 100000,
) -> Session:
    """Создать новую сессию."""
    metadata = SessionMetadata(
        session_id=session_id,
        created_at=datetime.now().isoformat(),
        updated_at=datetime.now().isoformat(),
        total_messages=0,
        total_tokens=0,
        model=llm_config.model,
        provider=llm_config.provider.value,
    )

    return Session(
        session_id=session_id,
        metadata=metadata,
        max_context_tokens=max_context_tokens,
    )
