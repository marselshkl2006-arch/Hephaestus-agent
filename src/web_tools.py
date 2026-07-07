"""
Web Tools - инструменты для работы с веб-ресурсами.

Включает:
- WebFetch - загрузка и обработка веб-страниц
- WebSearch - поиск в интернете
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any

try:
    import requests
    HAS_REQUESTS = True
except ImportError:
    HAS_REQUESTS = False

try:
    from bs4 import BeautifulSoup
    HAS_BS4 = True
except ImportError:
    HAS_BS4 = False


@dataclass
class WebFetchResult:
    """Результат загрузки веб-страницы."""
    success: bool
    url: str
    title: str | None = None
    content: str | None = None
    markdown: str | None = None
    error: str | None = None


@dataclass
class SearchResult:
    """Результат поиска."""
    title: str
    url: str
    snippet: str


class WebFetchTool:
    """Инструмент для загрузки веб-страниц."""

    def __init__(self, timeout: int = 30):
        """
        Инициализация WebFetch.

        Args:
            timeout: Таймаут запроса в секундах
        """
        if not HAS_REQUESTS:
            raise ImportError("requests library required. Install: pip install requests")
        self.timeout = timeout

    def _html_to_markdown(self, html: str, url: str) -> str:
        """
        Конвертировать HTML в Markdown.

        Args:
            html: HTML контент
            url: URL страницы

        Returns:
            Markdown текст
        """
        if not HAS_BS4:
            # Fallback: простая очистка HTML
            text = re.sub(r'<script[^>]*>.*?</script>', '', html, flags=re.DOTALL)
            text = re.sub(r'<style[^>]*>.*?</style>', '', text, flags=re.DOTALL)
            text = re.sub(r'<[^>]+>', '', text)
            text = re.sub(r'\s+', ' ', text)
            return text.strip()

        # Используем BeautifulSoup для парсинга
        soup = BeautifulSoup(html, 'html.parser')

        # Удаляем скрипты и стили
        for script in soup(["script", "style"]):
            script.decompose()

        # Извлекаем текст
        lines = []

        # Заголовок
        title = soup.find('title')
        if title:
            lines.append(f"# {title.get_text().strip()}\n")

        # Основной контент
        main = soup.find('main') or soup.find('article') or soup.find('body')
        if main:
            # Заголовки
            for h in main.find_all(['h1', 'h2', 'h3', 'h4', 'h5', 'h6']):
                level = int(h.name[1])
                lines.append(f"\n{'#' * level} {h.get_text().strip()}\n")

            # Параграфы
            for p in main.find_all('p'):
                text = p.get_text().strip()
                if text:
                    lines.append(f"{text}\n")

            # Списки
            for ul in main.find_all('ul'):
                for li in ul.find_all('li'):
                    lines.append(f"- {li.get_text().strip()}")
                lines.append("")

            # Ссылки
            for a in main.find_all('a', href=True):
                text = a.get_text().strip()
                href = a['href']
                if text and href:
                    lines.append(f"[{text}]({href})")

        markdown = '\n'.join(lines)
        markdown = re.sub(r'\n{3,}', '\n\n', markdown)  # Убираем лишние переносы
        return markdown.strip()

    def fetch(self, url: str, prompt: str | None = None) -> WebFetchResult:
        """
        Загрузить и обработать веб-страницу.

        Args:
            url: URL для загрузки
            prompt: Опциональный промпт для обработки контента через LLM

        Returns:
            Результат загрузки
        """
        try:
            # Загружаем страницу
            response = requests.get(
                url,
                timeout=self.timeout,
                headers={'User-Agent': 'Mozilla/5.0 (compatible; ClaudeCode/1.0)'}
            )
            response.raise_for_status()

            # Конвертируем в Markdown
            markdown = self._html_to_markdown(response.text, url)

            # Извлекаем заголовок
            title_match = re.search(r'^# (.+)$', markdown, re.MULTILINE)
            title = title_match.group(1) if title_match else None

            return WebFetchResult(
                success=True,
                url=url,
                title=title,
                content=response.text[:1000],  # Первые 1000 символов HTML
                markdown=markdown,
            )

        except Exception as e:
            return WebFetchResult(
                success=False,
                url=url,
                error=str(e),
            )


class WebSearchTool:
    """Инструмент для поиска в интернете."""

    def __init__(self, api_key: str | None = None):
        """
        Инициализация WebSearch.

        Args:
            api_key: API ключ для поискового сервиса (опционально)
        """
        if not HAS_REQUESTS:
            raise ImportError("requests library required. Install: pip install requests")
        self.api_key = api_key

    def search(
        self,
        query: str,
        max_results: int = 5,
    ) -> list[SearchResult]:
        """
        Поиск в интернете через DuckDuckGo HTML.

        Args:
            query: Поисковый запрос
            max_results: Максимальное количество результатов

        Returns:
            Список результатов поиска
        """
        try:
            # Используем DuckDuckGo HTML (не требует API ключа)
            headers = {
                'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36'
            }

            params = {
                'q': query,
                'kl': 'ru-ru',  # Регион
            }

            response = requests.get(
                'https://html.duckduckgo.com/html/',
                params=params,
                headers=headers,
                timeout=10
            )

            if response.status_code != 200:
                return self._fallback_search(query)

            # Парсим результаты
            results = []

            if HAS_BS4:
                soup = BeautifulSoup(response.text, 'html.parser')

                # Ищем результаты поиска
                for result_div in soup.find_all('div', class_='result')[:max_results]:
                    try:
                        # Заголовок и ссылка
                        title_tag = result_div.find('a', class_='result__a')
                        if not title_tag:
                            continue

                        title = title_tag.get_text().strip()
                        url = title_tag.get('href', '')

                        # Сниппет
                        snippet_tag = result_div.find('a', class_='result__snippet')
                        snippet = snippet_tag.get_text().strip() if snippet_tag else ""

                        if title and url:
                            results.append(SearchResult(
                                title=title,
                                url=url,
                                snippet=snippet
                            ))
                    except Exception:
                        continue

            if results:
                return results

            # Fallback если парсинг не сработал
            return self._fallback_search(query)

        except Exception as e:
            print(f"⚠️ Ошибка поиска: {e}")
            return self._fallback_search(query)

    def _fallback_search(self, query: str) -> list[SearchResult]:
        """Реальный поиск через DuckDuckGo Instant Answer API (бесплатно, без ключа)."""
        import urllib.parse
        results = []

        # Пробуем DuckDuckGo Instant Answer API
        try:
            import requests as _req
            encoded = urllib.parse.quote(query)
            # DuckDuckGo HTML поиск
            url = f"https://html.duckduckgo.com/html/?q={encoded}"
            headers = {"User-Agent": "Mozilla/5.0 (compatible; HephaestusBot/1.0)"}
            resp = _req.get(url, headers=headers, timeout=10)
            if resp.status_code == 200:
                from bs4 import BeautifulSoup
                soup = BeautifulSoup(resp.text, "html.parser")
                for result_div in soup.select(".result__body")[:8]:
                    title_el = result_div.select_one(".result__title")
                    url_el = result_div.select_one(".result__url")
                    snippet_el = result_div.select_one(".result__snippet")
                    if title_el and url_el:
                        title = title_el.get_text(strip=True)
                        href = title_el.select_one("a")
                        link = href["href"] if href and href.get("href") else str(url_el.get_text(strip=True))
                        # Убираем DuckDuckGo redirect
                        if "uddg=" in link:
                            link = urllib.parse.unquote(link.split("uddg=")[1].split("&")[0])
                        snippet = snippet_el.get_text(strip=True) if snippet_el else ""
                        results.append(SearchResult(title=title, url=link, snippet=snippet))
                if results:
                    return results
        except Exception:
            pass

        # Fallback на Bing (парсинг HTML)
        try:
            import requests as _req
            encoded = urllib.parse.quote(query)
            url = f"https://www.bing.com/search?q={encoded}"
            headers = {"User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"}
            resp = _req.get(url, headers=headers, timeout=10)
            if resp.status_code == 200:
                from bs4 import BeautifulSoup
                soup = BeautifulSoup(resp.text, "html.parser")
                for li in soup.select("li.b_algo")[:6]:
                    h2 = li.select_one("h2 a")
                    snippet_el = li.select_one(".b_caption p")
                    if h2:
                        results.append(SearchResult(
                            title=h2.get_text(strip=True),
                            url=h2.get("href", ""),
                            snippet=snippet_el.get_text(strip=True) if snippet_el else ""
                        ))
                if results:
                    return results
        except Exception:
            pass

        # Последний fallback — хотя бы ссылки
        encoded_query = urllib.parse.quote(query)
        return [
            SearchResult(
                title=f"DuckDuckGo: {query}",
                url=f"https://duckduckgo.com/?q={encoded_query}",
                snippet="Откройте для поиска в DuckDuckGo"
            ),
            SearchResult(
                title=f"Google: {query}",
                url=f"https://www.google.com/search?q={encoded_query}",
                snippet="Откройте для поиска в Google"
            ),
        ]


def create_web_tools(api_key: str | None = None) -> dict[str, Any]:
    """
    Создать web инструменты.

    Args:
        api_key: API ключ для поискового сервиса

    Returns:
        Словарь с инструментами
    """
    tools = {}

    if HAS_REQUESTS:
        tools['web_fetch'] = WebFetchTool()
        tools['web_search'] = WebSearchTool(api_key=api_key)

    return tools
