"""
Session Manager — сохранение и загрузка сессий диалога.
"""
from __future__ import annotations

import json
import os
from datetime import datetime
from pathlib import Path
from dataclasses import dataclass, asdict


SESSIONS_DIR = Path.home() / ".hephaestus" / "sessions"


@dataclass
class Session:
    session_id: str
    created_at: str
    updated_at: str
    messages: list[dict]
    summary: str = ""
    tags: list[str] = None

    def __post_init__(self):
        if self.tags is None:
            self.tags = []


class SessionManager:
    def __init__(self):
        SESSIONS_DIR.mkdir(parents=True, exist_ok=True)

    def save(self, messages: list, summary: str = "") -> str:
        """Сохранить текущую сессию. Возвращает session_id."""
        now = datetime.now().strftime("%Y%m%d_%H%M%S")
        session_id = f"ses_{now}"

        session = Session(
            session_id=session_id,
            created_at=now,
            updated_at=now,
            messages=[{"role": m.role, "content": m.content if isinstance(m.content, str) else str(m.content)} for m in messages],
            summary=summary or self._auto_summary(messages),
        )

        path = SESSIONS_DIR / f"{session_id}.json"
        with open(path, "w", encoding="utf-8") as f:
            json.dump(asdict(session), f, ensure_ascii=False, indent=2)

        return session_id

    def load(self, session_id: str) -> list[dict] | None:
        """Загрузить сессию по ID. Возвращает список сообщений."""
        path = SESSIONS_DIR / f"{session_id}.json"
        if not path.exists():
            # Попробуем частичное совпадение
            matches = list(SESSIONS_DIR.glob(f"*{session_id}*.json"))
            if not matches:
                return None
            path = matches[0]

        with open(path, encoding="utf-8") as f:
            data = json.load(f)
        return data.get("messages", [])

    def list_sessions(self, limit: int = 10) -> list[dict]:
        """Список последних сессий."""
        sessions = []
        for p in sorted(SESSIONS_DIR.glob("*.json"), reverse=True)[:limit]:
            try:
                with open(p, encoding="utf-8") as f:
                    data = json.load(f)
                sessions.append({
                    "id": data["session_id"],
                    "created": data["created_at"],
                    "messages": len(data.get("messages", [])),
                    "summary": data.get("summary", "")[:60],
                })
            except Exception:
                continue
        return sessions

    def delete(self, session_id: str) -> bool:
        path = SESSIONS_DIR / f"{session_id}.json"
        if path.exists():
            path.unlink()
            return True
        return False

    def _auto_summary(self, messages: list) -> str:
        """Авто-краткое описание из первого сообщения."""
        for m in messages:
            if hasattr(m, 'role') and m.role == "user":
                content = m.content if isinstance(m.content, str) else ""
                return content[:80]
        return "Без описания"
