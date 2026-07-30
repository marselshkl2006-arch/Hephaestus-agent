"""LLM Client - единый интерфейс для работы с разными провайдерами LLM."""
from __future__ import annotations

import json
import os
import re
from abc import ABC, abstractmethod
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any, Iterator, Callable

try:
    import requests
    HAS_REQUESTS = True
except ImportError:
    HAS_REQUESTS = False

try:
    import openai
    HAS_OPENAI = True
except ImportError:
    HAS_OPENAI = False


class LLMProvider(Enum):
    OLLAMA = "ollama"
    OPENAI = "openai"
    ANTHROPIC = "anthropic"
    OPENROUTER = "openrouter"
    KOBOLDCPP = "koboldcpp"
    LLAMA_SERVER = "llama_server"      # 🔥 НОВЫЙ! Для llama.cpp сервер
    CUSTOM = "custom"                  # 🔥 НОВЫЙ! Кастомный OpenAI-совместимый


@dataclass
class LLMMessage:
    role: str
    content: str


@dataclass
class LLMResponse:
    content: str | list
    stop_reason: str
    usage: dict[str, int]
    model: str
    tool_use_blocks: list[dict] | None = None


@dataclass
class LLMConfig:
    provider: LLMProvider
    model: str
    api_key: str | None = None
    base_url: str | None = None
    temperature: float = 0.1
    max_tokens: int = 4096
    stream: bool = False


class LLMClient(ABC):
    def __init__(self, config: LLMConfig):
        self.config = config

    @abstractmethod
    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        pass

    def _build_tools_prompt(self, tools: list[dict]) -> str:
        """Строим описание инструментов для промпта.

        Формат — Hermes-style <tool_call>{...}</tool_call>: короткий,
        однозначный, без вариаций ключей. Раньше здесь был СВОЙ формат
        ("tool"/"params"), отличный от формата в SYSTEM_PROMPT
        ("name"/"arguments") — они склеивались в один system-промпт и
        модель получала два противоречащих друг другу примера формата
        одновременно. Плюс тут не закрывался ```json блок перед списком
        инструментов, из-за чего пример вызова визуально сливался со
        схемой всех инструментов. Оба момента исправлены.
        """
        tools_json = json.dumps(tools, ensure_ascii=False, indent=2)
        return f"""You have access to tools. To call one, output ONLY this, nothing else:

<tool_call>
{{"name": "tool_name", "arguments": {{"param1": "value1"}}}}
</tool_call>

Rules:
- Exactly one <tool_call> block, valid JSON inside it, no text before/after.
- Use only tools and parameter names listed below.
- If no tool is needed, answer normally in plain text (no <tool_call>).

Available tools:
```json
{tools_json}
```"""

    def _parse_json_tool_call(self, text: str) -> list[dict]:
        """Парсим JSON tool call из ответа модели."""
        tool_use_blocks = []

        patterns = [
            r'json\s*(\{.*?\})\s*',
            r'\s*(\{.*?\})\s*',
            r'({\s"tool"\s:.*?})',
        ]

        for pattern in patterns:
            matches = re.findall(pattern, text, re.DOTALL)
            for match in matches:
                try:
                    data = json.loads(match.strip())
                    if "tool" in data and "params" in data:
                        tool_use_blocks.append({
                            "id": f"call_{len(tool_use_blocks)}",
                            "name": data["tool"],
                            "input": data.get("params", {})
                        })
                except json.JSONDecodeError:
                    continue

            if tool_use_blocks:
                break

        return tool_use_blocks

    def _extract_tool_calls(self, text: str) -> list[dict]:
        """Извлекаем tool calls из текста модели в любом формате.

        Раньше здесь были non-greedy регулярки вида r'\\{.*?\\}', которые
        обрывались на первой встреченной '}' и потому ломались на любом
        вызове с вложенным объектом параметров (path+content и т.п.).
        Теперь используется посимвольный баланс скобок — см. json_extract.py.
        """
        from .json_extract import extract_tool_calls
        return extract_tool_calls(text)

    def _fallback_tool_calling(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None
    ) -> LLMResponse:
        """JSON-based fallback для моделей без нативного tool calling."""
        tools_prompt = self._build_tools_prompt(tools)
        enhanced_system = f"{system or ''}\n\n{tools_prompt}"

        response = self.complete(messages, system=enhanced_system)

        tool_use_blocks = self._extract_tool_calls(response.content or "")
        if tool_use_blocks:
            response.tool_use_blocks = tool_use_blocks
            response.stop_reason = "tool_use"

        return response

    def complete_with_tools(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None
    ) -> LLMResponse:
        return self._fallback_tool_calling(messages, tools, system)


