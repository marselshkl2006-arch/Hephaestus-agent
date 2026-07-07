"""
HTTP Tools - инструменты для работы с REST API.
Поддержка GET, POST, PUT, DELETE запросов.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

try:
    import requests
    HAS_REQUESTS = True
except ImportError:
    HAS_REQUESTS = False


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str = ""


class HttpRequestTool:
    """HTTP запросы к REST API."""

    def __init__(self, timeout: int = 30):
        """
        Инициализация HttpRequestTool.

        Args:
            timeout: Таймаут запроса в секундах
        """
        if not HAS_REQUESTS:
            raise ImportError("requests library required. Install: pip install requests")
        self.timeout = timeout

    def request(
        self,
        url: str,
        method: str = "GET",
        headers: dict[str, str] | None = None,
        data: dict[str, Any] | str | None = None,
        params: dict[str, str] | None = None,
        auth: tuple[str, str] | None = None,
        json_data: dict[str, Any] | None = None
    ) -> ToolResult:
        """
        Выполнить HTTP запрос.

        Args:
            url: URL для запроса
            method: HTTP метод (GET, POST, PUT, DELETE, PATCH)
            headers: HTTP заголовки
            data: Данные для отправки (form data)
            params: Query параметры
            auth: Кортеж (username, password) для Basic Auth
            json_data: JSON данные для отправки

        Returns:
            ToolResult с ответом сервера
        """
        try:
            method = method.upper()

            # Подготовка заголовков
            if headers is None:
                headers = {}

            # Если не указан Content-Type и есть json_data - устанавливаем
            if json_data and "Content-Type" not in headers:
                headers["Content-Type"] = "application/json"

            # Выполняем запрос
            response = requests.request(
                method=method,
                url=url,
                headers=headers,
                data=data,
                json=json_data,
                params=params,
                auth=auth,
                timeout=self.timeout
            )

            # Формируем результат
            output = f"Status: {response.status_code} {response.reason}\n"
            output += f"URL: {response.url}\n"
            output += f"\nHeaders:\n"
            for key, value in response.headers.items():
                output += f"  {key}: {value}\n"

            output += f"\nBody:\n"

            # Пытаемся распарсить JSON
            try:
                json_body = response.json()
                output += json.dumps(json_body, indent=2, ensure_ascii=False)
            except:
                # Если не JSON - выводим как текст
                output += response.text[:1000]  # Первые 1000 символов

            # Проверяем успешность
            success = 200 <= response.status_code < 300

            return ToolResult(
                success=success,
                output=output,
                error="" if success else f"HTTP {response.status_code}: {response.reason}"
            )

        except requests.exceptions.Timeout:
            return ToolResult(
                success=False,
                output="",
                error=f"Request timeout ({self.timeout}s)"
            )
        except requests.exceptions.ConnectionError as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Connection error: {str(e)}"
            )
        except requests.exceptions.RequestException as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Request error: {str(e)}"
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def get(self, url: str, **kwargs) -> ToolResult:
        """GET запрос."""
        return self.request(url, method="GET", **kwargs)

    def post(self, url: str, **kwargs) -> ToolResult:
        """POST запрос."""
        return self.request(url, method="POST", **kwargs)

    def put(self, url: str, **kwargs) -> ToolResult:
        """PUT запрос."""
        return self.request(url, method="PUT", **kwargs)

    def delete(self, url: str, **kwargs) -> ToolResult:
        """DELETE запрос."""
        return self.request(url, method="DELETE", **kwargs)

    def patch(self, url: str, **kwargs) -> ToolResult:
        """PATCH запрос."""
        return self.request(url, method="PATCH", **kwargs)


class ApiDocsTool:
    """Получение документации API (OpenAPI/Swagger)."""

    def fetch_openapi(self, url: str) -> ToolResult:
        """
        Загрузить OpenAPI/Swagger документацию.

        Args:
            url: URL к OpenAPI спецификации (обычно /openapi.json или /swagger.json)

        Returns:
            ToolResult с документацией API
        """
        if not HAS_REQUESTS:
            return ToolResult(
                success=False,
                output="",
                error="requests library required. Install: pip install requests"
            )

        try:
            response = requests.get(url, timeout=30)
            response.raise_for_status()

            spec = response.json()

            # Формируем читаемый вывод
            output = f"API: {spec.get('info', {}).get('title', 'Unknown')}\n"
            output += f"Version: {spec.get('info', {}).get('version', 'Unknown')}\n"
            output += f"Description: {spec.get('info', {}).get('description', 'N/A')}\n"
            output += f"\nBase URL: {spec.get('servers', [{}])[0].get('url', 'N/A')}\n"

            # Список endpoints
            output += f"\nEndpoints:\n"
            paths = spec.get('paths', {})
            for path, methods in paths.items():
                for method, details in methods.items():
                    if method.upper() in ['GET', 'POST', 'PUT', 'DELETE', 'PATCH']:
                        summary = details.get('summary', 'No description')
                        output += f"  {method.upper():6} {path:40} - {summary}\n"

            return ToolResult(
                success=True,
                output=output
            )

        except requests.exceptions.RequestException as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to fetch API docs: {str(e)}"
            )
        except json.JSONDecodeError:
            return ToolResult(
                success=False,
                output="",
                error="Invalid OpenAPI specification (not valid JSON)"
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )
