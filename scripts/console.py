"""Цветной вывод с иконками для консоли и логов CI (Jenkins AnsiColor, GitLab CI).

ПРАВИЛО: любой цветной вывод — только через функции этого модуля (ANSI-коды руками в скриптах не собирать):
  from console import section, ok, fail, warn, info, detail, status_line, banner
  section("Проверка окружения")          # ▶ заголовок шага, жирный голубой
  ok("готово за 30 с")                   # ✅ зелёный
  fail("упало (код 1)")                  # ❌ красный, жирный
  warn("нет токена — пропускаю")         # ⚠️ жёлтый
  info("отчёт: …")                       # ℹ️ синий
  detail("> python …")                   # команда, подробности — серый
  fmt_duration(2349)                     # «39 мин 9 с» — длительности везде (лог, отчёты)
Низкоуровневое — paint(text, *codes): одна строка целиком в один цвет.

Палитра — xterm-16: одинаково на светлой и тёмной теме и в консоли Windows. Серый — 90 (bright black): читается и на
белом, и на тёмном фоне; 37 на светлой теме не виден, 2 (dim) рисуют не везде.

Цвета выключаются переменной NO_COLOR; в CI (JENKINS_URL, GITLAB_CI, FORCE_COLOR) и в терминале — включены, в
файл/пайп — нет.
"""
import os
import sys

RESET, BOLD = "\033[0m", "\033[1m"
RED, GREEN, YELLOW, BLUE, MAGENTA, CYAN = "\033[31m", "\033[32m", "\033[33m", "\033[34m", "\033[35m", "\033[36m"
GREY = "\033[90m"


def _enabled():
    if os.environ.get("NO_COLOR"):
        return False
    if os.environ.get("JENKINS_URL") or os.environ.get("GITLAB_CI") or os.environ.get("FORCE_COLOR"):
        return True
    return hasattr(sys.stdout, "isatty") and sys.stdout.isatty()


COLOR = _enabled()
if COLOR and os.name == "nt":  # консоль Windows: включить разбор ANSI-последовательностей (VT)
    try:
        import ctypes
        k = ctypes.windll.kernel32
        h = k.GetStdHandle(-11)
        mode = ctypes.c_ulong()
        if k.GetConsoleMode(h, ctypes.byref(mode)):
            k.SetConsoleMode(h, mode.value | 0x0004)
    except (AttributeError, OSError):
        pass


def _params(codes):
    return ";".join(c[2:-1] for c in codes if c and c.startswith("\033[") and c.endswith("m"))


def paint(text, *codes):
    """Строка целиком в один цвет: одна SGR-последовательность (\\033[1;31m, а не \\033[1m\\033[31m — иначе часть
    просмотрщиков лога показывает вторую текстом «[1m[31m»), сброс в конце. Отступ и переводы строк в начале — до
    кода цвета: ESC первым байтом строки просмотрщики рисуют текстом чаще."""
    if not (COLOR and codes):
        return str(text)
    text = str(text)
    body = text.lstrip(" \n")
    lead = text[:len(text) - len(body)]
    return f"{lead}\033[{_params(codes)}m{body}{RESET}" if body else text


def fmt_duration(seconds):
    """Длительность для людей: «0.7 с», «7.2 с», «39 мин 9 с», «2 ч 5 мин 3 с»; нулевые части опускаются
    («2 мин», «1 ч 3 с»). Меньше минуты — с десятыми (целое число секунд — без них); от минуты — целые секунды.
    None / не число — ""."""
    try:
        s = float(seconds)
    except (TypeError, ValueError):
        return ""
    s = max(0.0, s)
    if round(s, 1) < 60:
        return f"{int(s)} с" if isinstance(seconds, int) else f"{s:.1f} с"
    total = int(round(s))
    h, m, sec = total // 3600, total // 60 % 60, total % 60
    parts = [f"{v} {u}" for v, u in ((h, "ч"), (m, "мин"), (sec, "с")) if v]
    return " ".join(parts) or "0 с"


def _say(text):
    print(text, flush=True)


def section(title, detail=""):  # noqa: F811 — параметр, а не функция detail()
    """Заголовок шага: пустая строка + ▶ жирным голубым (+ серым команда/подробности)."""
    _say("\n" + paint(f"▶ {title}", BOLD, CYAN) + (("\n" + paint(f"  {detail}", GREY)) if detail else ""))


def ok(msg):
    _say(paint(f"✅ {msg}", GREEN))


def fail(msg):
    _say(paint(f"❌ {msg}", BOLD, RED))


def warn(msg, indent=""):
    _say(paint(f"{indent}⚠️  {msg}", YELLOW))


def info(msg):
    _say(paint(f"ℹ️  {msg}", BLUE))


def detail(text):
    """Подробности серым: команда запуска, строки таблицы."""
    _say(paint(text, GREY))


STATUS_ICON = {"success": "✅", "fail": "❌", "timeout": "⏱️", "aborted": "⏹️"}
STATUS_RU = {"success": "успешно", "fail": "есть ошибки", "timeout": "превышено время", "aborted": "прерван"}


def status_line(name, status, detail=""):  # noqa: F811
    """Итог одной строкой: иконка + цвет по статусу, статус словами."""
    color = GREEN if status == "success" else RED
    text = f"{STATUS_ICON.get(status, '❌')} {name}: {STATUS_RU.get(status, status)}" + (f" — {detail}" if detail else "")
    _say(paint(text, BOLD, color))


def banner(title):
    """Рамка для итога — видно при прокрутке лога."""
    line = "═" * max(40, len(title) + 4)
    _say("\n" + paint(line, BOLD) + "\n" + paint(f"  {title}", BOLD) + "\n" + paint(line, BOLD))
