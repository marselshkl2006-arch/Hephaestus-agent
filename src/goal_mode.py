"""
Goal Mode — режим достижения цели.

Раньше здесь был отдельный статичный планировщик: одним вызовом LLM
составлялся фиксированный список шагов (JSON), после чего шаги слепо
выполнялись один за другим — без реакции на промежуточные результаты,
без возможности сделать несколько инструментов ради одного шага, и с
мёртвым VERIFY_PROMPT, который никогда не вызывался.

Теперь Goal Mode — тонкая обвязка над тем же адаптивным циклом
инструментов, что и обычный chat(): модель сама решает, что делать
дальше, на основе РЕАЛЬНЫХ результатов уже выполненных шагов, может
использовать сколько угодно инструментов подряд, сама себя
перепланирует при неудаче, и продолжает пока не сочтёт цель
выполненной (или не кончится бюджет итераций). Список шагов для
отчёта и обучения skill_learner собирается постфактум из реально
выполненных вызовов инструментов (через _progress_hook), а не
придумывается заранее.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable


@dataclass
class GoalStep:
    step_num: int
    description: str
    tool: str = ""
    params: dict = field(default_factory=dict)
    status: str = "pending"  # pending, done, failed
    result: str = ""


@dataclass
class Goal:
    description: str
    steps: list[GoalStep] = field(default_factory=list)
    status: str = "planning"  # planning, executing, done, failed
    report: str = ""


class GoalModeAgent:
    """
    Режим цели — прогоняет цель через адаптивный цикл инструментов
    основного агента, наблюдая за каждым реальным вызовом инструмента,
    чтобы построить чек-лист/отчёт и накормить skill_learner.
    """

    # Системная рамка поверх обычного SYSTEM_PROMPT — не меняет формат
    # вызовов (<tool_call>...), а только говорит модели не
    # останавливаться после одного действия.
    GOAL_FRAMING = """ЗАДАЧА ВЫШЕ — это ЦЕЛЬ, а не разовый запрос.

Работай самостоятельно, шаг за шагом, вызывая столько инструментов
подряд, сколько нужно. Если для цели нужно сначала что-то изучить
(структуру проекта, существующий код, репозиторий) — сделай это
ПЕРЕД тем как писать/создавать что-либо, и используй увиденное.
Если какой-то шаг не удался — не сдавайся, попробуй скорректировать
подход и продолжай.

Останавливайся и пиши финальный текстовый ответ ТОЛЬКО когда цель
полностью выполнена (или ты уверен, что дальше двигаться нельзя).
Не проси подтверждения — действуй."""

    MAX_GOAL_ITERATIONS = 80  # цели обычно требуют больше шагов, чем обычный чат

    def __init__(self, agent, on_progress: Callable | None = None):
        self.agent = agent
        self.on_progress = on_progress or print
        self.current_goal: Goal | None = None

    def pursue(self, goal_text: str, save_report_to: str = "") -> str:
        """Выполнить цель от начала до конца через адаптивный цикл инструментов."""
        self.on_progress(f"\n🎯 Цель: {goal_text}")

        goal = Goal(description=goal_text, status="executing")
        self.current_goal = goal

        # Наблюдаем за каждым реальным вызовом инструмента внутри chat(),
        # чтобы построить чек-лист постфактум — без выдумывания шагов заранее.
        def _on_tool_executed(name: str, params: dict, result) -> None:
            step_num = len(goal.steps) + 1
            status = "done" if result.success else "failed"
            text_result = result.output if result.success else (result.error or "")
            goal.steps.append(GoalStep(
                step_num=step_num,
                description=f"{name}({', '.join(f'{k}={v!r}' for k, v in list(params.items())[:3])})",
                tool=name,
                params=params,
                status=status,
                result=text_result,
            ))
            icon = "✅" if result.success else "❌"
            self.on_progress(f"⚒️  Шаг {step_num}: {name} — {icon} {str(text_result)[:100]}")

        goal_prompt = f"{goal_text}\n\n{self.GOAL_FRAMING}"

        self.agent._progress_hook = _on_tool_executed
        try:
            final_text = self.agent.chat(
                goal_prompt,
                max_iterations=self.MAX_GOAL_ITERATIONS,
                force_all_tools=True,
            )
        except Exception as e:
            self.on_progress(f"⚠️  Ошибка при выполнении цели: {e}")
            final_text = f"Цель прервана ошибкой: {e}"
        finally:
            self.agent._progress_hook = None

        done = sum(1 for s in goal.steps if s.status == "done")
        failed = sum(1 for s in goal.steps if s.status == "failed")
        goal.status = "done" if failed == 0 else ("failed" if done == 0 else "done")

        report_lines = [
            f"## Отчёт по цели: {goal_text}\n",
            f"✅ Выполнено шагов: {done}",
            f"❌ Ошибок: {failed}\n",
        ]
        if goal.steps:
            report_lines.append("### Шаги:")
            for s in goal.steps:
                icon = "✅" if s.status == "done" else "❌"
                report_lines.append(f"{icon} {s.step_num}. {s.description}")
                if s.result:
                    report_lines.append(f"   Результат: {str(s.result)[:150]}")
            report_lines.append("")
        report_lines.append("### Итог модели:")
        report_lines.append(final_text or "(без финального текста)")

        report = "\n".join(report_lines)
        goal.report = report

        if save_report_to:
            Path(save_report_to).parent.mkdir(parents=True, exist_ok=True)
            Path(save_report_to).write_text(report, encoding="utf-8")
            self.on_progress(f"\n💾 Отчёт сохранён: {save_report_to}")

        self.on_progress(f"\n🏁 Цель завершена: {done} шагов выполнено, {failed} ошибок")
        return report
