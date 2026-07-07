"""
Rich Diagrams — красивые диаграммы в терминале через Rich.
Поддерживает: классы, дерево файлов, зависимости, flowchart, таблицы.
Без внешних зависимостей — только Rich который уже установлен.
"""
from __future__ import annotations

import ast
import os
from pathlib import Path

from rich.console import Console
from rich.tree import Tree
from rich.table import Table
from rich.panel import Panel
from rich.text import Text
from rich.columns import Columns
from rich import box

from .real_tools import ToolResult

console = Console()


class RichDiagramTool:
    """Диаграммы прямо в терминале через Rich."""

    def class_diagram(self, file_path: str, save_to: str = "") -> ToolResult:
        """Диаграмма классов Python файла в ASCII/Unicode."""
        try:
            path = Path(file_path)
            if not path.exists():
                return ToolResult(success=False, output="", error=f"Файл не найден: {file_path}")

            source = path.read_text(encoding="utf-8")
            tree = ast.parse(source)

            classes = []
            for node in ast.walk(tree):
                if isinstance(node, ast.ClassDef):
                    bases = [b.id if isinstance(b, ast.Name) else str(ast.unparse(b)) for b in node.bases]
                    methods = []
                    attrs = []
                    for item in node.body:
                        if isinstance(item, ast.FunctionDef):
                            args = [a.arg for a in item.args.args if a.arg != "self"]
                            visibility = "+" if not item.name.startswith("_") else "-"
                            methods.append(f"{visibility}{item.name}({', '.join(args)})")
                        elif isinstance(item, ast.Assign):
                            for t in item.targets:
                                if isinstance(t, ast.Name):
                                    attrs.append(f"  {t.id}")
                    classes.append({
                        "name": node.name,
                        "bases": bases,
                        "methods": methods[:8],  # не больше 8
                        "attrs": attrs[:5],
                    })

            if not classes:
                return ToolResult(success=False, output="", error="Классов не найдено")

            # Строим Rich таблицу-диаграмму
            output_lines = []
            output_lines.append(f"\n📐 Диаграмма классов: {path.name}\n")

            for cls in classes:
                # Заголовок класса
                base_str = f" ← {', '.join(cls['bases'])}" if cls["bases"] else ""
                output_lines.append(f"┌─ 📦 {cls['name']}{base_str}")

                if cls["attrs"]:
                    output_lines.append("│  Атрибуты:")
                    for a in cls["attrs"]:
                        output_lines.append(f"│    {a}")

                if cls["methods"]:
                    output_lines.append("│  Методы:")
                    for m in cls["methods"]:
                        output_lines.append(f"│    {m}")
                output_lines.append("└" + "─" * 40)

            # Связи наследования
            inheritance = []
            class_names = {c["name"] for c in classes}
            for cls in classes:
                for base in cls["bases"]:
                    if base in class_names:
                        inheritance.append(f"  {base} ──► {cls['name']}")

            if inheritance:
                output_lines.append("\n🔗 Наследование:")
                output_lines.extend(inheritance)

            result = "\n".join(output_lines)

            # Сохраняем если нужно
            if save_to:
                Path(save_to).parent.mkdir(parents=True, exist_ok=True)
                Path(save_to).write_text(result, encoding="utf-8")
                result += f"\n\n💾 Сохранено: {save_to}"

            return ToolResult(success=True, output=result)

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def file_tree(self, directory: str = ".", max_depth: int = 3,
                  save_to: str = "") -> ToolResult:
        """Дерево файлов и папок."""
        try:
            root = Path(directory).resolve()
            if not root.exists():
                return ToolResult(success=False, output="", error=f"Папка не найдена: {directory}")

            lines = [f"\n📂 {root}\n"]

            def walk(path: Path, prefix: str = "", depth: int = 0):
                if depth >= max_depth:
                    return
                try:
                    items = sorted(path.iterdir(), key=lambda x: (x.is_file(), x.name))
                except PermissionError:
                    return
                for i, item in enumerate(items):
                    if item.name.startswith(".") or item.name in ("venv", "__pycache__", "node_modules"):
                        continue
                    is_last = i == len(items) - 1
                    connector = "└── " if is_last else "├── "
                    icon = "📁 " if item.is_dir() else "📄 "
                    size = ""
                    if item.is_file():
                        try:
                            sz = item.stat().st_size
                            size = f" ({sz:,} B)" if sz < 1024 else f" ({sz//1024} KB)"
                        except Exception:
                            pass
                    lines.append(f"{prefix}{connector}{icon}{item.name}{size}")
                    if item.is_dir():
                        extension = "    " if is_last else "│   "
                        walk(item, prefix + extension, depth + 1)

            walk(root)
            result = "\n".join(lines)

            if save_to:
                Path(save_to).parent.mkdir(parents=True, exist_ok=True)
                Path(save_to).write_text(result, encoding="utf-8")
                result += f"\n\n💾 Сохранено: {save_to}"

            return ToolResult(success=True, output=result)

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def dependency_graph(self, file_path: str, save_to: str = "") -> ToolResult:
        """Граф импортов Python файла."""
        try:
            path = Path(file_path)
            if not path.exists():
                return ToolResult(success=False, output="", error=f"Файл не найден: {file_path}")

            source = path.read_text(encoding="utf-8")
            tree = ast.parse(source)

            stdlib = {"os", "sys", "re", "json", "ast", "math", "time", "datetime",
                      "pathlib", "typing", "dataclasses", "collections", "itertools",
                      "functools", "subprocess", "threading", "abc", "io", "copy"}
            imports = {"stdlib": [], "third_party": [], "local": []}

            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    for alias in node.names:
                        pkg = alias.name.split(".")[0]
                        if pkg in stdlib:
                            imports["stdlib"].append(alias.name)
                        elif pkg.startswith("."):
                            imports["local"].append(alias.name)
                        else:
                            imports["third_party"].append(alias.name)
                elif isinstance(node, ast.ImportFrom):
                    mod = node.module or ""
                    pkg = mod.split(".")[0] if mod else ""
                    if node.level and node.level > 0:
                        imports["local"].append(f".{mod}")
                    elif pkg in stdlib:
                        imports["stdlib"].append(mod)
                    else:
                        imports["third_party"].append(mod)

            lines = [f"\n🔗 Граф зависимостей: {path.name}\n"]
            lines.append(f"  📦 {path.stem}")
            lines.append("  │")

            if imports["local"]:
                lines.append("  ├── 🏠 Локальные модули")
                for imp in sorted(set(imports["local"])):
                    lines.append(f"  │   └── {imp}")

            if imports["third_party"]:
                lines.append("  ├── 📚 Сторонние пакеты")
                for imp in sorted(set(imports["third_party"])):
                    lines.append(f"  │   └── {imp}")

            if imports["stdlib"]:
                lines.append("  └── 🐍 Стандартная библиотека")
                for imp in sorted(set(imports["stdlib"])):
                    lines.append(f"      └── {imp}")

            result = "\n".join(lines)

            if save_to:
                Path(save_to).parent.mkdir(parents=True, exist_ok=True)
                Path(save_to).write_text(result, encoding="utf-8")
                result += f"\n\n💾 Сохранено: {save_to}"

            return ToolResult(success=True, output=result)

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def mermaid_to_ascii(self, mermaid_code: str, save_to: str = "") -> ToolResult:
        """
        Конвертирует Mermaid код в ASCII диаграмму.
        Поддерживает flowchart и classDiagram.
        """
        try:
            lines_in = mermaid_code.strip().split("\n")
            result_lines = ["\n📊 Диаграмма\n"]

            if mermaid_code.strip().startswith("classDiagram"):
                result_lines.append("Диаграмма классов:")
                for line in lines_in[1:]:
                    line = line.strip()
                    if not line:
                        continue
                    if "<|--" in line:
                        parts = line.split("<|--")
                        result_lines.append(f"  {parts[1].strip()} ──► {parts[0].strip()} (наследование)")
                    elif ":" in line and not line.startswith("%%"):
                        cls, member = line.split(":", 1)
                        result_lines.append(f"  📦 {cls.strip()}")
                        result_lines.append(f"      {member.strip()}")

            elif any(x in mermaid_code for x in ["graph ", "flowchart "]):
                result_lines.append("Блок-схема:")
                for line in lines_in[1:]:
                    line = line.strip()
                    if not line or line.startswith("%%"):
                        continue
                    # A --> B
                    for arrow in ["-->", "->", "==>", "-.->", "-->"]:
                        if arrow in line:
                            parts = line.split(arrow, 1)
                            src = parts[0].strip().strip("[](){}")
                            dst = parts[1].strip().strip("[](){}")
                            # Убираем метки
                            if "|" in dst:
                                label, dst = dst.split("|", 2)[1], dst.split("|", 2)[2]
                                result_lines.append(f"  [{src}] ──{label}──► [{dst}]")
                            else:
                                result_lines.append(f"  [{src}] ──────► [{dst}]")
                            break
            else:
                result_lines.append(mermaid_code)

            result = "\n".join(result_lines)

            if save_to:
                Path(save_to).parent.mkdir(parents=True, exist_ok=True)
                Path(save_to).write_text(result, encoding="utf-8")
                result += f"\n\n💾 Сохранено: {save_to}"

            return ToolResult(success=True, output=result)

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


def create_diagram_tools() -> dict:
    tool = RichDiagramTool()
    return {"diagram": tool}
