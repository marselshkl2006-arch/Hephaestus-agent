#!/usr/bin/env python3
"""Драйвер живой TUI-сессии Гефеста через настоящий pty.

Посылает клавиши по таймингам, пишет сырой экран в лог, в конце шлёт Ctrl+C.
Проверяем реальным временем: диффы, вопрос разрешения + /allow once,
Esc-прерывание хода, /undo, /sessions.
"""
import os, pty, sys, time, fcntl, termios, struct, select, signal

BIN = sys.argv[1]
HOME = sys.argv[2]
SCENARIO = sys.argv[3]  # путь к файлу сценария: строки "WAIT 30" / "TYPE текст" / "KEY \\r" / "EXPECT Маркер" — только WAIT/TYPE/KEY
OUT = sys.argv[4]

os.environ["HEPHAESTUS_HOME"] = HOME
os.environ["NVIDIA_API_KEY"] = open(os.path.expanduser("~/Nvidia_api.txt")).read().strip()

master, slave = pty.openpty()
# Размер терминала 120x40 — иначе ratatui видит 0x0.
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))

pid = os.fork()
if pid == 0:
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    os.dup2(slave, 0); os.dup2(slave, 1); os.dup2(slave, 2)
    os.close(master); os.close(slave)
    os.execv(BIN, [BIN])
    os._exit(1)

os.close(slave)
log = open(OUT, "wb")
buf = b""

def pump(seconds):
    global buf
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([master], [], [], 0.2)
        if master in r:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                return False
            if not chunk:
                return False
            buf += chunk
            log.write(chunk); log.flush()
    return True

def send(text):
    os.write(master, text.encode("utf-8"))

steps = [l.rstrip("\n") for l in open(SCENARIO) if l.strip() and not l.startswith("#")]
for step in steps:
    parts = step.split(" ", 1)
    cmd = parts[0]
    arg = parts[1] if len(parts) > 1 else ""
    if cmd == "WAIT":
        alive = pump(float(arg))
        if not alive:
            print("ПРОЦЕСС УМЕР на шаге:", step); break
    elif cmd == "TYPE":
        send(arg + "\r")
        pump(0.3)
    elif cmd == "KEY":
        # KEY \r, KEY \\x1b (Esc), KEY \\x03 (Ctrl+C)
        send(arg.encode().decode("unicode_escape"))
        pump(0.3)

pump(2)
try:
    os.kill(pid, signal.SIGKILL)
except ProcessLookupError:
    pass
log.close()
print("СЕССИЯ ЗАВЕРШЕНА, лог:", OUT, "байт:", os.path.getsize(OUT))
