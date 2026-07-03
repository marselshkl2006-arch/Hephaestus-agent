"""
AskUserQuestion - инструмент для интерактивных вопросов с вариантами ответов.

Аналог AskUserQuestion из оригинального Claude Code.
Позволяет задавать вопросы с 2-4 вариантами ответов, поддерживает single/multi select,
опциональные preview для визуального сравнения вариантов.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

try:
    from rich.console import Console
    from rich.panel import Panel
    from rich.prompt import Prompt
    from rich.syntax import Syntax
    from rich.table import Table
    from rich.text import Text
    HAS_RICH = True
except ImportError:
    HAS_RICH = False


@dataclass
class QuestionOption:
    """Вариант ответа на вопрос."""
    label: str
    description: str
    preview: str | None = None


@dataclass
class Question:
    """Вопрос с вариантами ответов."""
    question: str
    header: str  # Короткий заголовок (max 12 chars)
    options: list[QuestionOption]
    multi_select: bool = False


@dataclass
class QuestionAnswer:
    """Ответ на вопрос."""
    question: str
    selected: list[str]  # Выбранные labels
    notes: str | None = None  # Дополнительные заметки пользователя


class AskUserQuestionTool:
    """Инструмент для задавания вопросов пользователю."""

    def __init__(self, use_rich: bool = True):
        """
        Инициализация инструмента.

        Args:
            use_rich: Использовать Rich для форматирования
        """
        self.use_rich = use_rich and HAS_RICH
        self.console = Console() if self.use_rich else None

    def _print_question(self, question: Question) -> None:
        """Вывести вопрос с вариантами."""
        if self.use_rich and self.console:
            # Rich форматирование
            self.console.print()
            self.console.print(f"[bold cyan]{question.header}[/bold cyan]")
            self.console.print(f"[bold]{question.question}[/bold]")
            self.console.print()

            # Таблица с вариантами
            table = Table(show_header=True, header_style="bold magenta", box=None)
            table.add_column("#", style="dim", width=3)
            table.add_column("Option", style="cyan")
            table.add_column("Description", style="white")

            for i, option in enumerate(question.options, 1):
                table.add_row(
                    str(i),
                    option.label,
                    option.description,
                )

            self.console.print(table)

            # Preview если есть
            if any(opt.preview for opt in question.options):
                self.console.print()
                self.console.print("[dim]Previews available - enter number to see preview[/dim]")

        else:
            # Простой вывод
            print(f"\n{question.header}")
            print(f"{question.question}\n")
            for i, option in enumerate(question.options, 1):
                print(f"  {i}. {option.label}")
                print(f"     {option.description}")
            print()

    def _show_preview(self, option: QuestionOption) -> None:
        """Показать preview для варианта."""
        if not option.preview:
            return

        if self.use_rich and self.console:
            self.console.print()
            panel = Panel(
                option.preview,
                title=f"Preview: {option.label}",
                border_style="cyan",
            )
            self.console.print(panel)
        else:
            print(f"\n--- Preview: {option.label} ---")
            print(option.preview)
            print("---\n")

    def _get_selection(
        self,
        question: Question,
    ) -> tuple[list[str], str | None]:
        """
        Получить выбор пользователя.

        Returns:
            (selected_labels, notes)
        """
        while True:
            if self.use_rich and self.console:
                if question.multi_select:
                    prompt_text = "[bold]Select options[/bold] (comma-separated numbers, or 'p<N>' for preview): "
                else:
                    prompt_text = "[bold]Select option[/bold] (number, or 'p<N>' for preview): "
                self.console.print(prompt_text, end="")
                choice = input().strip()
            else:
                if question.multi_select:
                    choice = input("Select options (comma-separated numbers): ").strip()
                else:
                    choice = input("Select option (number): ").strip()

            # Preview команда
            if choice.lower().startswith('p'):
                try:
                    preview_num = int(choice[1:])
                    if 1 <= preview_num <= len(question.options):
                        self._show_preview(question.options[preview_num - 1])
                        continue
                except (ValueError, IndexError):
                    pass
                print("Invalid preview command. Use 'p<number>' (e.g., 'p1')")
                continue

            # Парсинг выбора
            try:
                if question.multi_select:
                    # Множественный выбор
                    numbers = [int(n.strip()) for n in choice.split(',')]
                    selected = []
                    for num in numbers:
                        if 1 <= num <= len(question.options):
                            selected.append(question.options[num - 1].label)
                        else:
                            raise ValueError(f"Invalid option number: {num}")

                    if not selected:
                        print("Please select at least one option")
                        continue

                    return selected, None

                else:
                    # Одиночный выбор
                    num = int(choice)
                    if 1 <= num <= len(question.options):
                        return [question.options[num - 1].label], None
                    else:
                        print(f"Please enter a number between 1 and {len(question.options)}")
                        continue

            except ValueError as e:
                print(f"Invalid input: {e}")
                continue

    def ask(
        self,
        questions: list[Question],
    ) -> dict[str, QuestionAnswer]:
        """
        Задать вопросы пользователю.

        Args:
            questions: Список вопросов (1-4)

        Returns:
            Словарь ответов {question_text: answer}
        """
        if not questions or len(questions) > 4:
            raise ValueError("Must provide 1-4 questions")

        answers = {}

        for question in questions:
            if len(question.options) < 2 or len(question.options) > 4:
                raise ValueError(f"Question must have 2-4 options, got {len(question.options)}")

            # Показываем вопрос
            self._print_question(question)

            # Получаем ответ
            selected, notes = self._get_selection(question)

            # Сохраняем ответ
            answers[question.question] = QuestionAnswer(
                question=question.question,
                selected=selected,
                notes=notes,
            )

        return answers


def ask_user_question(
    questions: list[dict],
    use_rich: bool = True,
) -> dict[str, dict]:
    """
    Удобная функция для задавания вопросов.

    Args:
        questions: Список вопросов в формате dict
        use_rich: Использовать Rich для форматирования

    Returns:
        Словарь ответов

    Example:
        >>> questions = [
        ...     {
        ...         "question": "Which library should we use?",
        ...         "header": "Library",
        ...         "multi_select": False,
        ...         "options": [
        ...             {"label": "React", "description": "Popular UI library"},
        ...             {"label": "Vue", "description": "Progressive framework"},
        ...         ]
        ...     }
        ... ]
        >>> answers = ask_user_question(questions)
    """
    tool = AskUserQuestionTool(use_rich=use_rich)

    # Конвертируем dict в Question объекты
    question_objects = []
    for q in questions:
        options = [
            QuestionOption(
                label=opt["label"],
                description=opt["description"],
                preview=opt.get("preview"),
            )
            for opt in q["options"]
        ]
        question_objects.append(
            Question(
                question=q["question"],
                header=q["header"],
                options=options,
                multi_select=q.get("multi_select", False),
            )
        )

    # Задаем вопросы
    answers = tool.ask(question_objects)

    # Конвертируем в dict
    return {
        q: {
            "selected": a.selected,
            "notes": a.notes,
        }
        for q, a in answers.items()
    }
