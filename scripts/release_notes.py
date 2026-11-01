"""Заметки релиза для GitHub из app/CHANGELOG.md (формат — скилл release, раздел «Заметки релиза»).

Что делает: берёт раздел версии (и всех версий после последней опубликованной — если какая-то не вышла, её пункты не
теряются), склеивает перенесённые строки пунктов (GitHub в релизах показывает каждый перевод строки как разрыв),
собирает страницу: заголовок-польза, пункты для сотрудников, «Технически» под спойлером, «Как установить».

Пишет: dist/notes.md (или --out). Запуск:
    python scripts/release_notes.py --version 0.7.2 --prev 0.7.0
    python scripts/release_notes.py --version 0.7.2 --prev 0.7.0 --stdout
"""
import common  # noqa: F401 — UTF-8 в консоли (раннер Windows — cp1252)

import argparse
import re

from console import ok


def vkey(v):
    """0.7.2 → (0, 7, 2, 1); бета 0.7.2-beta.1 → (0, 7, 2, 0, 1) — раньше стабильной."""
    main, _, pre = v.partition("-")
    nums = tuple(int(x) for x in main.split(".") if x.isdigit())
    if not pre:
        return nums + (1,)
    return nums + (0,) + tuple(int(x) for x in re.findall(r"\d+", pre))


def unwrap(text):
    """Пункт, перенесённый на следующие строки (отступ), — в одну строку."""
    out = []
    for line in text.splitlines():
        if out and line.startswith("  ") and line.strip() and not line.lstrip().startswith(("- ", "* ")):
            out[-1] += " " + line.strip()
        else:
            out.append(line.rstrip())
    return "\n".join(out).strip()


def sections(md):
    """[(версия, заголовок, пункты для людей, [(имя, текст)] технических)] — как в CHANGELOG, новые первыми."""
    res = []
    for sec in re.split(r"^## ", md, flags=re.M)[1:]:
        head, _, body = sec.partition("\n")
        ver, _, title = head.strip().partition(" — ")
        user, *rest = re.split(r"^### ", body, flags=re.M)
        tech = []
        for t in rest:
            name, _, txt = t.partition("\n")
            tech.append((name.strip(), unwrap(txt)))
        res.append((ver.strip(), title.strip(), unwrap(user), tech))
    return res


def build(md, version, prev, asset):
    """Текст релиза: версия `version` и все невыпущенные между `prev` и ней."""
    all_ = sections(md)
    hi = vkey(version)
    if prev:
        lo = vkey(prev)
        picked = [s for s in all_ if lo < vkey(s[0]) <= hi]
    else:  # без последней опубликованной — только раздел этой версии
        picked = [s for s in all_ if s[0] == version]
    if not picked:
        raise SystemExit(f"в app/CHANGELOG.md нет раздела «## {version}» — допишите заметки и запустите снова")
    ver, title, user, _ = picked[0]
    parts = [f"## {title}" if title else f"## Domini VPN Monitor {ver}", user]
    for v, t, u, _ in picked[1:]:
        if u:
            parts += [f"### Ещё в {v}" + (f" — {t}" if t else ""), u]
    tech = [(v, name, txt) for v, _, _, ts in picked for name, txt in ts if txt]
    if tech:
        body = "\n\n".join((f"**{v}**\n\n" if len(picked) > 1 else "") + txt for v, _, txt in tech)
        parts.append(f"<details>\n<summary>Технически — для разработчиков и админов</summary>\n\n{body}\n\n</details>")
    parts.append(
        "### Как установить\n"
        "Уже стоит — обновится само до последней версии за несколько минут, ничего делать не нужно. Первый раз — "
        f"скачайте **`{asset}`** ниже и запустите двойным кликом (Windows 10 и 11, без прав администратора). "
        "Контрольные суммы — в файлах релиза."
    )
    return "\n\n".join(p for p in parts if p) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--version", required=True, help="версия релиза, как в app/Cargo.toml")
    ap.add_argument("--prev", default="", help="последняя опубликованная версия (latest.version); пусто — только эта")
    ap.add_argument("--asset", default="domini-vpn-monitor.exe", help="имя exe в релизе")
    ap.add_argument("--out", default="dist/notes.md", help="куда записать, от корня репо")
    ap.add_argument("--stdout", action="store_true", help="напечатать текст")
    a = ap.parse_args()
    md = (common.ROOT / "app/CHANGELOG.md").read_text(encoding="utf-8")
    text = build(md, a.version, a.prev.strip(), a.asset)
    common.write_text(common.ROOT / a.out, text)
    if a.stdout:
        print(text)
    ok(f"заметки релиза {a.version}: {a.out}")


if __name__ == "__main__":
    main()
