//! Режим трея: иконка-светофор, меню со срезом, проверки по расписанию и по левому клику, обновление,
//! запись метрик, локальный API и окно в браузере (`api.rs`).

use crate::win;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use vpn_monitor::metrics::{Meta, Record, Store};
use vpn_monitor::monitor::{Monitor, Snapshot};
use vpn_monitor::probe::{Fail, ProbeResult};
use vpn_monitor::score::{Badge, Color};
use vpn_monitor::settings::{Role, Settings};
use vpn_monitor::{VERSION, error, icon, info, log, update, warn};

enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    Checking,
    Snapshot(Box<Snapshot>),
    LatestVersion(String),
    UpdateError(String),
    RestartInto(PathBuf),
    Quit,
    /// Сообщение страницы меню (`h:520`, `hide`, `history`, `quit`), v0.5.
    FlyoutIpc(String),
}

pub enum Cmd {
    CheckNow,
}

/// Состояние, общее для трея, проверок, обновления и API.
pub struct Shared {
    pub settings: Settings,
    pub dir: PathBuf,
    pub snapshot: Option<Snapshot>,
    pub checking: bool,
    pub latest: Option<String>,
    pub update_error: Option<String>,
    pub store: Store,
    pub api_url: Option<String>,
    pub api_error: Option<String>,
    pub diag: Option<serde_json::Value>,
    pub diag_running: bool,
    /// Выбор сотрудника: вид меню, канал, пауза, уведомления (user.json; до 0.5.3 — prefs.json), v0.5.
    pub prefs: vpn_monitor::prefs::Prefs,
    /// Город, где нас видит интернет (по внешнему адресу), v0.6; обновляется раз в 6 часов.
    pub city: Option<String>,
    /// Удалённые настройки (v0.7): номер версии настроек (растёт при каждом применении office.json), какие ключи
    /// пришли из office.json и когда применены.
    pub settings_rev: u64,
    pub office_keys: Option<Vec<String>>,
    pub office_at: Option<i64>,
    pub city_at: Option<std::time::Instant>,
    /// Сообщения админов к показу (v0.9) и последний разобранный файл (для пересчёта после «Закрыть»).
    pub announce: Vec<vpn_monitor::announce::Shown>,
    pub announce_file: vpn_monitor::announce::File,
}

pub type SharedRef = Arc<Mutex<Shared>>;

pub fn lock(s: &SharedRef) -> std::sync::MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

struct Ui {
    tray: TrayIcon,
    title: MenuItem,
    status: MenuItem,
    targets: Vec<(String, MenuItem)>,
    internet: MenuItem,
    stability: MenuItem,
    updated: MenuItem,
    check_now: MenuItem,
    open_window: MenuItem,
    open_log: MenuItem,
    open_dir: MenuItem,
    autostart: CheckMenuItem,
    version: MenuItem,
    quit: MenuItem,
    checking: bool,
    paused: bool,
    last: Option<Snapshot>,
    latest: Option<String>,
    update_error: Option<String>,
}

