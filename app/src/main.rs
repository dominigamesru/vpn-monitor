//! VPN Monitor — точка входа.
//!
//! Режимы (R1.3–R1.4):
//! - без аргументов (двойной клик) — режим установки;
//! - `--tray` — режим трея; `--tray --replace` — то же, предварительно закрыв запущенную копию (после обновления);
//! - `--check` — одна проверка с выводом в консоль и в лог (для отладки);
//! - `--version`.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod win;

use vpn_monitor::{
    VERSION, error, info, log, monitor::Monitor, secrets::Secrets, settings::Settings,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);

    if has("--version") || has("-V") {
        attach_console();
        println!("vpn-monitor {VERSION}");
        return;
    }

    let embedded = Settings::embedded();
    let dir = embedded.install_dir();
    std::panic::set_hook(Box::new(|p| {
        error!(
            "panic",
            "{p}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
    }));

    if has("--check") {
        attach_console();
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or(dir);
        let (settings, src) = Settings::load(&dir);
        log::init(&dir.join("vpn-monitor.log"), true);
        log_start("check", &dir, &src);
        let mut m = Monitor::new(settings, secrets_path(&dir));
        let snap = m.run_check(true);
        println!();
        println!(
            "Итог: {} {:?} — {}",
            snap.verdict.color.ru(),
            snap.verdict.badge,
            snap.verdict.explain
        );
        if let Some(p) = log::path() {
            println!("Лог: {}", p.display());
        }
        return;
    }

    #[cfg(windows)]
    {
        if has("--tray") {
            app::run(has("--replace"));
        } else {
            install::run();
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("Трей и установка — только Windows. Здесь доступны --check и --version.");
        std::process::exit(2);
    }
}

pub fn secrets_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("secrets.env")
}

/// Шапка лога при старте: всё, что нужно, чтобы по логу понять окружение. Без секретов.
pub fn log_start(mode: &str, dir: &std::path::Path, settings_src: &str) {
    info!("start", "==== VPN Monitor {VERSION}, режим {mode} ====");
    info!(
        "start",
        "exe: {}",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );
    info!(
        "start",
        "аргументы: {:?}",
        std::env::args().skip(1).collect::<Vec<_>>()
    );
    info!(
        "start",
        "ОС: {} {} · компьютер: {} · пользователь: {}",
        std::env::consts::OS,
        os_version(),
        std::env::var("COMPUTERNAME").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    info!("start", "папка: {}", dir.display());
    info!("start", "настройки: {settings_src}");
    info!(
        "start",
        "секреты ({}): {}",
        secrets_path(dir).display(),
        Secrets::load(&secrets_path(dir)).describe()
    );
    let proxies: Vec<String> = [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok().map(|v| format!("{k}={v}")))
    .collect();
    info!(
        "start",
        "прокси из окружения: {}",
        if proxies.is_empty() {
            "нет".into()
        } else {
            proxies.join(" ")
        }
    );
}

#[cfg(windows)]
fn os_version() -> String {
    win::os_version()
}
#[cfg(not(windows))]
fn os_version() -> String {
    String::new()
}

fn attach_console() {
    #[cfg(windows)]
    win::attach_console();
}
