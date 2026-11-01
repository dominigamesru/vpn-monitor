# VPN Monitor — локальный API (для скриптов и ИИ-агентов)

Адрес: `http://127.0.0.1:{port}` — только с этой машины. Меняющие запросы (POST, DELETE) требуют заголовка
`X-VPN-Monitor: 1`. Ответы — JSON (UTF-8), обратно совместимы: поля только добавляются. Секретов в ответах нет.

| Метод | Адрес | Что делает |
|---|---|---|
| GET | `/api/state` | текущее состояние: светофор, оценка VPN, задержка, скорость, стабильность, срез по целям, версия, последняя версия, пороги |
| GET | `/api/history?range=day` | проверки за последние 24 часа (`range=week` — сегодня и 7 полных дней) |
| POST | `/api/check` | запустить проверку сейчас (если уже идёт — `started: false`) |
| POST | `/api/update` | проверить обновление сейчас (последняя версия — в `latest.version` релиза `latest`) |
| GET | `/api/support` | черновик заявки в поддержку: можно ли создать из приложения, тема, описание, файлы |
| POST | `/api/support` | создать заявку в Jira Service Desk от имени сотрудника: `{summary, description, priority, attach: [имена файлов]}` → `{created: {key, url, attach_failed}}` или `{error}`; заявка открывается в браузере |
| GET | `/api/tickets` | созданные из приложения заявки (`jira-tickets.json`), новые первыми |
| POST | `/api/diagnostics` | запустить диагностику подключения; результат — `GET /api/diagnostics` (`running`, `report`) |
| GET | `/api/log?lines=300` | хвост лога |
| GET | `/api/credentials` | кто вошёл логином AD (без пароля) |
| POST | `/api/credentials` | войти: `{"login": "...", "password": "..."}` — проверка на Jira, Confluence, SVN, при успехе пароль уходит в Диспетчер учётных данных Windows |
| DELETE | `/api/credentials` | выйти — удалить логин AD и токен GitLab из Диспетчера |
| GET | `/api/help` | эта инструкция |

Поля записи истории: `t` — время, мс UNIX; `c` — цвет (`green`/`yellow`/`red`); `b` — знак (`score`, `novpn` — VPN не
включён, `weak` — слабый интернет); `s` — оценка VPN 1–99; `lat` — задержка, мс (медиана последних проверок); `spd` —
скорость, Мбит/с; `inet` — интернет, Мбит/с (только в проверках, где он мерился); `stab` — стабильность 0–1; `tg` — по
целям (`r` — ответила, `ok` — проба прошла, `lat`, `mbps`, `f` — причина провала).

## Готовый скилл для агента

```markdown
---
name: vpn-monitor
description: Состояние VPN в офис Domini на этой машине — проверить, запустить проверку, посмотреть историю.
---
1. Состояние: `curl -s http://127.0.0.1:{port}/api/state` — смотри `snapshot.color`, `snapshot.badge`, `snapshot.score`.
   `badge = novpn` — VPN не включён; `weak` — плохой сам интернет; иначе оценка 1–99 (≥70 — хорошо).
2. Проверить сейчас: `curl -s -X POST -H "X-VPN-Monitor: 1" http://127.0.0.1:{port}/api/check`, подождать, пока
   `/api/state` → `checking: false`.
3. Почему плохо: `POST /api/diagnostics`, затем `GET /api/diagnostics` до `running: false`; по каждой цели — шаги
   (DNS, маршрут, TCP, HTTP, вход) и `hint`.
4. История: `/api/history?range=day` — ищи записи с `b = novpn`, `lat` выше `thresholds.latency_bad_ms`, `spd` ниже
   `thresholds.speed_bad_mbps`.
Порт — 5151; если занят, приложение берёт следующий (до 5160).
```
