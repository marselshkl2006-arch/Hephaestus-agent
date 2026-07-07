"""
Tool Execution Pipeline - middleware для прозрачности выполнения команд.
Обрабатывает вход и выход из execute_tool без изменения основного кода.
"""
from __future__ import annotations

from typing import Callable, Any, Optional
from dataclasses import dataclass

from .progress_display import progress_display, ActionType, ActionStatus
from .result_verifier import result_verifier, VerificationResult


@dataclass
class ToolExecutionContext:
    """Контекст выполнения инструмента."""
    tool_name: str
    params: dict
    action_type: ActionType
    should_verify: bool = False
    verification_params: Optional[dict] = None


class ToolExecutionPipeline:
    """
    Pipeline для выполнения инструментов с прозрачностью и верификацией.

    Архитектура:
    1. Before execution: показываем что начали
    2. Execute: вызываем оригинальную функцию
    3. After execution: проверяем результат и показываем статус

    В C# это будет:
    - Middleware pipeline (ASP.NET Core style)
    - Или Decorator через attributes
    - Или Interceptor pattern
    """

    def __init__(self):
        self.progress = progress_display
        self.verifier = result_verifier

        # Маппинг инструментов на типы действий
        self.action_type_map = {
            # File operations
            "file_read": ActionType.READING,
            "file_write": ActionType.WRITING,
            "file_edit": ActionType.EDITING,
            "file_prepend": ActionType.EDITING,
            "file_delete": ActionType.EDITING,
            "file_move": ActionType.EDITING,
            "file_copy": ActionType.EDITING,
            "file_search": ActionType.SEARCHING,
            "file_list": ActionType.SEARCHING,

            # Bash
            "bash": ActionType.EXECUTING,

            # Git
            "git_status": ActionType.ANALYZING,
            "git_diff": ActionType.ANALYZING,
            "git_commit": ActionType.CONFIGURING,
            "git_push": ActionType.DEPLOYING,
            "git_pull": ActionType.CONFIGURING,
            "git_branch": ActionType.CONFIGURING,
            "git_checkout": ActionType.CONFIGURING,
            "git_log": ActionType.ANALYZING,

            # Docker
            "docker_run": ActionType.DEPLOYING,
            "docker_stop": ActionType.CONFIGURING,
            "docker_ps": ActionType.MONITORING,
            "docker_logs": ActionType.MONITORING,
            "docker_build": ActionType.BUILDING,

            # Database
            "db_query": ActionType.SEARCHING,
            "db_execute": ActionType.EXECUTING,
            "db_backup": ActionType.CONFIGURING,

            # Package management
            "package_install": ActionType.INSTALLING,
            "package_search": ActionType.SEARCHING,

            # Testing
            "test_run": ActionType.TESTING,
            "test_generator": ActionType.CONFIGURING,

            # Web
            "web_fetch": ActionType.SEARCHING,
            "web_search": ActionType.SEARCHING,
            "http_request": ActionType.EXECUTING,

            # Analysis
            "project_analyze": ActionType.ANALYZING,
            "project_analyzer": ActionType.ANALYZING,
        }

        # Инструменты которые требуют верификации
        self.verification_map = {
            "package_install": self._verify_package_install,
            "docker_run": self._verify_docker_run,
            "file_write": self._verify_file_write,
        }

    def execute(
        self,
        tool_name: str,
        params: dict,
        executor: Callable[[], Any]
    ) -> Any:
        """
        Выполнить инструмент через pipeline.

        Args:
            tool_name: Имя инструмента
            params: Параметры инструмента
            executor: Функция которая выполняет инструмент

        Returns:
            Результат выполнения (ToolResult)
        """
        # 1. Before execution - показываем что начали
        context = self._create_context(tool_name, params)
        self._before_execution(context)

        # 2. Execute - вызываем оригинальную функцию
        result = executor()

        # 3. After execution - проверяем и показываем результат
        self._after_execution(context, result)

        return result

    def _create_context(self, tool_name: str, params: dict) -> ToolExecutionContext:
        """Создать контекст выполнения."""
        action_type = self.action_type_map.get(tool_name, ActionType.EXECUTING)
        should_verify = tool_name in self.verification_map

        return ToolExecutionContext(
            tool_name=tool_name,
            params=params,
            action_type=action_type,
            should_verify=should_verify,
            verification_params=params if should_verify else None
        )

    def _before_execution(self, context: ToolExecutionContext):
        """Действия перед выполнением."""
        # Формируем описание действия
        description = self._format_description(context.tool_name, context.params)

        # Показываем начало действия
        self.progress.start_action(context.action_type, description)

        # Добавляем детали если есть
        details = self._extract_details(context.tool_name, context.params)
        for detail in details:
            self.progress.add_detail(detail)

    def _after_execution(self, context: ToolExecutionContext, result: Any):
        """Действия после выполнения."""
        # Проверяем успешность
        success = getattr(result, 'success', True)

        if success:
            # Если нужна верификация - проверяем
            if context.should_verify:
                verification = self._verify_result(context)
                if verification:
                    if verification.verified:
                        self.progress.finish_action(True, verification.message)
                    else:
                        self.progress.update_status(ActionStatus.WARNING, verification.message)
                        if verification.details:
                            self.progress.add_detail(verification.details)
                else:
                    self.progress.finish_action(True, "Выполнено")
            else:
                self.progress.finish_action(True, "Выполнено")
        else:
            # Ошибка
            error_msg = getattr(result, 'error', 'Неизвестная ошибка')
            self.progress.finish_action(False, error_msg)

    def _format_description(self, tool_name: str, params: dict) -> str:
        """Форматировать описание действия."""
        # Специальные форматы для разных инструментов
        if tool_name == "file_read":
            return f"файла {params.get('file_path', '?')}"
        elif tool_name == "file_write":
            return f"файла {params.get('file_path', '?')}"
        elif tool_name == "file_edit":
            return f"файла {params.get('file_path', '?')}"
        elif tool_name == "bash":
            cmd = params.get('command', '?')
            # Обрезаем длинные команды
            if len(cmd) > 50:
                cmd = cmd[:50] + "..."
            return f"команды: {cmd}"
        elif tool_name == "package_install":
            pkg = params.get('package') or params.get('packages', '?')
            return f"пакета {pkg}"
        elif tool_name == "docker_run":
            image = params.get('image', '?')
            return f"Docker контейнера: {image}"
        elif tool_name == "test_run":
            path = params.get('path', 'all tests')
            return f"тестов: {path}"
        else:
            return tool_name

    def _extract_details(self, tool_name: str, params: dict) -> list[str]:
        """Извлечь детали для отображения."""
        details = []

        if tool_name == "bash":
            command = params.get('command', '')
            if len(command) > 50:
                details.append(f"Полная команда: {command}")

        elif tool_name == "package_install":
            manager = params.get('manager', 'auto')
            if manager != 'auto':
                details.append(f"Менеджер: {manager}")
            if params.get('dev'):
                details.append("Режим: dev dependency")

        elif tool_name == "docker_run":
            if params.get('ports'):
                details.append(f"Порты: {params['ports']}")
            if params.get('volumes'):
                details.append(f"Volumes: {params['volumes']}")

        return details

    def _verify_result(self, context: ToolExecutionContext) -> Optional[VerificationResult]:
        """Верифицировать результат выполнения."""
        verifier_func = self.verification_map.get(context.tool_name)
        if not verifier_func:
            return None

        return verifier_func(context.verification_params)

    def _verify_package_install(self, params: dict) -> VerificationResult:
        """Проверить установку пакета."""
        packages = params.get('package') or params.get('packages', '')
        return self.verifier.verify_package_install(packages)

    def _verify_docker_run(self, params: dict) -> VerificationResult:
        """Проверить запуск Docker контейнера."""
        container_name = params.get('name')
        return self.verifier.verify_docker_container(container_name)

    def _verify_file_write(self, params: dict) -> VerificationResult:
        """Проверить создание файла."""
        file_path = params.get('file_path', '')
        return self.verifier.verify_file_exists(file_path)


# Глобальный экземпляр pipeline
tool_pipeline = ToolExecutionPipeline()
