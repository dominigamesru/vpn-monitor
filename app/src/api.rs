//! Локальный HTTP API и окно в браузере (R7, ADR 0005). Только `127.0.0.1`, порт 5151; занят — следующий, до 10
//! попыток. Защита от чужих страниц: проверяется заголовок `Host`, а меняющие запросы (POST) требуют заголовка
//! `X-VPN-Monitor: 1` — браузер не пошлёт его с чужого сайта без preflight, на который мы не отвечаем.

use crate::app::{Cmd, SharedRef, lock};
use crate::win;
use serde_json::{Value, json};
use std::io::Read;
use std::sync::mpsc::Sender;
use std::time::UNIX_EPOCH;
use tiny_http::{Header, Method, Request, Response, Server};
use vpn_monitor::metrics::local_day_start;
use vpn_monitor::probe::ProbeResult;
use vpn_monitor::secrets::Secrets;
use vpn_monitor::{VERSION, diag, error, info, log};

const PAGE: &str = include_str!("../web/index.html");
const HELP: &str = include_str!("../web/api-help.md");
const FLYOUT: &str = include_str!("../web/flyout.html");
const POPPER: &str = include_str!("../web/vendor/popper.min.js");
const TIPPY: &str = include_str!("../web/vendor/tippy.min.js");
const TIPPY_CSS: &str = include_str!("../web/vendor/tippy.css");
const CHANGELOG: &str = include_str!("../CHANGELOG.md");
pub const BASE_PORT: u16 = 5151;

/// Поднимает сервер в фоне; возвращает адрес окна.
pub fn start(
    shared: SharedRef,
    cmd: Sender<Cmd>,
    update_now: Sender<()>,
) -> Result<String, String> {
    let mut last_err = String::new();
    for port in BASE_PORT..BASE_PORT + 10 {
        match Server::http(("127.0.0.1", port)) {
            Ok(server) => {
                let url = format!("http://127.0.0.1:{port}/");
                std::thread::spawn(move || serve(server, port, shared, cmd, update_now));
                return Ok(url);
            }
            Err(e) => last_err = format!("порт {port}: {e}"),
        }
    }
    Err(format!(
        "порты {BASE_PORT}–{} заняты ({last_err})",
        BASE_PORT + 9
    ))
}

fn serve(server: Server, port: u16, shared: SharedRef, cmd: Sender<Cmd>, update_now: Sender<()>) {
    for req in server.incoming_requests() {
        if let Err(e) = handle(req, port, &shared, &cmd, &update_now) {
            error!("api", "{e}");
        }
    }
}

fn header(req: &Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn ctype(v: &str) -> Header {
    Header::from_bytes(&b"Content-Type"[..], v.as_bytes()).expect("валидный заголовок")
}

/// Страницы и библиотеки встроены в exe: кеш WebView2 и браузера не должен показывать прошлую версию после
/// обновления (папка данных WebView2 одна на все версии).
fn no_store() -> Header {
    Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).expect("заголовок")
}

fn json_resp(v: &Value, code: u16) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(v.to_string())
        .with_status_code(code)
        .with_header(ctype("application/json; charset=utf-8"))
        .with_header(
            Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).expect("заголовок"),
        )
}

