"""
Caching System - кеширование LLM запросов и результатов инструментов.
Оптимизация производительности и снижение затрат.
"""
from __future__ import annotations

import hashlib
import json
import time
from dataclasses import dataclass, asdict
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

from .real_tools import ToolResult


@dataclass
class CacheEntry:
    """Запись в кеше."""
    key: str
    value: Any
    created_at: float
    expires_at: float | None
    hits: int
    size_bytes: int


class CacheManager:
    """Менеджер кеша."""

    def __init__(self, workspace_root: str = ".", max_size_mb: int = 100):
        self.workspace_root = Path(workspace_root)
        self.cache_dir = Path.home() / ".claude_code" / "cache"
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        self.cache_file = self.cache_dir / "cache.json"
        self.max_size_bytes = max_size_mb * 1024 * 1024

        self.cache: dict[str, CacheEntry] = {}
        self.stats = {
            "hits": 0,
            "misses": 0,
            "evictions": 0
        }

        self._load_cache()

    def _load_cache(self) -> None:
        """Загрузить кеш из файла."""
        if not self.cache_file.exists():
            return

        try:
            with open(self.cache_file, 'r', encoding='utf-8') as f:
                data = json.load(f)

            # Загружаем записи
            for key, entry_data in data.get("entries", {}).items():
                entry = CacheEntry(**entry_data)

                # Проверяем не истек ли срок
                if entry.expires_at and time.time() > entry.expires_at:
                    continue

                self.cache[key] = entry

            # Загружаем статистику
            self.stats = data.get("stats", self.stats)

        except Exception as e:
            print(f"Warning: Failed to load cache: {e}")

    def _save_cache(self) -> None:
        """Сохранить кеш в файл."""
        try:
            data = {
                "entries": {key: asdict(entry) for key, entry in self.cache.items()},
                "stats": self.stats
            }

            with open(self.cache_file, 'w', encoding='utf-8') as f:
                json.dump(data, f, indent=2, ensure_ascii=False)

        except Exception as e:
            print(f"Error saving cache: {e}")

    def _generate_key(self, namespace: str, data: Any) -> str:
        """
        Сгенерировать ключ кеша.

        Args:
            namespace: Пространство имен (llm, tool, etc.)
            data: Данные для хеширования

        Returns:
            Ключ кеша
        """
        # Конвертируем в JSON для стабильного хеширования
        json_data = json.dumps(data, sort_keys=True, ensure_ascii=False)
        hash_obj = hashlib.sha256(json_data.encode())
        return f"{namespace}:{hash_obj.hexdigest()[:16]}"

    def _get_size(self, value: Any) -> int:
        """Получить размер значения в байтах."""
        try:
            json_str = json.dumps(value, ensure_ascii=False)
            return len(json_str.encode())
        except:
            return 0

    def _evict_if_needed(self, new_size: int) -> None:
        """Удалить старые записи если нужно место."""
        current_size = sum(entry.size_bytes for entry in self.cache.values())

        if current_size + new_size <= self.max_size_bytes:
            return

        # Сортируем по времени создания (LRU)
        sorted_entries = sorted(
            self.cache.items(),
            key=lambda x: (x[1].hits, x[1].created_at)
        )

        # Удаляем пока не освободим место
        for key, entry in sorted_entries:
            if current_size + new_size <= self.max_size_bytes:
                break

            del self.cache[key]
            current_size -= entry.size_bytes
            self.stats["evictions"] += 1

    def get(self, namespace: str, data: Any) -> Any | None:
        """
        Получить значение из кеша.

        Args:
            namespace: Пространство имен
            data: Данные для поиска

        Returns:
            Значение или None если не найдено
        """
        key = self._generate_key(namespace, data)
        entry = self.cache.get(key)

        if not entry:
            self.stats["misses"] += 1
            return None

        # Проверяем срок действия
        if entry.expires_at and time.time() > entry.expires_at:
            del self.cache[key]
            self.stats["misses"] += 1
            return None

        # Увеличиваем счетчик обращений
        entry.hits += 1
        self.stats["hits"] += 1

        return entry.value

    def set(
        self,
        namespace: str,
        data: Any,
        value: Any,
        ttl_seconds: int | None = None
    ) -> None:
        """
        Сохранить значение в кеш.

        Args:
            namespace: Пространство имен
            data: Данные для ключа
            value: Значение для сохранения
            ttl_seconds: Время жизни в секундах (None = бесконечно)
        """
        key = self._generate_key(namespace, data)
        size = self._get_size(value)

        # Проверяем нужно ли освободить место
        self._evict_if_needed(size)

        # Создаем запись
        now = time.time()
        expires_at = now + ttl_seconds if ttl_seconds else None

        entry = CacheEntry(
            key=key,
            value=value,
            created_at=now,
            expires_at=expires_at,
            hits=0,
            size_bytes=size
        )

        self.cache[key] = entry
        self._save_cache()

    def delete(self, namespace: str, data: Any) -> bool:
        """
        Удалить значение из кеша.

        Args:
            namespace: Пространство имен
            data: Данные для поиска

        Returns:
            True если удалено
        """
        key = self._generate_key(namespace, data)
        if key in self.cache:
            del self.cache[key]
            self._save_cache()
            return True
        return False

    def clear(self, namespace: str | None = None) -> int:
        """
        Очистить кеш.

        Args:
            namespace: Пространство имен (None = все)

        Returns:
            Количество удаленных записей
        """
        if namespace is None:
            count = len(self.cache)
            self.cache.clear()
            self._save_cache()
            return count

        # Удаляем только записи из указанного namespace
        keys_to_delete = [k for k in self.cache.keys() if k.startswith(f"{namespace}:")]
        for key in keys_to_delete:
            del self.cache[key]

        self._save_cache()
        return len(keys_to_delete)

    def get_stats(self) -> dict[str, Any]:
        """Получить статистику кеша."""
        total_size = sum(entry.size_bytes for entry in self.cache.values())
        hit_rate = 0.0
        total_requests = self.stats["hits"] + self.stats["misses"]

        if total_requests > 0:
            hit_rate = self.stats["hits"] / total_requests

        return {
            "entries": len(self.cache),
            "size_bytes": total_size,
            "size_mb": total_size / (1024 * 1024),
            "max_size_mb": self.max_size_bytes / (1024 * 1024),
            "hits": self.stats["hits"],
            "misses": self.stats["misses"],
            "hit_rate": hit_rate,
            "evictions": self.stats["evictions"]
        }


