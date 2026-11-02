//! Локальный MCP-сервер (v0.7, `docs/specs/ai-agent-mcp.md`): AI-агент сотрудника (Claude Code, Claude Desktop)
//! спрашивает у VPN Monitor статус, историю, отчёт для поддержки. Транспорт — Streamable HTTP на `/mcp` того же
//! локального порта: POST с JSON-RPC 2.0, ответ — `application/json` (без SSE). Здесь — протокол; данные даёт
//! приложение через [`Data`]. Секретов в ответах нет — те же правила, что в логе и сводке.

use serde_json::{Value, json};

pub const PROTOCOL: &str = "2025-06-18";

/// Откуда MCP берёт данные (в приложении — общее состояние трея; в тестах — заглушка).
pub trait Data {
    /// Сводка для поддержки — несколько строк текста.
    fn summary(&self) -> String;
    /// Состояние для машины: вывод, маршрут, цифры звеньев, сервисы (JSON без секретов).
    fn status(&self) -> Value;
    /// История: `hour` / `day` / `week` — сводка (сколько проверок, когда было плохо, медианы).
    fn history(&self, range: &str) -> Value;
    /// Проверить сейчас и вернуть итог (может занять до минуты).
    fn check(&self) -> String;
    /// Диагностика подключения: шаги проверки каждого сервиса (последний отчёт или новый запуск).
    fn diagnostics(&self) -> Value;
    /// Хвост лога без секретов.
    fn log_tail(&self, lines: usize) -> String;
    /// Отчёт HTML (тёмная и светлая тема) — для показа агентом.
    fn report_html(&self) -> String;
    fn version(&self) -> String;
    /// Уведомление Windows сотруднику (заголовок, текст, ссылка https) — агент привлекает внимание (v0.9).
    fn notify(&self, title: &str, text: &str, link: Option<&str>) -> String;
}

fn tools() -> Value {
    let no_args = json!({"type": "object", "properties": {}, "additionalProperties": false});
    json!([
        {"name": "vpn_status", "title": "Состояние VPN",
         "description": "Можно ли сейчас работать и в чём причина, если нет: вывод, оценка 1–99, звенья маршрута интернет → VPN → офис → мир с цифрами, сервисы офиса и мира, клиент и профиль VPN, советы.",
         "inputSchema": no_args, "annotations": {"readOnlyHint": true}},
        {"name": "vpn_history", "title": "История VPN",
         "description": "История за час, сутки или неделю: сколько проверок, когда и сколько было плохо, медианы задержки VPN, скорости SVN и интернета.",
         "inputSchema": {"type": "object", "properties": {"range": {"type": "string", "enum": ["hour", "day", "week"], "description": "период, по умолчанию day"}}, "additionalProperties": false},
         "annotations": {"readOnlyHint": true}},
        {"name": "vpn_check", "title": "Проверить VPN сейчас",
         "description": "Запускает проверку прямо сейчас (до минуты) и возвращает свежий итог. Безопасно: только читает сервисы офиса.",
         "inputSchema": no_args, "annotations": {"readOnlyHint": false, "idempotentHint": true}},
        {"name": "vpn_report", "title": "Отчёт для поддержки",
         "description": "Полный отчёт для запроса в поддержку: сводка (пользователь, состояние, сервисы, клиенты VPN, профиль), советы, диагностика и хвост лога. Без паролей и токенов.",
         "inputSchema": no_args, "annotations": {"readOnlyHint": true}},
        {"name": "vpn_notify", "title": "Уведомить сотрудника",
         "description": "Показывает уведомление Windows на компьютере сотрудника: заголовок, текст и необязательная ссылка (https) — кнопка «Открыть». Чтобы привлечь внимание: задача готова, нужен ответ, что-то сломалось.",
         "inputSchema": {"type": "object", "properties": {
             "title": {"type": "string", "description": "заголовок, до 80 символов"},
             "text": {"type": "string", "description": "текст, до 300 символов"},
             "link": {"type": "string", "description": "ссылка https:// — откроется по кнопке «Открыть» или клику"}},
           "required": ["title"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "idempotentHint": false, "destructiveHint": false}},
        {"name": "vpn_diagnostics", "title": "Диагностика подключения",
         "description": "Шаги проверки каждого сервиса: DNS, соединение, вход, что сделать.",
         "inputSchema": no_args, "annotations": {"readOnlyHint": true}},
    ])
}

fn resources() -> Value {
    json!([
        {"uri": "vpn://status", "name": "status", "title": "Состояние VPN — текст", "mimeType": "text/plain"},
        {"uri": "vpn://report.html", "name": "report", "title": "Отчёт VPN — HTML", "mimeType": "text/html"},
    ])
}

fn text(t: String) -> Value {
    json!({"content": [{"type": "text", "text": t}]})
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Один JSON-RPC запрос. `None` — уведомление (ответа нет, HTTP 202).
pub fn handle(req: &Value, data: &dyn Data) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = id?; // без id — уведомление (notifications/initialized и др.)
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => ok(
            &id,
            json!({
                "protocolVersion": PROTOCOL,
                "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false}},
                "serverInfo": {"name": "domini-vpn-monitor", "title": "Domini VPN Monitor", "version": data.version()},
                "instructions": "Состояние VPN сотрудника: vpn_status — сейчас, vpn_history — за период, vpn_report — для запроса в поддержку. Отвечай по-русски, начинай с вывода и причины.",
            }),
        ),
        "ping" => ok(&id, json!({})),
        "tools/list" => ok(&id, json!({"tools": tools()})),
        "resources/list" => ok(&id, json!({"resources": resources()})),
        "resources/read" => match params.get("uri").and_then(|u| u.as_str()) {
            Some("vpn://status") => ok(
                &id,
                json!({"contents": [{"uri": "vpn://status", "mimeType": "text/plain", "text": data.summary()}]}),
            ),
            Some("vpn://report.html") => ok(
                &id,
                json!({"contents": [{"uri": "vpn://report.html", "mimeType": "text/html", "text": data.report_html()}]}),
            ),
            _ => err(&id, -32602, "неизвестный ресурс"),
        },
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match name {
                "vpn_status" => {
                    let mut r = text(data.summary());
                    r["structuredContent"] = data.status();
                    ok(&id, r)
                }
                "vpn_history" => {
                    let range = args.get("range").and_then(|r| r.as_str()).unwrap_or("day");
                    let range = if ["hour", "day", "week"].contains(&range) {
                        range
                    } else {
                        "day"
                    };
                    let h = data.history(range);
                    let mut r = text(pretty(&h));
                    r["structuredContent"] = h;
                    ok(&id, r)
                }
                "vpn_check" => ok(&id, text(data.check())),
                "vpn_report" => ok(
                    &id,
                    text(format!(
                        "{}\n\nДиагностика:\n{}\n\nЛог (последние строки):\n{}",
                        data.summary(),
                        pretty(&data.diagnostics()),
                        data.log_tail(80)
                    )),
                ),
                "vpn_diagnostics" => ok(&id, text(pretty(&data.diagnostics()))),
                "vpn_notify" => {
                    let title: String = args["title"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .chars()
                        .take(80)
                        .collect();
                    if title.is_empty() {
                        return Some(err(&id, -32602, "нужен title"));
                    }
                    let body: String = args["text"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(300)
                        .collect();
                    let link = args["link"].as_str().filter(|l| l.starts_with("https://"));
                    ok(&id, text(data.notify(&title, &body, link)))
                }
                _ => err(&id, -32602, &format!("неизвестный инструмент: {name}")),
            }
        }
        _ => err(&id, -32601, &format!("метод не поддерживается: {method}")),
    })
}

