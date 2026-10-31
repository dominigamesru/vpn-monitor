"""Прогон обновления «последний опубликованный релиз → эта сборка» на Windows-runner (ADR 0004).

Поднимает локальную копию раскладки релизов (latest/latest.version, vX/settings.json, vX/vpn-monitor.exe), ставит в
папку установки exe последнего релиза с его же settings.json (адрес релизов → локальный сервер), запускает его в
режиме трея и ждёт, что он сам обновится на новую сборку: скачает, сверит SHA-256, перезапустится, и новая версия
ответит по API. Ловит главное: старый обновлятель не понимает settings.json новой версии.

Запуск (из корня репо, Windows): python .github/scripts/upgrade_test.py --exe app/target/release/vpn-monitor.exe
Код возврата: 0 — обновилось (или нечего проверять: версии совпадают), 1 — не обновилось.
"""

import argparse
import functools
import hashlib
import http.server
import json
import os
import pathlib
import shutil
import subprocess
import sys
import threading
import time
import tomllib
import urllib.request
import zlib

REPO = "dominigamesru/vpn-monitor"
GH = f"https://github.com/{REPO}/releases/download"
PORT = 8765
BASE = f"http://127.0.0.1:{PORT}"


def fetch(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=60) as r:
        return r.read()


def main() -> int:
    # консоль Windows-runner — cp1252; лог и сообщения — по-русски
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--exe", required=True, help="новая сборка vpn-monitor.exe")
    ap.add_argument("--timeout", type=int, default=150, help="сколько ждать обновления, с")
    args = ap.parse_args()

    root = pathlib.Path(__file__).resolve().parents[2]
    new_ver = tomllib.loads((root / "app/Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    old_ver = fetch(f"{GH}/latest/latest.version").decode().strip()
    print(f"последний релиз {old_ver}, эта сборка {new_ver}")
    if old_ver == new_ver:
        print("::notice::Версия не поднята — обновлению не на что переходить, прогон пропущен.")
        return 0

    work = pathlib.Path(os.environ.get("RUNNER_TEMP", ".")) / "upgrade-test"
    shutil.rmtree(work, ignore_errors=True)
    srv = work / "releases"
    (srv / "latest").mkdir(parents=True)
    (srv / f"v{new_ver}").mkdir()

    # новая сборка в локальной раскладке релизов — так же, как её публикует release.yml
    exe = pathlib.Path(args.exe).read_bytes()
    new_settings = json.loads((root / "app/config/settings.json").read_text(encoding="utf-8"))
    new_settings["release_base_url"] = BASE
    new_settings["exe"] = {
        "version": new_ver,
        "sha256": hashlib.sha256(exe).hexdigest(),
        "crc32": f"{zlib.crc32(exe) & 0xFFFFFFFF:08x}",
        "size": len(exe),
    }
    (srv / f"v{new_ver}/vpn-monitor.exe").write_bytes(exe)
    (srv / f"v{new_ver}/settings.json").write_text(json.dumps(new_settings, ensure_ascii=False, indent=2), encoding="utf-8")
    (srv / "latest/latest.version").write_text(new_ver + "\n", encoding="utf-8")

    # установленная копия — последний релиз со своими же настройками, но адрес релизов — локальный
    install = pathlib.Path(os.environ["LOCALAPPDATA"]) / "DominiVPNMonitor"
    shutil.rmtree(install, ignore_errors=True)
    install.mkdir(parents=True)
    old_exe = install / f"vpn-monitor-{old_ver}.exe"
    old_exe.write_bytes(fetch(f"{GH}/v{old_ver}/vpn-monitor.exe"))
    old_settings = json.loads(fetch(f"{GH}/v{old_ver}/settings.json"))
    old_settings["release_base_url"] = BASE
    (install / "settings.json").write_text(json.dumps(old_settings, ensure_ascii=False, indent=2), encoding="utf-8")

    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(srv))
    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()

    print(f"запускаю {old_exe.name} --tray")
    subprocess.Popen([str(old_exe), "--tray"], cwd=install)
    log = install / "vpn-monitor.log"
    ok = False
    deadline = time.time() + args.timeout
    while time.time() < deadline:
        time.sleep(3)
        text = log.read_text(encoding="utf-8", errors="replace") if log.exists() else ""
        if "ERROR [update]" in text or "не создать иконку" in text:
            break
        for port in range(5151, 5161):
            try:
                st = json.loads(fetch(f"http://127.0.0.1:{port}/api/state"))
            except Exception:
                continue
            if st.get("version") == new_ver:
                ok = True
            break
        if ok:
            break

    print("---- vpn-monitor.log ----")
    print(log.read_text(encoding="utf-8", errors="replace") if log.exists() else "(лога нет)")
    subprocess.run(["taskkill", "/F", "/IM", f"vpn-monitor-{old_ver}.exe"], capture_output=True)
    subprocess.run(["taskkill", "/F", "/IM", f"vpn-monitor-{new_ver}.exe"], capture_output=True)
    httpd.shutdown()
    if ok:
        print(f"OK: {old_ver} сам обновился до {new_ver}, новая версия отвечает по API")
        return 0
    print(f"::error::{old_ver} не обновился до {new_ver} за {args.timeout} с — см. лог выше")
    return 1


if __name__ == "__main__":
    sys.exit(main())
