"""Общее для скриптов: кодировка консоли, пути, .env, запись файлов (UTF-8 и LF).

Каждый скрипт начинается с `import common` — это включает UTF-8 в консоли и даёт ROOT, ENV, write_text/write_json.
"""
import json
import os
import sys
from pathlib import Path

# Консоль Windows по умолчанию не UTF-8: наш вывод и вывод дочерних процессов — строго UTF-8, иначе кириллица
# превращается в «Ð”Ð²Ðµ». Читающая сторона: PowerShell — [Console]::OutputEncoding = [Text.UTF8Encoding]::new();
# cmd — chcp 65001.
os.environ.setdefault("PYTHONUTF8", "1")
os.environ.setdefault("PYTHONIOENCODING", "utf-8")
if os.name == "nt":
    # кодовая страница консоли — UTF-8: её наследуют дочерние процессы (git, npm …; иначе в русской Windows — OEM-866)
    try:
        import ctypes
        ctypes.windll.kernel32.SetConsoleOutputCP(65001)
        ctypes.windll.kernel32.SetConsoleCP(65001)
    except (AttributeError, OSError):
        pass
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
REPORTS = ROOT / "reports"


from console import fmt_duration  # noqa: E402,F401 — длительность для людей («39 мин 9 с»): лог, отчёты, страницы


# --- запись файлов: концы строк LF ------------------------------------------------------------------------------------
# Всё, что скрипты пишут и что может попасть в git, — UTF-8 и LF: open(..., "w") на Windows пишет CRLF, и git ругается
# «CRLF will be replaced by LF».

def write_text(path, text, append=False):
    """Текстовый файл UTF-8 с LF (папки создаются). -> path."""
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "a" if append else "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    return path


def write_json(path, data, indent=2, **kw):
    """JSON UTF-8 (кириллица как есть) с LF и переводом строки в конце. kw — в json.dumps (sort_keys, default)."""
    return write_text(path, json.dumps(data, ensure_ascii=False, indent=indent, **kw) + "\n")


def open_text(path, mode="w"):
    """open() для записи текста потоком (json.dump, построчно) — UTF-8 и LF."""
    return open(path, mode, encoding="utf-8", newline="\n")


# --- .env -------------------------------------------------------------------------------------------------------------

ENV_FILE = Path(os.environ.get("PROJECT_ENV_FILE") or ROOT / ".env")  # рабочая копия (worktree) без своего .env


def load_env():
    """.env -> dict; переменные окружения с теми же именами главнее файла (так их задаёт CI)."""
    env = {}
    if ENV_FILE.exists():
        for line in ENV_FILE.read_text(encoding="utf-8").splitlines():
            if "=" in line and not line.lstrip().startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip()
    env.update({k: v for k, v in os.environ.items() if k in env})
    return env


ENV = load_env()


def require_env(*keys):
    """Обязательные ключи .env: нет или заглушка — выход с подсказкой (значения не печатаются)."""
    missing = [k for k in keys if not ENV.get(k) or ENV[k].startswith(("your_", "paste_"))]
    if missing:
        raise SystemExit(f"не заданы {', '.join(missing)} в .env (скилл /setup-env, образец — .env.example)")
    return [ENV[k] for k in keys]