pub fn run(replace: bool, first_run: bool) {
    let embedded = Settings::embedded();
    let dir = embedded.install_dir();
    log::init(&dir.join("vpn-monitor.log"), false);
    let (base_settings, src) = Settings::load(&dir);
    crate::log_start("трей", &dir, &src);
    // удалённые настройки (v0.7): последняя скачанная копия office.json — сразу, до сети
    let (settings, office_keys) = match std::fs::read_to_string(dir.join(vpn_monitor::office::FILE))
    {
        Ok(text) => match vpn_monitor::office::apply(&base_settings, &text) {
            Ok((s, keys)) => {
                info!(
                    "office",
                    "удалённые настройки из копии: {}",
                    keys.join(", ")
                );
                (s, Some(keys))
            }
            Err(e) => {
                warn!("office", "копия удалённых настроек не применена: {e}");
                (base_settings.clone(), None)
            }
        },
        Err(_) => (base_settings.clone(), None),
    };

    let instance = if replace {
        win::replace_running(Duration::from_secs(15))
    } else {
        win::try_single_instance()
    };
    let Some(_instance) = instance else {
        info!("tray", "уже запущена другая копия — выхожу");
        return;
    };
    // Переход на постоянный путь: версии до 0.5.4 ставят обновление в `vpn-monitor-X.Y.Z.exe`. Запущены из папки
    // установки под таким именем — копируем себя в `vpn-monitor.exe`, переносим автозапуск и перезапускаемся оттуда
    // (иконка в трее у Windows привязана к пути exe).
    if let Ok(me) = std::env::current_exe() {
        let name = me
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let in_install = me.parent().is_some_and(|p| p == dir.as_path());
        if in_install && name != crate::install::STABLE_EXE {
            match crate::install::place_exe(&me, &dir, crate::install::Place::Copy) {
                Ok(stable) => {
                    info!("tray", "перехожу на постоянный путь {}", stable.display());
                    if let Err(e) = win::autostart_set(&stable) {
                        error!("tray", "автозапуск: {e}");
                    }
                    if let Err(e) = win::desktop_shortcut(&stable) {
                        warn!("tray", "ярлык на рабочем столе: {e}");
                    }
                    match win::spawn_detached(&stable, &["--tray", "--replace"]) {
                        Ok(()) => return,
                        Err(e) => error!(
                            "tray",
                            "не запустить {}: {e} — работаю отсюда",
                            stable.display()
                        ),
                    }
                }
                Err(e) => error!("tray", "{e} — работаю из {name}"),
            }
        }
        crate::install::cleanup_old_exes(&dir, &me);
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    {
        let p = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e| {
            let _ = p.send_event(UserEvent::Tray(e));
        }));
        let p = proxy.clone();
        MenuEvent::set_event_handler(Some(move |e| {
            let _ = p.send_event(UserEvent::Menu(e));
        }));
        let p = proxy.clone();
        win::watch_quit(move || {
            let _ = p.send_event(UserEvent::Quit);
        });
    }

    let meta = Meta {
        computer: std::env::var("COMPUTERNAME").unwrap_or_default(),
        user: std::env::var("USERNAME").unwrap_or_default(),
        version: VERSION.into(),
        login: vpn_monitor::creds::load_ad()
            .map(|c| c.login)
            .unwrap_or_default(),
        period: String::new(),
    };
    let store = Store::open(&dir, meta);
    if let Err(e) = win::register_notifications(&dir) {
        warn!("notify", "уведомления не зарегистрированы: {e}");
    }
    info!(
        "metrics",
        "история: {} проверок из metrics_7d.json",
        store.records.len()
    );
    let shared: SharedRef = Arc::new(Mutex::new(Shared {
        settings: settings.clone(),
        dir: dir.clone(),
        snapshot: None,
        city: None,
        city_at: None,
        announce: vec![],
        announce_file: Default::default(),
        checking: false,
        latest: None,
        update_error: None,
        store,
        api_url: None,
        api_error: None,
        diag: None,
        diag_running: false,
        prefs: vpn_monitor::prefs::Prefs::load(&dir),
        settings_rev: 0,
        office_keys,
        office_at: None,
    }));

    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
    let (upd_tx, upd_rx) = mpsc::channel::<()>();
    match crate::api::start(shared.clone(), cmd_tx.clone(), upd_tx) {
        Ok(url) => {
            info!("api", "окно и API: {url}");
            // первый запуск после установки и нет входа — сразу открыть окно на вкладке входа
            let creds = vpn_monitor::secrets::Secrets::load(&crate::secrets_path(&dir));
            if first_run && creds.ad_login().is_none() && creds.get("CONFLUENCE_TOKEN").is_none() {
                info!("tray", "входа нет — открываю окно на вкладке «Подключение»");
                win::open_url(&format!("{url}#welcome")); // v0.9: три шага первого запуска
            }
            lock(&shared).api_url = Some(url);
        }
        Err(e) => {
            error!("api", "окно и API не запустились: {e}");
            lock(&shared).api_error = Some(e);
        }
    }
    spawn_checker(
        settings.clone(),
        crate::secrets_path(&dir),
        cmd_rx,
        proxy.clone(),
        shared.clone(),
    );
    let settings_base_url = base_settings.release_base_url.clone();
    spawn_office(base_settings, dir.clone(), shared.clone());
    spawn_tickets(dir.clone(), shared.clone());
    spawn_announce(settings_base_url.clone(), dir.clone(), shared.clone());
    spawn_updater(
        settings.clone(),
        dir.clone(),
        proxy.clone(),
        upd_rx,
        shared.clone(),
    );
    let api_url = lock(&shared).api_url.clone();

    let mut ui: Option<Ui> = None;
    let mut flyout: Option<crate::flyout::Flyout> = None;
    let fly_proxy = proxy.clone();
    event_loop.run(move |event, target, flow| {
        // меню открыто — 4 раза в секунду смотрим, не ушёл ли человек в другое окно; иначе — раз в 5 с (пауза)
        let open = flyout.as_ref().is_some_and(|f| f.is_visible());
        *flow = ControlFlow::WaitUntil(
            std::time::Instant::now() + Duration::from_millis(if open { 250 } else { 5000 }),
        );
        if let Some(f) = flyout.as_mut()
            && f.lost_to_other_window()
        {
            f.hide();
        }
        match event {
            Event::NewEvents(StartCause::Init) => match build_ui(&settings, api_url.is_some()) {
                Ok(u) => {
                    // меню у иконки на WebView2; не вышло — остаётся системное меню и левый клик = «Проверить»
                    if let Some(url) = &api_url {
                        match crate::flyout::Flyout::new(
                            target,
                            &format!("{url}flyout"),
                            dir.join("WebView2"),
                            fly_proxy.clone(),
                            UserEvent::FlyoutIpc,
                        ) {
                            Ok(mut f) => {
                                info!("tray", "меню у иконки — WebView2");
                                u.tray.set_show_menu_on_right_click(false);
                                // для проверки в CI: открыть меню сразу, без клика по иконке
                                if std::env::var_os("VPN_MONITOR_SHOW_FLYOUT").is_some() {
                                    info!("flyout", "VPN_MONITOR_SHOW_FLYOUT — открываю меню");
                                    f.toggle(tray_icon::Rect {
                                        position: tao::dpi::PhysicalPosition::new(1200.0, 1040.0),
                                        size: tao::dpi::PhysicalSize::new(24, 24),
                                    });
                                }
                                flyout = Some(f);
                            }
                            Err(e) => {
                                warn!("tray", "меню на WebView2 не создано ({e}) — системное меню")
                            }
                        }
                    }
                    ui = Some(u);
                }
                Err(e) => {
                    error!("tray", "не создать иконку в трее: {e}");
                    *flow = ControlFlow::Exit;
                }
            },
            Event::UserEvent(ev) => {
                let Some(u) = ui.as_mut() else { return };
                match ev {
                    UserEvent::Tray(TrayIconEvent::Click {
                        button,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    }) if flyout.is_some()
                        && matches!(button, MouseButton::Left | MouseButton::Right) =>
                    {
                        if let Some(f) = flyout.as_mut() {
                            f.toggle(rect);
                        }
                    }
                    UserEvent::Tray(TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }) => {
                        if u.checking {
                            info!("tray", "клик во время проверки — игнорирую");
                        } else {
                            info!("tray", "левый клик — проверка сейчас");
                            let _ = cmd_tx.send(Cmd::CheckNow);
                        }
                    }
                    UserEvent::Tray(TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }) if flyout.is_none() => {
                        if let Some(url) = &api_url {
                            info!("tray", "двойной клик — открываю окно");
                            win::open_url(url);
                        }
                    }
                    UserEvent::Tray(_) => {}
                    UserEvent::Menu(m) => {
                        if m.id == *u.check_now.id() && !u.checking {
                            let _ = cmd_tx.send(Cmd::CheckNow);
                        } else if m.id == *u.open_window.id() {
                            if let Some(url) = &api_url {
                                win::open_url(url);
                            }
                        } else if m.id == *u.open_log.id() {
                            if let Some(p) = log::path() {
                                win::open_in_notepad(&p);
                            }
                        } else if m.id == *u.open_dir.id() {
                            win::open(&dir);
                        } else if m.id == *u.autostart.id() {
                            toggle_autostart(u);
                        } else if m.id == *u.quit.id() {
                            info!("tray", "выход по меню");
                            *flow = ControlFlow::Exit;
                        }
                    }
                    UserEvent::FlyoutIpc(msg) => {
                        static FIRST: std::sync::Once = std::sync::Once::new();
                        if msg != "loaded" && msg != "probe" {
                            FIRST.call_once(|| {
                                let m: String = msg.chars().take(40).collect();
                                info!("flyout", "страница меню на связи (первое сообщение: {m})");
                            });
                        }
                        if let Some(h) = msg.strip_prefix("h:").and_then(|v| v.parse::<f64>().ok())
                        {
                            static DRAWN: std::sync::Once = std::sync::Once::new();
                            DRAWN.call_once(|| {
                                info!("flyout", "страница меню нарисована: высота {h}");
                            });
                            if let Some(f) = flyout.as_mut() {
                                f.resize(h);
                            }
                        } else if msg == "loaded" {
                            // страница загрузилась — через 0 с и ещё раз, когда скрипт точно отработал
                            if let Some(f) = flyout.as_ref() {
                                f.probe();
                            }
                            let p = fly_proxy.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(Duration::from_secs(5));
                                let _ = p.send_event(UserEvent::FlyoutIpc("probe".into()));
                            });
                        } else if msg == "probe" {
                            if let Some(f) = flyout.as_ref() {
                                f.probe();
                            }
                        } else if let Some(e) = msg.strip_prefix("err:") {
                            let e: String = e.chars().take(300).collect();
                            warn!("flyout", "ошибка на странице меню: {e}");
                        } else if msg == "hide" {
                            if let Some(f) = flyout.as_mut() {
                                f.hide();
                            }
                        } else if let Some(u) = msg.strip_prefix("url:") {
                            // ссылка из сообщения админов (v0.9) — только https
                            if u.starts_with("https://") {
                                if let Some(f) = flyout.as_mut() {
                                    f.hide();
                                }
                                win::open_url(u);
                            }
                        } else if let Some(tab) = msg.strip_prefix("open:") {
                            // окно в браузере на нужной вкладке и разделе: history, settings, settings/diagnostics
                            if let Some(f) = flyout.as_mut() {
                                f.hide();
                            }
                            if let Some(url) = &api_url {
                                let tab: String = tab
                                    .chars()
                                    .filter(|c| c.is_ascii_alphanumeric() || *c == '/' || *c == '-')
                                    .collect();
                                win::open_url(&format!("{url}#{tab}"));
                            }
                        } else if msg == "quit" {
                            info!("tray", "выход из меню");
                            *flow = ControlFlow::Exit;
                        }
                    }
                    UserEvent::Checking => {
                        u.checking = true;
                        refresh(u);
                    }
                    UserEvent::Snapshot(s) => {
                        u.checking = false;
                        u.last = Some(*s);
                        refresh(u);
                    }
                    UserEvent::LatestVersion(v) => {
                        u.latest = Some(v);
                        u.update_error = None;
                        refresh(u);
                    }
                    UserEvent::UpdateError(e) => {
                        u.update_error = Some(e);
                        refresh(u);
                    }
                    UserEvent::RestartInto(exe) => {
                        info!("update", "перезапуск в {}", exe.display());
                        if let Err(e) = win::autostart_set(&exe) {
                            error!("update", "{e}");
                        }
                        match win::spawn_detached(&exe, &["--tray", "--replace"]) {
                            Ok(()) => *flow = ControlFlow::Exit,
                            Err(e) => error!("update", "{e}"),
                        }
                    }
                    UserEvent::Quit => {
                        info!("tray", "попросили закрыться (обновление или установка)");
                        *flow = ControlFlow::Exit;
                    }
                }
            }
            // Focused(false) приходит и когда фокус уходит внутрь WebView2 — по нему не прячем (см. lost_to_other_window)
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if let Some(u) = ui.as_mut() {
                    let paused = lock(&shared).prefs.paused(vpn_monitor::metrics::now_ms());
                    if paused != u.paused {
                        u.paused = paused;
                        refresh(u);
                    }
                }
            }
            Event::LoopDestroyed => {
                info!("tray", "==== выход ====");
            }
            _ => {}
        }
    });
}

