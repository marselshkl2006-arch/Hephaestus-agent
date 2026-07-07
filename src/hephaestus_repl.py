"""
Гефест — REPL (Read-Eval-Print Loop).
Интерактивный режим с полным дизайном.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Callable

try:
    import readline
    HAS_READLINE = True
except ImportError:
    HAS_READLINE = False

from rich.console import Console
from rich.markdown import Markdown
from rich.progress import Progress, SpinnerColumn, TextColumn
from rich.prompt import Prompt, Confirm
from rich.panel import Panel
from rich.table import Table
from rich.text import Text
from rich.syntax import Syntax

from .hephaestus_logo import show_logo_animated, show_startup_banner

console = Console()


# ─── UI helpers ─────────────────────────────────────────────────────────────

class HephaestusPrompt:
    """Промпт в стиле Гефеста."""

    def __init__(self):
        self.multiline_buffer: list[str] = []

    def get_input(self) -> str:
        try:
            try:
                from prompt_toolkit import prompt as pt_prompt
                from prompt_toolkit.styles import Style
                prompt_str = "⚡ гефест> "
                style = Style.from_dict({"": "bold ansired"})
                line = pt_prompt(prompt_str, style=style)
            except Exception:
                import sys
                sys.stdout.write("\033[1;31m⚡ гефест\033[0m\033[1;33m>\033[0m ")
                sys.stdout.flush()
                line = input()

            if len(line) > 100:
                console.print("[dim]📋 большой текст обнаружен[/dim]")

            if line.endswith("\\"):
                self.multiline_buffer.append(line[:-1])
                console.print("[dim]... (продолжение)[/dim]")
                return self.get_input()

            if self.multiline_buffer:
                self.multiline_buffer.append(line)
                result = "\n".join(self.multiline_buffer)
                self.multiline_buffer = []
                return result

            return line
        except (KeyboardInterrupt, EOFError):
            raise


class ForgeSpinner:
    """Спиннер Гефеста — кузница работает."""

    VERBS = [
        "⚒️  Кую...", "🔥 Плавлю...", "⚡ Обрабатываю...",
        "🔧 Собираю...", "💭 Думаю...", "🛠️  Создаю...",
        "🔩 Анализирую...", "⚙️  Выполняю...",
    ]

    def __init__(self, action: str = "Работаю"):
        self.action = action
        self._idx = 0

    def __enter__(self):
        self._progress = Progress(
            SpinnerColumn(spinner_name="bouncingBar", style="bold red"),
            TextColumn("[bold yellow]{task.description}"),
            console=console,
        )
        self._progress.start()
        self._task = self._progress.add_task(self.action, total=None)
        return self

    def __exit__(self, *_):
        self._progress.stop()

    def update(self, text: str | None = None):
        label = text or self.VERBS[self._idx % len(self.VERBS)]
        self._progress.update(self._task, description=label)
        self._idx += 1


def show_success(msg: str) -> None:
    console.print(f"[bold green]✅ {msg}[/bold green]")

def show_error(msg: str) -> None:
    console.print(f"[bold red]❌ {msg}[/bold red]")

def show_warning(msg: str) -> None:
    console.print(f"[bold yellow]⚠️  {msg}[/bold yellow]")

def show_info(msg: str) -> None:
    console.print(f"[bold cyan]ℹ️  {msg}[/bold cyan]")

def show_tool_call(name: str, params: str) -> None:
    console.print(f"\n[bold red]⚒️  [/bold red][yellow]{name}[/yellow][dim]({params})[/dim]")

def show_tool_result(output: str, success: bool = True) -> None:
    if success:
        preview = output[:200].replace("\n", " ") if output else "OK"
        console.print(f"   [green]✓[/green] [dim]{preview}[/dim]")
    else:
        console.print(f"   [red]✗[/red] [dim]{output[:200]}[/dim]")

def show_thinking(text: str = "") -> None:
    if text:
        console.print(f"\n[dim]💭 {text.strip()[:200]}[/dim]")

def show_debug(tool_name: str, params: dict, result, elapsed_ms: float) -> None:
    """Отладочный вывод — показывает реальные данные о вызове инструмента."""
    import json
    console.print(f"\n[dim]🐛 DEBUG ── {tool_name} ──────────────────[/dim]")
    console.print(f"[dim]  Параметры: {json.dumps(params, ensure_ascii=False, indent=2)}[/dim]")
    console.print(f"[dim]  Время: {elapsed_ms:.1f}ms | Успех: {result.success}[/dim]")
    if result.output:
        preview = result.output[:300].replace("\n", "↵ ")
        console.print(f"[dim]  Вывод: {preview}[/dim]")
    if result.error:
        console.print(f"[dim]  Ошибка: {result.error[:200]}[/dim]")
    console.print(f"[dim]{'─' * 45}[/dim]")

def print_response(text: str) -> None:
    console.print()
    console.print(Markdown(text))
    console.print()


# ─── REPL ────────────────────────────────────────────────────────────────────

class HephaestusREPL:
    """Полный REPL для агента Гефест."""

    HISTORY_FILE = Path.home() / ".hephaestus_history"

    def __init__(self):
        self.running = False
        self.prompt = HephaestusPrompt()
        self._setup_readline()

    def _setup_readline(self):
        if not HAS_READLINE:
            return
        if self.HISTORY_FILE.exists():
            try:
                readline.read_history_file(str(self.HISTORY_FILE))
            except Exception:
                pass
        readline.set_history_length(1000)
        readline.parse_and_bind("tab: complete")

    def _save_readline(self):
        if not HAS_READLINE:
            return
        try:
            self.HISTORY_FILE.parent.mkdir(parents=True, exist_ok=True)
            readline.write_history_file(str(self.HISTORY_FILE))
        except Exception:
            pass

    def _show_help(self) -> None:
        help_text = """
