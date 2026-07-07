"""
Web Surfer — полноценный веб-сёрфинг для Гефеста.
Поиск, парсинг страниц, следование ссылкам, извлечение данных.
"""
from __future__ import annotations

import json
import re
import urllib.parse
from pathlib import Path

from .real_tools import ToolResult


class WebSurferTool:
    """Полноценный веб-сёрфинг — поиск и парсинг страниц."""

    def __init__(self):
        self._session = None

    def _get_session(self):
        if self._session is None:
            try:
                import requests
                s = requests.Session()
                s.headers.update({
                    "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
                                  "(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                    "Accept-Language": "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7",
                })
                self._session = s
            except ImportError:
                return None
        return self._session

    def search(self, query: str, max_results: int = 8,
               engine: str = "duckduckgo") -> ToolResult:
        """
        Поиск в интернете. Возвращает реальные результаты с URL и сниппетами.
        engine: duckduckgo (default), bing
        """
        try:
            if engine == "duckduckgo":
                return self._search_ddg(query, max_results)
            else:
                return self._search_bing(query, max_results)
        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def _search_ddg(self, query: str, max_results: int) -> ToolResult:
        """DuckDuckGo поиск — пробует несколько методов."""
        import requests

        session = self._get_session()
        results = []

        # Метод 1: DDG HTML search (самый надёжный)
        try:
            from bs4 import BeautifulSoup
            encoded = urllib.parse.quote(query)
            url = f"https://html.duckduckgo.com/html/?q={encoded}"
            resp = session.get(url, timeout=12)
            soup = BeautifulSoup(resp.text, "html.parser")

            for r in soup.select(".result__body")[:max_results]:
                title_el = r.select_one(".result__title a")
                snippet_el = r.select_one(".result__snippet")
                if not title_el:
                    continue
                title = title_el.get_text(strip=True)
                href = title_el.get("href", "")
                if "uddg=" in href:
                    try:
                        href = urllib.parse.unquote(href.split("uddg=")[1].split("&")[0])
                    except Exception:
                        pass
                snippet = snippet_el.get_text(strip=True) if snippet_el else ""
                if href and title and href.startswith("http"):
                    results.append({"title": title, "url": href, "snippet": snippet})
        except Exception:
            pass

        # Метод 2: DDG Lite если основной не дал результатов
        if not results:
            try:
                from bs4 import BeautifulSoup
                encoded = urllib.parse.quote(query)
                url = f"https://lite.duckduckgo.com/lite/?q={encoded}"
                resp = session.get(url, timeout=12)
                soup = BeautifulSoup(resp.text, "html.parser")
                for row in soup.select("tr")[:max_results * 3]:
                    link = row.select_one("a.result-link")
                    snip = row.select_one(".result-snippet")
                    if link and link.get("href", "").startswith("http"):
                        results.append({
                            "title": link.get_text(strip=True),
                            "url": link["href"],
                            "snippet": snip.get_text(strip=True) if snip else ""
                        })
            except Exception:
                pass

        if not results:
            # Fallback на Bing
            return self._search_bing(query, max_results)

        lines = [f"🔍 Результаты поиска: «{query}»\n"]
        for i, r in enumerate(results[:max_results], 1):
            lines.append(f"{i}. **{r['title']}**")
            lines.append(f"   🔗 {r['url']}")
            if r["snippet"]:
                lines.append(f"   {r['snippet']}")
            lines.append("")
        return ToolResult(success=True, output="\n".join(lines))

    def _search_bing(self, query: str, max_results: int) -> ToolResult:
        """Bing поиск как fallback."""
        import requests
        from bs4 import BeautifulSoup

        session = self._get_session()
        encoded = urllib.parse.quote(query)
        url = f"https://www.bing.com/search?q={encoded}&setlang=ru"

        resp = session.get(url, timeout=15)
        soup = BeautifulSoup(resp.text, "html.parser")
        results = []

        for li in soup.select("li.b_algo")[:max_results]:
            h2 = li.select_one("h2 a")
            caption = li.select_one(".b_caption p")
            if h2:
                results.append({
                    "title": h2.get_text(strip=True),
                    "url": h2.get("href", ""),
                    "snippet": caption.get_text(strip=True) if caption else "",
                })

        if not results:
            return ToolResult(success=False, output="", error="Bing не вернул результаты")

        lines = [f"🔍 Результаты Bing: «{query}»\n"]
        for i, r in enumerate(results, 1):
            lines.append(f"{i}. **{r['title']}**")
            lines.append(f"   🔗 {r['url']}")
            if r["snippet"]:
                lines.append(f"   {r['snippet']}")
            lines.append("")

        return ToolResult(success=True, output="\n".join(lines))

    def fetch_page(self, url: str, extract: str = "text",
                   save_to: str = "") -> ToolResult:
        """
        Загрузить страницу и извлечь содержимое.
        extract: text (чистый текст), links (все ссылки), both
        """
        try:
            import requests
            from bs4 import BeautifulSoup

            session = self._get_session()
            resp = session.get(url, timeout=20)
            resp.raise_for_status()
            resp.encoding = resp.apparent_encoding or "utf-8"

            soup = BeautifulSoup(resp.text, "html.parser")

            # Убираем мусор
            for tag in soup(["script", "style", "nav", "footer",
                              "header", "aside", "form", "iframe"]):
                tag.decompose()

            result_parts = [f"📄 {url}\n"]

            if extract in ("text", "both"):
                # Пытаемся использовать trafilatura если есть
                try:
                    import trafilatura
                    text = trafilatura.extract(resp.text, include_links=False,
                                               include_images=False)
                    if text:
                        result_parts.append(text[:4000])
                    else:
                        raise ValueError("trafilatura вернул пустой результат")
                except Exception:
                    # Fallback: извлекаем текст вручную
                    main = (soup.find("main") or soup.find("article") or
                            soup.find(id="content") or soup.find(class_="content") or
                            soup.body or soup)
                    text = main.get_text(separator="\n", strip=True)
                    # Убираем лишние пустые строки
                    lines = [l.strip() for l in text.split("\n") if l.strip()]
                    result_parts.append("\n".join(lines[:200]))

            if extract in ("links", "both"):
                links = []
                for a in soup.find_all("a", href=True):
                    href = a["href"]
                    if href.startswith("http") and a.get_text(strip=True):
                        links.append(f"  • {a.get_text(strip=True)[:60]} → {href}")
                if links:
                    result_parts.append(f"\n🔗 Ссылки на странице ({len(links)}):")
                    result_parts.extend(links[:30])

            output = "\n".join(result_parts)

            if save_to:
                Path(save_to).parent.mkdir(parents=True, exist_ok=True)
                Path(save_to).write_text(output, encoding="utf-8")
                output += f"\n\n💾 Сохранено: {save_to}"

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def research(self, query: str, depth: int = 3,
                 save_to: str = "") -> ToolResult:
        """
        Глубокий поиск: ищет → открывает топ-N страниц → собирает информацию.
        depth: сколько страниц открыть (1-5)
        """
        try:
            # 1. Поиск
            search_result = self.search(query, max_results=depth + 2)
            if not search_result.success:
                return search_result

            # Извлекаем URL из результатов
            urls = re.findall(r"🔗 (https?://[^\s]+)", search_result.output)[:depth]

            lines = [f"🔬 Исследование: «{query}»\n",
                     f"Найдено источников: {len(urls)}\n",
                     "=" * 50, ""]

            # 2. Читаем каждую страницу
            for i, url in enumerate(urls, 1):
                lines.append(f"## Источник {i}: {url}")
                page = self.fetch_page(url)
                if page.success:
                    # Берём первые 800 символов
                    content = page.output.split("\n", 1)[1] if "\n" in page.output else page.output
                    lines.append(content[:800])
                else:
                    lines.append(f"❌ Не удалось загрузить: {page.error}")
                lines.append("")

            output = "\n".join(lines)

            if save_to:
                Path(save_to).parent.mkdir(parents=True, exist_ok=True)
                Path(save_to).write_text(output, encoding="utf-8")
                output += f"\n\n💾 Сохранено: {save_to}"

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))


def create_web_surfer() -> dict:
    tool = WebSurferTool()
    return {"web_surfer": tool}