/// Тело POST /mcp: один запрос или пакет. `None` — только уведомления (HTTP 202 без тела).
pub fn handle_body(body: &str, data: &dyn Data) -> Result<Option<Value>, Value> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| err(&Value::Null, -32700, &format!("не JSON: {e}")))?;
    match v {
        Value::Array(items) => {
            let out: Vec<Value> = items.iter().filter_map(|r| handle(r, data)).collect();
            Ok((!out.is_empty()).then_some(Value::Array(out)))
        }
        one => Ok(handle(&one, data)),
    }
}

/// Запрос с чужой страницы в браузере не пускаем: `Origin` — только локальный (защита от DNS rebinding / CSRF).
pub fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => {
            let o = o.trim_end_matches('/');
            o.starts_with("http://127.0.0.1:") || o.starts_with("http://localhost:") || o == "null"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl Data for Fake {
        fn summary(&self) -> String {
            "Состояние: SVN качает медленно".into()
        }
        fn status(&self) -> Value {
            json!({"score": 47, "cause": "vpn"})
        }
        fn history(&self, range: &str) -> Value {
            json!({"range": range, "checks": 3})
        }
        fn check(&self) -> String {
            "проверено".into()
        }
        fn diagnostics(&self) -> Value {
            json!({"targets": []})
        }
        fn log_tail(&self, _: usize) -> String {
            "лог".into()
        }
        fn report_html(&self) -> String {
            "<!doctype html>".into()
        }
        fn version(&self) -> String {
            "0.7.1".into()
        }
        fn notify(&self, title: &str, _: &str, link: Option<&str>) -> String {
            format!(
                "показано: {title}{}",
                link.map(|l| format!(" ({l})")).unwrap_or_default()
            )
        }
    }

    fn call(body: Value) -> Value {
        handle_body(&body.to_string(), &Fake).unwrap().unwrap()
    }

    #[test]
    fn initialize_and_list() {
        let r = call(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": PROTOCOL}}),
        );
        assert_eq!(r["result"]["serverInfo"]["name"], "domini-vpn-monitor");
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL);
        let r = call(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "vpn_status",
                "vpn_history",
                "vpn_check",
                "vpn_report",
                "vpn_notify",
                "vpn_diagnostics"
            ]
        );
        // уведомление — без ответа
        assert!(
            handle_body(
                &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
                &Fake
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn tools_call_and_errors() {
        let r = call(
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "vpn_status", "arguments": {}}}),
        );
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("SVN")
        );
        assert_eq!(r["result"]["structuredContent"]["score"], 47);
        let r = call(
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "vpn_history", "arguments": {"range": "bogus"}}}),
        );
        assert_eq!(r["result"]["structuredContent"]["range"], "day");
        let r = call(
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "nope"}}),
        );
        assert_eq!(r["error"]["code"], -32602);
        assert!(handle_body("{", &Fake).is_err());
        let r = call(
            json!({"jsonrpc": "2.0", "id": 6, "method": "resources/read", "params": {"uri": "vpn://report.html"}}),
        );
        assert_eq!(r["result"]["contents"][0]["mimeType"], "text/html");
        let r = call(
            json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "vpn_notify", "arguments": {"title": "Сборка готова", "link": "http://bad"}}}),
        );
        assert_eq!(r["result"]["content"][0]["text"], "показано: Сборка готова"); // не https — без ссылки
        let r = call(
            json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": {"name": "vpn_notify", "arguments": {}}}),
        );
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn origin_check() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("http://127.0.0.1:5151")));
        assert!(!origin_allowed(Some("https://evil.example")));
    }
}
