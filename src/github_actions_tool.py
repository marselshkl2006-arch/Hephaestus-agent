"""
GitHub Actions Tool - работа с GitHub Actions workflow.
Запуск, мониторинг, управление CI/CD pipeline.
"""
from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

from .real_tools import ToolResult


@dataclass
class WorkflowRun:
    """Информация о запуске workflow."""
    run_id: int
    name: str
    status: str
    conclusion: str | None
    created_at: str
    updated_at: str
    html_url: str


@dataclass
class WorkflowJob:
    """Информация о job в workflow."""
    job_id: int
    name: str
    status: str
    conclusion: str | None
    started_at: str | None
    completed_at: str | None


class GithubActionsTool:
    """Инструмент для работы с GitHub Actions."""

    def __init__(self, workspace_root: str = "."):
        self.workspace_root = Path(workspace_root)

    def _run_gh_command(self, args: list[str]) -> tuple[bool, str, str]:
        """
        Выполнить команду gh CLI.

        Args:
            args: Аргументы команды

        Returns:
            (success, stdout, stderr)
        """
        try:
            result = subprocess.run(
                ["gh"] + args,
                cwd=self.workspace_root,
                capture_output=True,
                text=True,
                timeout=30
            )
            return result.returncode == 0, result.stdout, result.stderr
        except subprocess.TimeoutExpired:
            return False, "", "Command timeout"
        except FileNotFoundError:
            return False, "", "gh CLI not installed"
        except Exception as e:
            return False, "", str(e)

    def list_workflows(self) -> ToolResult:
        """
        Список всех workflow в репозитории.

        Returns:
            ToolResult со списком workflow
        """
        success, stdout, stderr = self._run_gh_command([
            "workflow", "list", "--json", "name,id,state,path"
        ])

        if not success:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to list workflows: {stderr}"
            )

        try:
            workflows = json.loads(stdout)
            if not workflows:
                return ToolResult(
                    success=True,
                    output="No workflows found",
                    error=None
                )

            output = "GitHub Actions Workflows:\n\n"
            for wf in workflows:
                output += f"[{wf['id']}] {wf['name']}\n"
                output += f"  State: {wf['state']}\n"
                output += f"  Path: {wf['path']}\n\n"

            return ToolResult(
                success=True,
                output=output,
                error=None
            )

        except json.JSONDecodeError:
            return ToolResult(
                success=False,
                output="",
                error="Failed to parse workflow list"
            )

    def trigger_workflow(
        self,
        workflow: str,
        ref: str = "main",
        inputs: dict[str, str] | None = None
    ) -> ToolResult:
        """
        Запустить workflow.

        Args:
            workflow: Имя или ID workflow
            ref: Ветка для запуска (default: main)
            inputs: Входные параметры для workflow

        Returns:
            ToolResult
        """
        args = ["workflow", "run", workflow, "--ref", ref]

        if inputs:
            for key, value in inputs.items():
                args.extend(["-f", f"{key}={value}"])

        success, stdout, stderr = self._run_gh_command(args)

        if not success:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to trigger workflow: {stderr}"
            )

        return ToolResult(
            success=True,
            output=f"Workflow triggered: {workflow}\nRef: {ref}\n{stdout}",
            error=None
        )

    def list_runs(
        self,
        workflow: str | None = None,
        limit: int = 10,
        status: str | None = None
    ) -> ToolResult:
        """
        Список запусков workflow.

        Args:
            workflow: Фильтр по workflow (опционально)
            limit: Максимальное количество результатов
            status: Фильтр по статусу (completed, in_progress, queued)

        Returns:
            ToolResult со списком запусков
        """
        args = ["run", "list", "--json", "databaseId,name,status,conclusion,createdAt,updatedAt,url", "--limit", str(limit)]

        if workflow:
            args.extend(["--workflow", workflow])

        if status:
            args.extend(["--status", status])

        success, stdout, stderr = self._run_gh_command(args)

        if not success:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to list runs: {stderr}"
            )

        try:
            runs = json.loads(stdout)
            if not runs:
                return ToolResult(
                    success=True,
                    output="No workflow runs found",
                    error=None
                )

            output = "Workflow Runs:\n\n"
            for run in runs:
                status_icon = "✓" if run['conclusion'] == "success" else "✗" if run['conclusion'] == "failure" else "⋯"
                output += f"{status_icon} [{run['databaseId']}] {run['name']}\n"
                output += f"  Status: {run['status']}"
                if run['conclusion']:
                    output += f" ({run['conclusion']})"
                output += "\n"
                output += f"  Created: {run['createdAt']}\n"
                output += f"  URL: {run['url']}\n\n"

            return ToolResult(
                success=True,
                output=output,
                error=None
            )

        except json.JSONDecodeError:
            return ToolResult(
                success=False,
                output="",
                error="Failed to parse runs list"
            )

    def get_run_status(self, run_id: str) -> ToolResult:
        """
        Получить статус конкретного запуска.

        Args:
            run_id: ID запуска

        Returns:
            ToolResult со статусом
        """
        success, stdout, stderr = self._run_gh_command([
            "run", "view", run_id, "--json", "name,status,conclusion,createdAt,updatedAt,url,jobs"
        ])

        if not success:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to get run status: {stderr}"
            )

        try:
            run = json.loads(stdout)

            output = f"Workflow Run: {run['name']}\n"
            output += f"ID: {run_id}\n"
            output += f"Status: {run['status']}\n"
            if run['conclusion']:
                output += f"Conclusion: {run['conclusion']}\n"
            output += f"Created: {run['createdAt']}\n"
            output += f"Updated: {run['updatedAt']}\n"
            output += f"URL: {run['url']}\n\n"

            if 'jobs' in run:
                output += "Jobs:\n"
                for job in run['jobs']:
                    status_icon = "✓" if job.get('conclusion') == "success" else "✗" if job.get('conclusion') == "failure" else "⋯"
                    output += f"  {status_icon} {job['name']}: {job['status']}"
                    if job.get('conclusion'):
                        output += f" ({job['conclusion']})"
                    output += "\n"

            return ToolResult(
                success=True,
                output=output,
                error=None
            )

        except json.JSONDecodeError:
            return ToolResult(
                success=False,
                output="",
                error="Failed to parse run status"
            )

    def get_run_logs(self, run_id: str) -> ToolResult:
        """
        Получить логи запуска.

        Args:
            run_id: ID запуска

        Returns:
            ToolResult с логами
        """
        success, stdout, stderr = self._run_gh_command([
            "run", "view", run_id, "--log"
        ])

        if not success:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to get logs: {stderr}"
            )

        return ToolResult(
            success=True,
            output=stdout,
            error=None
        )

    def cancel_run(self, run_id: str) -> ToolResult:
        """
        Отменить запуск workflow.

        Args:
            run_id: ID запуска

        Returns:
            ToolResult
        """
        success, stdout, stderr = self._run_gh_command([
            "run", "cancel", run_id
        ])

        if not success:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to cancel run: {stderr}"
            )

        return ToolResult(
            success=True,
            output=f"Run cancelled: {run_id}\n{stdout}",
            error=None
        )

    def rerun_workflow(self, run_id: str, failed_only: bool = False) -> ToolResult:
        """
        Перезапустить workflow.

        Args:
            run_id: ID запуска
            failed_only: Перезапустить только failed jobs

        Returns:
            ToolResult
        """
        args = ["run", "rerun", run_id]
        if failed_only:
            args.append("--failed")

        success, stdout, stderr = self._run_gh_command(args)

        if not success:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to rerun workflow: {stderr}"
            )

        return ToolResult(
            success=True,
            output=f"Workflow rerun triggered: {run_id}\n{stdout}",
            error=None
        )

    def watch_run(self, run_id: str) -> ToolResult:
        """
        Следить за выполнением workflow в реальном времени.

        Args:
            run_id: ID запуска

        Returns:
            ToolResult с финальным статусом
        """
        success, stdout, stderr = self._run_gh_command([
            "run", "watch", run_id
        ])

        if not success:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to watch run: {stderr}"
            )

        return ToolResult(
            success=True,
            output=stdout,
            error=None
        )

    def create_workflow_file(
        self,
        name: str,
        triggers: list[str],
        jobs: dict[str, Any]
    ) -> ToolResult:
        """
        Создать файл workflow.

        Args:
            name: Название workflow
            triggers: Список триггеров (push, pull_request, etc.)
            jobs: Определение jobs

        Returns:
            ToolResult
        """
        workflows_dir = self.workspace_root / ".github" / "workflows"
        workflows_dir.mkdir(parents=True, exist_ok=True)

        # Формируем YAML
        workflow = {
            "name": name,
            "on": triggers,
            "jobs": jobs
        }

        # Конвертируем в YAML (простая реализация)
        yaml_content = f"name: {name}\n\n"
        yaml_content += "on:\n"
        for trigger in triggers:
            yaml_content += f"  - {trigger}\n"
        yaml_content += "\njobs:\n"

        # Сохраняем
        filename = name.lower().replace(" ", "-") + ".yml"
        file_path = workflows_dir / filename

        try:
            with open(file_path, 'w', encoding='utf-8') as f:
                f.write(yaml_content)

            return ToolResult(
                success=True,
                output=f"Workflow file created: {file_path}",
                error=None
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to create workflow file: {e}"
            )


# Пример использования
if __name__ == "__main__":
    tool = GithubActionsTool()

    # Список workflow
    result = tool.list_workflows()
    print(result.output if result.success else result.error)
