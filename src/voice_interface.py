"""
voice_interface.py — голосовой интерфейс для Гефеста.

Архитектура нарочно простая и не лезет в ядро агента:
    микрофон → faster-whisper (offline STT) → agent.chat(text) → TTS

Это ровно тот же принцип, что и REPL: берём текст откуда-то и отдаём
в agent.chat(). Голос — просто другой источник текста и другой канал
вывода, а не отдельный агент.

Запуск (push-to-talk — просто и предсказуемо, без головной боли с
wake-word и ложными срабатываниями):

    python3 -m src.voice_interface --provider custom --model deepseek-chat \\
        --base-url https://... --api-key sk-...

    # Локальная модель через llama-server, как раньше:
    python3 -m src.voice_interface --provider llama_server --model default \\
        --base-url http://127.0.0.1:8080

Первый запуск скачает модель Whisper (~500MB для 'small') — нужен
интернет один раз, дальше всё работает offline.
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

import numpy as np


# ─────────────────────────────────────────────────────────────────────────────
# STT — faster-whisper, offline
# ─────────────────────────────────────────────────────────────────────────────
class SpeechToText:
    """Offline распознавание речи через faster-whisper."""

    def __init__(self, model_size: str = "small", device: str = "cpu",
                 compute_type: str = "int8", language: str = "ru"):
        try:
            from faster_whisper import WhisperModel
        except ImportError:
            print("❌ faster-whisper не установлен. Поставь: pip install faster-whisper --break-system-packages")
            sys.exit(1)

        print(f"⏳ Загружаю модель Whisper '{model_size}' ({device}/{compute_type})... "
              f"при первом запуске скачается (~несколько сотен МБ).")
        self.model = WhisperModel(model_size, device=device, compute_type=compute_type)
        self.language = language
        print("✅ Whisper готов.")

    def transcribe(self, audio: np.ndarray, sample_rate: int = 16000) -> str:
        """audio — float32 numpy массив, моно, sample_rate Гц (faster-whisper ждёт 16kHz)."""
        segments, _info = self.model.transcribe(
            audio,
            language=self.language,
            vad_filter=True,  # отрезает тишину по краям — меньше мусора в тексте
        )
        return " ".join(seg.text.strip() for seg in segments).strip()


# ─────────────────────────────────────────────────────────────────────────────
# Запись с микрофона — push-to-talk (Enter/Enter, без wake-word)
# ─────────────────────────────────────────────────────────────────────────────
class MicRecorder:
    """Пишет с микрофона, пока не нажат Enter второй раз."""

    SAMPLE_RATE = 16000  # то, что ждёт Whisper

    def __init__(self):
        try:
            import sounddevice  # noqa: F401
        except ImportError:
            print("❌ sounddevice не установлен. Поставь: pip install sounddevice --break-system-packages")
            print("   Также может понадобиться: sudo apt install portaudio19-dev")
            sys.exit(1)

    def record_until_enter(self) -> np.ndarray:
        import sounddevice as sd

        frames: list[np.ndarray] = []
        stop_event = threading.Event()

        def _callback(indata, _frames, _time, _status):
            frames.append(indata.copy())

        def _wait_for_enter():
            input()  # блокирует до второго Enter
            stop_event.set()

        print("🎙️  Говори... (Enter — закончить запись)")
        waiter = threading.Thread(target=_wait_for_enter, daemon=True)
        waiter.start()

        with sd.InputStream(samplerate=self.SAMPLE_RATE, channels=1,
                             dtype="float32", callback=_callback):
            while not stop_event.is_set():
                stop_event.wait(0.05)

        if not frames:
            return np.zeros(0, dtype=np.float32)
        return np.concatenate(frames, axis=0).flatten()


# ─────────────────────────────────────────────────────────────────────────────
# TTS — piper (если настроен) с fallback на espeak-ng, оба offline
# ─────────────────────────────────────────────────────────────────────────────
class TextToSpeech:
    """
    Озвучка ответа. Приоритет: piper (естественнее) → espeak-ng (грубее,
    зато почти всегда уже стоит на Linux и не требует скачивания модели
    голоса). Если нет ни того ни другого — молча печатает текст.
    """

    def __init__(self, piper_model: str | None = None, voice: str = "ru"):
        self.piper_bin = shutil.which("piper")
        self.piper_model = piper_model
        self.espeak_bin = shutil.which("espeak-ng") or shutil.which("espeak")
        self.voice = voice

        if self.piper_bin and self.piper_model:
            print(f"🔊 TTS: piper ({Path(self.piper_model).name})")
        elif self.espeak_bin:
            print(f"🔊 TTS: {Path(self.espeak_bin).name} (fallback — качество грубее, "
                  f"для голоса получше настрой piper --piper-model)")
        else:
            print("🔇 TTS недоступен (нет ни piper, ни espeak-ng) — ответы будут только текстом.")
            print("   Поставь: sudo apt install espeak-ng")

    def speak(self, text: str) -> None:
        if not text.strip():
            return
        text = text[:1000]  # не зачитываем километровые простыни целиком

        if self.piper_bin and self.piper_model:
            self._speak_piper(text)
        elif self.espeak_bin:
            self._speak_espeak(text)
        # иначе — тихо, текст и так уже напечатан в чате

    def _speak_piper(self, text: str) -> None:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            wav_path = f.name
        try:
            proc = subprocess.run(
                [self.piper_bin, "--model", self.piper_model, "--output_file", wav_path],
                input=text.encode("utf-8"),
                capture_output=True,
                timeout=60,
            )
            if proc.returncode != 0:
                print(f"⚠️  piper упал ({proc.stderr.decode(errors='ignore')[:200]}), "
                      f"перехожу на espeak-ng" if self.espeak_bin else "")
                if self.espeak_bin:
                    self._speak_espeak(text)
                return
            self._play_wav(wav_path)
        finally:
            try:
                os.unlink(wav_path)
            except OSError:
                pass

    def _speak_espeak(self, text: str) -> None:
        subprocess.run([self.espeak_bin, "-v", self.voice, text],
                        capture_output=True, timeout=60)

    @staticmethod
    def _play_wav(path: str) -> None:
        player = shutil.which("paplay") or shutil.which("aplay") or shutil.which("ffplay")
        if not player:
            return
        args = [player, path] if player.endswith(("paplay", "aplay")) else [player, "-autoexit", "-nodisp", path]
        subprocess.run(args, capture_output=True, timeout=60)


# ─────────────────────────────────────────────────────────────────────────────
# Главный цикл
# ─────────────────────────────────────────────────────────────────────────────
def run_voice_loop(agent, stt: SpeechToText, tts: TextToSpeech | None) -> None:
    recorder = MicRecorder()
    print("\n⚡ Гефест слушает. Нажми Enter, чтобы начать говорить. Ctrl+C — выход.\n")

    while True:
        try:
            input("Нажми Enter, чтобы говорить...")
        except (KeyboardInterrupt, EOFError):
            print("\n👋 Выход.")
            return

        audio = recorder.record_until_enter()
        if audio.size < 1600:  # меньше 0.1с — явно пустая запись
            print("(тишина, пропускаю)")
            continue

        print("⏳ Распознаю...")
        text = stt.transcribe(audio)
        if not text:
            print("(не расслышал, попробуй ещё раз)")
            continue

        print(f"🗣️  Ты сказал: {text}")

        try:
            response = agent.chat(text)
        except Exception as e:
            response = f"Ошибка: {e}"

        print(f"⚡ Гефест: {response}\n")
        if tts:
            tts.speak(response)


def main():
    parser = argparse.ArgumentParser(description="⚡ Гефест — голосовой интерфейс")
    # Те же флаги провайдера, что у hephaestus.py и telegram_bot.py —
    # ничего нового изобретать не нужно, конфиг LLM везде одинаковый.
    parser.add_argument("--provider", choices=[
        "ollama", "openai", "anthropic", "openrouter",
        "koboldcpp", "llama_server", "custom",
    ])
    parser.add_argument("--model")
    parser.add_argument("--base-url")
    parser.add_argument("--api-key")
    parser.add_argument("--workspace")
    parser.add_argument("--temperature", type=float, default=0.1)

    parser.add_argument("--whisper-model", default="small",
                         help="tiny/base/small/medium/large-v3 (по умолчанию small — баланс скорости и качества)")
    parser.add_argument("--whisper-device", default="cpu", choices=["cpu", "cuda"])
    parser.add_argument("--whisper-compute-type", default="int8",
                         help="int8 — быстрее на CPU, float16 — если есть GPU")
    parser.add_argument("--language", default="ru")

    parser.add_argument("--piper-model", help="Путь к .onnx модели голоса piper (опционально)")
    parser.add_argument("--voice", default="ru", help="Голос для espeak-ng fallback")
    parser.add_argument("--no-tts", action="store_true", help="Отключить озвучку, только текст")

    args = parser.parse_args()

    from .hephaestus import HephaestusAgent
    from .llm_client import LLMConfig, LLMProvider, auto_detect_provider

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

    config = LLMConfig(
        provider=provider,
        model=model,
        api_key=args.api_key,
        base_url=args.base_url,
        temperature=args.temperature,
    )
    agent = HephaestusAgent(llm_config=config, workspace_root=args.workspace)

    stt = SpeechToText(
        model_size=args.whisper_model,
        device=args.whisper_device,
        compute_type=args.whisper_compute_type,
        language=args.language,
    )
    tts = None if args.no_tts else TextToSpeech(piper_model=args.piper_model, voice=args.voice)

    run_voice_loop(agent, stt, tts)


if __name__ == "__main__":
    main()