# ─────────────────────────────────────────
# LlamaServerClient — для llama.cpp сервера
# ─────────────────────────────────────────
class LlamaServerClient(LLMClient):
    """Клиент для llama.cpp сервера (OpenAI-совместимый API)."""

    def __init__(self, config: LLMConfig):
        super().__init__(config)
        if not HAS_OPENAI:
            raise ImportError("openai library required: pip install openai")
        base_url = config.base_url or os.getenv("LLAMA_SERVER_URL", "http://127.0.0.1:8080")
        self.base_url = base_url.rstrip("/")
        self.client = openai.OpenAI(
            api_key="not-needed",
            base_url=f"{self.base_url}/v1"
        )
        # Сохраняем имя модели
        self.model_name = config.model or "default"

    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            oai_messages.append({"role": m.role, "content": m.content})

        try:
            response = self.client.chat.completions.create(
                model="default",
                messages=oai_messages,
                temperature=self.config.temperature,
                max_tokens=self.config.max_tokens,
            )

            content = response.choices[0].message.content or ""

            return LLMResponse(
                content=content,
                stop_reason=response.choices[0].finish_reason or "stop",
                usage={
                    "prompt_tokens": response.usage.prompt_tokens,
                    "completion_tokens": response.usage.completion_tokens,
                },
                model=self.model_name,
            )
        except Exception as e:
            # Если ошибка — пробуем через requests напрямую (fallback)
            return self._fallback_complete(messages, system)

    def _fallback_complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        """Fallback через requests если openai клиент не работает."""
        import requests

        url = f"{self.base_url}/v1/chat/completions"
        payload = {
            "model": "default",
            "messages": [{"role": m.role, "content": m.content} for m in messages],
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
        }
        if system:
            payload["messages"].insert(0, {"role": "system", "content": system})

        response = requests.post(url, json=payload, timeout=600)
        response.raise_for_status()
        data = response.json()

        content = data["choices"][0]["message"]["content"]

        return LLMResponse(
            content=content,
            stop_reason=data["choices"][0].get("finish_reason", "stop"),
            usage=data.get("usage", {}),
            model=self.model_name,
        )

    def complete_with_tools(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None
    ) -> LLMResponse:
        """Tool calling через JSON fallback (llama-server не поддерживает нативный tools)."""
        # Просто используем fallback с JSON промптом
        return self._fallback_tool_calling(messages, tools, system)

# ─────────────────────────────────────────
# CustomClient — для любых OpenAI-совместимых API
# ─────────────────────────────────────────
class CustomClient(LLMClient):
    """Кастомный клиент для OpenAI-совместимых API (любой сервер)."""

    def __init__(self, config: LLMConfig):
        super().__init__(config)
        if not HAS_OPENAI:
            raise ImportError("openai library required: pip install openai")
        if not config.base_url:
            raise ValueError("base_url required for custom provider")
        api_key = config.api_key or os.getenv("CUSTOM_API_KEY", "not-needed")
        self.client = openai.OpenAI(
            api_key=api_key,
            base_url=config.base_url.rstrip("/")
        )

    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            oai_messages.append({"role": m.role, "content": m.content})

        response = self.client.chat.completions.create(
            model=self.config.model,
            messages=oai_messages,
            temperature=self.config.temperature,
            max_tokens=self.config.max_tokens,
        )

        return LLMResponse(
            content=response.choices[0].message.content or "",
            stop_reason=response.choices[0].finish_reason or "stop",
            usage={
                "prompt_tokens": response.usage.prompt_tokens,
                "completion_tokens": response.usage.completion_tokens,
            },
            model=response.model or self.config.model,
        )

    def complete_with_tools(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None
    ) -> LLMResponse:
        """Нативный tool calling через кастомный OpenAI-совместимый API."""
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            oai_messages.append({"role": m.role, "content": m.content})

        oai_tools = []
        for t in tools:
            oai_tools.append({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t.get("description", ""),
                    "parameters": t.get("input_schema", {"type": "object", "properties": {}}),
                },
            })

        try:
            response = self.client.chat.completions.create(
                model=self.config.model,
                messages=oai_messages,
                tools=oai_tools,
                tool_choice="auto",
                temperature=self.config.temperature,
                max_tokens=self.config.max_tokens,
            )

            message = response.choices[0].message
            tool_use_blocks = []
            if message.tool_calls:
                for tc in message.tool_calls:
                    try:
                        args = json.loads(tc.function.arguments)
                    except json.JSONDecodeError:
                        args = {}
                    tool_use_blocks.append({
                        "id": tc.id,
                        "name": tc.function.name,
                        "input": args,
                    })

            if not tool_use_blocks and message.content:
                tool_use_blocks = self._extract_tool_calls(message.content)

            return LLMResponse(
                content=message.content or "",
                stop_reason="tool_use" if tool_use_blocks else response.choices[0].finish_reason,
                usage={
                    "prompt_tokens": response.usage.prompt_tokens,
                    "completion_tokens": response.usage.completion_tokens,
                },
                model=response.model,
                tool_use_blocks=tool_use_blocks if tool_use_blocks else None,
            )

        except Exception:
            return self._fallback_tool_calling(messages, tools, system)