class LLMCache:
    """Кеш для LLM запросов."""

    def __init__(self, cache_manager: CacheManager, ttl_seconds: int = 3600):
        self.cache_manager = cache_manager
        self.ttl_seconds = ttl_seconds

    def get_response(
        self,
        messages: list[dict],
        model: str,
        system: str | None = None
    ) -> str | None:
        """
        Получить закешированный ответ.

        Args:
            messages: История сообщений
            model: Модель
            system: Системный промпт

        Returns:
            Ответ или None
        """
        cache_key = {
            "messages": messages,
            "model": model,
            "system": system
        }

        return self.cache_manager.get("llm", cache_key)

    def cache_response(
        self,
        messages: list[dict],
        model: str,
        response: str,
        system: str | None = None
    ) -> None:
        """
        Закешировать ответ.

        Args:
            messages: История сообщений
            model: Модель
            response: Ответ
            system: Системный промпт
        """
        cache_key = {
            "messages": messages,
            "model": model,
            "system": system
        }

        self.cache_manager.set("llm", cache_key, response, self.ttl_seconds)


class ToolCache:
    """Кеш для результатов инструментов."""

    def __init__(self, cache_manager: CacheManager, ttl_seconds: int = 300):
        self.cache_manager = cache_manager
        self.ttl_seconds = ttl_seconds

    def get_result(self, tool_name: str, params: dict) -> ToolResult | None:
        """
        Получить закешированный результат.

        Args:
            tool_name: Имя инструмента
            params: Параметры

        Returns:
            ToolResult или None
        """
        cache_key = {
            "tool": tool_name,
            "params": params
        }

        cached = self.cache_manager.get("tool", cache_key)
        if cached:
            return ToolResult(**cached)
        return None

    def cache_result(
        self,
        tool_name: str,
        params: dict,
        result: ToolResult
    ) -> None:
        """
        Закешировать результат.

        Args:
            tool_name: Имя инструмента
            params: Параметры
            result: Результат
        """
        cache_key = {
            "tool": tool_name,
            "params": params
        }

        # Конвертируем ToolResult в dict
        result_dict = {
            "success": result.success,
            "output": result.output,
            "error": result.error
        }

        self.cache_manager.set("tool", cache_key, result_dict, self.ttl_seconds)


# Пример использования
if __name__ == "__main__":
    # Создаем менеджер кеша
    cache_manager = CacheManager(max_size_mb=50)

    # LLM кеш
    llm_cache = LLMCache(cache_manager)

    messages = [{"role": "user", "content": "Hello"}]
    model = "llama3.2:1b"

    # Проверяем кеш
    cached = llm_cache.get_response(messages, model)
    if cached:
        print(f"Cache hit: {cached}")
    else:
        print("Cache miss")
        # Симулируем ответ
        response = "Hello! How can I help you?"
        llm_cache.cache_response(messages, model, response)
        print(f"Cached response: {response}")

    # Статистика
    stats = cache_manager.get_stats()
    print(f"\nCache stats:")
    print(f"  Entries: {stats['entries']}")
    print(f"  Size: {stats['size_mb']:.2f} MB")
    print(f"  Hit rate: {stats['hit_rate']:.2%}")
