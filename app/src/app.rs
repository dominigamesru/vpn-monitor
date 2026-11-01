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
    last: Option<Snapshot>,
    latest: Option<String>,
    update_error: Option<String>,
}

pub fn run(replace: bool, first_run: bool) {
    let embedded = Settings::embedded();
    let dir = embedded.install_dir();
    log::init(&dir.join("vpn-monitor.log"), false);
    let (settings, src) = Settings::load(&dir);
    crate::log_start("трей", &dir, &src);

    let instance = if replace {
        win::replace_running(Duration::from_secs(15))
    } else {
        win::try_single_instance()
    };
    let Some(_instance) = instance else {
        info!("tray", "уже запущена другая копия — выхожу");
        return;
    };
    if let Ok(me) = std::env::current_exe() {
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
    info!(
        "metrics",
        "история: {} проверок из metrics_7d.json",
        store.records.len()
    );
    let shared: SharedRef = Arc::new(Mutex::new(Shared {
        settings: settings.clone(),
        dir: dir.clone(),
        snapshot: None,
        checking: false,
        latest: None,
        update_error: None,
        store,
        api_url: None,
        api_error: None,
        diag: None,
        diag_running: false,
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
                win::open_url(&format!("{url}#conn"));
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
    spawn_updater(
        settings.clone(),
        dir.clone(),
        proxy.clone(),
        upd_rx,
        shared.clone(),
    );
    let api_url = lock(&shared).api_url.clone();

    let mut ui: Option<Ui> = None;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => match build_ui(&settings, api_url.is_some()) {
                Ok(u) => ui = Some(u),
                Err(e) => {
                    error!("tray", "не создать иконку в трее: {e}");
                    *flow = ControlFlow::Exit;
                }
            },
            Event::UserEvent(ev) => {
                let Some(u) = ui.as_mut() else { return };
                match ev {
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
                    }) => {
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
        let mut wait = Duration::from_secs(2); // первая проверка — сразу после старта
        let mut manual = false;
        loop {
            match rx.recv_timeout(wait) {
                Ok(Cmd::CheckNow) => manual = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            let _ = proxy.send_event(UserEvent::Checking);
            {
                let mut sh = lock(&shared);
                sh.checking = true;
                m.baseline = sh
                    .store
                    .baseline(&sh.settings.thresholds, m.last_client.as_deref());
            }
            let snap = m.run_check(manual);
            manual = false;
            {
                let mut sh = lock(&shared);
                sh.checking = false;
                if let Err(e) = sh
                    .store
                    .push(Record::from_snapshot(&snap, snap.internet_fresh))
                {
                    error!("metrics", "не записать метрики: {e}");
                }
                sh.snapshot = Some(snap.clone());
            }
            // R3.4: зелёный — раз в 3 минуты, иначе — раз в минуту
            wait = Duration::from_secs(if snap.verdict.color == Color::Green {
                green
            } else {
                alert
            });
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
            match update::fetch_latest_version(&base) {
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
                    if update::needs_update(VERSION, &latest) && tries < 3 {
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
    Ok(dest)
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
        last: None,
        latest: None,
        update_error: None,
    })
}

fn make_icon(b: Badge, c: Color, checking: bool) -> Icon {
    Icon::from_rgba(icon::render(b, c, checking), icon::SIZE, icon::SIZE)
        .expect("иконка 32×32 RGBA")
}

fn refresh(u: &mut Ui) {
    let (badge, color) = u.last.as_ref().map_or((Badge::Unknown, Color::Red), |s| {
        (s.verdict.badge, s.verdict.color)
    });
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

    let mut tip = format!("Domini VPN — {status}");
    if let Some(l) = v.latency {
        tip += &format!("\nЗадержка {l:.0} мс");
    }
    if let Some(sp) = v.speed {
        tip += &format!(", скорость {sp:.1} Мбит/с");
    }
    let tip: String = tip.chars().take(120).collect();
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
