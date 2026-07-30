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
                parse_mode="HTML"
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
            await update.message.reply_text(text, parse_mode="HTML")

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
                    parse_mode="HTML"
                )
                return

            if not self.agent:
                await update.message.reply_text("❌ Агент не подключён")
                return

            text = update.message.text
            await update.message.reply_text("⚒ Кую...", parse_mode="HTML")

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
    parser.add_argument("--provider", choices=[
        "ollama", "openai", "anthropic", "openrouter",
        "koboldcpp", "llama_server", "custom",
    ])
    parser.add_argument("--model", help="Название модели")
    parser.add_argument("--base-url", help="Базовый URL (Ollama/llama_server/KoboldCPP/Custom)")
    parser.add_argument("--api-key", help="API ключ (OpenAI/Anthropic/OpenRouter/Custom)")
    parser.add_argument("--workspace", help="Рабочая папка агента")
    parser.add_argument("--temperature", type=float, default=0.1)
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
    from src.llm_client import LLMConfig, LLMProvider, auto_detect_provider

    # Тот же принцип выбора провайдера, что и в hephaestus.py:
    # если --provider не передан явно — автоопределение (env/доступные сервера),
    # а не жёсткая заглушка на ollama.
    if args.provider:
        provider = LLMProvider(args.provider)
        default_models = {
            "ollama": os.getenv("OLLAMA_MODEL", "llama3.2:3b"),
            "openai": "gpt-4o",
            "anthropic": "claude-sonnet-4-6",
            "openrouter": os.getenv("OPENROUTER_MODEL", "qwen/qwen-2.5-coder-32b-instruct"),
            "koboldcpp": "local-model",
        }
        model = args.model or default_models.get(args.provider, "default")
    else:
        provider, model = auto_detect_provider()
        if args.model:
            model = args.model

    # base_url: явный аргумент > переменные окружения под конкретный провайдер > None
    # (раньше здесь всегда подставлялся OLLAMA_HOST, даже для custom/openai/anthropic —
    # то есть бот физически не мог обратиться ни к чему кроме Ollama-совместимого сервера).
    base_url = args.base_url
    if not base_url:
        if provider == LLMProvider.OLLAMA:
            base_url = os.getenv("OLLAMA_HOST", "http://localhost:11434")
        elif provider == LLMProvider.LLAMA_SERVER:
            base_url = os.getenv("LLAMA_SERVER_URL", "http://127.0.0.1:8080")
        elif provider == LLMProvider.CUSTOM:
            base_url = os.getenv("CUSTOM_BASE_URL")
        elif provider == LLMProvider.KOBOLDCPP:
            base_url = os.getenv("KOBOLDCPP_URL")

    config = LLMConfig(
        provider=provider,
        model=model,
        api_key=args.api_key or os.getenv("LLM_API_KEY"),
        base_url=base_url,
        temperature=args.temperature,
    )
    agent = HephaestusAgent(llm_config=config, workspace_root=args.workspace)

    bot = HephaestusTelegramBot(token=token, agent=agent)
    bot.run()
