"""
Docker Tools - инструменты для управления Docker контейнерами.
Список, запуск, остановка, логи, exec команды.
"""
from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str = ""


class DockerTool:
    """Управление Docker контейнерами."""

    def __init__(self, timeout: int = 30):
        self.timeout = timeout

    def list_containers(self, all_containers: bool = False) -> ToolResult:
        """Список контейнеров.

        Args:
            all_containers: Показать все контейнеры (включая остановленные)

        Returns:
            ToolResult со списком контейнеров
        """
        try:
            cmd = ["docker", "ps", "--format", "{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}"]
            if all_containers:
                cmd.append("-a")

            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self.timeout
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Docker error: {result.stderr}"
                )

            output = "ID\t\tName\t\tStatus\t\tImage\n"
            output += "=" * 80 + "\n"
            output += result.stdout

            return ToolResult(success=True, output=output)

        except FileNotFoundError:
            return ToolResult(
                success=False,
                output="",
                error="Docker not found. Install Docker first."
            )
        except subprocess.TimeoutExpired:
            return ToolResult(
                success=False,
                output="",
                error="Command timeout"
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def list_images(self) -> ToolResult:
        """Список Docker образов."""
        try:
            result = subprocess.run(
                ["docker", "images", "--format", "{{.Repository}}:{{.Tag}}\t{{.ID}}\t{{.Size}}"],
                capture_output=True,
                text=True,
                timeout=self.timeout
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Docker error: {result.stderr}"
                )

            output = "Image\t\t\tID\t\tSize\n"
            output += "=" * 80 + "\n"
            output += result.stdout

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def run_container(
        self,
        image: str,
        name: str | None = None,
        ports: dict[str, str] | None = None,
        env: dict[str, str] | None = None,
        volumes: dict[str, str] | None = None,
        detach: bool = True,
        command: str | None = None
    ) -> ToolResult:
        """Запустить контейнер.

        Args:
            image: Docker образ
            name: Имя контейнера
            ports: Маппинг портов {"8080": "80"}
            env: Переменные окружения {"KEY": "value"}
            volumes: Маппинг volumes {"/host/path": "/container/path"}
            detach: Запустить в фоне
            command: Команда для выполнения

        Returns:
            ToolResult с ID контейнера
        """
        try:
            cmd = ["docker", "run"]

            if detach:
                cmd.append("-d")

            if name:
                cmd.extend(["--name", name])

            if ports:
                for host_port, container_port in ports.items():
                    cmd.extend(["-p", f"{host_port}:{container_port}"])

            if env:
                for key, value in env.items():
                    cmd.extend(["-e", f"{key}={value}"])

            if volumes:
                for host_path, container_path in volumes.items():
                    cmd.extend(["-v", f"{host_path}:{container_path}"])

            cmd.append(image)

            if command:
                cmd.extend(command.split())

            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self.timeout
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Failed to run container: {result.stderr}"
                )

            container_id = result.stdout.strip()
            output = f"Container started: {container_id}\n"
            if name:
                output += f"Name: {name}\n"
            output += f"Image: {image}\n"

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def stop_container(self, container: str) -> ToolResult:
        """Остановить контейнер.

        Args:
            container: ID или имя контейнера

        Returns:
            ToolResult с результатом
        """
        try:
            result = subprocess.run(
                ["docker", "stop", container],
                capture_output=True,
                text=True,
                timeout=self.timeout
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Failed to stop container: {result.stderr}"
                )

            return ToolResult(
                success=True,
                output=f"Container stopped: {container}"
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def remove_container(self, container: str, force: bool = False) -> ToolResult:
        """Удалить контейнер.

        Args:
            container: ID или имя контейнера
            force: Принудительное удаление (даже если запущен)

        Returns:
            ToolResult с результатом
        """
        try:
            cmd = ["docker", "rm"]
            if force:
                cmd.append("-f")
            cmd.append(container)

            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self.timeout
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Failed to remove container: {result.stderr}"
                )

            return ToolResult(
                success=True,
                output=f"Container removed: {container}"
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def logs(self, container: str, tail: int = 100, follow: bool = False) -> ToolResult:
        """Получить логи контейнера.

        Args:
            container: ID или имя контейнера
            tail: Количество последних строк
            follow: Следить за логами (не поддерживается)

        Returns:
            ToolResult с логами
        """
        try:
            cmd = ["docker", "logs", "--tail", str(tail), container]

            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self.timeout
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Failed to get logs: {result.stderr}"
                )

            output = f"=== Logs for {container} (last {tail} lines) ===\n\n"
            output += result.stdout
            if result.stderr:
                output += "\n\nStderr:\n" + result.stderr

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def exec_command(self, container: str, command: str) -> ToolResult:
        """Выполнить команду в контейнере.

        Args:
            container: ID или имя контейнера
            command: Команда для выполнения

        Returns:
            ToolResult с результатом команды
        """
        try:
            result = subprocess.run(
                ["docker", "exec", container, "sh", "-c", command],
                capture_output=True,
                text=True,
                timeout=self.timeout
            )

            output = result.stdout
            if result.stderr:
                output += "\n\nStderr:\n" + result.stderr

            success = result.returncode == 0

            return ToolResult(
                success=success,
                output=output,
                error="" if success else f"Command failed with exit code {result.returncode}"
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def inspect(self, container: str) -> ToolResult:
        """Получить детальную информацию о контейнере.

        Args:
            container: ID или имя контейнера

        Returns:
            ToolResult с информацией
        """
        try:
            result = subprocess.run(
                ["docker", "inspect", container],
                capture_output=True,
                text=True,
                timeout=self.timeout
            )

            if result.returncode != 0:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Failed to inspect container: {result.stderr}"
                )

            # Парсим JSON и форматируем
            data = json.loads(result.stdout)[0]
            
            output = f"=== Container Info: {container} ===\n\n"
            output += f"ID: {data.get('Id', 'N/A')[:12]}\n"
            output += f"Name: {data.get('Name', 'N/A')}\n"
            output += f"Image: {data.get('Config', {}).get('Image', 'N/A')}\n"
            output += f"Status: {data.get('State', {}).get('Status', 'N/A')}\n"
            output += f"Running: {data.get('State', {}).get('Running', False)}\n"
            
            # Порты
            ports = data.get('NetworkSettings', {}).get('Ports', {})
            if ports:
                output += "\nPorts:\n"
                for container_port, host_bindings in ports.items():
                    if host_bindings:
                        for binding in host_bindings:
                            output += f"  {binding.get('HostPort')} -> {container_port}\n"

            # Volumes
            mounts = data.get('Mounts', [])
            if mounts:
                output += "\nVolumes:\n"
                for mount in mounts:
                    output += f"  {mount.get('Source')} -> {mount.get('Destination')}\n"

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )
