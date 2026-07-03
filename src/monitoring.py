"""
Monitoring System - система мониторинга с метриками и алертами.
Отслеживает использование токенов, время выполнения, ошибки.
"""
from __future__ import annotations

import json
import time
from collections import defaultdict
from dataclasses import dataclass, asdict
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Callable

from .real_tools import ToolResult


@dataclass
class Metric:
    """Метрика."""
    name: str
    value: float
    timestamp: float
    tags: dict[str, str]


@dataclass
class Alert:
    """Алерт."""
    alert_id: str
    name: str
    condition: str
    threshold: float
    current_value: float
    triggered_at: float
    resolved_at: float | None
    severity: str  # info, warning, error, critical


class MetricsCollector:
    """Сборщик метрик."""

    def __init__(self, workspace_root: str = "."):
        self.workspace_root = Path(workspace_root)
        self.metrics_dir = Path.home() / ".claude_code" / "metrics"
        self.metrics_dir.mkdir(parents=True, exist_ok=True)

        self.metrics: list[Metric] = []
        self.aggregates: dict[str, dict[str, float]] = defaultdict(lambda: {
            "count": 0,
            "sum": 0,
            "min": float('inf'),
            "max": float('-inf'),
            "avg": 0
        })

    def record(self, name: str, value: float, tags: dict[str, str] | None = None) -> None:
        """
        Записать метрику.

        Args:
            name: Название метрики
            value: Значение
            tags: Теги для фильтрации
        """
        metric = Metric(
            name=name,
            value=value,
            timestamp=time.time(),
            tags=tags or {}
        )

        self.metrics.append(metric)

        # Обновляем агрегаты
        agg = self.aggregates[name]
        agg["count"] += 1
        agg["sum"] += value
        agg["min"] = min(agg["min"], value)
        agg["max"] = max(agg["max"], value)
        agg["avg"] = agg["sum"] / agg["count"]

        # Сохраняем периодически
        if len(self.metrics) % 100 == 0:
            self._flush_metrics()

    def _flush_metrics(self) -> None:
        """Сохранить метрики на диск."""
        if not self.metrics:
            return

        try:
            date_str = datetime.now().strftime("%Y-%m-%d")
            metrics_file = self.metrics_dir / f"metrics_{date_str}.jsonl"

            with open(metrics_file, 'a', encoding='utf-8') as f:
                for metric in self.metrics:
                    f.write(json.dumps(asdict(metric)) + "\n")

            self.metrics.clear()

        except Exception as e:
            print(f"Error flushing metrics: {e}")

    def get_aggregate(self, name: str) -> dict[str, float]:
        """Получить агрегированные данные по метрике."""
        return dict(self.aggregates.get(name, {}))

    def get_metrics(
        self,
        name: str | None = None,
        since: float | None = None,
        tags: dict[str, str] | None = None
    ) -> list[Metric]:
        """
        Получить метрики с фильтрацией.

        Args:
            name: Фильтр по имени
            since: Фильтр по времени (timestamp)
            tags: Фильтр по тегам

        Returns:
            Список метрик
        """
        metrics = self.metrics

        if name:
            metrics = [m for m in metrics if m.name == name]

        if since:
            metrics = [m for m in metrics if m.timestamp >= since]

        if tags:
            metrics = [
                m for m in metrics
                if all(m.tags.get(k) == v for k, v in tags.items())
            ]

        return metrics


