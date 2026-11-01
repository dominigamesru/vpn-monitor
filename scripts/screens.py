"""Галерея экранов: docs/screens.html — настоящие страницы приложения (меню у иконки и окно) во всех основных
состояниях, на моках API. Обновлять перед каждым релизом, который меняет интерфейс (правило в CLAUDE.md).

Страницы и библиотеки берутся из app/web/ как есть и лежат в галерее по одному разу; каждый кадр собирается в
браузере: страница + своё состояние + `window.ipc`, объявленный ровно как в wry (так галерея ловит и ошибки вроде
`const ipc`, из-за которых меню не запускалось в 0.5.0–0.5.1). Тема — иконкой в правом верхнем углу галереи.

  python scripts/screens.py           # пересобрать docs/screens.html
  python scripts/screens.py --check   # код 1, если docs/screens.html устарела (страницы менялись, а галерея — нет)
"""
import argparse
import html
import json
import time

import common
from console import fail, ok

WEB = common.ROOT / "app" / "web"
OUT = common.ROOT / "docs" / "screens.html"
CHANGELOG = common.ROOT / "app" / "CHANGELOG.md"

TH = {"svn_good_mbps": 8, "svn_bad_mbps": 2, "inet_good_mbps": 8, "inet_bad_mbps": 2,
      "inet_lat_warn_ms": 1000, "inet_lat_bad_ms": 3000, "vpn_good_ms": 300, "vpn_bad_ms": 700}
OFFICE = ("Confluence", "Jira", "SVN")


