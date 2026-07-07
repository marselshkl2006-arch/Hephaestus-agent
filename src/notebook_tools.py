"""
Notebook Tools - инструменты для работы с Jupyter notebooks.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from .real_tools import ToolResult


@dataclass
class NotebookReadTool:
    """Инструмент для чтения Jupyter notebook."""

    name: str = "notebook_read"
    description: str = "Прочитать Jupyter notebook (.ipynb)"

    def execute(self, file_path: str) -> ToolResult:
        """
        Прочитать notebook.

        Args:
            file_path: Путь к .ipynb файлу

        Returns:
            Результат выполнения
        """
        if new_source and not new_content:
            new_content = new_source

        try:
            path = Path(file_path)
            if not path.exists():
                return ToolResult(success=False, output="", error=f"Файл не найден: {file_path}")

            with open(path) as f:
                notebook = json.load(f)

            cells = notebook.get("cells", [])
            lines = [f"📓 Notebook: {path.name}\n"]
            lines.append(f"Ячеек: {len(cells)}\n")

            for i, cell in enumerate(cells, 1):
                cell_type = cell.get("cell_type", "unknown")
                source = "".join(cell.get("source", []))

                lines.append(f"## Ячейка {i} ({cell_type})")
                lines.append(source[:200] + ("..." if len(source) > 200 else ""))
                lines.append("")

            return ToolResult(success=True, output="\n".join(lines))

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class NotebookEditTool:
    """Инструмент для редактирования ячейки notebook."""

    name: str = "notebook_edit"
    description: str = "Редактировать ячейку в Jupyter notebook"

    def execute(
        self,
        file_path: str,
        cell_index: int,
        new_content: str = "",
        new_source: str = "",   # алиас — модель часто присылает new_source
    ) -> ToolResult:
        """
        Редактировать ячейку.
        new_source — алиас для new_content (модели часто присылают new_source).

        Args:
            file_path: Путь к .ipynb файлу
            cell_index: Индекс ячейки (начиная с 0)
            new_content: Новое содержимое

        Returns:
            Результат выполнения
        """
        if new_source and not new_content:
            new_content = new_source

        try:
            path = Path(file_path)
            if not path.exists():
                return ToolResult(success=False, output="", error=f"Файл не найден: {file_path}")

            with open(path) as f:
                notebook = json.load(f)

            cells = notebook.get("cells", [])

            if cell_index < 0 or cell_index >= len(cells):
                return ToolResult(success=False, output="", error=f"Неверный индекс ячейки: {cell_index}")

            # Обновляем содержимое
            cells[cell_index]["source"] = new_content.split("\n")

            # Сохраняем
            with open(path, "w") as f:
                json.dump(notebook, f, indent=2, ensure_ascii=False)

            return ToolResult(
                success=True,
                output=f"✅ Ячейка {cell_index} обновлена в {path.name}",
            )

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


@dataclass
class NotebookCreateTool:
    """Инструмент для создания нового notebook."""

    name: str = "notebook_create"
    description: str = "Создать новый Jupyter notebook"

    def execute(self, file_path: str) -> ToolResult:
        """
        Создать новый notebook.

        Args:
            file_path: Путь к новому .ipynb файлу

        Returns:
            Результат выполнения
        """
        try:
            path = Path(file_path)

            # Базовая структура notebook
            notebook = {
                "cells": [],
                "metadata": {
                    "kernelspec": {
                        "display_name": "Python 3",
                        "language": "python",
                        "name": "python3"
                    },
                    "language_info": {
                        "name": "python",
                        "version": "3.10.0"
                    }
                },
                "nbformat": 4,
                "nbformat_minor": 5
            }

            with open(path, "w") as f:
                json.dump(notebook, f, indent=2, ensure_ascii=False)

            return ToolResult(
                success=True,
                output=f"✅ Notebook создан: {path.name}",
            )

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


def create_notebook_tools() -> dict[str, any]:
    """
    Создать инструменты для работы с notebooks.

    Returns:
        Словарь инструментов
    """
    return {
        "notebook_read": NotebookReadTool(),
        "notebook_edit": NotebookEditTool(),
        "notebook_create": NotebookCreateTool(),
    }
