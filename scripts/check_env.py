"""Проверка окружения: что установлено, чего не хватает и откуда это взять.

Пишет дамп reports/env/env_report.json и падает с кодом 1, если не хватает обязательного, — с описанием, что поставить
руками и откуда. Значения из .env не печатает — только «задано / пусто».

  python scripts/check_env.py
"""
import re
import shutil
import subprocess
import sys

import common
from console import banner, detail, fail, ok, section, warn

HOWTO = {
    "python": "Python 3.11+ x64: https://www.python.org/downloads/windows/ (или winget install Python.Python.3.13)",
    "venv": "python -m venv .venv && .venv\\Scripts\\pip install -r requirements.txt",
    "git": "Git for Windows: https://git-scm.com/download/win (или winget install Git.Git)",
    "env": "Заполнить .env по .env.example (скилл /setup-env)",
}


def run(cmd):
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        return (p.stdout + p.stderr).strip()
    except (OSError, subprocess.TimeoutExpired) as e:
        return f"ERR {e}"


def example_keys():
    """Ключи из .env.example (и закомментированные `# KEY=` — они необязательные)."""
    required, optional = [], []
    for line in (common.ROOT / ".env.example").read_text(encoding="utf-8").splitlines():
        m = re.match(r"^(#\s*)?([A-Z][A-Z0-9_]*)=", line)
        if m:
            (optional if m.group(1) else required).append(m.group(2))
    return required, optional


def checks():
    """-> [(id, название, ок, подробности, обязательно)]."""
    out = []
    v = sys.version_info
    out.append(("python", "Python 3.11+", v >= (3, 11), f"{v.major}.{v.minor}.{v.micro}", True))
    out.append(("venv", ".venv", (common.ROOT / ".venv").is_dir(), str(common.ROOT / ".venv"), True))
    git = shutil.which("git")
    out.append(("git", "Git", bool(git), run(["git", "--version"]) if git else "нет в PATH", True))
    required, _ = example_keys()
    empty = [k for k in required if not common.ENV.get(k) or common.ENV[k].startswith(("your_", "paste_"))]
    out.append(("env", ".env", not empty, "пусто: " + ", ".join(empty) if empty else "всё задано", True))
    return out


def main():
    section("Проверка окружения")
    rows = checks()
    for _id, name, good, info_, _req in rows:
        (ok if good else fail)(f"{name}: {info_}")
    common.write_json(common.REPORTS / "env" / "env_report.json",
                      [{"id": i, "name": n, "ok": g, "detail": d, "required": r} for i, n, g, d, r in rows])
    missing = [r for r in rows if not r[2]]
    if missing:
        banner("Что поставить")
        for _id, name, _g, _d, req in missing:
            (fail if req else warn)(name)
            detail(f"  {HOWTO.get(_id, '')}")
    sys.exit(1 if any(r[4] for r in missing) else 0)


if __name__ == "__main__":
    main()