class AlertManager:
    """Менеджер алертов."""

    def __init__(self, metrics_collector: MetricsCollector):
        self.metrics_collector = metrics_collector
        self.alerts: dict[str, Alert] = {}
        self.alert_handlers: list[Callable[[Alert], None]] = []
        self.rules: list[dict[str, Any]] = []

    def add_rule(
        self,
        name: str,
        metric_name: str,
        condition: str,
        threshold: float,
        severity: str = "warning"
    ) -> None:
        """
        Добавить правило алерта.

        Args:
            name: Название алерта
            metric_name: Метрика для проверки
            condition: Условие (gt, lt, eq)
            threshold: Порог
            severity: Серьезность
        """
        rule = {
            "name": name,
            "metric_name": metric_name,
            "condition": condition,
            "threshold": threshold,
            "severity": severity
        }
        self.rules.append(rule)

    def add_handler(self, handler: Callable[[Alert], None]) -> None:
        """Добавить обработчик алертов."""
        self.alert_handlers.append(handler)

    def check_rules(self) -> list[Alert]:
        """
        Проверить все правила.

        Returns:
            Список сработавших алертов
        """
        triggered = []

        for rule in self.rules:
            metric_name = rule["metric_name"]
            agg = self.metrics_collector.get_aggregate(metric_name)

            if not agg or agg["count"] == 0:
                continue

            current_value = agg["avg"]
            threshold = rule["threshold"]
            condition = rule["condition"]

            # Проверяем условие
            should_trigger = False
            if condition == "gt" and current_value > threshold:
                should_trigger = True
            elif condition == "lt" and current_value < threshold:
                should_trigger = True
            elif condition == "eq" and abs(current_value - threshold) < 0.001:
                should_trigger = True

            if should_trigger:
                alert_id = f"{rule['name']}_{int(time.time())}"

                # Проверяем не сработал ли уже
                if alert_id not in self.alerts:
                    alert = Alert(
                        alert_id=alert_id,
                        name=rule["name"],
                        condition=f"{metric_name} {condition} {threshold}",
                        threshold=threshold,
                        current_value=current_value,
                        triggered_at=time.time(),
                        resolved_at=None,
                        severity=rule["severity"]
                    )

                    self.alerts[alert_id] = alert
                    triggered.append(alert)

                    # Вызываем обработчики
                    for handler in self.alert_handlers:
                        try:
                            handler(alert)
                        except Exception as e:
                            print(f"Alert handler error: {e}")

        return triggered

    def resolve_alert(self, alert_id: str) -> bool:
        """Разрешить алерт."""
        alert = self.alerts.get(alert_id)
        if alert and alert.resolved_at is None:
            alert.resolved_at = time.time()
            return True
        return False

    def get_active_alerts(self) -> list[Alert]:
        """Получить активные алерты."""
        return [a for a in self.alerts.values() if a.resolved_at is None]