fn spawn_checker(
    settings: Settings,
    secrets: PathBuf,
    rx: mpsc::Receiver<Cmd>,
    proxy: EventLoopProxy<UserEvent>,
    shared: SharedRef,
) {
    std::thread::spawn(move || {
        let (green, alert) = (
            settings.check_interval_green_sec,
            settings.check_interval_alert_sec,
        );
        let mut m = Monitor::new(settings, secrets);
        let mut rev = 0u64;
        let mut notifier = vpn_monitor::notify::Notifier::default();
        let mut wait = Duration::from_secs(2); // первая проверка — сразу после старта
        let mut manual = false;
        let mut last_check = std::time::SystemTime::now();
        loop {
            match rx.recv_timeout(wait) {
                Ok(Cmd::CheckNow) => manual = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            // удалённые настройки поменялись — проверка берёт новые (пороги, цели) без перезапуска
            {
                let sh = lock(&shared);
                if sh.settings_rev != rev {
                    rev = sh.settings_rev;
                    m.settings = sh.settings.clone();
                }
            }
            // пауза: проверки по расписанию не идут (по кнопке «Проверить» — идут)
            let now_ms = vpn_monitor::metrics::now_ms();
            if !manual
                && let Some(until) = lock(&shared).prefs.pause_until_ms.filter(|u| *u > now_ms)
            {
                wait = Duration::from_millis(((until - now_ms) as u64).min(60_000));
                continue;
            }
            // сон: между проверками прошло намного больше интервала — первые 2 минуты провалы не считаем
            let gap = std::time::SystemTime::now()
                .duration_since(last_check)
                .unwrap_or_default();
            if gap > wait + Duration::from_secs(90) {
                info!(
                    "check",
                    "перерыв {} с — похоже, ноутбук спал: 2 минуты провалы не считаю",
                    gap.as_secs()
                );
                m.grace_until = Some(std::time::Instant::now() + Duration::from_secs(120));
            }
            let _ = proxy.send_event(UserEvent::Checking);
            // своя норма — для текущей сети (v0.9): дома, в отеле и через телефон нормы разные
            let net_now = vpn_monitor::netctx::current().name;
            {
                let mut sh = lock(&shared);
                sh.checking = true;
                m.baseline = sh.store.baseline_for(
                    &sh.settings.thresholds,
                    m.last_client.as_deref(),
                    (!net_now.is_empty()).then_some(net_now.as_str()),
                    &sh.prefs.forgot_nets,
                );
            }
            let snap = m.run_check(manual);
            manual = false;
            last_check = std::time::SystemTime::now();
            {
                let mut sh = lock(&shared);
                sh.checking = false;
                // «после сна» в историю не пишем — это не данные о VPN
                if !snap.waking
                    && let Err(e) = sh
                        .store
                        .push(Record::from_snapshot(&snap, snap.internet_fresh))
                {
                    error!("metrics", "не записать метрики: {e}");
                }
                sh.snapshot = Some(snap.clone());
                // город — раз в 6 часов и только когда есть интернет; отдельным потоком, чтобы не держать проверку
                let due = sh
                    .city_at
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(6 * 3600));
                if due && snap.route.is_some() {
                    sh.city_at = Some(std::time::Instant::now());
                    let sh2 = shared.clone();
                    std::thread::spawn(move || match vpn_monitor::net::city() {
                        Ok(c) => {
                            info!("geo", "город: {c}");
                            lock(&sh2).city = Some(c);
                        }
                        Err(e) => warn!("geo", "город не определён: {e}"),
                    });
                }
            }
            // R3.4: зелёный — раз в 3 минуты, иначе — раз в минуту
            wait = Duration::from_secs(if snap.verdict.color == Color::Green {
                green
            } else {
                alert
            });
            // уведомления: только о подтверждённой смене состояния и с лимитами (notify.rs)
            let (prefs, paused) = {
                let sh = lock(&shared);
                (
                    sh.prefs.notify.clone(),
                    sh.prefs.paused(vpn_monitor::metrics::now_ms()),
                )
            };
            if !paused {
                for n in notifier.observe(&observation(&snap), &prefs, std::time::Instant::now()) {
                    info!("notify", "{}: {}", n.title, n.body);
                    if let Err(e) = win::toast(&n.title, &n.body) {
                        warn!("notify", "{e}");
                    }
                }
            }
            if proxy
                .send_event(UserEvent::Snapshot(Box::new(snap)))
                .is_err()
            {
                return;
            }
            while rx.try_recv().is_ok() {} // клики, пришедшие во время проверки, не копим
        }
    });
}

