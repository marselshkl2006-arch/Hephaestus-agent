"""
Реальные инструменты для работы с файлами, shell командами и git.
Заменяет заглушки из tools.py.
"""
from __future__ import annotations

import difflib
import glob
import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .command_validator import CommandValidator


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str | None = None


class BashTool:
    """Выполнение shell команд с захватом вывода."""

    def __init__(self, timeout: int = 30, max_output_size: int = 1024 * 1024, workspace_root: str | None = None):
        self.timeout = timeout
        self.max_output_size = max_output_size
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else None
        self.validator = CommandValidator()

    def execute(self, command: str, cwd: str | None = None) -> ToolResult:
        """Выполнить shell команду."""
        try:
            # Если cwd не указан, используем workspace_root
            if cwd is None and self.workspace_root:
                cwd = str(self.workspace_root)

            # Валидируем команду
            validation = self.validator.validate_command(command)

            if not validation.is_valid:
                return ToolResult(
                    success=False,
                    output="",
                    error=validation.error or "Команда не прошла валидацию"
                )

            # Используем исправленную команду если есть
            if validation.fixed_command:
                command = validation.fixed_command
                if validation.warning:
                    print(f"⚠️  {validation.warning}")

            result = subprocess.run(
                command,
                shell=True,
                capture_output=True,
                text=True,
                timeout=self.timeout,
                cwd=cwd,
            )

            stdout = result.stdout
            stderr = result.stderr

            # Ограничение размера вывода
            if len(stdout) > self.max_output_size:
                stdout = stdout[:self.max_output_size] + "\n... (output truncated)"

            if len(stderr) > self.max_output_size:
                stderr = stderr[:self.max_output_size] + "\n... (output truncated)"

            return ToolResult(
                success=result.returncode == 0,
                output=stdout,
                error=stderr if stderr else None,
            )

        except subprocess.TimeoutExpired:
            return ToolResult(
                success=False,
                output="",
                error=f"Command timed out after {self.timeout} seconds",
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error executing command: {str(e)}",
            )


class FileReadTool:
    """Чтение файлов с проверкой прав доступа."""

    def __init__(self, max_file_size: int = 10 * 1024 * 1024):  # 10MB
        self.max_file_size = max_file_size

    def read(self, file_path: str, encoding: str = "utf-8") -> ToolResult:
        """Прочитать файл."""
        try:
            # Если путь относительный, разрешаем относительно текущей директории
            path = Path(file_path)
            if not path.is_absolute():
                path = path.resolve()
            else:
                path = path.resolve()

            if not path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"File not found: {file_path}",
                )

            if not path.is_file():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Not a file: {file_path}",
                )

            # Проверка размера
            file_size = path.stat().st_size
            if file_size > self.max_file_size:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"File too large: {file_size} bytes (max: {self.max_file_size})",
                )

            # Проверка на бинарный файл
            with open(path, "rb") as f:
                chunk = f.read(8192)
                if b"\x00" in chunk:
                    return ToolResult(
                        success=False,
                        output="",
                        error="Binary file detected",
                    )

            # Чтение файла
            content = path.read_text(encoding=encoding)

            return ToolResult(
                success=True,
                output=content,
            )

        except UnicodeDecodeError:
            return ToolResult(
                success=False,
                output="",
                error=f"Unable to decode file with encoding: {encoding}",
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error reading file: {str(e)}",
            )


class FileWriteTool:
    """Запись файлов с созданием директорий."""

    def __init__(self, workspace_root: str | None = None):
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else Path.cwd()

    def write(self, file_path: str, content: str, encoding: str = "utf-8") -> ToolResult:
        """Записать файл."""
        try:
            # Если путь относительный, разрешаем относительно workspace_root
            path = Path(file_path)
            if not path.is_absolute():
                path = (self.workspace_root / path).resolve()
            else:
                path = path.resolve()

            # Workspace boundary check отключен - агент может работать везде
            # if not self._is_within_workspace(path):
            #     return ToolResult(
            #         success=False,
            #         output="",
            #         error=f"File outside workspace: {file_path}",
            #     )

            # Создание директорий
            path.parent.mkdir(parents=True, exist_ok=True)

            # Запись файла
            path.write_text(content, encoding=encoding)

            # Валидация: проверяем что записали реальный код, а не описание
            if len(content) < 100:  # Короткие файлы проверяем особенно тщательно
                content_lower = content.lower()
                suspicious_phrases = [
                    "вставьте здесь",
                    "insert here",
                    "updated code",
                    "обновленный код",
                    "placeholder",
                    "todo:",
                    "// add code",
                    "# add code"
                ]
                for phrase in suspicious_phrases:
                    if phrase in content_lower:
                        return ToolResult(
                            success=False,
                            output="",
                            error=f"CRITICAL ERROR: You wrote a placeholder/description instead of actual code! Content: '{content[:100]}'. You MUST write real, compilable code, not comments or descriptions!",
                        )

            return ToolResult(
                success=True,
                output=f"File written: {file_path} ({len(content)} bytes)",
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error writing file: {str(e)}",
            )

    def _is_within_workspace(self, path: Path) -> bool:
        """Проверить, что путь внутри workspace."""
        try:
            path.relative_to(self.workspace_root)
            return True
        except ValueError:
            return False


