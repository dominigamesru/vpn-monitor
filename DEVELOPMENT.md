# Разработка Domini VPN Monitor

Правила работы (термины, секреты, ветки, релизы, совместимость JSON) — [CLAUDE.md](CLAUDE.md). Что где в
документации — [docs/README.md](docs/README.md).

## Где что
| Где | Что |
|---|---|
| `app/` | приложение (Rust, [ADR 0001](docs/adr/0001-rust.md)): `cd app && cargo test`; настройки по умолчанию — `app/config/settings.json`; меню у иконки — `app/web/flyout.html`, окно — `app/web/index.html`, API — `app/src/api.rs` (`/api/help`) |
| `app/src/route.rs` | модель маршрута: звенья по медиане трёх проверок, виновник, каскад |
| `.github/workflows/` | `ci` — clippy, тесты, обновление «последний релиз → эта сборка» на Windows; `release` — релиз, когда в `main` приходит новая версия; `promote` — смена последней версии (откат) |
| `.github/scripts/upgrade_test.py` | E2E на Windows: опубликованная версия сама обновляется на сборку, переходит на `domini-vpn-monitor.exe`, меню в WebView2 загружается и рисуется |
| `scripts/` | служебные скрипты (Python): `screens.py` — галерея экранов, `check_env.py`, `net_speed.py` |
| `docs/` | устройство и решения: [app.md](docs/app.md), [ADR](docs/adr/), [сценарии и состояния](docs/ux/scenarios.md), [спеки](docs/specs/), [прототипы](docs/prototypes/), [галерея экранов](docs/screens.html) |
| `.claude/skills/` | скиллы для Claude Code: `release`, `web-ui`, `ux-scenarios` и др. |

## Сборка и проверки
```bash
cd app && cargo test
```
```bash
cd app && cargo clippy --target x86_64-pc-windows-gnu --all-targets -- -D warnings
```
Трей, установка и WebView2 — только Windows; на Linux доступны `--check` и тесты. Полная проверка — CI на Windows.

## Интерфейс
- Сначала сценарии и строка в таблице состояний [docs/ux/scenarios.md](docs/ux/scenarios.md) (скилл `ux-scenarios`),
  для крупной доработки — HTML-прототип с вариантами в `docs/prototypes/`, потом код (скилл `web-ui`).
- Перед релизом, который меняет интерфейс: `python scripts/screens.py` — галерея [docs/screens.html](docs/screens.html)
  из настоящих страниц; скриншоты для README — из неё (скилл `release`).

## Выпуск версии
Скилл `release` (`.claude/skills/release/SKILL.md`). Коротко: поднять `version` в `app/Cargo.toml` (`0.6.0` —
стабильная, `0.6.0-beta.1` — бета) → pull request → зелёный CI → слияние в `main` → CI публикует релиз и обновляет
канал; установленные копии переходят сами. Откат — Actions → `promote` → старая версия
([ADR 0004](docs/adr/0004-delivery-github-releases.md)).

## Скрипты
```bash
python -m venv .venv && .venv/Scripts/pip install -r requirements.txt
```
```bash
python scripts/check_env.py
```
Кириллица в консоли Windows: скрипты пишут UTF-8; в PowerShell — `[Console]::OutputEncoding = [Text.UTF8Encoding]::new()`,
в cmd — `chcp 65001`. Как писать скрипты — [docs/scripts-guide.md](docs/scripts-guide.md).
