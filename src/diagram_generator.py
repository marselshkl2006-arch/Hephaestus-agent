"""
Diagram Generator - генерация диаграмм из кода и данных.
Поддерживает Mermaid, PlantUML, ASCII диаграммы.
"""
from __future__ import annotations

import ast
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .real_tools import ToolResult


@dataclass
class DiagramNode:
    """Узел диаграммы."""
    id: str
    label: str
    type: str  # class, function, module, etc.
    properties: dict[str, Any]


@dataclass
class DiagramEdge:
    """Связь между узлами."""
    source: str
    target: str
    label: str | None
    type: str  # inherits, calls, imports, etc.


class DiagramGenerator:
    """Генератор диаграмм."""

    def __init__(self, workspace_root: str = "."):
        self.workspace_root = Path(workspace_root)

    def generate_class_diagram(self, file_path: str, format: str = "mermaid") -> ToolResult:
        """
        Сгенерировать диаграмму классов из Python файла.

        Args:
            file_path: Путь к Python файлу
            format: Формат диаграммы (mermaid, plantuml, ascii)

        Returns:
            ToolResult с диаграммой
        """
        try:
            path = Path(file_path)
            if not path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"File not found: {file_path}"
                )

            with open(path, 'r', encoding='utf-8') as f:
                source = f.read()

            tree = ast.parse(source)

            # Извлекаем классы и их связи
            nodes = []
            edges = []

            for node in ast.walk(tree):
                if isinstance(node, ast.ClassDef):
                    # Добавляем класс
                    class_node = DiagramNode(
                        id=node.name,
                        label=node.name,
                        type="class",
                        properties={}
                    )

                    # Извлекаем методы
                    methods = []
                    for item in node.body:
                        if isinstance(item, ast.FunctionDef):
                            methods.append(item.name)

                    class_node.properties["methods"] = methods

                    # Извлекаем атрибуты
                    attributes = []
                    for item in node.body:
                        if isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
                            attributes.append(item.target.id)

                    class_node.properties["attributes"] = attributes

                    nodes.append(class_node)

                    # Добавляем связи наследования
                    for base in node.bases:
                        base_name = self._get_name(base)
                        edge = DiagramEdge(
                            source=node.name,
                            target=base_name,
                            label=None,
                            type="inherits"
                        )
                        edges.append(edge)

            # Генерируем диаграмму в нужном формате
            if format == "mermaid":
                diagram = self._generate_mermaid_class_diagram(nodes, edges)
            elif format == "plantuml":
                diagram = self._generate_plantuml_class_diagram(nodes, edges)
            elif format == "ascii":
                diagram = self._generate_ascii_class_diagram(nodes, edges)
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Unknown format: {format}"
                )

            return ToolResult(
                success=True,
                output=diagram,
                error=None
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to generate diagram: {e}"
            )

    def _get_name(self, node: ast.expr) -> str:
        """Получить имя из AST узла."""
        if isinstance(node, ast.Name):
            return node.id
        elif isinstance(node, ast.Attribute):
            return f"{self._get_name(node.value)}.{node.attr}"
        else:
            return ast.unparse(node)

    def _generate_mermaid_class_diagram(
        self,
        nodes: list[DiagramNode],
        edges: list[DiagramEdge]
    ) -> str:
        """Сгенерировать Mermaid диаграмму классов."""
        lines = ["classDiagram"]

        # Добавляем классы
        for node in nodes:
            lines.append(f"    class {node.id} {{")

            # Атрибуты
            for attr in node.properties.get("attributes", []):
                lines.append(f"        +{attr}")

            # Методы
            for method in node.properties.get("methods", []):
                lines.append(f"        +{method}()")

            lines.append("    }")

        # Добавляем связи
        for edge in edges:
            if edge.type == "inherits":
                lines.append(f"    {edge.target} <|-- {edge.source}")
            elif edge.type == "uses":
                lines.append(f"    {edge.source} --> {edge.target}")

        return "\n".join(lines)

    def _generate_plantuml_class_diagram(
        self,
        nodes: list[DiagramNode],
        edges: list[DiagramEdge]
    ) -> str:
        """Сгенерировать PlantUML диаграмму классов."""
        lines = ["@startuml"]

        # Добавляем классы
        for node in nodes:
            lines.append(f"class {node.id} {{")

            # Атрибуты
            for attr in node.properties.get("attributes", []):
                lines.append(f"  +{attr}")

            # Методы
            for method in node.properties.get("methods", []):
                lines.append(f"  +{method}()")

            lines.append("}")

        # Добавляем связи
        for edge in edges:
            if edge.type == "inherits":
                lines.append(f"{edge.target} <|-- {edge.source}")
            elif edge.type == "uses":
                lines.append(f"{edge.source} --> {edge.target}")

        lines.append("@enduml")
        return "\n".join(lines)

    def _generate_ascii_class_diagram(
        self,
        nodes: list[DiagramNode],
        edges: list[DiagramEdge]
    ) -> str:
        """Сгенерировать ASCII диаграмму классов."""
        lines = []

        for node in nodes:
            lines.append("┌" + "─" * 30 + "┐")
            lines.append(f"│ {node.label:<28} │")
            lines.append("├" + "─" * 30 + "┤")

            # Атрибуты
            for attr in node.properties.get("attributes", [])[:3]:
                lines.append(f"│ + {attr:<26} │")

            # Методы
            for method in node.properties.get("methods", [])[:3]:
                lines.append(f"│ + {method}()<{' ' * (23 - len(method))} │")

            lines.append("└" + "─" * 30 + "┘")
            lines.append("")

        return "\n".join(lines)

    def generate_flowchart(
        self,
        steps: list[dict[str, Any]],
        format: str = "mermaid"
    ) -> ToolResult:
        """
        Сгенерировать блок-схему из списка шагов.

        Args:
            steps: Список шагов [{"id": "1", "label": "Start", "type": "start", "next": "2"}, ...]
            format: Формат диаграммы (mermaid, plantuml, ascii)

        Returns:
            ToolResult с диаграммой
        """
        try:
            if format == "mermaid":
                diagram = self._generate_mermaid_flowchart(steps)
            elif format == "plantuml":
                diagram = self._generate_plantuml_flowchart(steps)
            elif format == "ascii":
                diagram = self._generate_ascii_flowchart(steps)
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Unknown format: {format}"
                )

            return ToolResult(
                success=True,
                output=diagram,
                error=None
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to generate flowchart: {e}"
            )

    def _generate_mermaid_flowchart(self, steps: list[dict[str, Any]]) -> str:
        """Сгенерировать Mermaid блок-схему."""
        lines = ["flowchart TD"]

        for step in steps:
            step_id = step["id"]
            label = step["label"]
            step_type = step.get("type", "process")

            # Определяем форму узла
            if step_type == "start" or step_type == "end":
                lines.append(f"    {step_id}([{label}])")
            elif step_type == "decision":
                lines.append(f"    {step_id}{{{label}}}")
            else:
                lines.append(f"    {step_id}[{label}]")

            # Добавляем связи
            next_step = step.get("next")
            if next_step:
                lines.append(f"    {step_id} --> {next_step}")

            # Для условий добавляем yes/no ветки
            if step_type == "decision":
                yes_step = step.get("yes")
                no_step = step.get("no")
                if yes_step:
                    lines.append(f"    {step_id} -->|Yes| {yes_step}")
                if no_step:
                    lines.append(f"    {step_id} -->|No| {no_step}")

        return "\n".join(lines)

    def _generate_plantuml_flowchart(self, steps: list[dict[str, Any]]) -> str:
        """Сгенерировать PlantUML блок-схему."""
        lines = ["@startuml"]

        for step in steps:
            step_id = step["id"]
            label = step["label"]
            step_type = step.get("type", "process")

            if step_type == "start":
                lines.append(f"start")
                lines.append(f":{label};")
            elif step_type == "end":
                lines.append(f":{label};")
                lines.append(f"stop")
            elif step_type == "decision":
                lines.append(f"if ({label}) then (yes)")
                yes_step = step.get("yes")
                if yes_step:
                    lines.append(f"  :{yes_step};")
                lines.append(f"else (no)")
                no_step = step.get("no")
                if no_step:
                    lines.append(f"  :{no_step};")
                lines.append(f"endif")
            else:
                lines.append(f":{label};")

        lines.append("@enduml")
        return "\n".join(lines)

    def _generate_ascii_flowchart(self, steps: list[dict[str, Any]]) -> str:
        """Сгенерировать ASCII блок-схему."""
        lines = []

        for step in steps:
            label = step["label"]
            step_type = step.get("type", "process")

            if step_type == "start" or step_type == "end":
                lines.append("  ╔═══════════════╗")
                lines.append(f"  ║ {label:<13} ║")
                lines.append("  ╚═══════════════╝")
            elif step_type == "decision":
                lines.append("      ╱╲")
                lines.append(f"     ╱  ╲")
                lines.append(f"    ╱ {label[:4]:<4} ╲")
                lines.append("   ╱      ╲")
                lines.append("  ╱________╲")
            else:
                lines.append("  ┌───────────────┐")
                lines.append(f"  │ {label:<13} │")
                lines.append("  └───────────────┘")

            # Стрелка вниз
            if step.get("next"):
                lines.append("        │")
                lines.append("        ▼")

        return "\n".join(lines)

    def generate_sequence_diagram(
        self,
        interactions: list[dict[str, Any]],
        format: str = "mermaid"
    ) -> ToolResult:
        """
        Сгенерировать диаграмму последовательности.

        Args:
            interactions: Список взаимодействий [{"from": "A", "to": "B", "message": "call"}, ...]
            format: Формат диаграммы (mermaid, plantuml)

        Returns:
            ToolResult с диаграммой
        """
        try:
            if format == "mermaid":
                diagram = self._generate_mermaid_sequence(interactions)
            elif format == "plantuml":
                diagram = self._generate_plantuml_sequence(interactions)
            else:
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Unknown format: {format}"
                )

            return ToolResult(
                success=True,
                output=diagram,
                error=None
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to generate sequence diagram: {e}"
            )

    def _generate_mermaid_sequence(self, interactions: list[dict[str, Any]]) -> str:
        """Сгенерировать Mermaid диаграмму последовательности."""
        lines = ["sequenceDiagram"]

        for interaction in interactions:
            from_actor = interaction["from"]
            to_actor = interaction["to"]
            message = interaction["message"]
            arrow = interaction.get("arrow", "->")

            if arrow == "->":
                lines.append(f"    {from_actor}->>{to_actor}: {message}")
            elif arrow == "-->":
                lines.append(f"    {from_actor}-->>{to_actor}: {message}")

        return "\n".join(lines)

    def _generate_plantuml_sequence(self, interactions: list[dict[str, Any]]) -> str:
        """Сгенерировать PlantUML диаграмму последовательности."""
        lines = ["@startuml"]

        for interaction in interactions:
            from_actor = interaction["from"]
            to_actor = interaction["to"]
            message = interaction["message"]
            arrow = interaction.get("arrow", "->")

            lines.append(f"{from_actor} {arrow} {to_actor}: {message}")

        lines.append("@enduml")
        return "\n".join(lines)


# Пример использования
if __name__ == "__main__":
    generator = DiagramGenerator()

    # Генерируем блок-схему
    steps = [
        {"id": "1", "label": "Start", "type": "start", "next": "2"},
        {"id": "2", "label": "Process", "type": "process", "next": "3"},
        {"id": "3", "label": "Check?", "type": "decision", "yes": "4", "no": "5"},
        {"id": "4", "label": "Success", "type": "end"},
        {"id": "5", "label": "Failure", "type": "end"}
    ]

    result = generator.generate_flowchart(steps, format="mermaid")
    print(result.output)
