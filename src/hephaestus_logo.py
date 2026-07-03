"""
Гефест — Logo Animation.
Анимация огня и молота бога кузнечного дела.
"""
from __future__ import annotations

import random
import time
from typing import List

from rich.console import Console
from rich.text import Text
from rich.panel import Panel

console = Console()

LOGO_ART = [
    "  ██╗  ██╗███████╗██████╗ ██╗  ██╗███████╗███████╗████████╗",
    "  ██║  ██║██╔════╝██╔══██╗██║  ██║██╔════╝██╔════╝╚══██╔══╝",
    "  ███████║█████╗  ██████╔╝███████║█████╗  ███████╗   ██║   ",
    "  ██╔══██║██╔══╝  ██╔═══╝ ██╔══██║██╔══╝  ╚════██║   ██║   ",
    "  ██║  ██║███████╗██║     ██║  ██║███████╗███████║   ██║   ",
    "  ╚═╝  ╚═╝╚══════╝╚═╝     ╚═╝  ╚═╝╚══════╝╚══════╝   ╚═╝   ",
]

FIRE_CHARS = ["░", "▒", "▓", "█", "🔥", "✦", "⚡"]
SPARKS = ["*", "·", "✦", "◦", "∘"]

FIRE_COLORS = [
    "bright_red", "red", "yellow", "bright_yellow",
    "orange1", "dark_orange", "bright_white",
]


def _fire_char(frame: int, col: int, row: int) -> tuple[str, str]:
    """Генерирует символ огня с цветом."""
    intensity = (frame + col * 3 + row * 7) % 20
    if intensity < 5:
        return random.choice(["▓", "█"]), "bright_red"
    elif intensity < 10:
        return random.choice(["▒", "▓"]), "red"
    elif intensity < 15:
        return random.choice(["░", "▒"]), "yellow"
    else:
        return random.choice(["·", " "]), "bright_yellow"


def show_logo_animated(duration: float = 2.5) -> None:
    """Анимированный старт с огнём."""
    try:
        import os
        frames = int(duration * 8)
        width = 62

        for frame in range(frames):
            os.system("clear" if os.name != "nt" else "cls")

            # Огонь сверху
            for row in range(3):
                line = Text()
                for col in range(width):
                    if random.random() < 0.4:
                        char, color = _fire_char(frame, col, row)
                        line.append(char, style=color)
                    else:
                        line.append(" ")
                console.print(line)

            # Логотип
            for i, art_line in enumerate(LOGO_ART):
                text = Text()
                for j, ch in enumerate(art_line):
                    if ch != " ":
                        # Мерцание
                        if (frame + i + j) % 7 == 0:
                            text.append(ch, style="bright_yellow bold")
                        else:
                            text.append(ch, style="bold red")
                    else:
                        text.append(ch)
                console.print(text)

            # Огонь снизу
            for row in range(2):
                line = Text()
                for col in range(width):
                    if random.random() < 0.35:
                        char, color = _fire_char(frame + 5, col, row)
                        line.append(char, style=color)
                    else:
                        line.append(" ")
                console.print(line)

            # Подпись
            spark = random.choice(SPARKS)
            console.print(
                f"\n[bold yellow]  {spark} AI Coding Agent · God of Forge & Tools {spark}[/bold yellow]\n",
            )

            time.sleep(0.12)

    except Exception:
        show_logo_simple()


def show_logo_simple() -> None:
    """Простой логотип без анимации."""
    logo = "\n".join(f"  [bold red]{line}[/bold red]" for line in LOGO_ART)
    console.print(logo)
    console.print("\n  [bold yellow]⚡ AI Coding Agent · God of Forge & Tools ⚡[/bold yellow]\n")


def show_startup_banner(provider: str = "?", model: str = "?", tools: int = 0, workspace: str = ".") -> None:
    """Баннер после логотипа."""
    banner = (
        f"[bold red]Гефест[/bold red] [dim]—[/dim] [bold yellow]Бог кузнечного дела и инструментов[/bold yellow]\n\n"
        f"[dim]Провайдер:[/dim]  [cyan]{provider}[/cyan]\n"
        f"[dim]Модель:[/dim]     [cyan]{model}[/cyan]\n"
        f"[dim]Инструменты:[/dim] [cyan]{tools}[/cyan]\n"
        f"[dim]Рабочая папка:[/dim] [cyan]{workspace}[/cyan]\n\n"
        f"[dim]/help — команды  ·  /tools — инструменты  ·  /exit — выход[/dim]"
    )
    console.print(Panel(
        banner,
        border_style="red",
        padding=(1, 3),
        title="[bold red]⚡ HEPHAESTUS ⚡[/bold red]",
        title_align="center",
    ))
    console.print()
