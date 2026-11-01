# vpn-monitor

Приложение в трее Windows: регулярно скачивает пробные данные с GitLab и SVN офиса, показывает состояние VPN
иконкой-светофором и шлёт метрики в Prometheus. Приложение — Rust ([ADR 0001](docs/adr/0001-rust.md)); вспомогательные
скрипты — Python 3.11+ в `.venv`, запуск из корня. Репозиторий — GitHub `dominigamesru/vpn-monitor` (перенесён из
GitLab `devops/vpn-monitor-cli`).

## Где что
- `scripts/` — скрипты (запуск из корня: `python scripts/<имя>.py`), как писать — [docs/scripts-guide.md](docs/scripts-guide.md):
  - `common.py` — UTF-8 в консоли, пути (`ROOT`, `REPORTS`), `.env` (`ENV`, `require_env`), запись файлов UTF-8 + LF;
  - `console.py` — цветной вывод с иконками (`section/ok/fail/warn/info/detail`), `fmt_duration`;
  - `check_env.py` — что установлено и чего не хватает;
  - `net_speed.py` — первый прототип замера канала до GitLab/SVN/интернета ([docs/net_speed.md](docs/net_speed.md)).
- `app/` — приложение в трее (Rust): `cargo test`, сборка exe — CI; как устроено и как тестировать — [docs/app.md](docs/app.md);
  окно в браузере — `app/web/index.html` (отдаёт локальный API, `app/src/api.rs`, [ADR 0005](docs/adr/0005-window-in-browser.md)).
- `.github/workflows/` — CI (`ci.yml`), релиз при новой версии в `main` (`release.yml`), смена последней версии (`promote.yml`).
- `docs/` — устройство и решения ([docs/README.md](docs/README.md) — что куда), схема — [docs/architecture.html](docs/architecture.html).
- `GLOSSARY.md` — термины.
- `reports/`, `tools/`, `.venv/`, `.env` — в `.gitignore`.

## Скиллы
`/setup-env` (окружение и `.env`) · `/grill-with-docs` (допрос плана + глоссарий и ADR) · `grilling` · `domain-modeling` ·
`web-ui` (всё, что в браузере) · `ux-scenarios` (сценарии и состояния до любой пользовательской функции).

**Любая веб-задача — окно приложения (`app/web/`), графики, HTML-страницы и отчёты — только со скиллом `web-ui`**
(`.claude/skills/web-ui`): он подключает встроенный `dataviz` для графиков и требует скриншоты в светлой и тёмной теме.
**Любая пользовательская функция — сначала скилл `ux-scenarios`**: сценарии вширь и строка в таблице состояний
`docs/ux/scenarios.md`, потом дизайн.
**Крупная доработка интерфейса (не ресёрч) — сначала HTML-прототип с несколькими вариантами** в `docs/prototypes/`,
опубликованный как Artifact: варианты рядом на одних данных, переключатель ситуаций из таблицы состояний. Код — после
выбора заказчика.

## Правила
- Термины — по `GLOSSARY.md` (корень репо): канонические слова пользователя, синонимы в `_Avoid_`. Новый или
  спорный термин — сразу в глоссарий (скилл `/grill-with-docs` → `domain-modeling`, формат там же). Решения, которые
  трудно отменить, — ADR в `docs/adr/`.
- Секреты (`*_TOKEN`, `*_PASSWORD`) из `.env` не читать вслух и не логировать; вписывает только пользователь. Новый
  ключ — сначала в `.env.example` с комментарием «что это и откуда взять».
- Скрипты — по [docs/scripts-guide.md](docs/scripts-guide.md): `import common` первым, файлы через
  `common.write_text/write_json` (UTF-8, LF), цвет — только `console.py`, ошибки для человека — `SystemExit("что делать")`.
- Доки — по [docs/README.md](docs/README.md): не пересказывать код; одна тема — один документ; HTML для людей — с
  тёмной и светлой темой.
- Ретро — `docs/process/retro.md` (роли, шаги), журнал `docs/process/retro-log.md`; после крупного результата, серии
  ~10 попыток, нового класса ошибки и перед компактом. Правки скиллов — только через `docs/process/skill-notes.md`.
- Новые папки — `git check-ignore -v <путь>` до коммита.
- Работа — в feature-ветках + pull request на GitHub; в `main` сливает пользователь.

## Сборка и поставка
- Приложение в трее и CI/CD ведёт Claude: сборка, проверки и публикация — GitHub Actions; ручных шагов в релизе нет.
- Поставка — GitHub Releases ([ADR 0004](docs/adr/0004-delivery-github-releases.md)):
  - **релиз** (`GLOSSARY.md`) — GitHub Release «Domini VPN Monitor vX.Y.Z» с `domini-vpn-monitor.exe`, настройками и `sha256` (плюс `vpn-monitor.exe` для обновлятелей до 0.5.4), всегда `make_latest: false`;
    опубликованный релиз не перезаписывается и не удаляется;
  - релиз `latest` — только `latest.version` (последняя версия); откат — workflow `promote` со старой версией.
- Секретов в настройках нет — репо публичное.
- **Все JSON обратно совместимы** — `settings.json`, `latest.version`, файлы метрик, ответы API, данные в Диспетчере
  учётных данных: поля только добавляются (новые — с `#[serde(default)]`), старые не удаляются и не меняют смысл,
  незнакомые поля и значения терпятся (`#[serde(other)]`, без `deny_unknown_fields`). Новое значение у старого поля
  (например, новый вид входа в `auth`) — нельзя: его прочитает старая версия; кладите в новое поле (`auth_kind`).
  Проверяют тесты `settings_json_readable_by_v0_2` и `*_compatible`; старый обновлятель разбирает `settings.json`
  новой версии своей схемой.
- Секреты CI — только в secrets репозитория/окружения GitHub; в workflow и логах не печатать.