/// Что видно в проверке — для решения об уведомлениях. Нет интернета, Wi‑Fi со входом, после сна — «неизвестно».
fn observation(s: &Snapshot) -> vpn_monitor::notify::Observation {
    use vpn_monitor::probe::Fail;
    let unknown = s.waking
        || matches!(
            s.verdict.badge,
            Badge::NoInternet | Badge::Captive | Badge::Unknown
        );
    let light: Vec<&ProbeResult> = s
        .targets
        .iter()
        .filter(|r| r.role == Some(Role::Light))
        .collect();
    let world: Vec<&ProbeResult> = s
        .targets
        .iter()
        .filter(|r| r.role == Some(Role::World))
        .collect();
    vpn_monitor::notify::Observation {
        office_up: (!unknown && !light.is_empty()).then(|| light.iter().any(|r| r.reachable)),
        world_up: (!unknown && !world.is_empty()).then(|| world.iter().any(|r| r.ok)),
        world_down_names: world
            .iter()
            .filter(|r| !r.ok)
            .map(|r| r.name.clone())
            .collect(),
        auth_failed: !unknown
            && s.targets
                .iter()
                .any(|r| r.fail == Some(Fail::Auth) && r.auth_used.contains("AD")),
        slow: s.verdict.color == Color::Yellow,
        client: s.client.as_ref().map(|c| c.name.clone()),
    }
}