# ─────────────────────────────────────────
# Anthropic
# ─────────────────────────────────────────
class AnthropicClient(LLMClient):
    def __init__(self, config: LLMConfig):
        super().__init__(config)
        try:
            import anthropic
            self.anthropic = anthropic
        except ImportError:
            raise ImportError("anthropic library required: pip install anthropic")
        api_key = config.api_key or os.getenv("ANTHROPIC_API_KEY")
        if not api_key:
            raise ValueError("Anthropic API key required")
        self.client = self.anthropic.Anthropic(api_key=api_key)

    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        anthropic_messages = [
            {"role": m.role, "content": m.content}
            for m in messages if m.role != "system"
        ]
        kwargs = {
            "model": self.config.model,
            "messages": anthropic_messages,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
        }
        if system:
            kwargs["system"] = system

        response = self.client.messages.create(**kwargs)
        content = "".join(
            block.text for block in response.content if hasattr(block, "text")
        )
        return LLMResponse(
            content=content,
            stop_reason=response.stop_reason,
            usage={
                "prompt_tokens": response.usage.input_tokens,
                "completion_tokens": response.usage.output_tokens,
            },
            model=response.model,
        )

    def complete_with_tools(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None
    ) -> LLMResponse:
        anthropic_messages = []
        for m in messages:
            if m.role == "system":
                continue
            if isinstance(m.content, str):
                anthropic_messages.append({"role": m.role, "content": m.content})
            else:
                anthropic_messages.append({"role": m.role, "content": m.content})

        anthropic_tools = []
        for t in tools:
            anthropic_tools.append({
                "name": t["name"],
                "description": t.get("description", ""),
                "input_schema": t.get("input_schema", {"type": "object", "properties": {}}),
            })

        kwargs = {
            "model": self.config.model,
            "messages": anthropic_messages,
            "tools": anthropic_tools,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
        }
        if system:
            kwargs["system"] = system

        response = self.client.messages.create(**kwargs)

        content_text = ""
        tool_use_blocks = []

        for block in response.content:
            if hasattr(block, "text"):
                content_text += block.text
            elif block.type == "tool_use":
                tool_use_blocks.append({
                    "id": block.id,
                    "name": block.name,
                    "input": block.input,
                })

        return LLMResponse(
            content=content_text,
            stop_reason=response.stop_reason,
            usage={
                "prompt_tokens": response.usage.input_tokens,
                "completion_tokens": response.usage.output_tokens,
            },
            model=response.model,
            tool_use_blocks=tool_use_blocks if tool_use_blocks else None,
        )


