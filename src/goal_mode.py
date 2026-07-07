"""
Goal Mode — режим достижения цели.
Агент сам разбивает цель на шаги и выполняет их до конца.
Plan → Execute → Verify → Report
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable


@dataclass
class GoalStep:
    step_num: int
    description: str
    tool: str = ""
    params: dict = field(default_factory=dict)
    status: str = "pending"  # pending, done, failed, skipped
    result: str = ""


@dataclass
class Goal:
    description: str
    steps: list[GoalStep] = field(default_factory=list)
    status: str = "planning"  # planning, executing, done, failed
    report: str = ""


class GoalModeAgent:
    """
    Режим цели — агент планирует и выполняет до конца.
    Использует основной агент для выполнения шагов.
    """

    PLAN_PROMPT = """Ты планировщик задач. Пользователь дал тебе ЦЕЛЬ.
Разбей её на конкретные шаги. Каждый шаг — один вызов инструмента.

ЦЕЛЬ: {goal}

Доступные инструменты: {tools}

ВАЖНО для шагов с кодом:
- Если нужно написать файл с кодом — используй file_write с ПОЛНЫМ кодом в params.content
- НЕ создавай пустые файлы — сразу пиши весь код
- Для Python файлов: content должен содержать весь рабочий Python код

Ответь JSON массивом шагов:
[
  {{"step": 1, "description": "что делаем", "tool": "tool_name", "params": {{"key": "value"}}}},
  {{"step": 2, "description": "что делаем", "tool": "bash", "params": {{"command": "python3 file.py"}}}}
]

Только JSON, без лишнего текста."""

    VERIFY_PROMPT = """Шаг выполнен. Проверь результат.
Шаг: {step}
Результат: {result}

Ответь одним словом: SUCCESS или FAILED или RETRY"""

    def __init__(self, agent, on_progress: Callable | None = None):
        self.agent = agent
        self.on_progress = on_progress or print
        self.current_goal: Goal | None = None

    def pursue(self, goal_text: str, save_report_to: str = "") -> str:
        """Выполнить цель от начала до конца."""
        self.on_progress(f"\n🎯 Цель: {goal_text}")
        self.on_progress("⚙️  Составляю план...")

        goal = Goal(description=goal_text)
        self.current_goal = goal

        # 1. Планирование
        tools_list = ", ".join(s["name"] for s in self.agent._get_tools_schema()[:20])
        plan_prompt = self.PLAN_PROMPT.format(goal=goal_text, tools=tools_list)

        try:
            from .llm_client import LLMMessage
            plan_response = self.agent.llm_client.complete(
                messages=[LLMMessage(role="user", content=plan_prompt)],
                system="Ты помощник-планировщик. Отвечай только JSON."
            )

            raw = plan_response.content.strip()
            # Извлекаем JSON
            import re
            match = re.search(r"\[.*\]", raw, re.DOTALL)
            if match:
                steps_data = json.loads(match.group())
            else:
                steps_data = json.loads(raw)

            for s in steps_data:
                goal.steps.append(GoalStep(
                    step_num=s.get("step", len(goal.steps) + 1),
                    description=s.get("description", ""),
                    tool=s.get("tool", "bash"),
                    params=s.get("params", {}),
                ))

        except Exception as e:
            # Fallback — выполняем как обычный запрос
            self.on_progress(f"⚠️  Не удалось составить план: {e}")
            self.on_progress("⚡ Выполняю напрямую...")
            return self.agent.chat(goal_text)

        if not goal.steps:
            return self.agent.chat(goal_text)

        self.on_progress(f"📋 План: {len(goal.steps)} шагов")
        for s in goal.steps:
            self.on_progress(f"  {s.step_num}. {s.description}")

        # 2. Выполнение
        goal.status = "executing"
        results = []

        for step in goal.steps:
            self.on_progress(f"\n⚒️  Шаг {step.step_num}: {step.description}")

            try:
                result = self.agent.execute_tool(step.tool, **step.params)
                step.result = result.output if result.success else result.error
                step.status = "done" if result.success else "failed"

                icon = "✅" if result.success else "❌"
                self.on_progress(f"   {icon} {step.result[:100]}")
                results.append(f"Шаг {step.step_num} ({step.status}): {step.result[:200]}")

            except Exception as e:
                step.status = "failed"
                step.result = str(e)
                self.on_progress(f"   ❌ Ошибка: {e}")
                results.append(f"Шаг {step.step_num} (failed): {e}")

        # 3. Финальный отчёт
        goal.status = "done"
        done = sum(1 for s in goal.steps if s.status == "done")
        failed = sum(1 for s in goal.steps if s.status == "failed")

        report_lines = [
            f"## Отчёт по цели: {goal_text}\n",
            f"✅ Выполнено: {done}/{len(goal.steps)}",
            f"❌ Ошибок: {failed}\n",
            "### Шаги:",
        ]
        for s in goal.steps:
            icon = "✅" if s.status == "done" else "❌"
            report_lines.append(f"{icon} {s.step_num}. {s.description}")
            if s.result:
                report_lines.append(f"   Результат: {s.result[:150]}")

        report = "\n".join(report_lines)
        goal.report = report

        if save_report_to:
            Path(save_report_to).parent.mkdir(parents=True, exist_ok=True)
            Path(save_report_to).write_text(report, encoding="utf-8")
            self.on_progress(f"\n💾 Отчёт сохранён: {save_report_to}")

        self.on_progress(f"\n🏁 Цель выполнена: {done}/{len(goal.steps)} шагов")
        return report