fn handle(
    mut req: Request,
    port: u16,
    shared: &SharedRef,
    cmd: &Sender<Cmd>,
    update_now: &Sender<()>,
) -> Result<(), String> {
    let host = header(&req, "Host").unwrap_or_default();
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return req
            .respond(json_resp(&json!({"error": "bad host"}), 403))
            .map_err(|e| e.to_string());
    }
    let method = req.method().clone();
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    if method == Method::Post && path == "/mcp" {
        // MCP-клиенты (Claude Code, Desktop) не шлют X-VPN-Monitor: защита — Origin только локальный и JSON
        let origin = header(&req, "Origin");
        let ct = header(&req, "Content-Type").unwrap_or_default();
        if !vpn_monitor::mcp::origin_allowed(origin.as_deref()) || !ct.contains("application/json")
        {
            return req
                .respond(json_resp(&json!({"error": "MCP: только локальные клиенты, Content-Type: application/json"}), 403))
                .map_err(|e| e.to_string());
        }
        let mut body = String::new();
        let _ = req.as_reader().take(256 * 1024).read_to_string(&mut body);
        // vpn_check ждёт проверку до минуты — отвечаем из своего потока, чтобы окно и API не замирали
        let data = McpData {
            shared: shared.clone(),
            cmd: cmd.clone(),
        };
        std::thread::spawn(move || {
            let resp = match vpn_monitor::mcp::handle_body(&body, &data) {
                Ok(Some(v)) => json_resp(&v, 200),
                Ok(None) => Response::from_string(String::new()).with_status_code(202),
                Err(e) => json_resp(&e, 400),
            };
            let _ = req.respond(resp);
        });
        return Ok(());
    }
    if method != Method::Get && header(&req, "X-VPN-Monitor").as_deref() != Some("1") {
        return req
            .respond(json_resp(
                &json!({"error": "нужен заголовок X-VPN-Monitor: 1"}),
                403,
            ))
            .map_err(|e| e.to_string());
    }
    let mut body = String::new();
    if method == Method::Post {
        let _ = req.as_reader().take(64 * 1024).read_to_string(&mut body);
    }
    if method == Method::Post && path == "/api/credentials" {
        // проверка логина на сервисах — до ~15 с; отвечаем из своего потока, чтобы окно не замирало
        let shared = shared.clone();
        let cmd = cmd.clone();
        std::thread::spawn(move || {
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let (login, password) = (
                v["login"].as_str().unwrap_or("").trim().to_string(),
                v["password"].as_str().unwrap_or("").to_string(),
            );
            let resp = if login.is_empty() || password.is_empty() {
                json_resp(
                    &json!({"saved": false, "message": "Введите логин и пароль AD."}),
                    400,
                )
            } else {
                let settings = lock(&shared).settings.clone();
                let r = vpn_monitor::login::login(&settings, &login, &password);
                if r.saved {
                    lock(&shared).store.set_login(&r.login);
                    let _ = cmd.send(Cmd::CheckNow);
                }
                json_resp(&serde_json::to_value(&r).unwrap_or(Value::Null), 200)
            };
            let _ = req.respond(resp);
        });
        return Ok(());
    }

    if path == "/flyout" || path.starts_with("/vendor/") {
        info!("api", "{method} {path}");
    }
    let resp = match (&method, path) {
        (Method::Get, "/") | (Method::Get, "/index.html") => Response::from_string(PAGE)
            .with_header(ctype("text/html; charset=utf-8"))
            .with_header(no_store()),
        (Method::Get, "/flyout") => Response::from_string(FLYOUT)
            .with_header(ctype("text/html; charset=utf-8"))
            .with_header(no_store()),
        (Method::Get, "/vendor/popper.min.js") => Response::from_string(POPPER)
            .with_header(ctype("text/javascript; charset=utf-8"))
            .with_header(no_store()),
        (Method::Get, "/vendor/tippy.min.js") => Response::from_string(TIPPY)
            .with_header(ctype("text/javascript; charset=utf-8"))
            .with_header(no_store()),
        (Method::Get, "/vendor/tippy.css") => Response::from_string(TIPPY_CSS)
            .with_header(ctype("text/css; charset=utf-8"))
            .with_header(no_store()),
        (Method::Get, "/favicon.ico") => {
            // тот же значок, что в трее, с текущим статусом
            let sh = lock(shared);
            let (badge, color) = sh
                .snapshot
                .as_ref()
                .map(|s| (s.verdict.badge, s.verdict.color))
                .unwrap_or((
                    vpn_monitor::score::Badge::Unknown,
                    vpn_monitor::score::Color::Gray,
                ));
            let ico =
                vpn_monitor::icon::to_ico(&vpn_monitor::icon::render(badge, color, sh.checking));
            Response::from_data(ico)
                .with_header(ctype("image/x-icon"))
                .with_header(
                    Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).expect("заголовок"),
                )
        }
        (Method::Get, "/mcp") => json_resp(
            &json!({"error": "MCP: только POST (Streamable HTTP без SSE)"}),
            405,
        ),
        (Method::Get, "/api/help") => {
            Response::from_string(HELP.replace("{port}", &port.to_string()))
                .with_header(ctype("text/markdown; charset=utf-8"))
        }
        (Method::Get, "/api/state") => json_resp(&state(shared), 200),
        (Method::Get, "/api/history") => {
            let range = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("range="))
                .unwrap_or("day");
            json_resp(&history(shared, range), 200)
        }
        (Method::Post, "/api/check") => {
            let busy = lock(shared).checking;
            if !busy {
                info!("api", "проверка по запросу из окна/API");
                let _ = cmd.send(Cmd::CheckNow);
            }
            json_resp(&json!({"started": !busy, "checking": true}), 202)
        }
        (Method::Post, "/api/update") => {
            // {"install": true} — поставить сейчас; без него — только узнать, есть ли обновление (ответ на месте)
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let (base, channel) = {
                let sh = lock(shared);
                (
                    sh.settings.release_base_url.clone(),
                    sh.prefs.channel.clone(),
                )
            };
            if v["install"].as_bool() == Some(true) {
                info!("api", "установка обновления по запросу");
                let _ = update_now.send(());
                json_resp(&json!({"installing": true}), 202)
            } else {
                match vpn_monitor::update::fetch_channel_version(&base, &channel) {
                    Ok(latest) => {
                        info!(
                            "api",
                            "проверка обновления: канал {channel}, последняя {latest}"
                        );
                        lock(shared).latest = Some(latest.clone());
                        json_resp(
                            &json!({"current": VERSION, "latest": latest, "channel": channel,
                            "available": vpn_monitor::update::needs_update(VERSION, &latest)}),
                            200,
                        )
                    }
                    Err(e) => json_resp(
                        &json!({"current": VERSION, "channel": channel, "error": e}),
                        200,
                    ),
                }
            }
        }
        (Method::Get, "/api/summary") => {
            Response::from_string(summary(shared)).with_header(ctype("text/plain; charset=utf-8"))
        }
        (Method::Post, "/api/uninstall") => {
            // убрать автозапуск, вход AD и токен, папку — после выхода (cmd ждёт 3 с и удаляет папку)
            let dir = lock(shared).dir.clone();
            info!("api", "удаление VPN Monitor с этого компьютера");
            let _ = win::autostart_remove();
            let _ = win::desktop_shortcut_remove();
            let _ = vpn_monitor::creds::delete_ad();
            let script = format!(
                "ping -n 4 127.0.0.1 >nul & rmdir /s /q \"{}\"",
                dir.display()
            );
            let _ = std::process::Command::new("cmd")
                .args(["/c", &script])
                .spawn();
            win::signal_quit();
            json_resp(&json!({"ok": true}), 200)
        }
        (Method::Get, "/api/changes") => {
            Response::from_string(CHANGELOG).with_header(ctype("text/markdown; charset=utf-8"))
        }
        (Method::Get, "/api/credentials") => json_resp(&credentials(), 200),
        (Method::Get, "/api/prefs") => json_resp(
            &serde_json::to_value(&lock(shared).prefs).unwrap_or(Value::Null),
            200,
        ),
        (Method::Post, "/api/prefs") => {
            // меняем только известные поля и только допустимыми значениями
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let mut sh = lock(shared);
            if let Some(view) = v["view"]
                .as_str()
                .filter(|x| vpn_monitor::prefs::VIEWS.contains(x))
            {
                sh.prefs.view = view.into();
            }
            if let Some(t) = v["theme"]
                .as_str()
                .filter(|x| vpn_monitor::prefs::THEMES.contains(x))
            {
                sh.prefs.theme = t.into();
            }
            if let Some(t) = v["window_theme"]
                .as_str()
                .filter(|x| vpn_monitor::prefs::WINDOW_THEMES.contains(x))
            {
                sh.prefs.window_theme = Some(t.into());
            }
            if let Some(ch) = v["channel"]
                .as_str()
                .filter(|x| vpn_monitor::prefs::CHANNELS.contains(x))
            {
                sh.prefs.channel = ch.into();
                info!("api", "канал обновлений: {ch}");
            }
            for (k, slot) in [("vpn", 0), ("auth", 1), ("update", 2), ("slow", 3)] {
                if let Some(b) = v["notify"][k].as_bool() {
                    match slot {
                        0 => sh.prefs.notify.vpn = b,
                        1 => sh.prefs.notify.auth = b,
                        2 => sh.prefs.notify.update = b,
                        _ => sh.prefs.notify.slow = b,
                    }
                }
            }
            if let Some(seen) = v["last_seen_version"].as_str() {
                sh.prefs.last_seen_version = Some(seen.chars().take(20).collect());
            }
            let dir = sh.dir.clone();
            match sh.prefs.save(&dir) {
                Ok(()) => json_resp(&serde_json::to_value(&sh.prefs).unwrap_or(Value::Null), 200),
                Err(e) => json_resp(&json!({"error": e}), 500),
            }
        }
        (Method::Post, "/api/pause") => {
            // {"minutes": 30 | 60 | 0 (до конца дня)} или {"resume": true}
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let mut sh = lock(shared);
            let now = vpn_monitor::metrics::now_ms();
            if v["resume"].as_bool() == Some(true) {
                sh.prefs.pause_until_ms = None;
                info!("api", "пауза снята");
            } else {
                let min = v["minutes"].as_i64().unwrap_or(30).clamp(0, 24 * 60);
                let until = vpn_monitor::prefs::pause_until(now, min);
                sh.prefs.pause_until_ms = Some(until);
                info!(
                    "api",
                    "пауза до {}",
                    log::hhmm(
                        std::time::UNIX_EPOCH + std::time::Duration::from_millis(until as u64)
                    )
                );
            }
            let dir = sh.dir.clone();
            let _ = sh.prefs.save(&dir);
            json_resp(&json!({"pause_until_ms": sh.prefs.pause_until_ms}), 200)
        }
        (Method::Delete, "/api/credentials") => match vpn_monitor::creds::delete_ad() {
            Ok(()) => {
                info!("login", "вход AD удалён из Диспетчера учётных данных");
                lock(shared).store.set_login("");
                json_resp(&credentials(), 200)
            }
            Err(e) => json_resp(&json!({"error": e}), 500),
        },
        (Method::Get, "/api/diagnostics") => {
            let sh = lock(shared);
            json_resp(&json!({"running": sh.diag_running, "report": sh.diag}), 200)
        }
        (Method::Post, "/api/diagnostics") => {
            let started = start_diag(shared);
            json_resp(&json!({"started": started, "running": true}), 202)
        }
        (Method::Get, "/api/log") => {
            let n: usize = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("lines="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(300)
                .min(5000);
            Response::from_string(tail_log(n)).with_header(ctype("text/plain; charset=utf-8"))
        }
        (Method::Post, "/api/autostart") => {
            let on = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["on"].as_bool())
                .unwrap_or(true);
            let res = if on {
                std::env::current_exe()
                    .map_err(|e| e.to_string())
                    .and_then(|p| win::autostart_set(&p))
            } else {
                win::autostart_remove()
            };
            match res {
                Ok(()) => {
                    info!(
                        "api",
                        "автозапуск {}",
                        if on {
                            "включён"
                        } else {
                            "выключен"
                        }
                    );
                    json_resp(&json!({"autostart": win::autostart_get().is_some()}), 200)
                }
                Err(e) => json_resp(&json!({"error": e}), 500),
            }
        }
        (Method::Post, "/api/open") => {
            let what = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["what"].as_str().map(String::from));
            let dir = lock(shared).dir.clone();
            match what.as_deref() {
                Some("log") => {
                    if let Some(p) = log::path() {
                        win::open_in_notepad(&p);
                    }
                }
                Some("secrets") => win::open_in_notepad(&crate::secrets_path(&dir)),
                _ => win::open(&dir),
            }
            json_resp(&json!({"ok": true}), 200)
        }
        _ => json_resp(
            &json!({"error": "нет такого адреса", "help": "/api/help"}),
            404,
        ),
    };
    req.respond(resp).map_err(|e| e.to_string())
}