def metric(raw):
    s = sorted(raw)
    return {"value": s[len(s) // 2] if raw else None, "raw": raw}


def route(inet, ims, vpn, svn, st, cause=None, why=None, bv=None, bo=None, color="good",
          services=None, auth=(), user="good", score=None):
    names = ("st_inet_speed", "st_inet_lat", "st_inet", "st_vpn", "st_svn", "st_office")
    r = {"inet": metric(inet), "inet_ms": metric(ims), "vpn": metric(vpn), "svn": metric(svn),
         "services": services or [{"name": n, "st": "good", "what": "отвечает"} for n in OFFICE],
         "cause": cause, "why": why, "because_vpn": bv, "because_office": bo, "color": color,
         "score": score, "auth_failed": list(auth), "st_user": user}
    r.update(dict(zip(names, st)))
    r["st_vpn_lat"] = "good" if (cause, why) == ("vpn", "speed") else r["st_vpn"]
    return r


def targets(down=(), svn_mbps=None, auth_fail=()):
    t = []
    for n, role, lat in (("Confluence", "light", 181), ("Jira", "light", 176), ("SVN", "speed", 179)):
        up = n not in down
        t.append({"id": n.lower(), "name": n, "role": role, "ok": up and n not in auth_fail, "reachable": up,
                  "latency": lat if up else None, "mbps": svn_mbps if n == "SVN" and up else None,
                  "via_vpn": True if up else None, "adapter": "OpenVPN Data Channel Offload",
                  "auth": "не принят" if n in auth_fail else "Basic (AD)",
                  "error": None if up else "Timeout: TCP 1.2.3.4:443 — туннель не пропускает трафик",
                  "fail": "Auth" if n in auth_fail else None})
    for n, lat, http in (("ChatGPT", 92, 200), ("Claude", 95, 403), ("Luma", 120, 200)):
        t.append({"id": n.lower(), "name": n, "role": "world", "ok": True, "reachable": True, "latency": lat,
                  "via_vpn": False, "adapter": "Беспроводная сеть", "http": http})
    return t


def svc(svn_st="good", svn_what="отвечает", others="good", others_what="отвечает"):
    return [{"name": "Confluence", "st": others, "what": others_what}, {"name": "Jira", "st": others, "what": others_what},
            {"name": "SVN", "st": svn_st, "what": svn_what}]


# Состояния — строки таблицы состояний docs/ux/scenarios.md
STATES = [
    ("ok", "Можно работать", dict(color="green", badge="score", score=93,
        route=route([46, 52, 48], [38, 41, 40], [192, 205, 198], [13.9, 12.8, 13.5], ["good"] * 6, score=93),
        targets=targets(svn_mbps=13.5))),
    ("slowsvn", "SVN качает медленно (VPN режет скорость)", dict(color="red", badge="score", score=42,
        route=route([15.6, 14.2, 15.0], [94, 223, 120], [348, 223, 240], [1.9, 3.4, 1.3],
                    ["good", "good", "good", "bad", "bad", "bad"], "vpn", "speed", None, "из‑за VPN", "bad",
                    svc("bad", "качает 1,9 Мбит/с — медленно"), score=42),
        targets=targets(svn_mbps=1.9))),
    ("slownet", "Медленный интернет", dict(color="yellow", badge="weak", score=61,
        route=route([4.1, 3.8, 4.4], [70, 85, 80], [260, 280, 270], [3.5, 3.2, 3.6],
                    ["warn", "good", "warn", "good", "warn", "warn"], "inet", "speed", None, "из‑за интернета", "warn",
                    svc("warn", "качает 3,5 Мбит/с — медленно"), score=61),
        targets=targets(svn_mbps=3.5))),
    ("lagnet", "Интернет с задержкой больше секунды", dict(color="yellow", badge="score", score=58,
        route=route([10.2, 11.0, 9.5], [1200, 1350, 1100], [1450, 1600, 1380], [5.2, 4.8, 6.1],
                    ["good", "warn", "warn", "bad", "warn", "warn"], "inet", "lat", "из‑за интернета", "из‑за интернета",
                    "warn", svc("warn", "качает 5,2 Мбит/с — медленно"), score=58),
        targets=targets(svn_mbps=5.2))),
    ("off", "VPN выключен", dict(color="red", badge="novpn", score=None,
        route=route([48, 50, 47], [40, 38, 42], [], [], ["good", "good", "good", "bad", "bad", "bad"], "vpn", "off",
                    None, "без VPN", "bad", [{"name": n, "st": "bad", "what": "недоступен без VPN"} for n in OFFICE]),
        targets=targets(down=OFFICE))),
    ("jira", "Один сервис не отвечает", dict(color="yellow", badge="score", score=74,
        route=route([46, 48, 47], [40, 39, 41], [199, 201, 204], [12.5, 13.1, 12.9],
                    ["good", "good", "good", "good", "good", "warn"], "office", "svc", None, None, "warn",
                    [{"name": "Confluence", "st": "good", "what": "отвечает"}, {"name": "Jira", "st": "bad", "what": "не отвечает"},
                     {"name": "SVN", "st": "good", "what": "отвечает"}], score=74),
        targets=targets(down=("Jira",), svn_mbps=12.9))),
    ("auth", "Пароль AD не подошёл", dict(color="yellow", badge="score", score=55,
        route=route([46, 48, 47], [40, 39, 41], [199, 201, 204], [12.5, 13.1, 12.9],
                    ["good", "good", "good", "good", "good", "warn"], "office", "auth", None, None, "warn",
                    svc(others="warn", others_what="вход AD не принят"), ("Confluence", "Jira"), "bad", score=55),
        targets=targets(svn_mbps=12.9, auth_fail=("Confluence", "Jira")))),
    ("offline", "Нет интернета", dict(color="gray", badge="offline", score=None, route=None, targets=targets(down=OFFICE))),
    ("captive", "Wi‑Fi требует входа", dict(color="gray", badge="captive", score=None, route=None, targets=targets(down=OFFICE))),
    ("paused", "Пауза", dict(color="green", badge="score", score=93, paused=True,
        route=route([46, 52, 48], [38, 41, 40], [192, 205, 198], [13.9, 12.8, 13.5], ["good"] * 6, score=93),
        targets=targets(svn_mbps=13.5))),
]


def history(now, hours, step_min):
    recs, n = [], int(hours * 60 / step_min)
    for i in range(n):
        t = now - (n - i) * step_min * 60000
        off = n * 0.62 < i < n * 0.66
        slow = n * 0.35 < i < n * 0.45
        recs.append({"t": t, "c": "red" if off else "yellow" if slow else "green", "b": "novpn" if off else "score",
                     "s": None if off else (55 if slow else 88), "lat": None if off else 200 + (i % 5) * 9 + (180 if slow else 0),
                     "spd": None if off else (2.5 if slow else 12 + (i % 3)), "inet": 15.0 if i % 6 == 0 else None,
                     "ilat": 40 + (i % 4) * 5, "ovh": None if off else 150 + (i % 5) * 9, "share": None, "vpn": "OpenVPN Connect"})
    return recs


def state_json(key, now, view="route"):
    st = dict(next(s for k, _, s in STATES if k == key))
    paused = st.pop("paused", False)
    snap = {"at": now - 60000, "color": st["color"], "badge": st["badge"], "score": st["score"],
            "client": {"name": "OpenVPN Connect", "profile": "vpn.dominigames.com [DG_Rykov_Sergey_main]", "adapter": "OpenVPN"},
            "targets": st["targets"], "route": st["route"], "full": True, "took_ms": 17800, "waking": False, "lite": None,
            "explain": "", "internet_latency": 40}
    return {"version": "0.6.0", "latest": "0.6.0", "checking": False, "thresholds": TH, "snapshot": snap,
            "login": "rykov_sergey", "city": "Москва", "updated_ms": now - 5 * 3600000, "autostart": True,
            "intervals": {"green_sec": 180, "alert_sec": 60, "full_min": 30, "internet_min": 15, "update_min": 30},
            "prefs": {"view": view, "channel": "latest", "pause_until_ms": now + 47 * 60000 if paused else None,
                      "theme": "", "last_seen_version": "0.6.0",
                      "notify": {"vpn": True, "auth": True, "update": True, "slow": False}},
            "services": [{"id": t["id"], "name": t["name"], "auth": t.get("auth"), "note": None, "ok": t["ok"],
                          "last_ok_ms": now - 60000 if t["ok"] else now - 3 * 3600000} for t in st["targets"] if t["role"] != "world"],
            "paused": paused, "targets": [], "dir": "C:\\Users\\sergi\\AppData\\Local\\DominiVPNMonitor"}


def frames(now):
    """Кадры галереи: (раздел, подпись, страница, ширина, высота, хеш, моки)."""
    hour, day, week = history(now, 1, 3), history(now, 24, 10), history(now, 24 * 7, 60)
    out = []
    for key, name, _ in STATES:
        out.append(("Меню у иконки — состояния", name, "flyout", 372, 0, "", {"state": state_json(key, now), "hour": hour}))
    for view, name in (("route", "Маршрут"), ("dash", "Приборная панель"), ("strip", "Лента часа")):
        out.append(("Меню у иконки — виды", name, "flyout", 372, 0, "", {"state": state_json("slowsvn", now, view), "hour": hour}))
    for key, name, tab in (("ok", "Окно — «Сейчас», всё в порядке", "now"), ("slowsvn", "Окно — «Сейчас», SVN медленный", "now"),
                           ("slowsvn", "Окно — «История»", "history"), ("ok", "Окно — «Настройки»", "settings")):
        out.append(("Окно в браузере", name, "index", 1100, 900, tab,
                    {"state": state_json(key, now), "hour": hour, "day": day, "week": week}))
    return out


SHIM = """<script>
(() => {
  const M = JSON.parse(document.getElementById("mock").textContent);
  // как wry: неконфигурируемое свойство — `const ipc` в странице сломается здесь так же, как в приложении
  Object.defineProperty(window, "ipc", { value: Object.freeze({ postMessage: (m) => parent.postMessage({ frame: M.frame, msg: String(m) }, "*") }) });
  const reply = (body) => new Response(typeof body === "string" ? body : JSON.stringify(body), { headers: { "content-type": typeof body === "string" ? "text/plain; charset=utf-8" : "application/json" } });
  window.fetch = async (u, o = {}) => {
    const url = new URL(u, "http://127.0.0.1:5151/"), p = url.pathname;
    if (p === "/api/state") { const s = M.state; s.prefs.theme = M.theme; return reply(s); }
    if (p === "/api/history") { const r = url.searchParams.get("range") || "day"; return reply({ range: r, from: Date.now() - ({ hour: 3600e3, day: 864e5, week: 6048e5 }[r] || 864e5), to: Date.now(), records: M[r] || M.hour }); }
    if (p === "/api/changes") return reply(M.changes);
    if (p === "/api/credentials") return reply({ login: "rykov_sergey", store: "Диспетчер учётных данных Windows", gitlab_oauth: false });
    if (p === "/api/diagnostics") return reply({ running: false, report: null });
    if (p === "/api/summary") return reply("Сводка для поддержки");
    if (p === "/api/log") return reply("лог");
    if (p === "/api/update") return reply({ current: "0.6.0", latest: "0.6.0", channel: "latest", available: false });
    return reply({ ok: true });
  };
  if (M.hash) location.hash = M.hash;
})();
</script>"""


def page_template(name):
    """Страница из app/web с библиотеками внутри (без запросов к серверу)."""
    doc = (WEB / f"{name}.html").read_text(encoding="utf-8")
    popper = (WEB / "vendor" / "popper.min.js").read_text(encoding="utf-8")
    tippy = (WEB / "vendor" / "tippy.min.js").read_text(encoding="utf-8")
    css = (WEB / "vendor" / "tippy.css").read_text(encoding="utf-8")
    doc = doc.replace('<script src="/vendor/popper.min.js"></script>', f"<script>{popper}</script>")
    doc = doc.replace('<script src="/vendor/tippy.min.js"></script>', f"<script>{tippy}</script>")
    doc = doc.replace('<link rel="stylesheet" href="/vendor/tippy.css">', f"<style>{css}</style>")
    doc = doc.replace('<link rel="icon" href="/favicon.ico">', "").replace('<link rel="icon" id="favicon" href="/favicon.ico">', "")
    # шим — до любых скриптов страницы
    return doc.replace("<head>", "<head><!--MOCK-->" + SHIM, 1)


GALLERY = """<!doctype html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Экраны VPN Monitor</title>
<style>
/* Галерея экранов: настоящие страницы приложения в iframe на моках. Собирает scripts/screens.py — руками не править. */
:root { --page: #e9edf2; --bg: #ffffff; --ink: #15202e; --muted: #5a6b80; --line: #e2e7ee; --accent: #2f5bea; }
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --page: #0b1017; --bg: #1b2430; --ink: #e8eef5; --muted: #93a3b7; --line: #2c3948; --accent: #6f93ff; color-scheme: dark } }
:root[data-theme="dark"] { --page: #0b1017; --bg: #1b2430; --ink: #e8eef5; --muted: #93a3b7; --line: #2c3948; --accent: #6f93ff; color-scheme: dark }
body { margin: 0; background: var(--page); color: var(--ink); font: 14px/1.45 "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif; }
.wrap { max-width: 1640px; margin: 0 auto; padding: 24px max(16px, env(safe-area-inset-right, 0px)) 48px max(16px, env(safe-area-inset-left, 0px)); display: grid; gap: 24px; }
header { position: relative; padding-right: 48px; }
h1 { font-size: 20px; margin: 0; }
h2 { font-size: 15px; margin: 0 0 10px; }
.lead { margin: 4px 0 0; color: var(--muted); max-width: 80ch; }
.themebtn { position: absolute; top: 0; right: 0; width: 36px; height: 36px; border-radius: 10px; border: 1px solid var(--line); background: var(--bg); color: var(--ink); display: grid; place-items: center; cursor: pointer; }
.themebtn svg { width: 18px; height: 18px; }
.grid { display: flex; flex-wrap: wrap; gap: 20px; align-items: flex-start; }
figure { margin: 0; display: grid; gap: 6px; max-width: 100%; }
figcaption { font-size: 12.5px; color: var(--muted); }
.box { border-radius: 12px; overflow: hidden; border: 1px solid var(--line); background: var(--bg); box-shadow: 0 8px 28px rgba(21,32,46,.12); max-width: 100%; }
.box.wide { overflow: auto; }
iframe { border: 0; display: block; }
</style>
</head>
<body>
<div class="wrap">
<header>
  <button class="themebtn" id="themeBtn" type="button"></button>
  <h1>Экраны VPN Monitor __VERSION__</h1>
  <p class="lead">Настоящие страницы приложения — меню у иконки и окно в браузере — в основных состояниях таблицы
  <code>docs/ux/scenarios.md</code>, на тестовых данных. Наведите на значения — там подсказки. Собрано
  <code>python scripts/screens.py</code> __BUILT__; обновляется перед каждым релизом, который меняет интерфейс.</p>
</header>
<div id="sections"></div>
</div>
<script type="text/plain" id="tpl-flyout">__FLYOUT__</script>
<script type="text/plain" id="tpl-index">__INDEX__</script>
<script type="application/json" id="frames">__FRAMES__</script>
<script>
"use strict";
const FR = JSON.parse(document.getElementById("frames").textContent);
const unesc = (t) => t.split("<\\\\/script").join("</script");  // обратное к esc_text() в screens.py
const TPL = { flyout: unesc(document.getElementById("tpl-flyout").textContent), index: unesc(document.getElementById("tpl-index").textContent) };
const THEMES = [["", "как в системе", "M12 3a9 9 0 1 0 0 18zM12 3a9 9 0 0 1 0 18"], ["light", "светлая", "M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10zM12 1v2M12 21v2M4.2 4.2l1.4 1.4M18.4 18.4l1.4 1.4M1 12h2M21 12h2M4.2 19.8l1.4-1.4M18.4 5.6l1.4-1.4"], ["dark", "тёмная", "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"]];
let ti = 0;
const frames = [];
function build() {
  const theme = THEMES[ti][0];
  const host = document.getElementById("sections"); host.replaceChildren(); frames.length = 0;
  let section = null, grid = null;
  FR.forEach((f, i) => {
    if (f.section !== section) { section = f.section; const s = document.createElement("section"); const h = document.createElement("h2"); h.textContent = section; grid = document.createElement("div"); grid.className = "grid"; s.append(h, grid); host.append(s); }
    const mock = { ...f.mock, frame: i, theme, hash: f.hash };
    const doc = TPL[f.page].replace("<!--MOCK-->", '<script type="application/json" id="mock">' + JSON.stringify(mock).replace(/</g, "\\\\u003c") + "<\\/script>");
    const fig = document.createElement("figure"); const box = document.createElement("div"); box.className = "box" + (f.page === "index" ? " wide" : "");
    const ifr = document.createElement("iframe"); ifr.width = f.w; ifr.height = f.h || 300; ifr.title = f.caption; ifr.srcdoc = doc;
    box.append(ifr); const cap = document.createElement("figcaption"); cap.textContent = f.caption; fig.append(box, cap); grid.append(fig); frames[i] = ifr;
  });
}
window.addEventListener("message", (e) => { const d = e.data || {}; const m = /^h:(\\d+)/.exec(d.msg || ""); if (m && frames[d.frame] && FR[d.frame].page === "flyout") frames[d.frame].height = Math.min(760, Math.max(160, +m[1])); if (/^err:/.test(d.msg || "")) console.error("кадр", d.frame, d.msg); });
function paintTheme() {
  const [k, n, d] = THEMES[ti];
  if (k) document.documentElement.setAttribute("data-theme", k); else document.documentElement.removeAttribute("data-theme");
  const b = document.getElementById("themeBtn");
  b.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="' + d + '"/></svg>';
  b.title = "Тема: " + n + " — нажмите, чтобы сменить"; b.setAttribute("aria-label", "Тема: " + n);
}
document.getElementById("themeBtn").addEventListener("click", () => { ti = (ti + 1) % THEMES.length; paintTheme(); build(); });
paintTheme(); build();
</script>
</body>
</html>
"""


def esc_text(s):
    """Текст внутри <script type="text/plain">: не дать закрыть тег раньше времени."""
    return s.replace("</script", "<\\/script")


def render(now):
    fr = [{"section": s, "caption": c, "page": p, "w": w, "h": h, "hash": hs, "mock": dict(m, changes=CHANGELOG.read_text(encoding="utf-8"))}
          for s, c, p, w, h, hs, m in frames(now)]
    import re
    version = re.search(r'^version = "([^"]+)"', (common.ROOT / "app" / "Cargo.toml").read_text(encoding="utf-8"), re.M).group(1)
    return (GALLERY.replace("__FLYOUT__", esc_text(page_template("flyout")))
            .replace("__INDEX__", esc_text(page_template("index")))
            .replace("__FRAMES__", json.dumps(fr, ensure_ascii=False).replace("</", "<\\/"))
            .replace("__VERSION__", html.escape(version))
            .replace("__BUILT__", time.strftime("%Y-%m-%d")))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--check", action="store_true", help="не писать, а проверить, что docs/screens.html собрана из текущих страниц")
    args = ap.parse_args()
    if args.check:
        if not OUT.exists():
            fail("docs/screens.html нет — соберите: python scripts/screens.py")
            raise SystemExit(1)
        old = OUT.read_text(encoding="utf-8")
        for name in ("flyout", "index"):
            if esc_text(page_template(name)) not in old:
                fail(f"docs/screens.html устарела: app/web/{name}.html менялась — соберите: python scripts/screens.py")
                raise SystemExit(1)
        ok("docs/screens.html собрана из текущих страниц")
        return
    common.write_text(OUT, render(int(time.time() * 1000)))
    ok(f"галерея: {OUT.relative_to(common.ROOT)} ({OUT.stat().st_size // 1024} КБ)")


if __name__ == "__main__":
    main()
