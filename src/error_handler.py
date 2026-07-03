"""
Система обработки ошибок для Claude Code.
Показывает пользователю понятные сообщения, логирует технические детали.
"""
import sys
import traceback
from pathlib import Path
from datetime import datetime
from typing import Optional


def get_error_log_file() -> Path:
    """Получить путь к файлу логов ошибок."""
    home = Path.home()
    log_dir = home / ".claude_code" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)

    date_str = datetime.now().strftime("%Y-%m-%d")
    return log_dir / f"errors_{date_str}.log"


def log_error_to_file(error: Exception, context: str = ""):
    """
    Логировать ошибку в файл (не показывать пользователю).

    Args:
        error: Исключение
        context: Контекст где произошла ошибка
    """
    try:
        log_file = get_error_log_file()

        with open(log_file, 'a', encoding='utf-8') as f:
            timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
            f.write(f"\n{'='*60}\n")
            f.write(f"[{timestamp}] Error in {context}\n")
            f.write(f"{'='*60}\n")
            f.write(f"Error: {error}\n")
            f.write(f"Type: {type(error).__name__}\n")
            f.write(f"\nTraceback:\n")
            traceback.print_exc(file=f)
            f.write(f"\n")
    except:
        # Если не можем записать в лог - ничего страшного
        pass


def get_user_friendly_error(error: Exception, context: str = "") -> str:
    """
    Получить понятное пользователю сообщение об ошибке.

    Args:
        error: Исключение
        context: Контекст где произошла ошибка

    Returns:
        Понятное сообщение для пользователя
    """
    error_type = type(error).__name__
    error_msg = str(error)

    # Логируем техническую информацию в файл
    log_error_to_file(error, context)

    # Возвращаем понятное сообщение пользователю
    if isinstance(error, FileNotFoundError):
        return f"❌ Файл не найден: {error_msg}\n💡 Проверьте путь к файлу"

    elif isinstance(error, PermissionError):
        return f"❌ Нет доступа: {error_msg}\n💡 Проверьте права доступа к файлу"

    elif isinstance(error, ConnectionError) or "connection" in error_msg.lower():
        return f"❌ Ошибка подключения\n💡 Проверьте интернет-соединение или доступность сервера"

    elif isinstance(error, TimeoutError) or "timeout" in error_msg.lower():
        return f"❌ Превышено время ожидания\n💡 Попробуйте ещё раз или проверьте соединение"

    elif isinstance(error, KeyError):
        return f"❌ Не найден ключ: {error_msg}\n💡 Проверьте конфигурацию"

    elif isinstance(error, ValueError):
        return f"❌ Неверное значение: {error_msg}\n💡 Проверьте введённые данные"

    elif isinstance(error, ImportError):
        return f"❌ Не установлена библиотека\n💡 Установите: pip install {error_msg.split()[-1] if error_msg else 'requirements'}"

    elif "503" in error_msg or "busy" in error_msg.lower():
        return f"❌ Сервер занят\n💡 Подождите немного и попробуйте снова"

    elif "401" in error_msg or "unauthorized" in error_msg.lower():
        return f"❌ Ошибка авторизации\n💡 Проверьте API ключ"

    elif "404" in error_msg:
        return f"❌ Ресурс не найден\n💡 Проверьте URL или путь"

    else:
        # Общая ошибка - не показываем технические детали
        log_file = get_error_log_file()
        return (
            f"❌ Произошла ошибка\n"
            f"💡 Подробности сохранены в: {log_file}\n"
            f"💡 Попробуйте ещё раз или обратитесь за помощью"
        )


def handle_error(error: Exception, context: str = "", show_traceback: bool = False) -> str:
    """
    Обработать ошибку: залогировать и вернуть понятное сообщение.

    Args:
        error: Исключение
        context: Контекст где произошла ошибка
        show_traceback: Показать traceback (только для отладки!)

    Returns:
        Сообщение для пользователя
    """
    # Логируем в файл
    log_error_to_file(error, context)

    # Если режим отладки - показываем traceback
    if show_traceback:
        traceback.print_exc()

    # Возвращаем понятное сообщение
    return get_user_friendly_error(error, context)


# Примеры использования:
if __name__ == "__main__":
    print("🧪 Тест системы обработки ошибок\n")

    # Тест 1: FileNotFoundError
    try:
        open("/nonexistent/file.txt")
    except Exception as e:
        print(handle_error(e, "reading file"))
        print()

    # Тест 2: ConnectionError
    try:
        raise ConnectionError("Failed to connect to server")
    except Exception as e:
        print(handle_error(e, "connecting to server"))
        print()

    # Тест 3: Общая ошибка
    try:
        raise RuntimeError("Something went wrong")
    except Exception as e:
        print(handle_error(e, "processing request"))
        print()

    print(f"📁 Логи ошибок: {get_error_log_file()}")
