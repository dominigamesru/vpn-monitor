//! Режим установки (двойной клик): закрыть запущенную копию, поставить себя в папку установки, настройки и
//! шаблон секретов рядом, автозапуск, запуск трея. Повторный запуск чинит любую сбитую установку (R1.5).

use crate::win;
use std::path::{Path, PathBuf};
use std::time::Duration;
use vpn_monitor::settings::{EMBEDDED, Settings};
use vpn_monitor::{VERSION, error, info, log, warn};

const SECRETS_TEMPLATE: &str = include_str!("../config/secrets.example.env");

pub fn run() {
    let settings = Settings::embedded();
    let dir = settings.install_dir();
    log::init(&dir.join("vpn-monitor.log"), false);
    crate::log_start("установка", &dir, "встроенные");
    match install(&dir) {
        Ok(Done {
            exe,
            secrets_new,
            secrets_empty,
        }) => {
            info!("install", "готово: {}", exe.display());
            let mut text = format!(
                "VPN Monitor {VERSION} установлен и запущен — иконка в трее (может прятаться под «^»).\n\n\
                 Папка: {}\nЛог: vpn-monitor.log в этой папке.",
                dir.display()
            );
            let _ = secrets_new;
            if secrets_empty {
                text += "\n\nСейчас откроется окно в браузере — войдите логином и паролем AD (вкладка «Подключение»). \
                         Пароль сохранится в Диспетчере учётных данных Windows.";
            }
            win::message("VPN Monitor", &text, false);
        }
        Err(e) => {
            error!("install", "{e}");
            win::message(
                "VPN Monitor — ошибка установки",
                &format!(
                    "{e}\n\nПодробности — в {}",
                    dir.join("vpn-monitor.log").display()
                ),
                true,
            );
        }
    }
}

struct Done {
    exe: PathBuf,
    secrets_new: bool,
    secrets_empty: bool,
}

fn install(dir: &Path) -> Result<Done, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("не создать папку {}: {e}", dir.display()))?;

    if win::is_running() {
        info!("install", "закрываю запущенную копию");
        win::signal_quit();
        // ждём, пока старая копия выйдет и освободит exe
        for _ in 0..50 {
            if !win::is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if win::is_running() {
            warn!("install", "старая копия не закрылась за 10 с — продолжаю");
        }
    }

    let me = std::env::current_exe().map_err(|e| format!("не узнать свой путь: {e}"))?;
    let target = place_exe(&me, dir, Place::Copy)?;

    std::fs::write(dir.join("settings.json"), EMBEDDED)
        .map_err(|e| format!("settings.json: {e}"))?;
    let secrets = crate::secrets_path(dir);
    let secrets_new = !secrets.exists();
    if secrets_new {
        std::fs::write(&secrets, SECRETS_TEMPLATE).map_err(|e| format!("secrets.env: {e}"))?;
        info!("install", "создан шаблон {}", secrets.display());
    }
    let loaded = vpn_monitor::secrets::Secrets::load(&secrets);
    let secrets_empty = loaded.ad_login().is_none() && loaded.get("CONFLUENCE_TOKEN").is_none();

    cleanup_old_exes(dir, &target);
    win::autostart_set(&target)?;
    info!(
        "install",
        "автозапуск: {}",
        win::autostart_get().unwrap_or_default()
    );
    win::spawn_detached(&target, &["--tray", "--first-run"])?;
    Ok(Done {
        exe: target,
        secrets_new,
        secrets_empty,
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn copy_with_retry(from: &Path, to: &Path) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..10 {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("не скопировать exe в {}: {last}", to.display()))
}

/// Постоянное имя exe в папке установки. Windows запоминает «показывать иконку в трее» по пути exe: до 0.5.4
/// каждая версия жила в `vpn-monitor-X.Y.Z.exe`, и после обновления иконка пряталась под «^» как новая программа.
pub const STABLE_EXE: &str = "vpn-monitor.exe";
/// Сюда переименовывается запущенный exe при обновлении (Windows разрешает переименовать запущенный exe, но не
/// перезаписать); удаляется при следующем запуске.
const OLD_EXE: &str = "vpn-monitor.old.exe";

pub enum Place {
    /// Установка двойным кликом: скопировать себя.
    Copy,
    /// Обновление: переместить скачанный файл.
    Move,
}

/// Кладёт exe в `<папка>/vpn-monitor.exe`. Занятый (запущенный) старый exe сначала переименовывается в
/// `vpn-monitor.old.exe`.
pub fn place_exe(src: &Path, dir: &Path, how: Place) -> Result<PathBuf, String> {
    let target = dir.join(STABLE_EXE);
    if same_file(src, &target) {
        return Ok(target);
    }
    if target.exists() && std::fs::remove_file(&target).is_err() {
        let old = dir.join(OLD_EXE);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&target, &old)
            .map_err(|e| format!("не освободить {}: {e}", target.display()))?;
        info!(
            "install",
            "запущенный {STABLE_EXE} переименован в {OLD_EXE}"
        );
    }
    match how {
        Place::Copy => copy_with_retry(src, &target)?,
        Place::Move => std::fs::rename(src, &target).map_err(|e| {
            format!(
                "не переместить {} → {}: {e}",
                src.display(),
                target.display()
            )
        })?,
    }
    info!("install", "{} → {}", src.display(), target.display());
    Ok(target)
}

/// Удаляет exe прошлых версий в папке установки и их папки WebView2 (`vpn-monitor-X.Y.Z.exe.WebView2`, до 0.5.3 —
/// своя на каждую версию; теперь общая `WebView2`). Занятые — пропускает, удалятся в следующий раз.
pub fn cleanup_old_exes(dir: &Path, keep: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let ours = (name.starts_with("vpn-monitor-")
            && (name.ends_with(".exe") || name.ends_with(".exe.part")))
            || name == OLD_EXE;
        if name.starts_with("vpn-monitor-") && name.ends_with(".exe.WebView2") && p.is_dir() {
            match std::fs::remove_dir_all(&p) {
                Ok(()) => info!("install", "удалена старая папка {name}"),
                Err(e) => info!("install", "старая папка {name} пока занята: {e}"),
            }
            continue;
        }
        if ours && !same_file(&p, keep) {
            match std::fs::remove_file(&p) {
                Ok(()) => info!("install", "удалён старый {name}"),
                Err(e) => info!("install", "старый {name} пока занят: {e}"),
            }
        }
    }
}
