"""
Автоопределение доступных LLM моделей и провайдеров.
"""
from __future__ import annotations

import os
from dataclasses import dataclass

try:
    import requests
    HAS_REQUESTS = True
except ImportError:
    HAS_REQUESTS = False


@dataclass
class AvailableModel:
    """Доступная модель."""
    provider: str
    model: str
    name: str
    size: str | None = None
    speed: str = "medium"  # fast, medium, slow


def detect_ollama_models() -> list[AvailableModel]:
    """Определить доступные модели Ollama."""
    if not HAS_REQUESTS:
        return []

    try:
        response = requests.get("http://localhost:11434/api/tags", timeout=2)
        if response.status_code == 200:
            data = response.json()
            models = []
            for model_info in data.get("models", []):
                name = model_info.get("name", "")
                size = model_info.get("details", {}).get("parameter_size", "")

                # Определяем скорость по размеру
                speed = "medium"
                if "1.5b" in name.lower() or "1b" in name.lower():
                    speed = "fast"
                elif "7b" in name.lower():
                    speed = "medium"
                elif "13b" in name.lower() or "70b" in name.lower():
                    speed = "slow"

                models.append(AvailableModel(
                    provider="ollama",
                    model=name,
                    name=f"Ollama: {name}",
                    size=size,
                    speed=speed
                ))
            return models
    except Exception:
        pass

    return []


def detect_anthropic_available() -> list[AvailableModel]:
    """Проверить доступность Anthropic API."""
    if os.getenv("ANTHROPIC_API_KEY"):
        return [
            AvailableModel(
                provider="anthropic",
                model="claude-3-5-sonnet-20241022",
                name="Anthropic: Claude 3.5 Sonnet",
                speed="fast"
            ),
            AvailableModel(
                provider="anthropic",
                model="claude-3-opus-20240229",
                name="Anthropic: Claude 3 Opus",
                speed="medium"
            ),
        ]
    return []


def detect_openai_available() -> list[AvailableModel]:
    """Проверить доступность OpenAI API."""
    if os.getenv("OPENAI_API_KEY"):
        return [
            AvailableModel(
                provider="openai",
                model="gpt-4o",
                name="OpenAI: GPT-4o",
                speed="fast"
            ),
            AvailableModel(
                provider="openai",
                model="gpt-4o-mini",
                name="OpenAI: GPT-4o Mini",
                speed="fast"
            ),
            AvailableModel(
                provider="openai",
                model="gpt-4-turbo",
                name="OpenAI: GPT-4 Turbo",
                speed="medium"
            ),
        ]
    return []


def detect_openrouter_available() -> list[AvailableModel]:
    """Проверить доступность OpenRouter API."""
    if os.getenv("OPENROUTER_API_KEY"):
        # Получаем модель из переменной окружения или используем дефолтную
        model = os.getenv("OPENROUTER_MODEL", "qwen/qwen-2.5-coder-32b-instruct")

        # Определяем скорость по названию модели
        speed = "fast"
        if "free" in model.lower():
            speed = "fast"
        elif "32b" in model.lower() or "70b" in model.lower():
            speed = "medium"
        elif "405b" in model.lower():
            speed = "slow"

        return [
            AvailableModel(
                provider="openrouter",
                model=model,
                name=f"OpenRouter: {model}",
                speed=speed
            ),
        ]
    return []


def detect_koboldcpp_available() -> list[AvailableModel]:
    """Проверить доступность KoboldCPP сервера."""
    if not HAS_REQUESTS:
        return []

    # Используем умную систему автопоиска
    try:
        from .koboldcpp_discovery import find_koboldcpp_server
        koboldcpp_url = find_koboldcpp_server(timeout=2.0)

        if koboldcpp_url:
            try:
                response = requests.get(f"{koboldcpp_url}/api/v1/model", timeout=2)
                if response.status_code == 200:
                    data = response.json()
                    model_name = data.get("result", "local-model")

                    return [
                        AvailableModel(
                            provider="koboldcpp",
                            model=model_name,
                            name=f"KoboldCPP: {model_name}",
                            speed="fast"
                        )
                    ]
            except Exception:
                pass

    except ImportError:
        # Fallback если модуль discovery не доступен
        pass

    # Fallback: проверяем известные адреса
    possible_urls = [
        os.getenv("KOBOLDCPP_URL", "http://localhost:5001"),
        "http://localhost:5001",
        "http://192.168.1.136:5001",
        "http://127.0.0.1:5001",
    ]

    for koboldcpp_url in possible_urls:
        try:
            response = requests.get(f"{koboldcpp_url}/api/v1/model", timeout=2)
            if response.status_code == 200:
                data = response.json()
                model_name = data.get("result", "local-model")

                return [
                    AvailableModel(
                        provider="koboldcpp",
                        model=model_name,
                        name=f"KoboldCPP: {model_name}",
                        speed="fast"
                    )
                ]
        except Exception:
            continue

    return []