class FileEditTool:
    """Редактирование файлов с поддержкой diff."""

    def __init__(self, workspace_root: str | None = None):
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else Path.cwd()
        self.read_tool = FileReadTool()
        self.write_tool = FileWriteTool(workspace_root)

    def preview_edit(self, file_path: str, old_text: str, new_text: str) -> tuple[bool, str, str]:
        """Предпросмотр изменений без записи.

        Returns:
            (success, diff, error_message)
        """
        # Читаем файл
        read_result = self.read_tool.read(file_path)
        if not read_result.success:
            return False, "", read_result.error

        content = read_result.output

        # Проверяем, что old_text есть в файле
        if old_text not in content:
            return False, "", f"Text not found in file: {old_text[:100]}..."

        # Заменяем текст
        new_content = content.replace(old_text, new_text, 1)

        # Генерируем diff
        diff = self._generate_diff(content, new_content, file_path)

        return True, diff, ""

    def edit(self, file_path: str, old_text: str, new_text: str) -> ToolResult:
        """Заменить текст в файле."""
        # Читаем файл
        read_result = self.read_tool.read(file_path)
        if not read_result.success:
            return read_result

        content = read_result.output

        # Проверяем, что old_text есть в файле
        if old_text not in content:
            return ToolResult(
                success=False,
                output="",
                error=f"Text not found in file: {old_text[:100]}...",
            )

        # Заменяем текст
        new_content = content.replace(old_text, new_text, 1)

        # Записываем файл
        write_result = self.write_tool.write(file_path, new_content)
        if not write_result.success:
            return write_result

        # Генерируем diff
        diff = self._generate_diff(content, new_content, file_path)

        return ToolResult(
            success=True,
            output=f"File edited: {file_path}\n\n{diff}",
        )

    def prepend(self, file_path: str, text: str) -> ToolResult:
        """Добавить текст в начало файла."""
        # Читаем файл
        read_result = self.read_tool.read(file_path)
        if not read_result.success:
            return read_result

        content = read_result.output

        # Добавляем текст в начало
        new_content = text + content

        # Записываем файл
        write_result = self.write_tool.write(file_path, new_content)
        if not write_result.success:
            return write_result

        # Генерируем diff
        diff = self._generate_diff(content, new_content, file_path)

        return ToolResult(
            success=True,
            output=f"Text prepended to: {file_path}\n\n{diff}",
        )

    def _generate_diff(self, old_content: str, new_content: str, filename: str) -> str:
        """Сгенерировать unified diff."""
        old_lines = old_content.splitlines(keepends=True)
        new_lines = new_content.splitlines(keepends=True)

        diff = difflib.unified_diff(
            old_lines,
            new_lines,
            fromfile=f"a/{filename}",
            tofile=f"b/{filename}",
            lineterm="",
        )

        return "".join(diff)


class GlobTool:
    """Поиск файлов по паттерну."""

    def __init__(self, workspace_root: str | None = None):
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else Path.cwd()

    def search(self, pattern: str, recursive: bool = True) -> ToolResult:
        """Найти файлы по паттерну."""
        try:
            # Если паттерн - простое имя файла без путей и без wildcards,
            # автоматически делаем рекурсивный поиск
            if '/' not in pattern and '*' not in pattern and '?' not in pattern:
                pattern = f"**/{pattern}"

            if recursive:
                matches = glob.glob(
                    str(self.workspace_root / pattern),
                    recursive=True,
                )
            else:
                matches = glob.glob(str(self.workspace_root / pattern))

            # Сортируем и делаем относительными путями
            relative_matches = []
            for match in sorted(matches):
                try:
                    rel_path = Path(match).relative_to(self.workspace_root)
                    relative_matches.append(str(rel_path))
                except ValueError:
                    continue

            output = "\n".join(relative_matches) if relative_matches else "No matches found"

            return ToolResult(
                success=True,
                output=output,
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error searching files: {str(e)}",
            )


