"""
Agent Tool - запуск подагентов для сложных задач.

Аналог Agent tool из оригинального Claude Code.
Позволяет запускать изолированных подагентов для параллельного выполнения задач,
исследования кодовой базы, планирования и других сложных операций.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any
from uuid import uuid4

from .context_manager import Session, create_session
from .llm_client import LLMConfig, LLMMessage, create_llm_client


class AgentType(Enum):
    """Типы подагентов."""
    GENERAL_PURPOSE = "general-purpose"
    EXPLORE = "explore"
    PLAN = "plan"


@dataclass
class AgentResult:
    """Результат работы подагента."""
    agent_id: str
    agent_type: AgentType
    prompt: str
    response: str
    success: bool
    error: str | None = None
    metadata: dict[str, Any] | None = None


class SubAgent:
    """Подагент для выполнения изолированной задачи."""

    def __init__(
        self,
        agent_id: str,
        agent_type: AgentType,
        llm_config: LLMConfig,
        workspace_root: Path,
    ):
        """
        Инициализация подагента.

        Args:
            agent_id: Уникальный ID агента
            agent_type: Тип агента
            llm_config: Конфигурация LLM
            workspace_root: Корневая директория workspace
        """
        self.agent_id = agent_id
        self.agent_type = agent_type
        self.llm_config = llm_config
        self.workspace_root = workspace_root
        self.llm_client = create_llm_client(llm_config)
        self.session = create_session(agent_id, llm_config)

    def _build_system_prompt(self) -> str:
        """Построить системный промпт для подагента."""
        base_prompt = f"""You are a specialized sub-agent running in isolated context.

Agent ID: {self.agent_id}
Agent Type: {self.agent_type.value}
Workspace: {self.workspace_root}

"""

        if self.agent_type == AgentType.GENERAL_PURPOSE:
            base_prompt += """Your role: General-purpose agent for researching complex questions,
searching for code, and executing multi-step tasks.

You have access to all standard tools: bash, file operations, git, grep, glob.
Focus on thorough research and providing detailed answers."""

        elif self.agent_type == AgentType.EXPLORE:
            base_prompt += """Your role: Fast agent specialized for exploring codebases.

Use this when you need to:
- Quickly find files by patterns (e.g., "src/components/**/*.tsx")
- Search code for keywords (e.g., "API endpoints")
- Answer questions about the codebase (e.g., "how do API endpoints work?")

Be thorough but efficient. Use glob and grep extensively."""

        elif self.agent_type == AgentType.PLAN:
            base_prompt += """Your role: Software architect agent for designing implementation plans.

Your task is to:
1. Analyze the requirements
2. Identify critical files and components
3. Consider architectural trade-offs
4. Create a step-by-step implementation plan

DO NOT execute any changes - only plan and analyze."""

        return base_prompt

    def execute(self, prompt: str) -> AgentResult:
        """
        Выполнить задачу подагента.

        Args:
            prompt: Задача для подагента

        Returns:
            Результат выполнения
        """
        try:
            # Добавляем промпт пользователя
            self.session.add_message("user", prompt)

            # Строим системный промпт
            system_prompt = self._build_system_prompt()

            # Получаем сообщения
            messages = [
                LLMMessage(role=msg.role, content=msg.content)
                for msg in self.session.get_messages()
            ]

            # Получаем ответ от LLM
            response = self.llm_client.complete(
                messages=messages,
                system=system_prompt,
            )

            # Сохраняем ответ
            self.session.add_message(
                "assistant",
                response.content,
                tokens=response.usage.get("completion_tokens", 0),
            )

            return AgentResult(
                agent_id=self.agent_id,
                agent_type=self.agent_type,
                prompt=prompt,
                response=response.content,
                success=True,
            )

        except Exception as e:
            return AgentResult(
                agent_id=self.agent_id,
                agent_type=self.agent_type,
                prompt=prompt,
                response="",
                success=False,
                error=str(e),
            )


class AgentTool:
    """Инструмент для запуска подагентов."""

    def __init__(
        self,
        llm_config: LLMConfig,
        workspace_root: Path,
    ):
        """
        Инициализация Agent Tool.

        Args:
            llm_config: Конфигурация LLM
            workspace_root: Корневая директория workspace
        """
        self.llm_config = llm_config
        self.workspace_root = workspace_root
        self.active_agents: dict[str, SubAgent] = {}

    def spawn(
        self,
        description: str,
        prompt: str,
        subagent_type: str = "general-purpose",
        run_in_background: bool = False,
    ) -> AgentResult:
        """
        Запустить подагента.

        Args:
            description: Краткое описание задачи (3-5 слов)
            prompt: Детальная задача для подагента
            subagent_type: Тип агента (general-purpose, explore, plan)
            run_in_background: Запустить в фоне (пока не поддерживается)

        Returns:
            Результат работы подагента
        """
        # Создаем ID агента
        agent_id = f"agent-{uuid4().hex[:8]}"

        # Определяем тип агента
        try:
            agent_type = AgentType(subagent_type)
        except ValueError:
            agent_type = AgentType.GENERAL_PURPOSE

        # Создаем подагента
        agent = SubAgent(
            agent_id=agent_id,
            agent_type=agent_type,
            llm_config=self.llm_config,
            workspace_root=self.workspace_root,
        )

        # Сохраняем в активных
        self.active_agents[agent_id] = agent

        # Выполняем задачу
        result = agent.execute(prompt)

        # Добавляем метаданные
        result.metadata = {
            "description": description,
            "run_in_background": run_in_background,
        }

        return result

    def get_agent(self, agent_id: str) -> SubAgent | None:
        """Получить активного агента по ID."""
        return self.active_agents.get(agent_id)

    def list_agents(self) -> list[dict]:
        """Список активных агентов."""
        return [
            {
                "agent_id": agent.agent_id,
                "agent_type": agent.agent_type.value,
            }
            for agent in self.active_agents.values()
        ]


def create_agent_tool(llm_config: LLMConfig, workspace_root: Path) -> AgentTool:
    """
    Создать Agent Tool.

    Args:
        llm_config: Конфигурация LLM
        workspace_root: Корневая директория workspace

    Returns:
        Agent Tool
    """
    return AgentTool(llm_config, workspace_root)
