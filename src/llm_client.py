"""LLM Client - единый интерфейс для работы с разными провайдерами LLM."""
from __future__ import annotations

import json
import os
import re
from abc import ABC, abstractmethod
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any, Iterator

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
    temperature: float = 0.1   # Низкая температура для надёжного JSON
    max_tokens: int = 4096
    stream: bool = False


class LLMClient(ABC):
    def __init__(self, config: LLMConfig):
        self.config = config

    @abstractmethod
    def complete(self, messages: list[LLMMessage], system: str | None = None) -> LLMResponse:
        pass

    def _build_tools_prompt(self, tools: list[dict]) -> str:
        """Строим JSON Schema описание инструментов для промпта."""
        tools_json = json.dumps(tools, ensure_ascii=False, indent=2)
        return f"""You have access to the following tools. To use a tool, respond with a JSON object in this exact format:

```json
{{
  "tool": "tool_name",
  "params": {{
    "param1": "value1",
    "param2": "value2"
  }}
}}
```

Available tools:
{tools_json}

IMPORTANT:
- Respond with ONLY the JSON object if you want to use a tool
- No extra text before or after the JSON when calling a tool
- Use the exact parameter names from the schema
- If you don't need a tool, just respond normally in text"""

    def _parse_json_tool_call(self, text: str) -> list[dict]:
        """Парсим JSON tool call из ответа модели."""
        tool_use_blocks = []

        # Ищем JSON в markdown блоке ```json ... ```
        patterns = [
            r'```json\s*(\{.*?\})\s*```',
            r'```\s*(\{.*?\})\s*```',
            r'(\{\s*"tool"\s*:.*?\})',
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
        """
        Извлекаем tool calls из текста модели в любом формате:
        1. {"name": "tool", "arguments": {...}}        ← OpenAI-подобный
        2. {"name": "tool", "parameters": {...}}       ← вариант
        3. {"tool": "tool", "params": {...}}           ← наш fallback формат
        4. ```json {...} ```                           ← в markdown блоке
        5. Несколько JSON объектов в тексте           ← мульти-вызов
        """
        import re
        results = []
        seen = set()

        # Кандидаты на JSON — сначала markdown блоки, потом голые объекты
        candidates = []
        for pattern in [r'```json\s*(\{.*?\})\s*```', r'```\s*(\{.*?\})\s*```']:
            candidates += re.findall(pattern, text, re.DOTALL)
        # Голые JSON объекты (жадный поиск всех { ... })
        for m in re.finditer(r'\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\}', text, re.DOTALL):
            candidates.append(m.group())

        known_tools = set()  # заполним из схем если нужно

        for raw in candidates:
            try:
                data = json.loads(raw.strip())
            except (json.JSONDecodeError, ValueError):
                continue

            name = None
            args = {}

            # Формат: {"name": ..., "arguments": ...}
            if "name" in data and "arguments" in data:
                name = data["name"]
                args = data["arguments"]
                if isinstance(args, str):
                    try: args = json.loads(args)
                    except: args = {}

            # Формат: {"name": ..., "parameters": ...}
            elif "name" in data and "parameters" in data:
                name = data["name"]
                args = data["parameters"]

            # Формат: {"tool": ..., "params": ...}
            elif "tool" in data and "params" in data:
                name = data["tool"]
                args = data.get("params", {})

            # Формат: {"tool": ..., "arguments": ...}
            elif "tool" in data and "arguments" in data:
                name = data["tool"]
                args = data["arguments"]
                if isinstance(args, str):
                    try: args = json.loads(args)
                    except: args = {}

            if not name or not isinstance(args, dict):
                continue

            sig = f"{name}:{json.dumps(args, sort_keys=True)}"
            if sig in seen:
                continue
            seen.add(sig)

            results.append({
                "id": f"extracted_{len(results)}",
                "name": name,
                "input": args,
            })

        return results

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
        """Нативный tool calling через Anthropic API."""
        anthropic_messages = []
        for m in messages:
            if m.role == "system":
                continue
            if isinstance(m.content, str):
                anthropic_messages.append({"role": m.role, "content": m.content})
            else:
                anthropic_messages.append({"role": m.role, "content": m.content})

        # Anthropic формат: input_schema
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
        """Нативный tool calling через OpenAI API."""
        oai_messages = []
        if system:
            oai_messages.append({"role": "system", "content": system})
        for m in messages:
            if isinstance(m.content, str):
                oai_messages.append({"role": m.role, "content": m.content})
            else:
                oai_messages.append({"role": m.role, "content": str(m.content)})

        # OpenAI формат
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
# Ollama — нативный tool calling через /v1
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
        """Нативный tool calling через OpenAI-совместимый /v1 endpoint Ollama."""
        if not HAS_OPENAI:
            # Fallback на JSON промпт если нет openai
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

        # OpenAI формат для Ollama
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

            # Если нативный tool_calls пустой — парсим JSON из текста
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
            # Если нативный tool calling не поддерживается моделью — JSON fallback
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
        """Нативный tool calling через OpenRouter."""
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

    # complete_with_tools наследуется как JSON fallback — KoboldCPP не поддерживает нативный


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
    else:
        raise ValueError(f"Unknown provider: {config.provider}")


def auto_detect_provider() -> tuple[LLMProvider, str]:
    """Авто-определение провайдера по переменным окружения."""
    if os.getenv("ANTHROPIC_API_KEY"):
        return LLMProvider.ANTHROPIC, "claude-3-5-sonnet-20241022"
    if os.getenv("OPENROUTER_API_KEY"):
        return LLMProvider.OPENROUTER, os.getenv("OPENROUTER_MODEL", "qwen/qwen-2.5-coder-32b-instruct")
    if os.getenv("OPENAI_API_KEY"):
        return LLMProvider.OPENAI, "gpt-4o"
    if os.getenv("OLLAMA_HOST"):
        return LLMProvider.OLLAMA, os.getenv("OLLAMA_MODEL", "llama3.2:3b")
    return LLMProvider.OLLAMA, "llama3.2:3b"
