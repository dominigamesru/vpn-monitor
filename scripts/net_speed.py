"""Замер канала до GitLab и SVN: задержка, скорость скачивания, реальные операции git/svn.

Запуск из корня репо:  python scripts/net_speed.py [--seconds 20] [--skip-git] [--skip-svn]
Креды — из .env (GITLAB_TOKEN, SVN_USER, SVN_PASSWORD; можно PROJECT_ENV_FILE=<путь к .env>); значения не печатаются.
Всё качается во временную папку и удаляется в конце.
"""
import argparse
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
from base64 import b64encode
from pathlib import Path

GITLAB = "https://gitlab.dominigames.com"
GIT_REPO = "sa/sa-hge-autotests"              # средний репо для пробного клона
GIT_ARCHIVE_PROJECT = "engine%2Fmobile"        # большой репо — для потоковой скорости (архив ветки)
GIT_ARCHIVE_REF = "main"
SVN_BASE = "http://svn.corp.dominigames.com/svn/DC1/DC1_FULL/GAME_Domini"
SVN_DIR = "data/sprites"                       # много файлов — скорость «файлов в секунду»


def load_env():
    path = os.environ.get("PROJECT_ENV_FILE") or str(Path(__file__).resolve().parents[1] / ".env")
    env = {}
    if Path(path).exists():
        for line in Path(path).read_text(encoding="utf-8").splitlines():
            if "=" in line and not line.lstrip().startswith("#"):
                k, v = line.split("=", 1)
                env[k.strip()] = v.strip().strip('"')
    return env


def mbit(nbytes, secs):
    return nbytes * 8 / 1e6 / secs if secs > 0 else 0.0


def latency(url):
    host = urllib.parse.urlparse(url).hostname
    port = 443 if url.startswith("https") else 80
    t = time.perf_counter()
    try:
        addr = socket.gethostbyname(host)
    except OSError as e:
        return f"DNS {host}: ошибка {e}"
    dns = (time.perf_counter() - t) * 1000
    t = time.perf_counter()
    try:
        socket.create_connection((addr, port), timeout=10).close()
    except OSError as e:
        return f"{host}: DNS {dns:.0f} мс, TCP ошибка {e}"
    tcp = (time.perf_counter() - t) * 1000
    return f"{host}: DNS {dns:.0f} мс, TCP connect {tcp:.0f} мс"


def stream(url, headers, seconds):
    """Качает поток до seconds секунд, возвращает (байт, секунд, ошибка)."""
    req = urllib.request.Request(url, headers=headers)
    got, t0 = 0, time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            while time.perf_counter() - t0 < seconds:
                chunk = r.read(256 * 1024)
                if not chunk:
                    break
                got += len(chunk)
    except Exception as e:  # noqa: BLE001
        return got, time.perf_counter() - t0, str(e)[:120]
    return got, time.perf_counter() - t0, None


def run_timed(cmd, cwd, env, timeout):
    t0 = time.perf_counter()
    try:
        r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
        return time.perf_counter() - t0, r.returncode, (r.stderr or "")[-200:]
    except subprocess.TimeoutExpired:
        return time.perf_counter() - t0, "timeout", ""


def dir_stats(path):
    files, size = 0, 0
    for root, _, names in os.walk(path):
        if ".git" in root.split(os.sep) or ".svn" in root.split(os.sep):
            continue
        for n in names:
            files += 1
            try:
                size += os.path.getsize(os.path.join(root, n))
            except OSError:
                pass
    return files, size