/// Удалённые настройки (v0.7): раз в 5 минут — office.json из релиза `latest`; поменялся — наложить на свои
/// настройки, сохранить копию, отдать проверке и окну. Файла нет — свои настройки.
/// Сообщения админов (v0.9, ADR 0014): `announce.json` из релиза `latest` раз в 5 минут; копия — в папке установки
/// (показать сразу после запуска без сети). Новое и «решено» — уведомление Windows всем.
fn spawn_announce(base_url: String, dir: PathBuf, shared: SharedRef) {
    std::thread::spawn(move || {
        let path = dir.join(vpn_monitor::announce::FILE);
        let mut last_err = String::new();
        let mut text = std::fs::read_to_string(&path).ok();
        loop {
            match vpn_monitor::announce::fetch(&base_url) {
                Ok(Some(t)) => {
                    if text.as_deref() != Some(t.as_str()) {
                        let _ = std::fs::write(&path, &t);
                    }
                    text = Some(t);
                    last_err.clear();
                }
                Ok(None) => {
                    let _ = std::fs::remove_file(&path);
                    text = None;
                    last_err.clear();
                }
                Err(e) => {
                    if e != last_err {
                        warn!("announce", "сообщения админов не скачать: {e}");
                        last_err = e;
                    }
                }
            }
            let file = match text.as_deref().map(vpn_monitor::announce::parse) {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    warn!("announce", "{e}");
                    Default::default()
                }
                None => Default::default(),
            };
            let toasts = {
                let mut sh = lock(&shared);
                let before = sh.prefs.announce.clone();
                let (shown, toasts) = vpn_monitor::announce::select(
                    &file,
                    &mut sh.prefs.announce,
                    vpn_monitor::metrics::now_ms(),
                );
                if sh.prefs.announce != before {
                    let d = sh.dir.clone();
                    let _ = sh.prefs.save(&d);
                }
                sh.announce = shown;
                sh.announce_file = file;
                toasts
            };
            for t in toasts {
                info!("announce", "сообщение админов: {}", t.title);
                if let Err(e) = win::toast(&t.title, &t.body) {
                    warn!("announce", "{e}");
                }
            }
            std::thread::sleep(vpn_monitor::announce::PERIOD);
        }
    });
}

/// Статусы заявок в поддержку (v0.8): раз в 15 минут спрашиваем Jira о нерешённых, смена — уведомление.
/// Заявок нет или все решены — сеть не трогаем.
fn spawn_tickets(dir: PathBuf, shared: SharedRef) {
    std::thread::spawn(move || {
        let mut last_err = String::new();
        std::thread::sleep(Duration::from_secs(90));
        loop {
            let open = vpn_monitor::jsm::open_tickets(&vpn_monitor::jsm::load_tickets(&dir));
            let (url, notify) = {
                let sh = lock(&shared);
                (sh.settings.support_url.clone(), sh.prefs.notify.tickets)
            };
            if !open.is_empty()
                && let (Some(p), Some(ad)) = (
                    vpn_monitor::jsm::parse_portal(&url),
                    vpn_monitor::creds::load_ad(),
                )
            {
                let (toasts, err) =
                    vpn_monitor::jsm::refresh_statuses(&dir, &p, &ad.login, &ad.password);
                for (title, body) in toasts {
                    info!("support", "{title}");
                    if notify && let Err(e) = win::toast(&title, &body) {
                        warn!("support", "{e}");
                    }
                }
                // ошибку (нет VPN, сменился пароль) пишем один раз, пока она та же
                let e = err.unwrap_or_default();
                if !e.is_empty() && e != last_err {
                    warn!("support", "статусы заявок не узнать: {e}");
                }
                last_err = e;
            }
            std::thread::sleep(vpn_monitor::jsm::STATUS_PERIOD);
        }
    });
}

