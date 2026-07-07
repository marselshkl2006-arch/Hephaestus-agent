"""
System Tools - инструменты для мониторинга системы.
Мониторинг CPU, RAM, Disk, процессов, портов.
"""
from __future__ import annotations

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str = ""


class SystemMonitorTool:
    """Мониторинг системных ресурсов."""

    def monitor(
        self,
        resource: str = "all",
        detailed: bool = False
    ) -> ToolResult:
        """Мониторинг системы.

        Args:
            resource: Что мониторить (all, cpu, memory, disk, processes, network)
            detailed: Подробная информация

        Returns:
            ToolResult с информацией о системе
        """
        if resource == "all":
            return self._monitor_all(detailed)
        elif resource == "cpu":
            return self._monitor_cpu(detailed)
        elif resource == "memory":
            return self._monitor_memory(detailed)
        elif resource == "disk":
            return self._monitor_disk(detailed)
        elif resource == "processes":
            return self._monitor_processes(detailed)
        elif resource == "network":
            return self._monitor_network(detailed)
        else:
            return ToolResult(
                success=False,
                output="",
                error=f"Unknown resource: {resource}. Use: all, cpu, memory, disk, processes, network"
            )

    def _monitor_all(self, detailed: bool) -> ToolResult:
        """Мониторинг всех ресурсов."""
        output = "=== System Monitor ===\n\n"

        # CPU
        cpu_result = self._monitor_cpu(False)
        if cpu_result.success:
            output += "CPU:\n" + cpu_result.output + "\n\n"

        # Memory
        mem_result = self._monitor_memory(False)
        if mem_result.success:
            output += "Memory:\n" + mem_result.output + "\n\n"

        # Disk
        disk_result = self._monitor_disk(False)
        if disk_result.success:
            output += "Disk:\n" + disk_result.output + "\n\n"

        # Network
        net_result = self._monitor_network(False)
        if net_result.success:
            output += "Network:\n" + net_result.output + "\n"

        return ToolResult(success=True, output=output)

    def _monitor_cpu(self, detailed: bool) -> ToolResult:
        """Мониторинг CPU."""
        try:
            # Используем top для получения CPU usage
            result = subprocess.run(
                ["top", "-bn1"],
                capture_output=True,
                text=True,
                timeout=5
            )

            if result.returncode != 0:
                return ToolResult(success=False, output="", error="Failed to get CPU info")

            # Парсим вывод top
            lines = result.stdout.split('\n')
            cpu_line = None
            for line in lines:
                if 'Cpu(s)' in line or '%Cpu' in line:
                    cpu_line = line
                    break

            if cpu_line:
                output = f"  {cpu_line.strip()}\n"

                # Количество ядер
                try:
                    with open('/proc/cpuinfo', 'r') as f:
                        cores = f.read().count('processor')
                    output += f"  Cores: {cores}\n"
                except:
                    pass

                # Load average
                try:
                    with open('/proc/loadavg', 'r') as f:
                        load = f.read().strip().split()[:3]
                    output += f"  Load average: {' '.join(load)}\n"
                except:
                    pass

                return ToolResult(success=True, output=output)
            else:
                return ToolResult(success=False, output="", error="Could not parse CPU info")

        except Exception as e:
            return ToolResult(success=False, output="", error=f"Error: {str(e)}")

    def _monitor_memory(self, detailed: bool) -> ToolResult:
        """Мониторинг памяти."""
        try:
            # Используем free для получения информации о памяти
            result = subprocess.run(
                ["free", "-h"],
                capture_output=True,
                text=True,
                timeout=5
            )

            if result.returncode != 0:
                return ToolResult(success=False, output="", error="Failed to get memory info")

            output = ""
            lines = result.stdout.strip().split('\n')
            for line in lines:
                output += f"  {line}\n"

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(success=False, output="", error=f"Error: {str(e)}")

    def _monitor_disk(self, detailed: bool) -> ToolResult:
        """Мониторинг дисков."""
        try:
            # Используем df для получения информации о дисках
            result = subprocess.run(
                ["df", "-h"],
                capture_output=True,
                text=True,
                timeout=5
            )

            if result.returncode != 0:
                return ToolResult(success=False, output="", error="Failed to get disk info")

            output = ""
            lines = result.stdout.strip().split('\n')
            # Показываем только основные разделы
            for line in lines:
                if line.startswith('Filesystem') or '/' in line:
                    output += f"  {line}\n"

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(success=False, output="", error=f"Error: {str(e)}")

    def _monitor_processes(self, detailed: bool) -> ToolResult:
        """Мониторинг процессов."""
        try:
            # Топ процессов по CPU
            result = subprocess.run(
                ["ps", "aux", "--sort=-%cpu"],
                capture_output=True,
                text=True,
                timeout=5
            )

            if result.returncode != 0:
                return ToolResult(success=False, output="", error="Failed to get process info")

            lines = result.stdout.strip().split('\n')
            # Показываем заголовок и топ 10 процессов
            output = ""
            for i, line in enumerate(lines):
                if i == 0 or i <= 10:
                    output += f"  {line}\n"
                else:
                    break

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(success=False, output="", error=f"Error: {str(e)}")

    def _monitor_network(self, detailed: bool) -> ToolResult:
        """Мониторинг сети."""
        try:
            # Используем ss для получения информации о портах
            result = subprocess.run(
                ["ss", "-tuln"],
                capture_output=True,
                text=True,
                timeout=5
            )

            if result.returncode != 0:
                # Fallback на netstat
                result = subprocess.run(
                    ["netstat", "-tuln"],
                    capture_output=True,
                    text=True,
                    timeout=5
                )

            if result.returncode != 0:
                return ToolResult(success=False, output="", error="Failed to get network info")

            lines = result.stdout.strip().split('\n')
            output = "  Listening ports:\n"
            
            # Показываем только LISTEN порты
            count = 0
            for line in lines:
                if 'LISTEN' in line or 'State' in line:
                    output += f"    {line}\n"
                    count += 1
                    if count > 15:  # Ограничиваем вывод
                        output += "    ...\n"
                        break

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(success=False, output="", error=f"Error: {str(e)}")


