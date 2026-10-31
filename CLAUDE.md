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
- `docs/` — устройство и решения ([docs/README.md](docs/README.md) — что куда), схема — [docs/architecture.html](docs/architecture.html).
- `GLOSSARY.md` — термины.
- `reports/`, `tools/`, `.venv/`, `.env` — в `.gitignore`.

## Скиллы
`/setup-env` (окружение и `.env`) · `/grill-with-docs` (допрос плана + глоссарий и ADR) · `grilling` · `domain-modeling`.

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
- **Релиз** (см. `GLOSSARY.md`) кладётся в `releases/<версия>/` — никогда не в `latest/`. **Настройки** остаются в
  `latest/`. Старые релизы не перезаписываются и не удаляются.
- Раскладка `latest/` и `releases/` и формат настроек — контракт со всеми установленными копиями
  ([ADR 0003](docs/adr/0003-update-from-share.md)): менять только с обратной совместимостью.
- Секреты CI — только в secrets репозитория/окружения GitHub; в workflow и логах не печатать.