fn spawn_office(base: Settings, dir: PathBuf, shared: SharedRef) {
    std::thread::spawn(move || {
        let path = dir.join(vpn_monitor::office::FILE);
        let mut last: Option<String> = std::fs::read_to_string(&path).ok();
        let mut last_err = String::new();
        std::thread::sleep(Duration::from_secs(20));
        loop {
            match vpn_monitor::office::fetch(&base.release_base_url) {
                Ok(text) if text == last => {}
                Ok(Some(text)) => match vpn_monitor::office::apply(&base, &text) {
                    Ok((s, keys)) => {
                        info!(
                            "office",
                            "применены удалённые настройки: {}",
                            keys.join(", ")
                        );
                        let _ = std::fs::write(&path, &text);
                        let mut sh = lock(&shared);
                        sh.settings = s;
                        sh.settings_rev += 1;
                        sh.office_keys = Some(keys);
                        sh.office_at = Some(vpn_monitor::metrics::now_ms());
                        last = Some(text);
                    }
                    Err(e) => warn!("office", "удалённые настройки не применены: {e}"),
                },
                Ok(None) => {
                    if last.is_some() {
                        info!("office", "удалённых настроек больше нет — возвращаю свои");
                        let _ = std::fs::remove_file(&path);
                        let mut sh = lock(&shared);
                        sh.settings = base.clone();
                        sh.settings_rev += 1;
                        sh.office_keys = None;
                        sh.office_at = Some(vpn_monitor::metrics::now_ms());
                        last = None;
                    }
                }
                // без сети это раз в 5 минут — пишем, только когда ошибка поменялась
                Err(e) if e != last_err => {
                    warn!("office", "не проверить удалённые настройки: {e}");
                    last_err = e;
                }
                Err(_) => {}
            }
            std::thread::sleep(vpn_monitor::office::PERIOD);
        }
    });
}