class GrepTool:
    """Поиск по содержимому файлов."""

    def __init__(self, workspace_root: str | None = None):
        self.workspace_root = Path(workspace_root).resolve() if workspace_root else Path.cwd()

    def search(
        self,
        pattern: str,
        file_pattern: str = "**/*",
        case_sensitive: bool = True,
        max_results: int = 100,
    ) -> ToolResult:
        """Найти текст в файлах."""
        try:
            flags = 0 if case_sensitive else re.IGNORECASE
            regex = re.compile(pattern, flags)

            results = []
            files_searched = 0

            # Ищем файлы
            for file_path in self.workspace_root.glob(file_pattern):
                if not file_path.is_file():
                    continue

                files_searched += 1

                try:
                    # Проверка на бинарный файл
                    with open(file_path, "rb") as f:
                        chunk = f.read(8192)
                        if b"\x00" in chunk:
                            continue

                    # Поиск в файле
                    with open(file_path, "r", encoding="utf-8", errors="ignore") as f:
                        for line_num, line in enumerate(f, 1):
                            if regex.search(line):
                                rel_path = file_path.relative_to(self.workspace_root)
                                results.append(f"{rel_path}:{line_num}:{line.rstrip()}")

                                if len(results) >= max_results:
                                    break

                except Exception:
                    continue

                if len(results) >= max_results:
                    break

            if not results:
                output = f"No matches found (searched {files_searched} files)"
            else:
                output = "\n".join(results)
                if len(results) >= max_results:
                    output += f"\n... (truncated at {max_results} results)"

            return ToolResult(
                success=True,
                output=output,
            )

        except re.error as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Invalid regex pattern: {str(e)}",
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error searching files: {str(e)}",
            )


class GitTool:
    """Git операции."""

    def __init__(self, repo_path: str | None = None):
        self.repo_path = Path(repo_path).resolve() if repo_path else Path.cwd()
        self.bash = BashTool()

    def status(self) -> ToolResult:
        """Получить статус репозитория."""
        return self.bash.execute("git status", cwd=str(self.repo_path))

    def diff(self, staged: bool = False) -> ToolResult:
        """Получить diff."""
        cmd = "git diff --staged" if staged else "git diff"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def log(self, max_count: int = 10) -> ToolResult:
        """Получить историю коммитов."""
        cmd = f"git log --oneline -n {max_count}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def add(self, files: list[str]) -> ToolResult:
        """Добавить файлы в staging."""
        files_str = " ".join(f'"{f}"' for f in files)
        cmd = f"git add {files_str}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def commit(self, message: str) -> ToolResult:
        """Создать коммит."""
        cmd = f'git commit -m "{message}"'
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def branch(self) -> ToolResult:
        """Получить список веток."""
        return self.bash.execute("git branch -a", cwd=str(self.repo_path))

    def push(self, remote: str = "origin", branch: str | None = None, force: bool = False) -> ToolResult:
        """
        Push изменения в remote.

        Args:
            remote: Имя remote (по умолчанию origin)
            branch: Имя ветки (если None, используется текущая)
            force: Force push (опасно!)
        """
        cmd = f"git push {remote}"
        if branch:
            cmd += f" {branch}"
        if force:
            cmd += " --force"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def pull(self, remote: str = "origin", branch: str | None = None) -> ToolResult:
        """
        Pull изменения из remote.

        Args:
            remote: Имя remote
            branch: Имя ветки
        """
        cmd = f"git pull {remote}"
        if branch:
            cmd += f" {branch}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def checkout(self, branch: str, create: bool = False) -> ToolResult:
        """
        Переключиться на ветку.

        Args:
            branch: Имя ветки
            create: Создать новую ветку
        """
        cmd = f"git checkout {'-b ' if create else ''}{branch}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def create_branch(self, branch_name: str) -> ToolResult:
        """Создать новую ветку."""
        cmd = f"git branch {branch_name}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def delete_branch(self, branch_name: str, force: bool = False) -> ToolResult:
        """
        Удалить ветку.

        Args:
            branch_name: Имя ветки
            force: Принудительное удаление
        """
        flag = "-D" if force else "-d"
        cmd = f"git branch {flag} {branch_name}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def stash(self, message: str | None = None) -> ToolResult:
        """
        Сохранить изменения в stash.

        Args:
            message: Сообщение для stash
        """
        cmd = "git stash"
        if message:
            cmd += f' push -m "{message}"'
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def stash_pop(self) -> ToolResult:
        """Применить последний stash."""
        return self.bash.execute("git stash pop", cwd=str(self.repo_path))

    def stash_list(self) -> ToolResult:
        """Список stash."""
        return self.bash.execute("git stash list", cwd=str(self.repo_path))

    def reset(self, mode: str = "mixed", commit: str = "HEAD") -> ToolResult:
        """
        Reset изменения.

        Args:
            mode: Режим reset (soft, mixed, hard)
            commit: Коммит для reset
        """
        cmd = f"git reset --{mode} {commit}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def remote(self, action: str = "list") -> ToolResult:
        """
        Операции с remote.

        Args:
            action: list, add, remove
        """
        if action == "list":
            cmd = "git remote -v"
        else:
            cmd = f"git remote {action}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def fetch(self, remote: str = "origin") -> ToolResult:
        """Fetch изменения из remote."""
        cmd = f"git fetch {remote}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def merge(self, branch: str) -> ToolResult:
        """Merge ветки."""
        cmd = f"git merge {branch}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def rebase(self, branch: str) -> ToolResult:
        """Rebase на ветку."""
        cmd = f"git rebase {branch}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def tag(self, tag_name: str | None = None, message: str | None = None) -> ToolResult:
        """
        Операции с тегами.

        Args:
            tag_name: Имя тега (если None, показать список)
            message: Сообщение для аннотированного тега
        """
        if not tag_name:
            cmd = "git tag -l"
        elif message:
            cmd = f'git tag -a {tag_name} -m "{message}"'
        else:
            cmd = f"git tag {tag_name}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def show(self, commit: str = "HEAD") -> ToolResult:
        """Показать детали коммита."""
        cmd = f"git show {commit}"
        return self.bash.execute(cmd, cwd=str(self.repo_path))

    def blame(self, file_path: str) -> ToolResult:
        """Git blame для файла."""
        cmd = f'git blame "{file_path}"'
        return self.bash.execute(cmd, cwd=str(self.repo_path))