fn start_diag(shared: &SharedRef) -> bool {
    let (settings, dir) = {
        let mut sh = lock(shared);
        if sh.diag_running {
            return false;
        }
        sh.diag_running = true;
        (sh.settings.clone(), sh.dir.clone())
    };
    let shared = shared.clone();
    std::thread::spawn(move || {
        info!("diag", "диагностика подключения: старт");
        let secrets = Secrets::load(&crate::secrets_path(&dir));
        let report = diag::run(&settings, &secrets);
        for t in &report.targets {
            info!("diag", "{}: {} — {}", t.name, t.verdict, t.hint);
        }
        info!("diag", "диагностика: готово за {} мс", report.took_ms);
        let mut sh = lock(&shared);
        sh.diag = serde_json::to_value(&report).ok();
        sh.diag_running = false;
    });
    true
}

/// Один абзац для чата с поддержкой: состояние, клиент и профиль, канал, туннель, с какого времени, версия.
fn summary(shared: &SharedRef) -> String {
    let sh = lock(shared);
    let mut out = vec![format!(
        "Domini VPN Monitor {VERSION} — сводка для поддержки"
    )];
    let login = vpn_monitor::creds::load_ad().map(|c| c.login);
    out.push(format!(
        "Пользователь: {} (AD); Windows: {}@{}; {}",
        login.as_deref().unwrap_or("вход AD не выполнен"),
        std::env::var("USERNAME").unwrap_or_default(),
        std::env::var("COMPUTERNAME").unwrap_or_default(),
        win::os_version()
    ));
    let Some(sn) = &sh.snapshot else {
        out.push("Проверок ещё не было.".into());
        return out.join("\n");
    };
    let v = &sn.verdict;
    let client = sn.client.as_ref().map(|c| c.name.as_str()).unwrap_or("VPN");
    let state = match (&v.badge, &sn.route) {
        (vpn_monitor::score::Badge::NoInternet, _) => "нет интернета".to_string(),
        (vpn_monitor::score::Badge::Captive, _) => "Wi‑Fi требует входа".to_string(),
        (_, Some(r)) => vpn_monitor::route::headline(r, client),
        _ => v.explain.clone(),
    };
    let since = sh
        .store
        .records
        .iter()
        .rev()
        .take_while(|r| {
            r.c == sh
                .store
                .records
                .last()
                .map(|l| l.c.clone())
                .unwrap_or_default()
        })
        .last()
        .map(|r| log::hhmm(UNIX_EPOCH + std::time::Duration::from_millis(r.t as u64)))
        .unwrap_or_default();
    out.push(format!(
        "Состояние: {state}; оценка {} ({}){}; проверено в {}{}",
        v.score.map(|x| x.to_string()).unwrap_or("—".into()),
        v.color.ru(),
        if since.is_empty() {
            String::new()
        } else {
            format!(", так с {since}")
        },
        log::hhmm(sn.at),
        if sh.prefs.paused(vpn_monitor::metrics::now_ms()) {
            "; проверки на паузе"
        } else {
            ""
        }
    ));
    if let Some(r) = &sn.route {
        out.push(format!("Каналы: {}", vpn_monitor::route::numbers(r)));
        if let Some(ms) = r.inet_ms.value {
            out.push(format!("Задержка до интернета: {ms:.0} мс"));
        }
    }
    out.push("Сервисы офиса:".into());
    for t in sn
        .targets
        .iter()
        .filter(|t| !matches!(t.role, Some(vpn_monitor::settings::Role::World)))
    {
        let state = if t.ok {
            "подключён".to_string()
        } else if t.fail == Some(vpn_monitor::probe::Fail::Auth) {
            "вход не принят".to_string()
        } else {
            format!(
                "не подключён ({})",
                t.error.as_deref().unwrap_or("не отвечает")
            )
        };
        let mut parts = vec![state];
        if let Some(l) = t.latency_ms.filter(|_| t.reachable) {
            parts.push(format!("{l:.0} мс"));
        }
        if let Some(m) = t.mbps.filter(|_| t.ok) {
            parts.push(format!("{m:.1} Мбит/с"));
        }
        if let Some(ok) = t.svn_info_ok {
            parts.push(format!(
                "svn info: {}{}",
                if ok {
                    "отвечает"
                } else {
                    "не отвечает как SVN"
                },
                t.svn_info_ms
                    .map(|m| format!(" за {m:.0} мс"))
                    .unwrap_or_default()
            ));
        }
        parts.push(format!("вход: {}", t.auth_used));
        if let Some(a) = &t.adapter {
            parts.push(format!(
                "через {a}{}",
                if t.via_vpn == Some(false) {
                    " (без VPN)"
                } else {
                    ""
                }
            ));
        }
        out.push(format!("  {} — {}", t.name, parts.join(", ")));
    }
    let world: Vec<String> = sn
        .targets
        .iter()
        .filter(|t| matches!(t.role, Some(vpn_monitor::settings::Role::World)))
        .map(|t| {
            format!(
                "{} — {}{}",
                t.name,
                if t.ok {
                    "открывается"
                } else {
                    "не открывается"
                },
                match t.via_vpn {
                    Some(true) => " через VPN",
                    Some(false) => " напрямую",
                    None => "",
                }
            )
        })
        .collect();
    if !world.is_empty() {
        out.push(format!("Мир: {}", world.join("; ")));
    }
    out.push(format!(
        "VPN рабочий: {}",
        sn.client
            .as_ref()
            .map(|c| format!("{}, адаптер {}", c.key(), c.adapter))
            .unwrap_or_else(|| "туннель не найден".into())
    ));
    let inst = vpn_monitor::vpn::installed(&sh.settings.vpn_clients);
    out.push(format!(
        "Установлены: {}",
        if inst.is_empty() {
            "известных клиентов VPN не найдено".to_string()
        } else {
            inst.iter()
                .map(|c| {
                    format!(
                        "{} ({}{})",
                        c.name,
                        if c.running {
                            "запущен"
                        } else {
                            "не запущен"
                        },
                        c.profile
                            .as_ref()
                            .map(|p| format!(", профиль {p}"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        }
    ));
    let adv = vpn_monitor::advice::advice(
        sn.route.as_ref(),
        &sn.targets,
        client,
        matches!(v.badge, vpn_monitor::score::Badge::Captive),
    );
    if !adv.is_empty() {
        out.push(format!("Советы приложения: {}", adv.join(" ")));
    }
    out.push(format!(
        "Город: {}; канал обновлений: {}",
        sh.city.as_deref().unwrap_or("не определён"),
        if sh.prefs.channel == "beta" {
            "бета"
        } else {
            "стабильный"
        }
    ));
    out.join("\n")
}

/// Кто вошёл — без пароля и токенов.
fn credentials() -> Value {
    let ad = vpn_monitor::creds::load_ad();
    let gl = vpn_monitor::creds::load_gitlab();
    json!({
        "login": ad.as_ref().map(|c| c.login.clone()),
        "store": "Диспетчер учётных данных Windows",
        "gitlab_oauth": gl.is_some(),
    })
}

fn ms(t: std::time::SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn probe_json(r: &ProbeResult) -> Value {
    json!({
        "id": r.target_id, "name": r.name,
        "role": r.role.map(|x| format!("{x:?}").to_lowercase()),
        "at": ms(r.at), "full": r.full, "reachable": r.reachable, "ok": r.ok,
        "ip": r.resolved, "latency": r.latency_ms, "mbps": r.mbps, "http": r.http_status,
        "fail": r.fail.map(|f| format!("{f:?}")), "error": r.error, "note": r.note, "auth": r.auth_used,
        "adapter": r.adapter, "via_vpn": r.via_vpn,
        "svn_info_ok": r.svn_info_ok, "svn_info_ms": r.svn_info_ms,
    })
}

fn state(shared: &SharedRef) -> Value {
    let sh = lock(shared);
    let s = &sh.settings;
    let snap = sh.snapshot.as_ref().map(|sn| {
        let v = &sn.verdict;
        json!({
            "at": ms(sn.at),
            "color": format!("{:?}", v.color).to_lowercase(),
            "badge": match v.badge {
                vpn_monitor::score::Badge::Score(_) => "score",
                vpn_monitor::score::Badge::NoVpn => "novpn",
                vpn_monitor::score::Badge::WeakInternet(_) => "weak",
                vpn_monitor::score::Badge::Unknown => "unknown",
                vpn_monitor::score::Badge::NoInternet => "offline",
                vpn_monitor::score::Badge::Captive => "captive",
                vpn_monitor::score::Badge::Paused => "paused",
            },
            "score": v.score, "latency": v.latency, "speed": v.speed, "stability": v.stability,
            "explain": v.explain,
            "parts": v.parts, "baseline": v.baseline,
            "client": sn.client,
            "waking": sn.waking, "lite": sn.lite,
            "internet_latency": sn.internet_latency_ms, "overhead": sn.overhead_ms, "share": sn.share,
            "route": sn.route, "full": sn.full, "took_ms": sn.took_ms,
            "advice": vpn_monitor::advice::advice(sn.route.as_ref(), &sn.targets,
                sn.client.as_ref().map(|c| c.name.as_str()).unwrap_or("клиент VPN"),
                matches!(v.badge, vpn_monitor::score::Badge::Captive)),
            "targets": sn.targets.iter().map(probe_json).collect::<Vec<_>>(),
            "internet": sn.internet.as_ref().map(probe_json),
        })
    });
    // для «Настроек»: по каждому сервису офиса — каким входом проверяем и когда он последний раз ответил
    let services: Vec<Value> = {
        let recs = sh
            .store
            .since(vpn_monitor::metrics::now_ms() - 7 * 86_400_000);
        s.targets.iter()
                .filter(|t| t.enabled && matches!(t.role, vpn_monitor::settings::Role::Light | vpn_monitor::settings::Role::Speed))
                .map(|t| {
                    let last_ok = recs.iter().rev().find(|r| r.tg.get(&t.id).is_some_and(|x| x.ok)).map(|r| r.t);
                    let p = sh.snapshot.as_ref().and_then(|sn| sn.targets.iter().find(|r| r.target_id == t.id));
                    json!({"id": t.id, "name": t.name, "auth": p.map(|r| r.auth_used.clone()),
                           "note": p.and_then(|r| r.note.clone()), "ok": p.map(|r| r.ok), "last_ok_ms": last_ok})
                })
                .collect::<Vec<_>>()
    };
    json!({
        "version": VERSION,
        "latest": sh.latest,
        "update_error": sh.update_error,
        "checking": sh.checking,
        "autostart": win::autostart_get().is_some(),
        "dir": sh.dir.display().to_string(),
        "computer": std::env::var("COMPUTERNAME").unwrap_or_default(),
        "user": std::env::var("USERNAME").unwrap_or_default(),
        "thresholds": s.thresholds,
        "intervals": {
            "green_sec": s.check_interval_green_sec, "alert_sec": s.check_interval_alert_sec,
            "full_min": s.full_probe_interval_min, "internet_min": s.internet_interval_min,
            "update_min": s.update_interval_min,
        },
        "targets": s.targets.iter().filter(|t| t.enabled).map(|t| json!({"id": t.id, "name": t.name, "role": format!("{:?}", t.role).to_lowercase()})).collect::<Vec<_>>(),
        "snapshot": snap,
        "login": vpn_monitor::creds::load_ad().map(|c| c.login),
        "prefs": sh.prefs,
        "paused": sh.prefs.paused(vpn_monitor::metrics::now_ms()),
        "gitlab_oauth_configured": s.gitlab_oauth.is_some(),
        "city": sh.city,
        "support_url": s.support_url,
        "office": {"keys": sh.office_keys, "at_ms": sh.office_at},
        "services": services,
        // когда поставлена эта версия: время файла exe (скачан при обновлении)
        "updated_ms": std::env::current_exe().ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .map(ms),
    })
}

fn history(shared: &SharedRef, range: &str) -> Value {
    let sh = lock(shared);
    let now = vpn_monitor::metrics::now_ms();
    let from = if range == "hour" {
        now - 3_600_000
    } else if range == "week" {
        local_day_start(now) - 7 * 86_400_000
    } else {
        now - 86_400_000
    };
    json!({"range": range, "from": from, "to": now, "records": sh.store.since(from)})
}

fn tail_log(n: usize) -> String {
    let Some(p) = log::path() else {
        return String::new();
    };
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Данные для MCP из общего состояния трея.
struct McpData {
    shared: SharedRef,
    cmd: Sender<Cmd>,
}

impl vpn_monitor::mcp::Data for McpData {
    fn summary(&self) -> String {
        summary(&self.shared)
    }
    fn status(&self) -> Value {
        let st = state(&self.shared);
        json!({
            "version": st["version"], "city": st["city"], "login": st["login"], "paused": st["paused"],
            "snapshot": {
                "at": st["snapshot"]["at"], "color": st["snapshot"]["color"], "badge": st["snapshot"]["badge"],
                "score": st["snapshot"]["score"], "client": st["snapshot"]["client"], "route": st["snapshot"]["route"],
                "advice": st["snapshot"]["advice"], "baseline": st["snapshot"]["baseline"],
                "targets": st["snapshot"]["targets"],
            },
            "thresholds": st["thresholds"],
        })
    }
    fn history(&self, range: &str) -> Value {
        let h = history(&self.shared, range);
        let recs = h["records"].as_array().cloned().unwrap_or_default();
        let med = |key: &str| {
            let mut v: Vec<f64> = recs.iter().filter_map(|r| r[key].as_f64()).collect();
            v.sort_by(|a, b| a.total_cmp(b));
            (!v.is_empty()).then(|| v[v.len() / 2])
        };
        // отрезки «плохо» (красный или VPN выключен), подряд идущие проверки — один отрезок
        let mut bad: Vec<Value> = vec![];
        let mut cur: Option<(i64, i64, String)> = None;
        for r in &recs {
            let t = r["t"].as_i64().unwrap_or(0);
            let is_bad = r["c"] == "red" || r["b"] == "novpn";
            match (&mut cur, is_bad) {
                (Some(c), true) => c.1 = t,
                (None, true) => cur = Some((t, t, r["b"].as_str().unwrap_or("").to_string())),
                (Some(_), false) => {
                    let (a, b, k) = cur.take().unwrap_or_default();
                    bad.push(
                        json!({"from": a, "to": b, "minutes": (b - a) / 60000 + 1, "kind": k}),
                    );
                }
                (None, false) => {}
            }
        }
        if let Some((a, b, k)) = cur {
            bad.push(json!({"from": a, "to": b, "minutes": (b - a) / 60000 + 1, "kind": k}));
        }
        json!({
            "range": range, "checks": recs.len(), "from": h["from"], "to": h["to"],
            "bad_periods": bad,
            "median": {"vpn_ms": med("lat"), "svn_mbps": med("spd"), "inet_mbps": med("inet"), "score": med("s")},
        })
    }
    fn check(&self) -> String {
        let before = lock(&self.shared).snapshot.as_ref().map(|s| s.at);
        let _ = self.cmd.send(Cmd::CheckNow);
        for _ in 0..90 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let sh = lock(&self.shared);
            if !sh.checking && sh.snapshot.as_ref().map(|s| s.at) != before {
                break;
            }
        }
        summary(&self.shared)
    }
    fn diagnostics(&self) -> Value {
        let (running, report) = {
            let sh = lock(&self.shared);
            (sh.diag_running, sh.diag.clone())
        };
        if report.is_none() && !running {
            start_diag(&self.shared);
            for _ in 0..60 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if !lock(&self.shared).diag_running {
                    break;
                }
            }
        }
        lock(&self.shared)
            .diag
            .clone()
            .unwrap_or(json!({"note": "диагностика ещё идёт"}))
    }
    fn log_tail(&self, lines: usize) -> String {
        tail_log(lines)
    }
    fn report_html(&self) -> String {
        let esc = |t: &str| {
            t.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        format!(
            "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\"><title>Отчёт Domini VPN Monitor</title><style>\
             :root{{--bg:#fff;--fg:#15202e;--mut:#5a6b80;--line:#e2e7ee}}\
             @media (prefers-color-scheme:dark){{:root{{--bg:#161f2a;--fg:#e6edf5;--mut:#8d9db2;--line:#263342}}}}\
             body{{background:var(--bg);color:var(--fg);font:14px/1.5 'Segoe UI',system-ui,sans-serif;max-width:760px;margin:24px auto;padding:0 16px}}\
             pre{{white-space:pre-wrap;border:1px solid var(--line);border-radius:10px;padding:12px}}h1{{font-size:18px}}p{{color:var(--mut)}}\
             </style></head><body><h1>Отчёт Domini VPN Monitor</h1><p>Сводка для поддержки — без паролей и токенов.</p><pre>{}</pre></body></html>",
            esc(&summary(&self.shared))
        )
    }
    fn version(&self) -> String {
        VERSION.to_string()
    }
}
