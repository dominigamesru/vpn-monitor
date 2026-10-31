# VPN Monitor

Приложение в трее Windows, которое показывает, работает ли VPN в офис. Оно регулярно скачивает пробные данные с GitLab
и SVN офиса (и для сравнения — из интернета), красит иконку как светофор и шлёт метрики в Prometheus.

Статус: приложение в трее v0.1 (Rust, `app/`) — Confluence, SVN, GitLab, интернет, светофор, самообновление из GitHub
Releases; прототип замера — `scripts/net_speed.py`. Схема — [docs/architecture.html](docs/architecture.html).

## Установить и проверить
Скачать `vpn-monitor.exe` из [последнего релиза](https://github.com/dominigamesru/vpn-monitor/releases), запустить двойным
кликом, вписать креды в открывшийся `secrets.env`. Подробно и про лог — [docs/app.md](docs/app.md).

## Быстрый старт (скрипты)

```text
python -m venv .venv && .venv\Scripts\pip install -r requirements.txt
copy .env.example .env                       # заполнить: GitLab, SVN (скилл /setup-env)
.venv\Scripts\python scripts\check_env.py    # что установлено, чего не хватает и откуда взять
.venv\Scripts\python scripts\net_speed.py    # разовый замер канала до GitLab / SVN / интернета
```

Кириллица в консоли: скрипты пишут UTF-8. В PowerShell один раз за сессию —
`[Console]::OutputEncoding = [Text.UTF8Encoding]::new()`, в cmd — `chcp 65001`.

## Документация
- [docs/README.md](docs/README.md) — что где лежит;
- [docs/app.md](docs/app.md) — приложение в трее: что умеет, как тестировать, лог, релиз;
- [GLOSSARY.md](GLOSSARY.md) — термины;
- [docs/scripts-guide.md](docs/scripts-guide.md) — как писать скрипты;
- [docs/net_speed.md](docs/net_speed.md) — что меряет прототип и как читать результат.
