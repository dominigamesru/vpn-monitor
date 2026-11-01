---
paths: ["app/web/**", "docs/**/*.html", "scripts/screens.py"]
---

# Страницы приложения и HTML-доки

Скилл `web-ui` — обязателен; здесь — грабли, на которые уже наступили.

- **Меню у иконки живёт в WebView2 (wry), а не в браузере.** wry объявляет `window.ipc` через
  `Object.defineProperty` (неконфигурируемое) — `const ipc`/`let ipc` на верхнем уровне страницы = SyntaxError при
  разборе, скрипт не запускается целиком, в логе тишина (0.5.0–0.5.1). Функцию назвать иначе (`toApp`); ловец
  ошибок — отдельным первым `<script>`, чтобы ошибки разбора доходили до лога (`err:` → `flyout`).
- **Имитация хозяина — по его исходникам:** в тестах Playwright объявлять `window.ipc` ровно как wry
  (`Object.defineProperty(window, "ipc", { value: Object.freeze({ postMessage }) })`), окно менять по `h:N`.
  Финальная проверка — CI на Windows (`upgrade_test.py`: «страница меню на связи» и «нарисована»).
- **Высота окна — по `document.body`, не `scrollHeight`** (тот не меньше окна — окно только растёт); повтор той же
  высоты не слать. `requestAnimationFrame` в скрытом окне не срабатывает — первое сообщение (`ready`) — сразу.
- **Не зависеть от библиотек в главном:** без Tippy меню работает (панели — свои), подсказки — только украшение.
- **`replaceChildren(..., null)` вставляет текст «null»** — фильтровать `null` перед вставкой.
- **Тема:** токены на `:root`, тёмные — в `@media (prefers-color-scheme: dark)` под `:root:not([data-theme="light"])`
  и в `:root[data-theme="dark"]`; выбор сотрудника — `user.json` → `theme`.
- **Данные из API — только `textContent`**, меняющие запросы — `POST` с `X-VPN-Monitor: 1`, пороги — из
  `/api/state` (`thresholds`), не зашивать.
- **Страницы встроены в exe и отдаются с `Cache-Control: no-store`** — папка данных WebView2 одна на все версии.
- **Галерея** `docs/screens.html` собирается `python scripts/screens.py`; CI (`--check`) падает, если страницы
  менялись, а галерея — нет.
