"""
Telegram Bot — управление Гефестом через Telegram.
Запуск: python3 -m src.telegram_bot --token YOUR_TOKEN
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import threading
from pathlib import Path

logger = logging.getLogger(__name__)

TELEGRAM_TOKEN_FILE = Path.home() / ".hephaestus" / "telegram_token.txt"


class HephaestusTelegramBot:
    """Telegram бот для управления Гефестом."""

    def __init__(self, token: str, agent=None):
        self.token = token
        self.agent = agent
        self._allowed_users: set[int] = set()
        self._load_allowed_users()

    def _load_allowed_users(self):
        """Загружаем список разрешённых пользователей."""
        cfg = Path.home() / ".hephaestus" / "telegram_users.json"
        if cfg.exists():
            try:
                data = json.loads(cfg.read_text())
                self._allowed_users = set(data.get("users", []))
            except Exception:
                pass

    def add_allowed_user(self, user_id: int):
        self._allowed_users.add(user_id)
        cfg = Path.home() / ".hephaestus" / "telegram_users.json"
        cfg.parent.mkdir(parents=True, exist_ok=True)
        cfg.write_text(json.dumps({"users": list(self._allowed_users)}))

    def run(self):
        """Запустить бота."""
        try:
            from telegram import Update
            from telegram.ext import (
                Application, CommandHandler, MessageHandler,
                filters, ContextTypes
            )
        except ImportError:
            print("❌ Установи: pip install python-telegram-bot")
            return

        async def start(update: Update, ctx: ContextTypes.DEFAULT_TYPE):
            user_id = update.effective_user.id
            username = update.effective_user.username or "unknown"
            await update.message.reply_text(
                f"⚡ Гефест активен!\n"
                f"Твой ID: `{user_id}`\n"
                f"Username: @{username}\n\n"
                f"Команды:\n"
                f"/start — приветствие\n"
                f"/reset — очистить историю\n"
                f"/tools — список инструментов\n"
                f"/allow <id> — добавить пользователя\n\n"
                f"Просто пиши запросы и я их выполню!",
                parse_mode="Markdown"
            )

        async def reset(update: Update, ctx: ContextTypes.DEFAULT_TYPE):
            if self.agent:
                self.agent.reset()
            await update.message.reply_text("✅ История очищена")

        async def tools_cmd(update: Update, ctx: ContextTypes.DEFAULT_TYPE):
            if not self.agent:
                await update.message.reply_text("❌ Агент не подключён")
                return
            schemas = self.agent._get_tools_schema()
            text = f"⚒ Инструменты ({len(schemas)}):\n"
            for s in schemas[:20]:
                text += f"• `{s['name']}` — {s['description'][:40]}\n"
            if len(schemas) > 20:
                text += f"...и ещё {len(schemas)-20}"
            await update.message.reply_text(text, parse_mode="Markdown")

        async def allow_cmd(update: Update, ctx: ContextTypes.DEFAULT_TYPE):
            if ctx.args:
                try:
                    uid = int(ctx.args[0])
                    self.add_allowed_user(uid)
                    await update.message.reply_text(f"✅ Пользователь {uid} добавлен")
                except ValueError:
                    await update.message.reply_text("❌ Укажи числовой ID")

        async def handle_message(update: Update, ctx: ContextTypes.DEFAULT_TYPE):
            user_id = update.effective_user.id

            # Проверка доступа
            if self._allowed_users and user_id not in self._allowed_users:
                await update.message.reply_text(
                    f"⛔ Доступ запрещён.\nТвой ID: `{user_id}`\n"
                    f"Попроси владельца добавить тебя через /allow {user_id}",
                    parse_mode="Markdown"
                )
                return

            if not self.agent:
                await update.message.reply_text("❌ Агент не подключён")
                return

            text = update.message.text
            await update.message.reply_text("⚒ Кую...", parse_mode="Markdown")

            # Запускаем агента в отдельном потоке
            loop = asyncio.get_event_loop()
            response = await loop.run_in_executor(None, self.agent.chat, text)

            # Telegram ограничение — 4096 символов
            if len(response) > 4000:
                parts = [response[i:i+4000] for i in range(0, len(response), 4000)]
                for part in parts:
                    await update.message.reply_text(part)
            else:
                await update.message.reply_text(response or "✅ Готово")

        app = Application.builder().token(self.token).build()
        app.add_handler(CommandHandler("start", start))
        app.add_handler(CommandHandler("reset", reset))
        app.add_handler(CommandHandler("tools", tools_cmd))
        app.add_handler(CommandHandler("allow", allow_cmd))
        app.add_handler(MessageHandler(filters.TEXT & ~filters.COMMAND, handle_message))

        print(f"⚡ Гефест Telegram бот запущен")
        app.run_polling(allowed_updates=Update.ALL_TYPES)


def save_token(token: str):
    TELEGRAM_TOKEN_FILE.parent.mkdir(parents=True, exist_ok=True)
    TELEGRAM_TOKEN_FILE.write_text(token)
    print(f"✓ Токен сохранён: {TELEGRAM_TOKEN_FILE}")


def load_token() -> str | None:
    if TELEGRAM_TOKEN_FILE.exists():
        return TELEGRAM_TOKEN_FILE.read_text().strip()
    return os.getenv("TELEGRAM_BOT_TOKEN")


if __name__ == "__main__":
    import argparse
    import sys
    sys.path.insert(0, str(Path(__file__).parent.parent))

    parser = argparse.ArgumentParser(description="Гефест Telegram Bot")
    parser.add_argument("--token", help="Telegram Bot Token")
    parser.add_argument("--save-token", help="Сохранить токен")
    parser.add_argument("--provider", default="ollama")
    parser.add_argument("--model", default="qwen2.5-coder:7b-instruct-q4_K_M")
    args = parser.parse_args()

    if args.save_token:
        save_token(args.save_token)
        sys.exit(0)

    token = args.token or load_token()
    if not token:
        print("❌ Укажи токен: --token YOUR_TOKEN")
        print("   Получи токен у @BotFather в Telegram")
        sys.exit(1)

    from src.hephaestus import HephaestusAgent
    from src.llm_client import LLMConfig, LLMProvider

    config = LLMConfig(
        provider=LLMProvider(args.provider),
        model=args.model,
        base_url=os.getenv("OLLAMA_HOST", "http://localhost:11434"),
    )
    agent = HephaestusAgent(llm_config=config)

    bot = HephaestusTelegramBot(token=token, agent=agent)
    bot.run()
