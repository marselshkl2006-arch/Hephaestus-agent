"""
Context Learning — контекстное обучение агента.
Агент запоминает паттерны ошибок и успехов, не повторяет их.
"""
from __future__ import annotations

import json
from pathlib import Path
from datetime import datetime


LEARNING_FILE = Path.home() / ".hephaestus" / "learning.json"


class ContextLearning:
    """Запоминает что работало и что нет."""

    def __init__(self):
        LEARNING_FILE.parent.mkdir(parents=True, exist_ok=True)
        self.data = self._load()

    def _load(self) -> dict:
        if LEARNING_FILE.exists():
            try:
                return json.loads(LEARNING_FILE.read_text(encoding="utf-8"))
            except Exception:
                pass
        return {"successes": [], "failures": [], "preferences": {}}

    def _save(self):
        LEARNING_FILE.write_text(
            json.dumps(self.data, ensure_ascii=False, indent=2),
            encoding="utf-8"
        )

    def record_success(self, tool: str, params: dict, context: str = ""):
        """Запомнить успешный вызов."""
        entry = {
            "tool": tool,
            "params_keys": list(params.keys()),
            "context": context[:100],
            "ts": datetime.now().isoformat(),
        }
        self.data["successes"].append(entry)
        self.data["successes"] = self.data["successes"][-100:]  # храним 100
        self._save()

    def record_failure(self, tool: str, error: str, params: dict):
        """Запомнить ошибку."""
        entry = {
            "tool": tool,
            "error": error[:200],
            "params_keys": list(params.keys()),
            "ts": datetime.now().isoformat(),
        }
        self.data["failures"].append(entry)
        self.data["failures"] = self.data["failures"][-50:]
        self._save()

    def set_preference(self, key: str, value: str):
        """Запомнить предпочтение пользователя."""
        self.data["preferences"][key] = value
        self._save()

    def get_context_hint(self) -> str:
        """Получить подсказку для системного промпта на основе опыта."""
        hints = []

        # Частые ошибки
        error_tools = {}
        for f in self.data["failures"]:
            t = f["tool"]
            error_tools[t] = error_tools.get(t, 0) + 1

        if error_tools:
            worst = max(error_tools, key=error_tools.get)
            if error_tools[worst] >= 2:
                hints.append(f"ВНИМАНИЕ: инструмент '{worst}' часто вызывает ошибки — будь особенно аккуратен.")

        # Предпочтения
        prefs = self.data.get("preferences", {})
        if prefs:
            pref_str = ", ".join(f"{k}: {v}" for k, v in list(prefs.items())[:5])
            hints.append(f"Предпочтения пользователя: {pref_str}")

        return "\n".join(hints)

    def get_stats(self) -> str:
        s = len(self.data["successes"])
        f = len(self.data["failures"])
        prefs = len(self.data["preferences"])
        return f"Успехов: {s} | Ошибок: {f} | Предпочтений: {prefs}"