fn spawn_updater(
    settings: Settings,
    dir: PathBuf,
    proxy: EventLoopProxy<UserEvent>,
    now_rx: mpsc::Receiver<()>,
    shared: SharedRef,
) {
    std::thread::spawn(move || {
        let base = settings.release_base_url.clone();
        let period = Duration::from_secs(settings.update_interval_min.max(1) * 60);
        let mut failed: Option<(String, u32)> = None;
        std::thread::sleep(Duration::from_secs(10));
        loop {
            let (channel, paused) = {
                let sh = lock(&shared);
                (
                    sh.prefs.channel.clone(),
                    sh.prefs.paused(vpn_monitor::metrics::now_ms()),
                )
            };
            match update::fetch_channel_version(&base, &channel) {
                Ok(latest) => {
                    info!("update", "последняя версия {latest}, своя {VERSION}");
                    let _ = proxy.send_event(UserEvent::LatestVersion(latest.clone()));
                    {
                        let mut sh = lock(&shared);
                        sh.latest = Some(latest.clone());
                        sh.update_error = None;
                    }
                    let tries = failed
                        .as_ref()
                        .filter(|(v, _)| *v == latest)
                        .map_or(0, |(_, n)| *n);
                    // во время паузы не обновляемся: сотрудник, скорее всего, на звонке
                    if update::needs_update(VERSION, &latest) && tries < 3 && !paused {
                        match apply_update(&base, &latest, &dir) {
                            Ok(exe) => {
                                let _ = proxy.send_event(UserEvent::RestartInto(exe));
                                return;
                            }
                            Err(e) => {
                                error!(
                                    "update",
                                    "обновление до {latest} не удалось (попытка {}): {e}",
                                    tries + 1
                                );
                                lock(&shared).update_error = Some(e.clone());
                                let _ = proxy.send_event(UserEvent::UpdateError(e));
                                failed = Some((latest, tries + 1));
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("update", "не узнать последнюю версию: {e}");
                    lock(&shared).update_error = Some(e.clone());
                    let _ = proxy.send_event(UserEvent::UpdateError(e));
                }
            }
            // ждём период или запрос «обновить сейчас» из окна (R7.3)
            match now_rx.recv_timeout(period) {
                Ok(()) => info!("update", "проверка обновления по запросу из окна"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(period),
            }
        }
    });
}

fn apply_update(base: &str, ver: &str, dir: &std::path::Path) -> Result<PathBuf, String> {
    info!("update", "ставлю версию {ver}: качаю настройки");
    let (raw, exe_info) = update::fetch_settings(base, ver)?;
    if exe_info.version.trim_start_matches('v') != ver {
        return Err(format!(
            "settings.json релиза {ver} описывает exe версии {}",
            exe_info.version
        ));
    }
    let dest = dir.join(update::exe_name(ver));
    info!("update", "качаю exe → {}", dest.display());
    let size = update::download_exe(base, ver, &exe_info.sha256, &dest)?;
    info!("update", "exe скачан ({size} байт), SHA-256 совпал");
    std::fs::write(dir.join("settings.json"), raw).map_err(|e| format!("settings.json: {e}"))?;
    // на постоянное место `vpn-monitor.exe` (запущенный — в `vpn-monitor.old.exe`)
    crate::install::place_exe(&dest, dir, crate::install::Place::Move)
}

fn toggle_autostart(u: &mut Ui) {
    let on = u.autostart.is_checked();
    let res = if on {
        std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|p| win::autostart_set(&p))
    } else {
        win::autostart_remove()
    };
    match res {
        Ok(()) => info!(
            "tray",
            "автозапуск {}",
            if on {
                "включён"
            } else {
                "выключен"
            }
        ),
        Err(e) => {
            error!("tray", "{e}");
            u.autostart.set_checked(!on);
        }
    }
}

fn build_ui(settings: &Settings, api_ok: bool) -> Result<Ui, String> {
    let menu = Menu::new();
    let item = |t: &str| MenuItem::new(t, false, None);
    let title = item("Domini VPN");
    let status = item("Первая проверка…");
    let targets: Vec<(String, MenuItem)> = settings
        .targets
        .iter()
        .filter(|t| t.enabled)
        .map(|t| (t.id.clone(), item(&format!("{}: —", t.name))))
        .collect();
    let internet = item(&format!("{}: —", settings.internet.name));
    let stability = item("Стабильность: —");
    let updated = item("Обновлено: —");
    let check_now = MenuItem::new("Проверить сейчас", true, None);
    let open_window = MenuItem::new(
        if api_ok {
            "Открыть окно — графики и диагностика"
        } else {
            "Окно недоступно — см. лог"
        },
        api_ok,
        None,
    );
    let open_log = MenuItem::new("Открыть лог", true, None);
    let open_dir = MenuItem::new("Открыть папку (секреты, настройки)", true, None);
    let autostart = CheckMenuItem::new("Автозапуск", true, win::autostart_get().is_some(), None);
    let version = item(&format!("Версия {VERSION}"));
    let quit = MenuItem::new("Выйти", true, None);

    let sep = PredefinedMenuItem::separator;
    let add = |m: &dyn tray_icon::menu::IsMenuItem| menu.append(m).map_err(|e| e.to_string());
    add(&title)?;
    add(&status)?;
    add(&sep())?;
    for (_, i) in &targets {
        add(i)?;
    }
    add(&internet)?;
    add(&stability)?;
    add(&updated)?;
    add(&sep())?;
    add(&check_now)?;
    add(&open_window)?;
    add(&open_log)?;
    add(&open_dir)?;
    add(&sep())?;
    add(&autostart)?;
    add(&version)?;
    add(&quit)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Domini VPN — первая проверка…")
        .with_icon(make_icon(Badge::Unknown, Color::Red, false))
        .build()
        .map_err(|e| e.to_string())?;
    info!("tray", "иконка в трее создана");
    Ok(Ui {
        tray,
        title,
        status,
        targets,
        internet,
        stability,
        updated,
        check_now,
        open_window,
        open_log,
        open_dir,
        autostart,
        version,
        quit,
        checking: false,
        paused: false,
        last: None,
        latest: None,
        update_error: None,
    })
}

fn make_icon(b: Badge, c: Color, checking: bool) -> Icon {
    Icon::from_rgba(icon::render(b, c, checking), icon::SIZE, icon::SIZE)
        .expect("иконка 32×32 RGBA")
}

/// Сервисы офиса проверялись без входа AD (проба пишет «нет входа…» в `note`).
fn need_login(s: &vpn_monitor::monitor::Snapshot) -> bool {
    s.targets.iter().any(|t| {
        t.note
            .as_deref()
            .is_some_and(|n| n.starts_with("нет входа"))
    })
}

fn refresh(u: &mut Ui) {
    let (badge, color) = u.last.as_ref().map_or((Badge::Unknown, Color::Red), |s| {
        (s.verdict.badge, s.verdict.color)
    });
    // нет входа AD (вышли в «Настройках» или ещё не входили) — жёлтый «!» на иконке, пока не войдут
    let need_login = u.last.as_ref().is_some_and(need_login);
    let (badge, color) = if u.paused {
        (Badge::Paused, Color::Gray)
    } else if need_login && !matches!(badge, Badge::NoInternet | Badge::Captive) {
        (Badge::NoVpn, Color::Yellow)
    } else {
        (badge, color)
    };
    let _ = u.tray.set_icon(Some(make_icon(badge, color, u.checking)));
    u.check_now.set_enabled(!u.checking);

    let mut ver = format!("Версия {VERSION}");
    match (&u.latest, &u.update_error) {
        (Some(l), _) if l == VERSION => ver += " · последняя",
        (Some(l), None) => ver += &format!(" · ставится {l}…"),
        (Some(l), Some(_)) => ver += &format!(" · не удалось обновить до {l}, см. лог"),
        (None, Some(_)) => ver += " · обновления недоступны, см. лог",
        (None, None) => {}
    }
    u.version.set_text(ver);

    let Some(s) = &u.last else {
        if u.checking {
            u.status.set_text("Идёт проверка…");
        }
        return;
    };
    let v = &s.verdict;
    let status = if u.checking {
        "Идёт проверка…".to_string()
    } else {
        match v.badge {
            Badge::NoVpn => "VPN не включён".to_string(),
            Badge::WeakInternet(_) => {
                format!("Слабый интернет · оценка VPN {:02}", v.score.unwrap_or(0))
            }
            Badge::Score(n) => format!("Оценка VPN {n:02} · {}", v.color.ru()),
            Badge::Unknown => "Нет данных".to_string(),
            Badge::NoInternet => "Нет интернета — VPN не оценивается".to_string(),
            Badge::Captive => "Wi‑Fi требует входа — откройте браузер".to_string(),
            Badge::Paused => "Проверки на паузе".to_string(),
        }
    };
    u.status.set_text(&status);
    // «Domini VPN · Happ / профиль» — клиент и профиль VPN (R4.3)
    u.title.set_text(match &s.client {
        Some(c) => format!(
            "Domini VPN · {}{}",
            c.name,
            c.profile
                .as_deref()
                .map(|p| format!(" / {p}"))
                .unwrap_or_default()
        ),
        None => "Domini VPN".into(),
    });
    for (id, item) in &u.targets {
        if let Some(r) = s.targets.iter().find(|r| &r.target_id == id) {
            item.set_text(target_line(r));
        }
    }
    u.internet.set_text(match &s.internet {
        Some(r) if r.mbps.is_some() => format!(
            "{}: {:.1} Мбит/с · {}",
            r.name,
            r.mbps.unwrap_or(0.0),
            log::hhmm(r.at)
        ),
        Some(r) => format!("{}: {}", r.name, fail_text(r)),
        None => "Интернет: ещё не мерился".to_string(),
    });
    u.stability.set_text(match v.stability {
        Some(st) => format!("Стабильность: {:.0}%", st * 100.0),
        None => "Стабильность: —".into(),
    });
    u.updated
        .set_text(format!("Обновлено: {}", log::hhmm(s.at)));

    // подсказка иконки — несколько строк: оценка, вывод, цифры звеньев, время (v0.6)
    let title = match v.score {
        _ if need_login && !u.paused => "⚠️ Требуется вход — Domini VPN Monitor".to_string(),
        Some(n) if matches!(v.badge, Badge::Score(_) | Badge::WeakInternet(_)) => {
            format!("Domini VPN Monitor — {n}/99")
        }
        _ => "Domini VPN Monitor".to_string(),
    };
    let client = s.client.as_ref().map(|c| c.name.as_str()).unwrap_or("VPN");
    let (line, nums) = if u.paused {
        ("Проверки на паузе".to_string(), String::new())
    } else {
        match (&v.badge, &s.route) {
            (Badge::NoInternet, _) => (
                "Нет интернета — VPN проверим, когда сеть вернётся".into(),
                String::new(),
            ),
            (Badge::Captive, _) => (
                "Wi‑Fi требует входа на своей странице".into(),
                String::new(),
            ),
            (Badge::Unknown, _) if s.waking => ("Проверяю после сна…".into(), String::new()),
            (_, Some(r)) => (
                vpn_monitor::route::headline(r, client),
                vpn_monitor::route::numbers(r),
            ),
            _ => (status.clone(), String::new()),
        }
    };
    let tip = vpn_monitor::route::tooltip(&[
        title,
        line,
        nums,
        format!("Проверено в {}", log::hhmm(s.at)),
    ]);
    let _ = u.tray.set_tooltip(Some(tip));
}

fn target_line(r: &ProbeResult) -> String {
    let mut s = format!("{}: ", r.name);
    if !r.reachable {
        return s + &fail_text(r);
    }
    if r.role == Some(Role::Speed) {
        match r.mbps {
            Some(m) if r.ok => s += &format!("{m:.1} Мбит/с · "),
            _ => {}
        }
    }
    if let Some(l) = r.latency_ms {
        s += &format!("{l:.0} мс");
    }
    if r.via_vpn == Some(false) {
        s += " · без ВПН";
    }
    if !r.ok {
        s += &format!(" · ⚠ {}", fail_text(r));
    } else if r.note.is_some() {
        s += " · ⚠ без кредов";
    }
    s
}

fn fail_text(r: &ProbeResult) -> String {
    match r.fail {
        Some(Fail::Dns) => "недоступен (DNS)".into(),
        Some(Fail::Connect) => "недоступен (нет соединения)".into(),
        Some(Fail::Timeout) => "таймаут".into(),
        Some(Fail::Auth) => "нет доступа — проверьте secrets.env".into(),
        Some(Fail::Http) => format!(
            "ошибка HTTP {}",
            r.http_status.map(|c| c.to_string()).unwrap_or_default()
        ),
        Some(Fail::Config) => "ошибка настроек".into(),
        None if r.note.is_some() => "нет кредов в secrets.env".into(),
        None => "ошибка".into(),
    }
}