# ⚡ Гефест — Команды

## Навигация
- `/help` — эта справка
- `/exit`, `/quit` — выход
- `/clear` — очистить экран
- `/reset` — очистить историю диалога

## Инструменты
- `/tools` — список всех инструментов
- `/models` — доступные модели

## История
- `/history` — последние команды

## Сессии
- `/sessions` — список сохранённых сессий
- `/save` — сохранить текущую сессию
- `/load <id>` — загрузить сессию по ID

## Обучение
- `/learn` — статистика контекстного обучения

## Советы
- Многострочный ввод: закончи строку на `\\`
- Ctrl+C — прервать текущий запрос
- Ctrl+D — выход
"""
        console.print(Markdown(help_text))

    def _show_tools(self, agent) -> None:
        schemas = agent._get_tools_schema()
        table = Table(
            title=f"⚒️  Инструменты Гефеста ({len(schemas)})",
            border_style="red",
            header_style="bold yellow",
            show_lines=False,
        )
        table.add_column("Инструмент", style="cyan", width=22)
        table.add_column("Описание", style="white")

        for s in schemas:
            table.add_row(s["name"], s["description"][:65])

        console.print(table)
        console.print()

    def run(self, agent, on_message: Callable[[str], str]) -> None:
        """Запустить REPL."""
        self.running = True

        # Логотип
        try:
            show_logo_animated(duration=2.0)
        except Exception:
            pass

        # Баннер
        show_startup_banner(
            provider=agent.llm_config.provider.value,
            model=agent.llm_config.model,
            tools=len(agent._get_tools_schema()),
            workspace=str(agent.workspace_root),
        )

        while self.running:
            try:
                line = self.prompt.get_input()
            except EOFError:
                if agent.messages:
                    sid = agent.save_session()
                    console.print(f"\n[bold yellow]⚡ До свидания![/bold yellow] [dim]Сессия: {sid}[/dim]\n")
                else:
                    show_success("\nДо свидания! ⚡")
                break
            except KeyboardInterrupt:
                show_info("\nПрервано")
                continue

            line = line.strip()
            if not line:
                continue

            # Команды
            if line.startswith("/"):
                parts = line.split(maxsplit=1)
                cmd = parts[0]
                args = parts[1] if len(parts) > 1 else ""

                if cmd in ("/exit", "/quit"):
                    if agent.messages:
                        sid = agent.save_session()
                        console.print(f"\n[bold yellow]⚡ До свидания, Марсель![/bold yellow]")
                        console.print(f"[dim]Сессия сохранена: [cyan]{sid}[/cyan][/dim]")
                        console.print(f"[dim]Загрузить: /load {sid}[/dim]\n")
                    else:
                        show_success("До свидания! ⚡")
                    break
                elif cmd == "/help":
                    self._show_help()
                elif cmd == "/clear":
                    os.system("clear" if os.name != "nt" else "cls")
                elif cmd == "/reset":
                    agent.reset()
                    show_success("История диалога очищена")
                elif cmd == "/tools":
                    self._show_tools(agent)
                elif cmd == "/sessions":
                    sessions = agent.session_manager.list_sessions()
                    if not sessions:
                        show_info("Нет сохранённых сессий")
                    else:
                        console.print("\n[bold]Последние сессии:[/bold]")
                        for s in sessions:
                            console.print(f"  [cyan]{s['id']}[/cyan] — {s['summary']} ([dim]{s['messages']} сообщ.[/dim])")
                        console.print()
                elif cmd == "/load":
                    if not args:
                        show_error("Укажи ID сессии: /load ses_20260614_123456")
                    elif agent.load_session(args):
                        show_success(f"Сессия {args} загружена ({len(agent.messages)} сообщений)")
                    else:
                        show_error(f"Сессия {args} не найдена. Используй /sessions")
                elif cmd == "/save":
                    sid = agent.save_session()
                    show_success(f"Сессия сохранена: {sid}")
                elif cmd == "/learn":
                    show_info(agent.learning.get_stats())
                elif cmd == "/history":
                    if HAS_READLINE:
                        n = readline.get_current_history_length()
                        console.print("\n[bold]История команд:[/bold]")
                        for i in range(max(1, n - 19), n + 1):
                            item = readline.get_history_item(i)
                            if item:
                                console.print(f"  [dim]{i}.[/dim] {item}")
                        console.print()
                    else:
                        show_warning("readline не доступен")
                elif cmd == "/models":
                    show_info(f"Текущая модель: {agent.llm_config.model}")
                else:
                    show_error(f"Неизвестная команда: {cmd}")
                    show_info("Введи /help для справки")
                continue

            # Запрос к агенту
            try:
                response = on_message(line)
                print_response(response)
            except KeyboardInterrupt:
                show_info("\nЗапрос прерван")
            except Exception as e:
                show_error(f"Ошибка: {e}")

        self._save_readline()
