"""
Repomap — карта репозитория для агента (как в Aider).
Сканирует проект, извлекает классы/функции, даёт модели контекст.
"""
from __future__ import annotations
import ast
from pathlib import Path
from .real_tools import ToolResult


class RepomapTool:
    def __init__(self, workspace: Path):
        self.workspace = workspace

    def scan(self, max_files: int = 50, save_to: str = "") -> ToolResult:
        """Карта всех Python файлов проекта — классы, функции, импорты."""
        try:
            lines = [f"🗺️  Карта репозитория: {self.workspace.name}\n"]
            py_files = [
                p for p in sorted(self.workspace.rglob("*.py"))
                if "venv" not in str(p) and "__pycache__" not in str(p)
            ][:max_files]

            total_classes, total_fns = 0, 0
            for path in py_files:
                rel = path.relative_to(self.workspace)
                try:
                    tree = ast.parse(path.read_text(encoding="utf-8", errors="ignore"))
                except SyntaxError:
                    continue

                classes, fns = [], []
                for node in ast.walk(tree):
                    if isinstance(node, ast.ClassDef):
                        methods = [n.name for n in ast.walk(node) if isinstance(n, ast.FunctionDef)][:5]
                        classes.append(f"  📦 {node.name}({', '.join(methods)})")
                        total_classes += 1
                    elif isinstance(node, ast.FunctionDef) and not any(
                        isinstance(p, ast.ClassDef) for p in ast.walk(tree)
                        if hasattr(p, 'body') and node in getattr(p, 'body', [])
                    ):
                        fns.append(f"  ƒ {node.name}")
                        total_fns += 1

                if classes or fns:
                    lines.append(f"📄 {rel}")
                    lines.extend(classes[:3])
                    lines.extend(fns[:5])
                    lines.append("")

            lines.append(f"\n📊 Итого: {len(py_files)} файлов, {total_classes} классов, {total_fns} функций")
            output = "\n".join(lines)

            if save_to:
                Path(save_to).parent.mkdir(parents=True, exist_ok=True)
                Path(save_to).write_text(output, encoding="utf-8")
                output += f"\n💾 Сохранено: {save_to}"

            return ToolResult(success=True, output=output)
        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def file_summary(self, file_path: str) -> ToolResult:
        """Краткое содержание одного файла."""
        try:
            path = Path(file_path)
            if not path.exists():
                return ToolResult(success=False, output="", error=f"Файл не найден: {file_path}")
            tree = ast.parse(path.read_text(encoding="utf-8"))
            lines = [f"📄 {path.name}\n"]
            for node in ast.walk(tree):
                if isinstance(node, ast.ClassDef):
                    lines.append(f"  📦 class {node.name}:")
                    for item in node.body:
                        if isinstance(item, ast.FunctionDef):
                            args = [a.arg for a in item.args.args if a.arg != "self"]
                            lines.append(f"      def {item.name}({', '.join(args)})")
                elif isinstance(node, ast.FunctionDef):
                    args = [a.arg for a in node.args.args]
                    lines.append(f"  ƒ def {node.name}({', '.join(args)})")
            return ToolResult(success=True, output="\n".join(lines))
        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))