def detect_all_models() -> list[AvailableModel]:
    """Определить все доступные модели с приоритетом на локальные."""
    models = []
    
    # 1. KoboldCPP (локальный сервер - ВЫСШИЙ ПРИОРИТЕТ)
    kobold_models = detect_koboldcpp_available()
    if kobold_models:
        models.extend(kobold_models)
        print(f"✅ Найдено KoboldCPP моделей: {len(kobold_models)}")
    
    # 2. Ollama (локальные модели)
    ollama_models = detect_ollama_models()
    if ollama_models:
        models.extend(ollama_models)
        print(f"✅ Найдено Ollama моделей: {len(ollama_models)}")
    
    # 3. OpenRouter (только если нет локальных)
    if not models:
        openrouter_models = detect_openrouter_available()
        if openrouter_models:
            models.extend(openrouter_models)
            print(f"✅ Найдено OpenRouter моделей: {len(openrouter_models)}")
    
    # 4. API провайдеры (только если нет локальных)
    if not models:
        models.extend(detect_anthropic_available())
        models.extend(detect_openai_available())
    
    return models

def get_recommended_model() -> AvailableModel | None:
    """Получить рекомендуемую модель (KoboldCPP > Ollama > OpenRouter > API)."""
    models = detect_all_models()
    
    if not models:
        return None
    
    # Приоритет: KoboldCPP > Ollama > OpenRouter > Anthropic > OpenAI
    for model in models:
        if model.provider == "koboldcpp":
            return model
    
    for model in models:
        if model.provider == "ollama":
            return model
    
    for model in models:
        if model.provider == "openrouter":
            return model
    
    for model in models:
        if model.provider == "anthropic":
            return model
    
    return models[0]
    """Получить самую быструю доступную модель."""
    models = detect_all_models()

    if not models:
        return None

    # Сортируем: сначала fast, потом medium, потом slow
    speed_order = {"fast": 0, "medium": 1, "slow": 2}
    models.sort(key=lambda m: speed_order.get(m.speed, 3))

    return models[0]


def get_recommended_model() -> AvailableModel | None:
    """Получить рекомендуемую модель (баланс скорости и качества)."""
    models = detect_all_models()

    if not models:
        return None

    # Приоритет: OpenRouter > Anthropic > OpenAI > Ollama medium > Ollama fast
    for model in models:
        if model.provider == "openrouter":
            return model

    for model in models:
        if model.provider == "anthropic":
            return model

    for model in models:
        if model.provider == "openai":
            return model

    for model in models:
        if model.provider == "ollama" and model.speed == "medium":
            return model

    return models[0]


def print_available_models():
    """Вывести список доступных моделей."""
    models = detect_all_models()

    if not models:
        print("❌ Нет доступных моделей")
        print("\nУстановите:")
        print("  - Ollama: https://ollama.com")
        print("  - OpenRouter API: export OPENROUTER_API_KEY='sk-or-v1-...'")
        print("  - Anthropic API: export ANTHROPIC_API_KEY='sk-ant-...'")
        print("  - OpenAI API: export OPENAI_API_KEY='sk-...'")
        return

    print(f"✅ Найдено моделей: {len(models)}\n")

    # Группируем по провайдерам
    by_provider = {}
    for model in models:
        if model.provider not in by_provider:
            by_provider[model.provider] = []
        by_provider[model.provider].append(model)

    for provider, provider_models in by_provider.items():
        print(f"📦 {provider.upper()}:")
        for model in provider_models:
            speed_emoji = {"fast": "⚡", "medium": "🔄", "slow": "🐌"}.get(model.speed, "❓")
            size_info = f" ({model.size})" if model.size else ""
            print(f"   {speed_emoji} {model.model}{size_info}")
        print()

    # Рекомендации
    fastest = get_fastest_model()
    recommended = get_recommended_model()

    if fastest:
        print(f"⚡ Самая быстрая: {fastest.name}")
    if recommended and recommended != fastest:
        print(f"⭐ Рекомендуемая: {recommended.name}")


if __name__ == "__main__":
    print_available_models()