class LogAnalyzerTool:
    """Анализ логов."""

    def analyze(
        self,
        log_file: str,
        pattern: str | None = None,
        level: str | None = None,
        tail: int = 100
    ) -> ToolResult:
        """Анализ лог файла.

        Args:
            log_file: Путь к лог файлу
            pattern: Паттерн для поиска (regex)
            level: Уровень логов (ERROR, WARN, INFO)
            tail: Количество последних строк

        Returns:
            ToolResult с результатами анализа
        """
        try:
            log_path = Path(log_file)
            if not log_path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Log file not found: {log_file}"
                )

            # Читаем последние N строк
            result = subprocess.run(
                ["tail", "-n", str(tail), str(log_path)],
                capture_output=True,
                text=True,
                timeout=10
            )

            if result.returncode != 0:
                return ToolResult(success=False, output="", error="Failed to read log file")

            lines = result.stdout.strip().split('\n')

            # Фильтруем по уровню
            if level:
                lines = [line for line in lines if level.upper() in line.upper()]

            # Фильтруем по паттерну
            if pattern:
                import re
                regex = re.compile(pattern, re.IGNORECASE)
                lines = [line for line in lines if regex.search(line)]

            # Статистика
            output = f"=== Log Analysis: {log_file} ===\n\n"
            output += f"Total lines analyzed: {len(lines)}\n"

            # Подсчёт по уровням
            errors = sum(1 for line in lines if 'ERROR' in line.upper())
            warnings = sum(1 for line in lines if 'WARN' in line.upper())
            info = sum(1 for line in lines if 'INFO' in line.upper())

            output += f"Errors: {errors}\n"
            output += f"Warnings: {warnings}\n"
            output += f"Info: {info}\n\n"

            # Показываем строки
            output += "Recent entries:\n"
            for line in lines[-20:]:  # Последние 20 строк
                output += f"  {line}\n"

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(success=False, output="", error=f"Error: {str(e)}")
