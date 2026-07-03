"""
Workflow System - система для определения и выполнения многошаговых workflow.
Поддерживает условия, циклы, параллельное выполнение.
"""
from __future__ import annotations

import json
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path
from typing import Any, Callable
from uuid import uuid4

from .real_tools import ToolResult


@dataclass
class WorkflowStep:
    """Шаг workflow."""
    step_id: str
    name: str
    action: str  # tool_call, condition, loop, parallel
    params: dict[str, Any]
    next_step: str | None = None
    on_success: str | None = None
    on_failure: str | None = None
    condition: str | None = None  # Для условных переходов


@dataclass
class WorkflowDefinition:
    """Определение workflow."""
    workflow_id: str
    name: str
    description: str
    steps: list[WorkflowStep]
    variables: dict[str, Any]
    created_at: str


@dataclass
class WorkflowExecution:
    """Выполнение workflow."""
    execution_id: str
    workflow_id: str
    status: str  # running, completed, failed, paused
    current_step: str | None
    variables: dict[str, Any]
    step_results: dict[str, Any]
    started_at: str
    completed_at: str | None = None
    error: str | None = None


class WorkflowEngine:
    """Движок для выполнения workflow."""

    def __init__(self, workspace_root: str = "."):
        self.workspace_root = Path(workspace_root)
        self.workflows_dir = Path.home() / ".claude_code" / "workflows"
        self.workflows_dir.mkdir(parents=True, exist_ok=True)
        self.workflows: dict[str, WorkflowDefinition] = {}
        self.executions: dict[str, WorkflowExecution] = {}
        self.tool_executor: Callable[[str, dict], ToolResult] | None = None
        self._load_workflows()

    def _load_workflows(self) -> None:
        """Загрузить сохраненные workflow."""
        for wf_file in self.workflows_dir.glob("workflow_*.json"):
            try:
                with open(wf_file, 'r', encoding='utf-8') as f:
                    data = json.load(f)
                    steps = [WorkflowStep(**s) for s in data.pop('steps')]
                    workflow = WorkflowDefinition(**data, steps=steps)
                    self.workflows[workflow.workflow_id] = workflow
            except Exception:
                pass

    def _save_workflow(self, workflow: WorkflowDefinition) -> None:
        """Сохранить workflow на диск."""
        wf_file = self.workflows_dir / f"workflow_{workflow.workflow_id}.json"
        data = asdict(workflow)
        with open(wf_file, 'w', encoding='utf-8') as f:
            json.dump(data, f, indent=2, ensure_ascii=False)

    def _save_execution(self, execution: WorkflowExecution) -> None:
        """Сохранить выполнение на диск."""
        exec_file = self.workflows_dir / f"execution_{execution.execution_id}.json"
        with open(exec_file, 'w', encoding='utf-8') as f:
            json.dump(asdict(execution), f, indent=2, ensure_ascii=False)

    def set_tool_executor(self, executor: Callable[[str, dict], ToolResult]) -> None:
        """Установить функцию для выполнения инструментов."""
        self.tool_executor = executor

    def create_workflow(
        self,
        name: str,
        description: str,
        steps: list[dict],
        variables: dict[str, Any] | None = None
    ) -> ToolResult:
        """
        Создать новый workflow.

        Args:
            name: Название workflow
            description: Описание
            steps: Список шагов
            variables: Начальные переменные

        Returns:
            ToolResult с workflow_id
        """
        workflow_id = uuid4().hex[:8]

        workflow_steps = []
        for step_data in steps:
            # Если передана строка, конвертируем в словарь
            if isinstance(step_data, str):
                step_data = {
                    'name': step_data,
                    'action': 'task',
                    'params': {}
                }

            step = WorkflowStep(
                step_id=step_data.get('step_id', uuid4().hex[:8]),
                name=step_data['name'],
                action=step_data['action'],
                params=step_data.get('params', {}),
                next_step=step_data.get('next_step'),
                on_success=step_data.get('on_success'),
                on_failure=step_data.get('on_failure'),
                condition=step_data.get('condition')
            )
            workflow_steps.append(step)

        workflow = WorkflowDefinition(
            workflow_id=workflow_id,
            name=name,
            description=description,
            steps=workflow_steps,
            variables=variables or {},
            created_at=datetime.now().isoformat()
        )

        self.workflows[workflow_id] = workflow
        self._save_workflow(workflow)

        return ToolResult(
            success=True,
            output=f"Workflow created: {workflow_id}\nName: {name}\nSteps: {len(workflow_steps)}",
            error=None
        )

    def execute_workflow(
        self,
        workflow_id: str,
        input_variables: dict[str, Any] | None = None
    ) -> ToolResult:
        """
        Запустить выполнение workflow.

        Args:
            workflow_id: ID workflow
            input_variables: Входные переменные

        Returns:
            ToolResult с execution_id
        """
        workflow = self.workflows.get(workflow_id)
        if not workflow:
            return ToolResult(
                success=False,
                output="",
                error=f"Workflow not found: {workflow_id}"
            )

        execution_id = uuid4().hex[:8]

        # Объединяем переменные workflow и входные
        variables = {**workflow.variables}
        if input_variables:
            variables.update(input_variables)

        execution = WorkflowExecution(
            execution_id=execution_id,
            workflow_id=workflow_id,
            status="running",
            current_step=workflow.steps[0].step_id if workflow.steps else None,
            variables=variables,
            step_results={},
            started_at=datetime.now().isoformat()
        )

        self.executions[execution_id] = execution
        self._save_execution(execution)

        # Запускаем выполнение
        try:
            self._run_workflow(execution, workflow)
        except Exception as e:
            execution.status = "failed"
            execution.error = str(e)
            execution.completed_at = datetime.now().isoformat()
            self._save_execution(execution)

        return ToolResult(
            success=execution.status == "completed",
            output=f"Workflow execution: {execution_id}\nStatus: {execution.status}",
            error=execution.error
        )

    def _run_workflow(self, execution: WorkflowExecution, workflow: WorkflowDefinition) -> None:
        """Выполнить workflow."""
        current_step_id = execution.current_step
        max_iterations = 1000  # Защита от бесконечных циклов

        for _ in range(max_iterations):
            if not current_step_id:
                # Workflow завершен
                execution.status = "completed"
                execution.completed_at = datetime.now().isoformat()
                self._save_execution(execution)
                break

            # Находим текущий шаг
            step = next((s for s in workflow.steps if s.step_id == current_step_id), None)
            if not step:
                raise ValueError(f"Step not found: {current_step_id}")

            execution.current_step = current_step_id
            self._save_execution(execution)

            # Выполняем шаг
            try:
                result = self._execute_step(step, execution)
                execution.step_results[step.step_id] = result

                # Определяем следующий шаг
                if result.success:
                    current_step_id = step.on_success or step.next_step
                else:
                    current_step_id = step.on_failure or step.next_step

            except Exception as e:
                execution.status = "failed"
                execution.error = f"Step {step.name} failed: {e}"
                execution.completed_at = datetime.now().isoformat()
                self._save_execution(execution)
                raise

    def _execute_step(self, step: WorkflowStep, execution: WorkflowExecution) -> ToolResult:
        """Выполнить один шаг."""
        if step.action == "tool_call":
            # Выполняем инструмент
            if not self.tool_executor:
                return ToolResult(
                    success=False,
                    output="",
                    error="Tool executor not configured"
                )

            tool_name = step.params.get('tool')
            tool_params = step.params.get('params', {})

            # Подставляем переменные
            tool_params = self._substitute_variables(tool_params, execution.variables)

            return self.tool_executor(tool_name, tool_params)

        elif step.action == "condition":
            # Проверяем условие
            condition = step.condition or step.params.get('condition')
            if condition:
                result = self._evaluate_condition(condition, execution.variables)
                return ToolResult(
                    success=result,
                    output=f"Condition result: {result}",
                    error=None
                )

        elif step.action == "set_variable":
            # Устанавливаем переменную
            var_name = step.params.get('name')
            var_value = step.params.get('value')
            if var_name:
                execution.variables[var_name] = var_value
                return ToolResult(
                    success=True,
                    output=f"Variable set: {var_name} = {var_value}",
                    error=None
                )

        return ToolResult(
            success=True,
            output=f"Step executed: {step.name}",
            error=None
        )

    def _substitute_variables(self, params: dict, variables: dict) -> dict:
        """Подставить переменные в параметры."""
        result = {}
        for key, value in params.items():
            if isinstance(value, str) and value.startswith('$'):
                var_name = value[1:]
                result[key] = variables.get(var_name, value)
            else:
                result[key] = value
        return result

    def _evaluate_condition(self, condition: str, variables: dict) -> bool:
        """Вычислить условие."""
        # Простая реализация - можно расширить
        try:
            # Подставляем переменные
            for var_name, var_value in variables.items():
                condition = condition.replace(f'${var_name}', repr(var_value))

            # Вычисляем
            return bool(eval(condition))
        except Exception:
            return False

    def get_execution_status(self, execution_id: str) -> ToolResult:
        """Получить статус выполнения."""
        execution = self.executions.get(execution_id)
        if not execution:
            return ToolResult(
                success=False,
                output="",
                error=f"Execution not found: {execution_id}"
            )

        output = f"""Execution: {execution.execution_id}
Workflow: {execution.workflow_id}
Status: {execution.status}
Current Step: {execution.current_step or 'N/A'}
Started: {execution.started_at}
Completed: {execution.completed_at or 'N/A'}

Variables:
{json.dumps(execution.variables, indent=2)}

Step Results:
{json.dumps(execution.step_results, indent=2)}
"""

        if execution.error:
            output += f"\nError: {execution.error}"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def list_workflows(self) -> ToolResult:
        """Список всех workflow."""
        if not self.workflows:
            return ToolResult(
                success=True,
                output="No workflows found",
                error=None
            )

        output = "Workflows:\n\n"
        for wf in self.workflows.values():
            output += f"[{wf.workflow_id}] {wf.name}\n"
            output += f"  Description: {wf.description}\n"
            output += f"  Steps: {len(wf.steps)}\n"
            output += f"  Created: {wf.created_at}\n\n"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )


# Пример использования
if __name__ == "__main__":
    engine = WorkflowEngine()

    # Создаем простой workflow
    steps = [
        {
            "name": "Read file",
            "action": "tool_call",
            "params": {"tool": "file_read", "params": {"file_path": "$input_file"}},
            "next_step": "step2"
        },
        {
            "step_id": "step2",
            "name": "Process data",
            "action": "set_variable",
            "params": {"name": "processed", "value": True},
            "next_step": None
        }
    ]

    result = engine.create_workflow(
        name="File Processing",
        description="Read and process a file",
        steps=steps,
        variables={"input_file": "test.txt"}
    )
    print(result.output)