# ─────────────────────────────────────────
# OpenAI
# ─────────────────────────────────────────
class OpenAIClient(LLMClient):
    def __init__(self, config: LLMConfig):
        super().__init__(config)
        if not HAS_OPENAI:
            raise ImportError("openai library required: pip install openai")
        api_key = config.api_key or os.getenv("OPENAI_API_KEY")
        if not api_key:
            raise ValueError("OpenAI API key required")
        self.client = openai.OpenAI(api_key=api_key)

    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            oai_messages.append({"role": m.role, "content": m.content})

        response = self.client.chat.completions.create(
            model=self.config.model,
            messages=oai_messages,
            temperature=self.config.temperature,
            max_tokens=self.config.max_tokens,
        )
        return LLMResponse(
            content=response.choices[0].message.content or "",
            stop_reason=response.choices[0].finish_reason,
            usage={
                "prompt_tokens": response.usage.prompt_tokens,
                "completion_tokens": response.usage.completion_tokens,
            },
            model=response.model,
        )

    def complete_with_tools(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None
    ) -> LLMResponse:
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            if isinstance(m.content, str):
                oai_messages.append({"role": m.role, "content": m.content})
            else:
                oai_messages.append({"role": m.role, "content": str(m.content)})

        oai_tools = []
        for t in tools:
            oai_tools.append({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t.get("description", ""),
                    "parameters": t.get("input_schema", {"type": "object", "properties": {}}),
                },
            })

        response = self.client.chat.completions.create(
            model=self.config.model,
            messages=oai_messages,
            tools=oai_tools,
            tool_choice="auto",
            temperature=self.config.temperature,
            max_tokens=self.config.max_tokens,
        )

        message = response.choices[0].message
        tool_use_blocks = []
        if message.tool_calls:
            for tc in message.tool_calls:
                try:
                    args = json.loads(tc.function.arguments)
                except json.JSONDecodeError:
                    args = {}
                tool_use_blocks.append({
                    "id": tc.id,
                    "name": tc.function.name,
                    "input": args,
                })

        return LLMResponse(
            content=message.content or "",
            stop_reason=response.choices[0].finish_reason,
            usage={
                "prompt_tokens": response.usage.prompt_tokens,
                "completion_tokens": response.usage.completion_tokens,
            },
            model=response.model,
            tool_use_blocks=tool_use_blocks if tool_use_blocks else None,
        )


