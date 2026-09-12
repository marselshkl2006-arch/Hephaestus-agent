#!/usr/bin/env python3
"""Проиграть pty-лог через эмулятор терминала pyte и проверить экраны
по контрольным точкам: рамки, дифф, вопрос разрешения, undo, sessions.

Нюансы:
- Alternate screen (?1049h/l): /exit возвращает нормальный буфер — пустой.
  Переключения вырезаем, чтобы всё рисовалось в одном буфере pyte.
- Маркеры ищем по ANSI-очищенному тексту всего лога (надёжнее, чем кадры:
  мелкий кадр мог не содержать строку в видимой области).
"""
import pyte, sys, re

raw = open(sys.argv[1], "rb").read()

# 1) Вырезаем переключение alternate screen (?1049h / ?1049l и связку с
#    сохранением курсора ?1049 — частные случаи уже покрыты общим ?..h/l).
raw2 = re.sub(rb"\x1b\[\?1049[hl]", b"", raw)

screen = pyte.Screen(120, 40)
stream = pyte.ByteStream(screen)
stream.feed(raw2)

# 2) ANSI-очищенный текст всего лога — для поиска маркеров.
clean = raw2.decode("utf-8", errors="replace")
clean = re.sub(r"\x1b\[[0-9;?]*[a-zA-Z]", "", clean)
clean = re.sub(r"\x1b\][^\x07\x1b]*(\x07|\x1b\\)", "", clean)
clean = re.sub(r"\x1b[()][B0]", "", clean)

# Маркеры — РЕАЛЬНЫЕ строки TUI (сверены по живым логам):
checks = {
    "TUI рамка (Диалог)": "Диалог",
    "Подсказка ввода": "Ввод (Enter",
    "file_write: TUI-LIVE": "TUI-LIVE",
    "Дифф (diff-блок)": "diff:",
    "Вопрос разрешения": "Требуется разрешение",
    "/allow выполнено": "Разрешено (один раз)",
    "Esc-прерывание": "Прервано",
    "/undo": "изменения отменены",
    "/sessions": "Сессии",
    "/toolcalls": "Tool-вызовы",
    "Ошибка LLM видна": "Ошибка LLM",
}

print("=== КОНТРОЛЬНЫЕ ТОЧКИ РЕАЛЬНОГО TUI ===")
ok = 0
for label, needle in checks.items():
    hit = needle in clean
    print(("✅" if hit else "❌"), label)
    ok += hit
print(f"\nПройдено: {ok}/{len(checks)}")

print("\n=== ФИНАЛЬНЫЙ ЭКРАН (последние 20 строк) ===")
for line in screen.display[-20:]:
    if line.strip():
        print(line.rstrip())