def main():
    for s in (sys.stdout, sys.stderr):
        try:
            s.reconfigure(encoding="utf-8")
        except (AttributeError, ValueError):
            pass
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=int, default=20, help="длительность каждого замера потока/операции")
    ap.add_argument("--skip-git", action="store_true")
    ap.add_argument("--skip-svn", action="store_true")
    a = ap.parse_args()
    env = load_env()
    tmp = Path(tempfile.mkdtemp(prefix="net_speed_"))
    print(f"Временная папка: {tmp}\n")
    try:
        print("== Задержка")
        print(" ", latency(GITLAB))
        print(" ", latency(SVN_BASE))
        print(" ", latency("https://proof.ovh.net"), " (интернет, для сравнения)")

        print("\n== Интернет: поток proof.ovh.net (для сравнения с VPN)")
        n, s, err = stream("https://proof.ovh.net/files/100Mb.dat", {"User-Agent": "Mozilla/5.0 net_speed"}, a.seconds)
        print(f"  {n/1e6:.1f} МБ за {s:.1f} с = {mbit(n, s):.1f} Мбит/с" + (f"  [ошибка: {err}]" if err else ""))

        if not a.skip_git:
            tok = env.get("GITLAB_TOKEN", "")
            print("\n== GitLab: поток (архив ветки engine/mobile)")
            url = f"{GITLAB}/api/v4/projects/{GIT_ARCHIVE_PROJECT}/repository/archive.zip?sha={GIT_ARCHIVE_REF}"
            n, s, err = stream(url, {"PRIVATE-TOKEN": tok}, a.seconds)
            print(f"  {n/1e6:.1f} МБ за {s:.1f} с = {mbit(n, s):.1f} Мбит/с ({n/1e6/s if s else 0:.2f} МБ/с)"
                  + (f"  [ошибка: {err}]" if err else ""))

            print(f"\n== git clone --depth 1 {GIT_REPO} (лимит {a.seconds * 3} с)")
            genv = dict(os.environ, GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="never")
            url = f"https://oauth2:{tok}@gitlab.dominigames.com/{GIT_REPO}.git"
            secs, rc, tail = run_timed(["git", "-c", "credential.helper=", "clone", "-q", "--depth", "1", "--branch", "bootstrap", url,
                                        str(tmp / "git")], tmp, genv, a.seconds * 3)
            gitdir = tmp / "git" / ".git"
            pack = sum(p.stat().st_size for p in gitdir.rglob("*") if p.is_file()) if gitdir.exists() else 0
            f, _ = dir_stats(tmp / "git")
            print(f"  код {rc}, {secs:.1f} с; скачано {pack/1e6:.1f} МБ = {mbit(pack, secs):.1f} Мбит/с; файлов {f}"
                  + (f"  [{tail.replace(tok, '***').strip()}]" if rc not in (0,) and tail else ""))

        if not a.skip_svn:
            user, pwd = env.get("SVN_USER", ""), env.get("SVN_PASSWORD", "")
            auth = {"Authorization": "Basic " + b64encode(f"{user}:{pwd}".encode()).decode()}
            print("\n== SVN: поток (один большой файл по HTTP)")
            big = None
            try:
                req = urllib.request.Request(f"{SVN_BASE}/data/movies/", headers=auth)
                listing = urllib.request.urlopen(req, timeout=30).read().decode("utf-8", "ignore")
                import re
                names = [h for h in re.findall(r'href="([^"?]+)"', listing) if not h.endswith("/") and not h.startswith(("http", "/", ".."))]
                big = names[0] if names else None
            except Exception as e:  # noqa: BLE001
                print(f"  листинг movies: ошибка {str(e)[:100]}")
            if big:
                n, s, err = stream(f"{SVN_BASE}/data/movies/{big}", auth, a.seconds)
                print(f"  {big}: {n/1e6:.1f} МБ за {s:.1f} с = {mbit(n, s):.1f} Мбит/с" + (f"  [ошибка: {err}]" if err else ""))
            else:
                print("  большой файл не найден — поток не замерен")

            print(f"\n== SVN: много мелких файлов из {SVN_DIR}, 16 потоков (как bootstrap), {a.seconds} с")
            import re
            from concurrent.futures import ThreadPoolExecutor

            def ls(url):
                req = urllib.request.Request(url, headers=auth)
                text = urllib.request.urlopen(req, timeout=30).read().decode("utf-8", "ignore")
                return [h for h in re.findall(r'href="([^"]+)"', text) if not h.startswith(("http", "?", "/", ".."))]

            files, dirs = [], [f"{SVN_BASE}/{SVN_DIR}/"]
            t_ls = time.perf_counter()
            while dirs and len(files) < 3000 and time.perf_counter() - t_ls < a.seconds * 3:
                d = dirs.pop(0)
                try:
                    for h in ls(d):
                        (dirs if h.endswith("/") else files).append(d + h)
                except Exception as e:  # noqa: BLE001
                    print(f"  листинг: ошибка {str(e)[:100]}")
                    break
            ls_secs = time.perf_counter() - t_ls
            got = {"n": 0, "b": 0}
            t0 = time.perf_counter()

            def fetch(u):
                if time.perf_counter() - t0 > a.seconds:
                    return
                try:
                    data = urllib.request.urlopen(urllib.request.Request(u, headers=auth), timeout=60).read()
                    got["n"] += 1
                    got["b"] += len(data)
                except Exception:  # noqa: BLE001
                    pass

            with ThreadPoolExecutor(16) as ex:
                list(ex.map(fetch, files))
            secs = time.perf_counter() - t0
            print(f"  листинг: {len(files)} файлов найдено за {ls_secs:.1f} с")
            print(f"  скачано {got['n']} файлов ({got['n']/secs if secs else 0:.1f} файл/с), {got['b']/1e6:.1f} МБ "
                  f"за {secs:.1f} с = {mbit(got['b'], secs):.1f} Мбит/с")
    finally:
        def _rw(func, path, _exc):
            os.chmod(path, 0o666)
            func(path)
        shutil.rmtree(tmp, onerror=_rw)
        print(f"\nВременная папка удалена: {not tmp.exists()}")


if __name__ == "__main__":
    sys.exit(main())
