"""
OCR Tool — распознавание текста с изображений.
Использует tesseract (нативный) или pytesseract.
"""
from __future__ import annotations

import subprocess
import tempfile
from pathlib import Path

from .real_tools import ToolResult


class OCRTool:
    """Распознавание текста с изображений через Tesseract."""

    SUPPORTED = [".png", ".jpg", ".jpeg", ".bmp", ".tiff", ".tif", ".gif", ".webp"]

    def __init__(self):
        self._has_tesseract = self._check_tesseract()
        self._has_pytesseract = self._check_pytesseract()

    def _check_tesseract(self) -> bool:
        try:
            r = subprocess.run(["tesseract", "--version"], capture_output=True, timeout=5)
            return r.returncode == 0
        except (FileNotFoundError, subprocess.TimeoutExpired):
            return False

    def _check_pytesseract(self) -> bool:
        try:
            import pytesseract
            return True
        except ImportError:
            return False

    def extract_text(
        self,
        image_path: str,
        lang: str = "rus+eng",
        psm: int = 3,
    ) -> ToolResult:
        """
        Извлечь текст из изображения.

        Args:
            image_path: Путь к изображению
            lang: Язык(и) для OCR (rus, eng, rus+eng)
            psm: Page Segmentation Mode (3=auto, 6=single block, 11=sparse)

        Returns:
            ToolResult с распознанным текстом
        """
        path = Path(image_path)
        if not path.exists():
            return ToolResult(success=False, output="", error=f"Файл не найден: {image_path}")

        if path.suffix.lower() not in self.SUPPORTED:
            return ToolResult(
                success=False, output="",
                error=f"Неподдерживаемый формат: {path.suffix}. Поддерживаются: {', '.join(self.SUPPORTED)}"
            )

        # Пробуем tesseract напрямую
        if self._has_tesseract:
            return self._run_tesseract(str(path), lang, psm)

        # Пробуем pytesseract
        if self._has_pytesseract:
            return self._run_pytesseract(str(path), lang, psm)

        return ToolResult(
            success=False, output="",
            error="Tesseract не установлен. Установите: sudo apt install tesseract-ocr tesseract-ocr-rus"
        )

    def _run_tesseract(self, image_path: str, lang: str, psm: int) -> ToolResult:
        try:
            with tempfile.NamedTemporaryFile(suffix=".txt", delete=False) as tmp:
                out_base = tmp.name.replace(".txt", "")

            cmd = [
                "tesseract", image_path, out_base,
                "-l", lang,
                "--psm", str(psm),
            ]
            result = subprocess.run(cmd, capture_output=True, text=True, timeout=60)

            out_file = Path(out_base + ".txt")
            if out_file.exists():
                text = out_file.read_text(encoding="utf-8").strip()
                out_file.unlink()
                if text:
                    lines = text.split("\n")
                    return ToolResult(
                        success=True,
                        output=f"✅ Распознан текст ({len(lines)} строк, {len(text)} символов):\n\n{text}"
                    )
                return ToolResult(success=False, output="", error="Текст не найден на изображении")
            return ToolResult(success=False, output="", error=result.stderr or "Tesseract вернул ошибку")
        except subprocess.TimeoutExpired:
            return ToolResult(success=False, output="", error="Timeout при распознавании (>60s)")
        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def _run_pytesseract(self, image_path: str, lang: str, psm: int) -> ToolResult:
        try:
            import pytesseract
            from PIL import Image
            img = Image.open(image_path)
            config = f"--psm {psm}"
            text = pytesseract.image_to_string(img, lang=lang, config=config).strip()
            if text:
                lines = text.split("\n")
                return ToolResult(
                    success=True,
                    output=f"✅ Распознан текст ({len(lines)} строк):\n\n{text}"
                )
            return ToolResult(success=False, output="", error="Текст не найден на изображении")
        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def get_languages(self) -> ToolResult:
        """Список доступных языков Tesseract."""
        if not self._has_tesseract:
            return ToolResult(success=False, output="", error="Tesseract не установлен")
        try:
            r = subprocess.run(["tesseract", "--list-langs"], capture_output=True, text=True, timeout=10)
            return ToolResult(success=True, output=r.stdout + r.stderr)
        except Exception as e:
            return ToolResult(success=False, output="", error=str(e))

    def status(self) -> ToolResult:
        """Проверить статус OCR."""
        lines = []
        if self._has_tesseract:
            r = subprocess.run(["tesseract", "--version"], capture_output=True, text=True)
            lines.append(f"✅ Tesseract: {r.stdout.splitlines()[0] if r.stdout else 'ok'}")
        else:
            lines.append("❌ Tesseract: не установлен (sudo apt install tesseract-ocr tesseract-ocr-rus)")

        if self._has_pytesseract:
            lines.append("✅ pytesseract: установлен")
        else:
            lines.append("⚠️  pytesseract: не установлен (pip install pytesseract pillow)")

        return ToolResult(success=self._has_tesseract or self._has_pytesseract, output="\n".join(lines))
