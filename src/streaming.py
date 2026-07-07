"""
Streaming Response - потоковая передача ответов от LLM.
Улучшает UX показывая ответ по мере генерации.
"""
from __future__ import annotations

import sys
import time
from typing import Iterator, Callable

from .llm_client import LLMClient, LLMMessage, LLMResponse


class StreamingHandler:
    """Обработчик потоковых ответов."""

    def __init__(self, on_token: Callable[[str], None] | None = None):
        """
        Args:
            on_token: Callback для каждого токена
        """
        self.on_token = on_token or self._default_token_handler
        self.buffer = ""
        self.total_tokens = 0

    def _default_token_handler(self, token: str) -> None:
        """Обработчик по умолчанию - вывод в stdout."""
        print(token, end="", flush=True)

    def handle_token(self, token: str) -> None:
        """
        Обработать токен.

        Args:
            token: Токен от LLM
        """
        self.buffer += token
        self.total_tokens += 1
        self.on_token(token)

    def get_full_response(self) -> str:
        """Получить полный ответ."""
        return self.buffer

    def reset(self) -> None:
        """Сбросить буфер."""
        self.buffer = ""
        self.total_tokens = 0


class StreamingLLMClient:
    """Обертка для LLM клиента с поддержкой streaming."""

    def __init__(self, llm_client: LLMClient):
        self.llm_client = llm_client

    def complete_streaming(
        self,
        messages: list[LLMMessage],
        system: str | None = None,
        on_token: Callable[[str], None] | None = None
    ) -> LLMResponse:
        """
        Получить ответ с потоковой передачей.

        Args:
            messages: История сообщений
            system: Системный промпт
            on_token: Callback для каждого токена

        Returns:
            LLMResponse с полным ответом
        """
        handler = StreamingHandler(on_token)

        # Проверяем поддерживает ли провайдер streaming
        if hasattr(self.llm_client, 'supports_streaming') and self.llm_client.supports_streaming():
            # Используем нативный streaming
            try:
                for token in self._stream_native(messages, system):
                    handler.handle_token(token)
            except Exception as e:
                # Fallback на обычный режим
                print(f"\n⚠️  Streaming failed, falling back to normal mode: {e}")
                response = self.llm_client.complete(messages, system)
                handler.buffer = response.content
                return response
        else:
            # Эмулируем streaming
            response = self.llm_client.complete(messages, system)
            self._emulate_streaming(response.content, handler)

        # Формируем финальный ответ
        return LLMResponse(
            content=handler.get_full_response(),
            model=self.llm_client.config.model,
            usage={
                "completion_tokens": handler.total_tokens,
                "prompt_tokens": 0,
                "total_tokens": handler.total_tokens
            },
            tool_use_blocks=[]
        )

    def _stream_native(
        self,
        messages: list[LLMMessage],
        system: str | None = None
    ) -> Iterator[str]:
        """
        Нативный streaming от провайдера.

        Args:
            messages: История сообщений
            system: Системный промпт

        Yields:
            Токены
        """
        # Это заглушка - реальная реализация зависит от провайдера
        # Для Ollama, OpenAI, Anthropic нужны разные реализации

        provider = self.llm_client.config.provider.value

        if provider == "ollama":
            yield from self._stream_ollama(messages, system)
        elif provider == "openai":
            yield from self._stream_openai(messages, system)
        elif provider == "anthropic":
            yield from self._stream_anthropic(messages, system)
        else:
            # Fallback - эмуляция
            response = self.llm_client.complete(messages, system)
            yield from self._chunk_text(response.content)

    def _stream_ollama(
        self,
        messages: list[LLMMessage],
        system: str | None = None
    ) -> Iterator[str]:
        """Streaming для Ollama."""
        import requests

        url = f"{self.llm_client.config.base_url}/api/chat"

        payload = {
            "model": self.llm_client.config.model,
            "messages": [{"role": m.role, "content": m.content} for m in messages],
            "stream": True
        }

        if system:
            payload["system"] = system

        try:
            response = requests.post(url, json=payload, stream=True, timeout=120)
            response.raise_for_status()

            for line in response.iter_lines():
                if line:
                    import json
                    data = json.loads(line)
                    if "message" in data and "content" in data["message"]:
                        yield data["message"]["content"]

        except Exception as e:
            print(f"Streaming error: {e}")
            return

    def _stream_openai(
        self,
        messages: list[LLMMessage],
        system: str | None = None
    ) -> Iterator[str]:
        """Streaming для OpenAI."""
        # Заглушка - нужна реализация с openai SDK
        response = self.llm_client.complete(messages, system)
        yield from self._chunk_text(response.content)

    def _stream_anthropic(
        self,
        messages: list[LLMMessage],
        system: str | None = None
    ) -> Iterator[str]:
        """Streaming для Anthropic."""
        # Заглушка - нужна реализация с anthropic SDK
        response = self.llm_client.complete(messages, system)
        yield from self._chunk_text(response.content)

    def _emulate_streaming(self, text: str, handler: StreamingHandler) -> None:
        """
        Эмулировать streaming для провайдеров без поддержки.

        Args:
            text: Полный текст
            handler: Обработчик токенов
        """
        # Разбиваем на слова и выводим с задержкой
        words = text.split()
        for i, word in enumerate(words):
            token = word + (" " if i < len(words) - 1 else "")
            handler.handle_token(token)
            time.sleep(0.02)  # 20ms задержка между словами

    def _chunk_text(self, text: str, chunk_size: int = 5) -> Iterator[str]:
        """
        Разбить текст на чанки для эмуляции streaming.

        Args:
            text: Текст
            chunk_size: Размер чанка в словах

        Yields:
            Чанки текста
        """
        words = text.split()
        for i in range(0, len(words), chunk_size):
            chunk = " ".join(words[i:i + chunk_size])
            if i + chunk_size < len(words):
                chunk += " "
            yield chunk
            time.sleep(0.05)  # Небольшая задержка


class StreamingREPL:
    """REPL с поддержкой streaming ответов."""

    def __init__(self, streaming_client: StreamingLLMClient):
        self.streaming_client = streaming_client
        self.messages: list[LLMMessage] = []

    def process_message(self, user_message: str, system: str | None = None) -> str:
        """
        Обработать сообщение с streaming.

        Args:
            user_message: Сообщение пользователя
            system: Системный промпт

        Returns:
            Полный ответ
        """
        # Добавляем сообщение пользователя
        self.messages.append(LLMMessage(role="user", content=user_message))

        print("\n🤖 Assistant: ", end="", flush=True)

        # Получаем ответ с streaming
        response = self.streaming_client.complete_streaming(
            messages=self.messages,
            system=system
        )

        print("\n")  # Новая строка после ответа

        # Добавляем ответ в историю
        self.messages.append(LLMMessage(role="assistant", content=response.content))

        return response.content


# Пример использования
if __name__ == "__main__":
    from .llm_client import LLMConfig, LLMProvider, create_llm_client

    # Создаем клиента
    config = LLMConfig(
        provider=LLMProvider.OLLAMA,
        model="llama3.2:1b",
        api_key=""
    )

    llm_client = create_llm_client(config)
    streaming_client = StreamingLLMClient(llm_client)

    # Тестируем streaming
    messages = [
        LLMMessage(role="user", content="Tell me a short story about a robot")
    ]

    print("Testing streaming response:\n")
    response = streaming_client.complete_streaming(messages)
    print(f"\n\nTotal tokens: {response.usage.get('completion_tokens', 0)}")