# ─────────────────────────────────────────
# Ollama
# ─────────────────────────────────────────
class OllamaClient(LLMClient):
    def __init__(self, config: LLMConfig):
        super().__init__(config)
        if not HAS_REQUESTS:
            raise ImportError("requests library required: pip install requests")
        self.base_url = config.base_url or os.getenv("OLLAMA_HOST", "http://localhost:11434")

    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        url = f"{self.base_url}/api/chat"
        ollama_messages = []
        if system:
            ollama_messages.append({"role": "system", "content": system})
        for m in messages:
            ollama_messages.append({"role": m.role, "content": m.content})

        payload = {
            "model": self.config.model,
            "messages": ollama_messages,
            "stream": False,
            "options": {
                "temperature": self.config.temperature,
                "num_predict": self.config.max_tokens,
            },
        }
        response = requests.post(url, json=payload, timeout=300)
        response.raise_for_status()
        data = response.json()

        return LLMResponse(
            content=data["message"]["content"],
            stop_reason=data.get("done_reason", "stop"),
            usage={
                "prompt_tokens": data.get("prompt_eval_count", 0),
                "completion_tokens": data.get("eval_count", 0),
            },
            model=self.config.model,
        )

    def complete_with_tools(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None
    ) -> LLMResponse:
        if not HAS_OPENAI:
            return self._fallback_tool_calling(messages, tools, system)

        client = openai.OpenAI(api_key="ollama", base_url=f"{self.base_url}/v1")

        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            if isinstance(m.content, str):
                oai_messages.append({"role": m.role, "content": m.content})
            else:
                oai_messages.append({"role": m.role, "content": str(m.content)})

        oai_tools = []
        for t in tools:
            oai_tools.append({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t.get("description", ""),
                    "parameters": t.get("input_schema", {"type": "object", "properties": {}}),
                },
            })

        try:
            response = client.chat.completions.create(
                model=self.config.model,
                messages=oai_messages,
                tools=oai_tools,
                tool_choice="auto",
                temperature=self.config.temperature,
                max_tokens=self.config.max_tokens,
            )

            message = response.choices[0].message
            tool_use_blocks = []
            if message.tool_calls:
                for tc in message.tool_calls:
                    try:
                        args = json.loads(tc.function.arguments)
                    except json.JSONDecodeError:
                        args = {}
                    tool_use_blocks.append({
                        "id": tc.id,
                        "name": tc.function.name,
                        "input": args,
                    })

            if not tool_use_blocks and message.content:
                tool_use_blocks = self._extract_tool_calls(message.content)

            return LLMResponse(
                content=message.content or "",
                stop_reason="tool_use" if tool_use_blocks else response.choices[0].finish_reason,
                usage={
                    "prompt_tokens": response.usage.prompt_tokens,
                    "completion_tokens": response.usage.completion_tokens,
                },
                model=response.model,
                tool_use_blocks=tool_use_blocks if tool_use_blocks else None,
            )
        except Exception as _e:
            return self._fallback_tool_calling(messages, tools, system)

    def stream_complete_with_tools(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None,
        on_token: Callable[[str], None] | None = None,
    ) -> LLMResponse:
        if not HAS_OPENAI:
            return self._fallback_tool_calling(messages, tools, system)

        client = openai.OpenAI(api_key="ollama", base_url=f"{self.base_url}/v1")
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            oai_messages.append({"role": m.role, "content": str(m.content)})

        oai_tools = [{
            "type": "function",
            "function": {
                "name": t["name"],
                "description": t.get("description", ""),
                "parameters": t.get("input_schema", {"type": "object", "properties": {}}),
            }
        } for t in tools]

        try:
            full_content = ""
            tool_calls_raw = []

            stream = client.chat.completions.create(
                model=self.config.model,
                messages=oai_messages,
                tools=oai_tools,
                tool_choice="auto",
                temperature=self.config.temperature,
                max_tokens=self.config.max_tokens,
                stream=True,
            )

            for chunk in stream:
                delta = chunk.choices[0].delta if chunk.choices else None
                if not delta:
                    continue
                if delta.content:
                    full_content += delta.content
                    if on_token:
                        on_token(delta.content)
                if delta.tool_calls:
                    for tc in delta.tool_calls:
                        idx = tc.index
                        while len(tool_calls_raw) <= idx:
                            tool_calls_raw.append({"id": "", "name": "", "args": ""})
                        if tc.id:
                            tool_calls_raw[idx]["id"] = tc.id
                        if tc.function:
                            if tc.function.name:
                                tool_calls_raw[idx][name] += tc.function.name
                            if tc.function.arguments:
                                tool_calls_raw[idx]["args"] += tc.function.arguments

            tool_use_blocks = []
            for tc in tool_calls_raw:
                if tc["name"]:
                    try:
                        args = json.loads(tc["args"]) if tc["args"] else {}
                    except Exception:
                        args = {}
                    tool_use_blocks.append({
                        "id": tc["id"] or f"stream_{len(tool_use_blocks)}",
                        "name": tc["name"],
                        "input": args,
                    })

            if not tool_use_blocks and full_content:
                tool_use_blocks = self._extract_tool_calls(full_content)

            return LLMResponse(
                content=full_content,
                stop_reason="tool_use" if tool_use_blocks else "stop",
                usage={"prompt_tokens": 0, "completion_tokens": 0},
                model=self.config.model,
                tool_use_blocks=tool_use_blocks if tool_use_blocks else None,
            )

        except Exception:
            return self._fallback_tool_calling(messages, tools, system)

    def stream_complete(self, messages: list[LLMMessage], system: str | None = None) -> Iterator[str]:
        url = f"{self.base_url}/api/chat"
        ollama_messages = []
        if system:
            ollama_messages.append({"role": "system", "content": system})
        for m in messages:
            ollama_messages.append({"role": m.role, "content": m.content})

        payload = {
            "model": self.config.model,
            "messages": ollama_messages,
            "stream": True,
            "options": {
                "temperature": self.config.temperature,
                "num_predict": self.config.max_tokens,
            },
        }
        response = requests.post(url, json=payload, stream=True, timeout=120)
        response.raise_for_status()
        for line in response.iter_lines():
            if line:
                data = json.loads(line)
                if "message" in data and "content" in data["message"]:
                    yield data["message"]["content"]