class MonitoringSystem:
    """Система мониторинга."""

    def __init__(self, workspace_root: str = "."):
        self.workspace_root = Path(workspace_root)
        self.metrics_collector = MetricsCollector(workspace_root)
        self.alert_manager = AlertManager(self.metrics_collector)

        # Настраиваем стандартные алерты
        self._setup_default_alerts()

    def _setup_default_alerts(self) -> None:
        """Настроить алерты по умолчанию."""
        # Высокое использование токенов
        self.alert_manager.add_rule(
            name="High Token Usage",
            metric_name="llm.tokens.total",
            condition="gt",
            threshold=10000,
            severity="warning"
        )

        # Медленное выполнение инструментов
        self.alert_manager.add_rule(
            name="Slow Tool Execution",
            metric_name="tool.execution.duration_ms",
            condition="gt",
            threshold=5000,
            severity="warning"
        )

        # Высокий процент ошибок
        self.alert_manager.add_rule(
            name="High Error Rate",
            metric_name="tool.execution.error_rate",
            condition="gt",
            threshold=0.1,
            severity="error"
        )

    def record_llm_request(
        self,
        provider: str,
        model: str,
        tokens: int,
        duration_ms: float
    ) -> None:
        """Записать метрики LLM запроса."""
        self.metrics_collector.record(
            "llm.request.count",
            1,
            {"provider": provider, "model": model}
        )

        self.metrics_collector.record(
            "llm.tokens.total",
            tokens,
            {"provider": provider, "model": model}
        )

        self.metrics_collector.record(
            "llm.request.duration_ms",
            duration_ms,
            {"provider": provider, "model": model}
        )

    def record_tool_execution(
        self,
        tool_name: str,
        success: bool,
        duration_ms: float
    ) -> None:
        """Записать метрики выполнения инструмента."""
        self.metrics_collector.record(
            "tool.execution.count",
            1,
            {"tool": tool_name, "success": str(success)}
        )

        self.metrics_collector.record(
            "tool.execution.duration_ms",
            duration_ms,
            {"tool": tool_name}
        )

        if not success:
            self.metrics_collector.record(
                "tool.execution.errors",
                1,
                {"tool": tool_name}
            )

    def get_dashboard(self) -> ToolResult:
        """
        Получить дашборд с метриками.

        Returns:
            ToolResult с дашбордом
        """
        output = "📊 Monitoring Dashboard\n\n"

        # LLM метрики
        llm_tokens = self.metrics_collector.get_aggregate("llm.tokens.total")
        llm_requests = self.metrics_collector.get_aggregate("llm.request.count")
        llm_duration = self.metrics_collector.get_aggregate("llm.request.duration_ms")

        output += "## LLM Metrics\n"
        if llm_requests.get("count", 0) > 0:
            output += f"- Requests: {llm_requests['count']}\n"
            output += f"- Total Tokens: {llm_tokens.get('sum', 0):.0f}\n"
            output += f"- Avg Tokens/Request: {llm_tokens.get('avg', 0):.0f}\n"
            output += f"- Avg Duration: {llm_duration.get('avg', 0):.0f}ms\n"
        else:
            output += "- No LLM requests yet\n"

        output += "\n"

        # Tool метрики
        tool_count = self.metrics_collector.get_aggregate("tool.execution.count")
        tool_duration = self.metrics_collector.get_aggregate("tool.execution.duration_ms")
        tool_errors = self.metrics_collector.get_aggregate("tool.execution.errors")

        output += "## Tool Metrics\n"
        if tool_count.get("count", 0) > 0:
            output += f"- Executions: {tool_count['count']}\n"
            output += f"- Avg Duration: {tool_duration.get('avg', 0):.0f}ms\n"
            output += f"- Errors: {tool_errors.get('sum', 0):.0f}\n"
            error_rate = tool_errors.get('sum', 0) / tool_count['count']
            output += f"- Error Rate: {error_rate:.1%}\n"
        else:
            output += "- No tool executions yet\n"

        output += "\n"

        # Активные алерты
        active_alerts = self.alert_manager.get_active_alerts()
        output += f"## Active Alerts ({len(active_alerts)})\n"
        if active_alerts:
            for alert in active_alerts:
                severity_icon = {
                    "info": "ℹ️",
                    "warning": "⚠️",
                    "error": "❌",
                    "critical": "🔥"
                }.get(alert.severity, "⚠️")

                output += f"{severity_icon} {alert.name}\n"
                output += f"   {alert.condition}\n"
                output += f"   Current: {alert.current_value:.2f}, Threshold: {alert.threshold:.2f}\n"
        else:
            output += "- No active alerts\n"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def check_health(self) -> ToolResult:
        """
        Проверить здоровье системы.

        Returns:
            ToolResult со статусом
        """
        # Проверяем алерты
        self.alert_manager.check_rules()
        active_alerts = self.alert_manager.get_active_alerts()

        # Определяем статус
        if not active_alerts:
            status = "✅ Healthy"
            severity = "info"
        elif any(a.severity == "critical" for a in active_alerts):
            status = "🔥 Critical"
            severity = "critical"
        elif any(a.severity == "error" for a in active_alerts):
            status = "❌ Error"
            severity = "error"
        else:
            status = "⚠️  Warning"
            severity = "warning"

        output = f"System Status: {status}\n\n"
        output += f"Active Alerts: {len(active_alerts)}\n"

        return ToolResult(
            success=severity != "critical",
            output=output,
            error=None
        )


# Пример использования
if __name__ == "__main__":
    monitoring = MonitoringSystem()

    # Записываем метрики
    monitoring.record_llm_request("ollama", "llama3.2:1b", 150, 1200)
    monitoring.record_tool_execution("file_read", True, 50)
    monitoring.record_tool_execution("bash", False, 2000)

    # Дашборд
    result = monitoring.get_dashboard()
    print(result.output)

    # Проверка здоровья
    result = monitoring.check_health()
    print("\n" + result.output)
