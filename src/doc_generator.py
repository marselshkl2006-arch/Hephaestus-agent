"""
Documentation Generator - автоматическая генерация документации из кода.
Поддерживает docstrings, README, API docs.
"""
from __future__ import annotations

import ast
import inspect
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .real_tools import ToolResult


@dataclass
class FunctionDoc:
    """Документация функции."""
    name: str
    signature: str
    docstring: str | None
    args: list[dict[str, Any]]
    returns: str | None
    decorators: list[str]


@dataclass
class ClassDoc:
    """Документация класса."""
    name: str
    docstring: str | None
    bases: list[str]
    methods: list[FunctionDoc]
    attributes: list[dict[str, Any]]


@dataclass
class ModuleDoc:
    """Документация модуля."""
    name: str
    path: str
    docstring: str | None
    classes: list[ClassDoc]
    functions: list[FunctionDoc]
    imports: list[str]


class DocGenerator:
    """Генератор документации."""

    def __init__(self, workspace_root: str = "."):
        self.workspace_root = Path(workspace_root)

    def analyze_file(self, file_path: str) -> ModuleDoc | None:
        """
        Анализировать Python файл и извлечь документацию.

        Args:
            file_path: Путь к файлу

        Returns:
            ModuleDoc или None если ошибка
        """
        try:
            path = Path(file_path)
            if not path.exists():
                return None

            with open(path, 'r', encoding='utf-8') as f:
                source = f.read()

            tree = ast.parse(source)

            # Извлекаем docstring модуля
            module_docstring = ast.get_docstring(tree)

            # Извлекаем импорты
            imports = []
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    for alias in node.names:
                        imports.append(alias.name)
                elif isinstance(node, ast.ImportFrom):
                    module = node.module or ""
                    for alias in node.names:
                        imports.append(f"{module}.{alias.name}")

            # Извлекаем классы
            classes = []
            for node in ast.walk(tree):
                if isinstance(node, ast.ClassDef):
                    class_doc = self._analyze_class(node)
                    classes.append(class_doc)

            # Извлекаем функции верхнего уровня
            functions = []
            for node in tree.body:
                if isinstance(node, ast.FunctionDef):
                    func_doc = self._analyze_function(node)
                    functions.append(func_doc)

            return ModuleDoc(
                name=path.stem,
                path=str(path),
                docstring=module_docstring,
                classes=classes,
                functions=functions,
                imports=imports
            )

        except Exception as e:
            print(f"Error analyzing {file_path}: {e}")
            return None

    def _analyze_class(self, node: ast.ClassDef) -> ClassDoc:
        """Анализировать класс."""
        docstring = ast.get_docstring(node)

        # Базовые классы
        bases = [self._get_name(base) for base in node.bases]

        # Методы
        methods = []
        for item in node.body:
            if isinstance(item, ast.FunctionDef):
                method_doc = self._analyze_function(item)
                methods.append(method_doc)

        # Атрибуты класса
        attributes = []
        for item in node.body:
            if isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
                attr = {
                    "name": item.target.id,
                    "type": self._get_annotation(item.annotation)
                }
                attributes.append(attr)

        return ClassDoc(
            name=node.name,
            docstring=docstring,
            bases=bases,
            methods=methods,
            attributes=attributes
        )

    def _analyze_function(self, node: ast.FunctionDef) -> FunctionDoc:
        """Анализировать функцию."""
        docstring = ast.get_docstring(node)

        # Аргументы
        args = []
        for arg in node.args.args:
            arg_info = {
                "name": arg.arg,
                "type": self._get_annotation(arg.annotation) if arg.annotation else None,
                "default": None
            }
            args.append(arg_info)

        # Значения по умолчанию
        defaults = node.args.defaults
        if defaults:
            for i, default in enumerate(defaults):
                arg_idx = len(args) - len(defaults) + i
                if arg_idx >= 0:
                    args[arg_idx]["default"] = ast.unparse(default)

        # Возвращаемое значение
        returns = self._get_annotation(node.returns) if node.returns else None

        # Декораторы
        decorators = [self._get_name(dec) for dec in node.decorator_list]

        # Сигнатура
        signature = self._build_signature(node.name, args, returns)

        return FunctionDoc(
            name=node.name,
            signature=signature,
            docstring=docstring,
            args=args,
            returns=returns,
            decorators=decorators
        )

    def _get_name(self, node: ast.expr) -> str:
        """Получить имя из AST узла."""
        if isinstance(node, ast.Name):
            return node.id
        elif isinstance(node, ast.Attribute):
            return f"{self._get_name(node.value)}.{node.attr}"
        else:
            return ast.unparse(node)

    def _get_annotation(self, node: ast.expr | None) -> str | None:
        """Получить аннотацию типа."""
        if node is None:
            return None
        return ast.unparse(node)

    def _build_signature(self, name: str, args: list[dict], returns: str | None) -> str:
        """Построить сигнатуру функции."""
        args_str = ", ".join([
            f"{arg['name']}: {arg['type']}" if arg['type'] else arg['name']
            for arg in args
        ])

        if returns:
            return f"{name}({args_str}) -> {returns}"
        else:
            return f"{name}({args_str})"

    def generate_markdown(self, module_doc: ModuleDoc) -> str:
        """
        Сгенерировать Markdown документацию.

        Args:
            module_doc: Документация модуля

        Returns:
            Markdown текст
        """
        md = f"# {module_doc.name}\n\n"

        if module_doc.docstring:
            md += f"{module_doc.docstring}\n\n"

        # Классы
        if module_doc.classes:
            md += "## Classes\n\n"
            for cls in module_doc.classes:
                md += f"### {cls.name}\n\n"

                if cls.bases:
                    md += f"**Inherits from:** {', '.join(cls.bases)}\n\n"

                if cls.docstring:
                    md += f"{cls.docstring}\n\n"

                if cls.attributes:
                    md += "**Attributes:**\n\n"
                    for attr in cls.attributes:
                        md += f"- `{attr['name']}`: {attr['type']}\n"
                    md += "\n"

                if cls.methods:
                    md += "**Methods:**\n\n"
                    for method in cls.methods:
                        md += f"#### `{method.signature}`\n\n"
                        if method.docstring:
                            md += f"{method.docstring}\n\n"

        # Функции
        if module_doc.functions:
            md += "## Functions\n\n"
            for func in module_doc.functions:
                md += f"### `{func.signature}`\n\n"
                if func.docstring:
                    md += f"{func.docstring}\n\n"

        return md

    def generate_api_docs(self, file_path: str) -> ToolResult:
        """
        Сгенерировать API документацию для файла.

        Args:
            file_path: Путь к Python файлу

        Returns:
            ToolResult с Markdown документацией
        """
        module_doc = self.analyze_file(file_path)
        if not module_doc:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to analyze file: {file_path}"
            )

        markdown = self.generate_markdown(module_doc)

        return ToolResult(
            success=True,
            output=markdown,
            error=None
        )

    def generate_readme(self, project_path: str = ".") -> ToolResult:
        """
        Сгенерировать README.md для проекта.

        Args:
            project_path: Путь к проекту

        Returns:
            ToolResult с README содержимым
        """
        project_dir = Path(project_path)

        # Ищем Python файлы
        py_files = list(project_dir.rglob("*.py"))

        if not py_files:
            return ToolResult(
                success=False,
                output="",
                error="No Python files found in project"
            )

        # Определяем название проекта
        project_name = project_dir.name

        readme = f"# {project_name}\n\n"
        readme += "## Overview\n\n"
        readme += f"Python project with {len(py_files)} modules.\n\n"

        # Анализируем главные модули
        main_modules = [f for f in py_files if f.parent == project_dir or f.parent.name == "src"]

        if main_modules:
            readme += "## Modules\n\n"
            for py_file in main_modules[:10]:  # Первые 10
                module_doc = self.analyze_file(str(py_file))
                if module_doc and module_doc.docstring:
                    readme += f"### {module_doc.name}\n\n"
                    readme += f"{module_doc.docstring}\n\n"

        readme += "## Installation\n\n"
        readme += "```bash\n"
        readme += "pip install -r requirements.txt\n"
        readme += "```\n\n"

        readme += "## Usage\n\n"
        readme += "```python\n"
        readme += f"# Example usage of {project_name}\n"
        readme += "```\n\n"

        return ToolResult(
            success=True,
            output=readme,
            error=None
        )

    def generate_docstrings(self, file_path: str) -> ToolResult:
        """
        Сгенерировать шаблоны docstrings для функций без документации.

        Args:
            file_path: Путь к Python файлу

        Returns:
            ToolResult со списком функций без docstrings
        """
        module_doc = self.analyze_file(file_path)
        if not module_doc:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to analyze file: {file_path}"
            )

        missing = []

        # Проверяем функции
        for func in module_doc.functions:
            if not func.docstring:
                template = self._generate_docstring_template(func)
                missing.append({
                    "function": func.name,
                    "signature": func.signature,
                    "template": template
                })

        # Проверяем методы классов
        for cls in module_doc.classes:
            for method in cls.methods:
                if not method.docstring:
                    template = self._generate_docstring_template(method)
                    missing.append({
                        "function": f"{cls.name}.{method.name}",
                        "signature": method.signature,
                        "template": template
                    })

        if not missing:
            return ToolResult(
                success=True,
                output="All functions have docstrings!",
                error=None
            )

        output = f"Found {len(missing)} functions without docstrings:\n\n"
        for item in missing:
            output += f"**{item['function']}**\n"
            output += f"```python\n{item['template']}\n```\n\n"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def _generate_docstring_template(self, func: FunctionDoc) -> str:
        """Сгенерировать шаблон docstring."""
        lines = ['"""']
        lines.append(f"{func.name} - TODO: Add description")
        lines.append("")

        if func.args:
            lines.append("Args:")
            for arg in func.args:
                arg_type = f" ({arg['type']})" if arg['type'] else ""
                lines.append(f"    {arg['name']}{arg_type}: TODO: Add description")
            lines.append("")

        if func.returns:
            lines.append("Returns:")
            lines.append(f"    {func.returns}: TODO: Add description")
            lines.append("")

        lines.append('"""')

        return "\n".join(lines)


# Пример использования
if __name__ == "__main__":
    generator = DocGenerator()

    # Генерируем документацию для этого файла
    result = generator.generate_api_docs(__file__)
    print(result.output[:500])