class FileDeleteTool:
    """Удаление файлов."""

    def __init__(self):
        pass

    def delete(self, file_path: str) -> ToolResult:
        """
        Удалить файл.

        Args:
            file_path: Путь к файлу

        Returns:
            ToolResult с результатом
        """
        try:
            path = Path(file_path).resolve()

            if not path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"File not found: {file_path}",
                )

            if not path.is_file():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Not a file: {file_path}",
                )

            # Удаляем файл
            path.unlink()

            return ToolResult(
                success=True,
                output=f"File deleted: {file_path}",
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error deleting file: {str(e)}",
            )


class FileMoveTool:
    """Перемещение/переименование файлов."""

    def __init__(self):
        pass

    def move(self, source: str, destination: str) -> ToolResult:
        """
        Переместить или переименовать файл.

        Args:
            source: Исходный путь
            destination: Целевой путь

        Returns:
            ToolResult с результатом
        """
        try:
            src_path = Path(source).resolve()
            dst_path = Path(destination).resolve()

            if not src_path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Source not found: {source}",
                )

            # Создаём директорию назначения если нужно
            dst_path.parent.mkdir(parents=True, exist_ok=True)

            # Перемещаем
            src_path.rename(dst_path)

            return ToolResult(
                success=True,
                output=f"Moved: {source} -> {destination}",
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error moving file: {str(e)}",
            )


class FileCopyTool:
    """Копирование файлов."""

    def __init__(self):
        pass

    def copy(self, source: str, destination: str) -> ToolResult:
        """
        Скопировать файл.

        Args:
            source: Исходный путь
            destination: Целевой путь

        Returns:
            ToolResult с результатом
        """
        try:
            import shutil

            src_path = Path(source).resolve()
            dst_path = Path(destination).resolve()

            if not src_path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Source not found: {source}",
                )

            if not src_path.is_file():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Not a file: {source}",
                )

            # Создаём директорию назначения если нужно
            dst_path.parent.mkdir(parents=True, exist_ok=True)

            # Копируем
            shutil.copy2(src_path, dst_path)

            return ToolResult(
                success=True,
                output=f"Copied: {source} -> {destination}",
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error copying file: {str(e)}",
            )


class FileExistsTool:
    """Проверка существования файла или директории."""

    def __init__(self):
        pass

    def exists(self, path: str) -> ToolResult:
        """
        Проверить существование файла или директории.

        Args:
            path: Путь для проверки

        Returns:
            ToolResult с результатом (output: "exists" или "not_found")
        """
        try:
            file_path = Path(path).resolve()
            exists = file_path.exists()

            if exists:
                if file_path.is_file():
                    file_type = "file"
                elif file_path.is_dir():
                    file_type = "directory"
                else:
                    file_type = "other"

                return ToolResult(
                    success=True,
                    output=f"exists ({file_type})",
                )
            else:
                return ToolResult(
                    success=True,
                    output="not_found",
                )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error checking path: {str(e)}",
            )


# Фабрика для создания инструментов
def create_tools(workspace_root: str | None = None) -> dict[str, Any]:
    """Создать все инструменты."""
    return {
        "bash": BashTool(timeout=300, workspace_root=workspace_root),  # 5 минут для pip install
        "file_read": FileReadTool(),
        "file_write": FileWriteTool(workspace_root),
        "file_edit": FileEditTool(workspace_root),
        "file_delete": FileDeleteTool(),
        "file_move": FileMoveTool(),
        "file_copy": FileCopyTool(),
        "file_exists": FileExistsTool(),
        "glob": GlobTool(workspace_root),
        "grep": GrepTool(workspace_root),
        "git": GitTool(workspace_root),
    }