# ─────────────────────────────────────────
# OpenRouter
# ─────────────────────────────────────────
class OpenRouterClient(LLMClient):
    def __init__(self, config: LLMConfig):
        super().__init__(config)
        if not HAS_OPENAI:
            raise ImportError("openai library required: pip install openai")
        api_key = config.api_key or os.getenv("OPENROUTER_API_KEY")
        if not api_key:
            raise ValueError("OpenRouter API key required")
        self.client = openai.OpenAI(
            api_key=api_key,
            base_url="https://openrouter.ai/api/v1"
        )

    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            content = m.content if isinstance(m.content, str) else json.dumps(m.content)
            oai_messages.append({"role": m.role, "content": content})

        response = self.client.chat.completions.create(
            model=self.config.model,
            messages=oai_messages,
            temperature=self.config.temperature,
            max_tokens=self.config.max_tokens,
        )
        return LLMResponse(
            content=response.choices[0].message.content or "",
            stop_reason=response.choices[0].finish_reason,
            usage={
                "prompt_tokens": response.usage.prompt_tokens,
                "completion_tokens": response.usage.completion_tokens,
            },
            model=response.model,
        )

    def complete_with_tools(
        self,
        messages: list[LLMMessage],
        tools: list[dict],
        system: str | None = None
    ) -> LLMResponse:
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            content = m.content if isinstance(m.content, str) else json.dumps(m.content)
            oai_messages.append({"role": m.role, "content": content})

        oai_tools = []
        for t in tools:
            oai_tools.append({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t.get("description", ""),
                    "parameters": t.get("input_schema", {"type": "object", "properties": {}}),
                },
            })

        try:
            response = self.client.chat.completions.create(
                model=self.config.model,
                messages=oai_messages,
                tools=oai_tools,
                tool_choice="auto",
                temperature=self.config.temperature,
                max_tokens=self.config.max_tokens,
            )

            message = response.choices[0].message
            tool_use_blocks = []
            if message.tool_calls:
                for tc in message.tool_calls:
                    try:
                        args = json.loads(tc.function.arguments)
                    except json.JSONDecodeError:
                        args = {}
                    tool_use_blocks.append({
                        "id": tc.id,
                        "name": tc.function.name,
                        "input": args,
                    })

            return LLMResponse(
                content=message.content or "",
                stop_reason=response.choices[0].finish_reason,
                usage={
                    "prompt_tokens": response.usage.prompt_tokens,
                    "completion_tokens": response.usage.completion_tokens,
                },
                model=response.model,
                tool_use_blocks=tool_use_blocks if tool_use_blocks else None,
            )
        except Exception:
            return self._fallback_tool_calling(messages, tools, system)


# ─────────────────────────────────────────
# KoboldCPP
# ─────────────────────────────────────────
class KoboldCPPClient(LLMClient):
    def __init__(self, config: LLMConfig):
        super().__init__(config)
        if not HAS_OPENAI:
            raise ImportError("openai library required: pip install openai")
        api_key = config.api_key or "not-needed"
        base_url = config.base_url or os.getenv("KOBOLDCPP_URL", "http://localhost:5001")
        self.client = openai.OpenAI(api_key=api_key, base_url=f"{base_url}/v1")

    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            oai_messages.append({"role": m.role, "content": m.content})

        response = self.client.chat.completions.create(
            model=self.config.model or "local-model",
            messages=oai_messages,
            temperature=self.config.temperature,
            max_tokens=self.config.max_tokens,
        )
        return LLMResponse(
            content=response.choices[0].message.content or "",
            stop_reason=response.choices[0].finish_reason,
            usage={
                "prompt_tokens": response.usage.prompt_tokens,
                "completion_tokens": response.usage.completion_tokens,
            },
            model=response.model,
        )


# ─────────────────────────────────────────
# Factory
# ─────────────────────────────────────────
def create_llm_client(config: LLMConfig) -> LLMClient:
    if config.provider == LLMProvider.OLLAMA:
        return OllamaClient(config)
    elif config.provider == LLMProvider.OPENAI:
        return OpenAIClient(config)
    elif config.provider == LLMProvider.ANTHROPIC:
        return AnthropicClient(config)
    elif config.provider == LLMProvider.OPENROUTER:
        return OpenRouterClient(config)
    elif config.provider == LLMProvider.KOBOLDCPP:
        return KoboldCPPClient(config)
    elif config.provider == LLMProvider.LLAMA_SERVER:
        return LlamaServerClient(config)
    elif config.provider == LLMProvider.CUSTOM:
        return CustomClient(config)
    else:
        raise ValueError(f"Unknown provider: {config.provider}")


def auto_detect_provider() -> tuple[LLMProvider, str]:
    """Авто-определение провайдера по переменным окружения."""

    # Сначала проверяем llama-server
    if os.getenv("LLAMA_SERVER_URL"):
        return LLMProvider.LLAMA_SERVER, os.getenv("LLAMA_MODEL", "qwen3.6-27b-uncensored")

    if os.getenv("ANTHROPIC_API_KEY"):
        return LLMProvider.ANTHROPIC, "claude-3-5-sonnet-20241022"
    if os.getenv("OPENROUTER_API_KEY"):
        return LLMProvider.OPENROUTER, os.getenv("OPENROUTER_MODEL", "qwen/qwen-2.5-coder-32b-instruct")
    if os.getenv("OPENAI_API_KEY"):
        return LLMProvider.OPENAI, "gpt-4o"
    if os.getenv("OLLAMA_HOST"):
        return LLMProvider.OLLAMA, os.getenv("OLLAMA_MODEL", "llama3.2:3b")
    return LLMProvider.OLLAMA, "llama3.2:3b"
