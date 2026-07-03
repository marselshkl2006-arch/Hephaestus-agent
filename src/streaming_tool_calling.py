"""
Streaming Tool Calling - потоковое выполнение инструментов.

Улучшенная версия tool calling с поддержкой streaming:
- Парсинг команд в реальном времени из потока LLM
- Немедленное выполнение найденных команд
- Параллельный вывод ответа и выполнение
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Iterator

from .tool_calling import ToolCall


@dataclass
class StreamingToolCall:
    """Вызов инструмента, найденный в потоке."""
    tool_name: str
    command: str
    start_pos: int
    end_pos: int
    executed: bool = False


class StreamingToolCallParser:
    """Парсер для извлечения команд из потока в реальном времени."""

    def __init__(self):
        """Инициализация парсера."""
        self.buffer = ""
        self.found_calls: list[StreamingToolCall] = []

        # Паттерны для распознавания начала блока кода
        self.code_block_start = re.compile(r'```(bash|sh)\s*\n')
        self.code_block_end = re.compile(r'\n```')

        # Состояние парсера
        self.in_code_block = False
        self.code_block_start_pos = -1
        self.code_block_content = ""

    def feed(self, chunk: str) -> list[StreamingToolCall]:
        """
        Добавить chunk в буфер и попытаться распарсить команды.

        Args:
            chunk: Новый кусок текста из потока

        Returns:
            Список новых найденных команд
        """
        self.buffer += chunk
        new_calls = []

        # Ищем блоки кода
        if not self.in_code_block:
            # Ищем начало блока
            match = self.code_block_start.search(self.buffer)
            if match:
                self.in_code_block = True
                self.code_block_start_pos = match.end()
                self.code_block_content = ""

        if self.in_code_block:
            # Ищем конец блока
            search_start = max(0, len(self.buffer) - len(chunk) - 10)
            match = self.code_block_end.search(self.buffer, search_start)

            if match:
                # Нашли конец блока
                end_pos = match.start()
                command = self.buffer[self.code_block_start_pos:end_pos].strip()

                if command:
                    # Создаем tool call
                    tool_call = StreamingToolCall(
                        tool_name="bash",
                        command=command,
                        start_pos=self.code_block_start_pos,
                        end_pos=end_pos,
                    )
                    self.found_calls.append(tool_call)
                    new_calls.append(tool_call)

                # Сбрасываем состояние
                self.in_code_block = False
                self.code_block_start_pos = -1
                self.code_block_content = ""

        return new_calls

    def get_all_calls(self) -> list[StreamingToolCall]:
        """Получить все найденные команды."""
        return self.found_calls

    def reset(self) -> None:
        """Сбросить состояние парсера."""
        self.buffer = ""
        self.found_calls = []
        self.in_code_block = False
        self.code_block_start_pos = -1
        self.code_block_content = ""


class StreamingToolExecutor:
    """Исполнитель инструментов в потоковом режиме."""

    def __init__(self, execute_callback):
        """
        Инициализация исполнителя.

        Args:
            execute_callback: Функция для выполнения инструмента
                             Сигнатура: (tool_name: str, **kwargs) -> ToolResult
        """
        self.execute_callback = execute_callback
        self.parser = StreamingToolCallParser()
        self.executed_calls: set[int] = set()  # Позиции выполненных команд

    def process_chunk(self, chunk: str) -> tuple[str, list[tuple[StreamingToolCall, any]]]:
        """
        Обработать chunk из потока.

        Args:
            chunk: Кусок текста из потока LLM

        Returns:
            (chunk, executed_results) - chunk для вывода и результаты выполнения
        """
        # Парсим новые команды
        new_calls = self.parser.feed(chunk)

        # Выполняем новые команды
        results = []
        for call in new_calls:
            if call.start_pos not in self.executed_calls:
                # Выполняем команду
                result = self.execute_callback(call.tool_name, command=call.command)
                results.append((call, result))
                self.executed_calls.add(call.start_pos)
                call.executed = True

        return chunk, results

    def reset(self) -> None:
        """Сбросить состояние."""
        self.parser.reset()
        self.executed_calls.clear()


def stream_with_tool_calling(
    llm_stream: Iterator[str],
    execute_callback,
    output_callback=None,
) -> tuple[str, list[tuple[StreamingToolCall, any]]]:
    """
    Обработать поток LLM с автоматическим выполнением команд.

    Args:
        llm_stream: Итератор с chunks от LLM
        execute_callback: Функция для выполнения инструментов
        output_callback: Опциональный callback для вывода chunks (для UI)

    Returns:
        (full_response, executed_results) - полный ответ и результаты выполнения

    Example:
        >>> def execute(tool_name, **kwargs):
        ...     return agent.execute_tool(tool_name, **kwargs)
        >>>
        >>> def output(chunk):
        ...     print(chunk, end='', flush=True)
        >>>
        >>> response, results = stream_with_tool_calling(
        ...     llm_client.stream_complete(messages),
        ...     execute_callback=execute,
        ...     output_callback=output,
        ... )
    """
    executor = StreamingToolExecutor(execute_callback)
    full_response = []
    all_results = []

    for chunk in llm_stream:
        # Обрабатываем chunk
        output_chunk, results = executor.process_chunk(chunk)

        # Сохраняем
        full_response.append(output_chunk)
        all_results.extend(results)

        # Выводим если нужно
        if output_callback:
            output_callback(output_chunk)

    return "".join(full_response), all_results
