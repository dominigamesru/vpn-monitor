//! Windows: одна копия в трее, сигнал «закройся» для старой копии, автозапуск, сообщения, консоль.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, ResetEvent, SetEvent,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MessageBoxW,
};

const MUTEX_NAME: &str = "Local\\DominiVPNMonitor.Tray";
const QUIT_EVENT: &str = "Local\\DominiVPNMonitor.Quit";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "DominiVPNMonitor";
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Владение «я — та самая копия в трее». Держится до конца процесса.
pub struct Instance(#[allow(dead_code)] usize);

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0 as HANDLE) };
    }
}

/// Пытается стать единственной копией. None — уже есть другая.
pub fn try_single_instance() -> Option<Instance> {
    unsafe {
        let name = wide(MUTEX_NAME);
        let h: HANDLE = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() {
            return None;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return None;
        }
        Some(Instance(h as usize))
    }
}

/// Просит запущенную копию закрыться и ждёт, пока она освободит место (до `timeout`).
pub fn replace_running(timeout: Duration) -> Option<Instance> {
    signal_quit();
    let start = Instant::now();
    loop {
        if let Some(i) = try_single_instance() {
            return Some(i);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Есть ли сейчас копия в трее.
pub fn is_running() -> bool {
    match try_single_instance() {
        Some(i) => {
            drop(i); // закрыли хэндл — мьютекс исчез, место свободно
            false
        }
        None => true,
    }
}

pub fn signal_quit() {
    unsafe {
        let name = wide(QUIT_EVENT);
        let h = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if !h.is_null() {
            SetEvent(h);
            CloseHandle(h);
        }
    }
}

/// Поток, который ждёт сигнала «закройся» и вызывает `on_quit`. Вызывать после того, как стали единственной копией.
pub fn watch_quit(on_quit: impl FnOnce() + Send + 'static) {
    let h = unsafe {
        let name = wide(QUIT_EVENT);
        let h = CreateEventW(std::ptr::null(), 0, 0, name.as_ptr());
        if h.is_null() {
            return;
        }
        ResetEvent(h); // сигнал, оставшийся с прошлого раза, нам не адресован
        h as usize
    };
    std::thread::spawn(move || {
        unsafe { WaitForSingleObject(h as HANDLE, INFINITE) };
        on_quit();
    });
}

pub fn autostart_get() -> Option<String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    hkcu.open_subkey(RUN_KEY)
        .ok()?
        .get_value::<String, _>(RUN_VALUE)
        .ok()
}

pub fn autostart_set(exe: &Path) -> Result<(), String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(RUN_KEY)
        .map_err(|e| format!("автозапуск: {e}"))?;
    key.set_value(RUN_VALUE, &format!("\"{}\" --tray", exe.display()))
        .map_err(|e| format!("автозапуск: {e}"))
}

pub fn autostart_remove() -> Result<(), String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_SET_VALUE)
        .map_err(|e| format!("автозапуск: {e}"))?;
    match key.delete_value(RUN_VALUE) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("автозапуск: {e}")),
    }
}

/// Запускает exe отдельным процессом без консоли.
pub fn spawn_detached(exe: &Path, args: &[&str]) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("запуск {}: {e}", exe.display()))
}

const SHORTCUT: &str = "Domini VPN Monitor.lnk";

/// PowerShell-строка в одинарных кавычках.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn powershell(script: &str) -> Result<(), String> {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .chars()
            .take(300)
            .collect())
    }
}

/// Ярлык «Domini VPN Monitor» на рабочем столе: открывает окно (`--open`), а если приложение не запущено —
/// запускает его. Рабочий стол — как его видит Windows (в т. ч. перенесённый в OneDrive).
pub fn desktop_shortcut(exe: &Path) -> Result<(), String> {
    let exe_s = exe.display().to_string();
    let dir = exe
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let script = format!(
        "$d=[Environment]::GetFolderPath('Desktop'); \
         $s=(New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $d {lnk})); \
         $s.TargetPath={exe}; $s.Arguments='--open'; $s.WorkingDirectory={dir}; \
         $s.IconLocation={icon}; $s.Description='Состояние VPN в офис и в мир'; $s.Save()",
        lnk = ps_quote(SHORTCUT),
        exe = ps_quote(&exe_s),
        dir = ps_quote(&dir),
        icon = ps_quote(&format!("{exe_s},0")),
    );
    powershell(&script)
}

pub fn desktop_shortcut_remove() -> Result<(), String> {
    powershell(&format!(
        "$p=Join-Path ([Environment]::GetFolderPath('Desktop')) {}; if (Test-Path $p) {{ Remove-Item $p }}",
        ps_quote(SHORTCUT)
    ))
}

/// Ярлык и `--open`: открыть окно запущенного приложения; не запущено — запустить и открыть, когда ответит.
pub fn open_or_start(exe: &Path) {
    let find = || {
        (crate::api::BASE_PORT..crate::api::BASE_PORT + 10).find(|p| {
            use std::io::{Read, Write};
            let Ok(mut s) = std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], *p)),
                std::time::Duration::from_millis(300),
            ) else {
                return false;
            };
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let req = format!("GET /api/state HTTP/1.0\r\nHost: 127.0.0.1:{p}\r\n\r\n");
            let mut buf = String::new();
            s.write_all(req.as_bytes()).is_ok()
                && s.take(4096).read_to_string(&mut buf).is_ok()
                && buf.contains("\"version\"")
        })
    };
    if let Some(p) = find() {
        open_url(&format!("http://127.0.0.1:{p}/#now"));
        return;
    }
    if spawn_detached(exe, &["--tray"]).is_err() {
        return;
    }
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Some(p) = find() {
            open_url(&format!("http://127.0.0.1:{p}/#now"));
            return;
        }
    }
}

/// Открывает файл или папку программой по умолчанию.
pub fn open(path: &Path) {
    let _ = std::process::Command::new("explorer")
        .arg(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Открывает адрес браузером по умолчанию.
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

pub fn open_in_notepad(path: &Path) {
    let _ = std::process::Command::new("notepad").arg(path).spawn();
}

pub fn message(title: &str, text: &str, warning: bool) {
    let (t, c) = (wide(text), wide(title));
    let icon = if warning {
        MB_ICONWARNING
    } else {
        MB_ICONINFORMATION
    };
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            t.as_ptr(),
            c.as_ptr(),
            MB_OK | icon | MB_SETFOREGROUND,
        )
    };
}

pub fn attach_console() {
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// «Windows 11 Pro 24H2 (сборка 26100)» из реестра.
pub fn os_version() -> String {
    let hklm = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE);
    let Ok(k) = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion") else {
        return String::new();
    };
    let get = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
    let build = get("CurrentBuild");
    let mut product = get("ProductName");
    // в реестре Windows 11 до сих пор пишет «Windows 10»
    if build.parse::<u32>().unwrap_or(0) >= 22000 {
        product = product.replace("Windows 10", "Windows 11");
    }
    format!("{product} {} (сборка {build})", get("DisplayVersion"))
}

/// AppUserModelID для уведомлений Windows (без него непакетное приложение не может их показывать).
pub const AUMID: &str = "DominiGames.VPNMonitor";

/// Регистрирует приложение для уведомлений: имя «Domini VPN» и значок (так делает Windows App SDK для
/// непакетных приложений — ключ в HKCU\Software\Classes\AppUserModelId).
pub fn register_notifications(dir: &Path) -> Result<(), String> {
    let icon = dir.join("notify-icon.ico");
    let px = vpn_monitor::icon::render(
        vpn_monitor::score::Badge::Unknown,
        vpn_monitor::score::Color::Gray,
        false,
    );
    let img = ico::IconImage::from_rgba_data(vpn_monitor::icon::SIZE, vpn_monitor::icon::SIZE, px);
    let mut dir_ico = ico::IconDir::new(ico::ResourceType::Icon);
    dir_ico.add_entry(ico::IconDirEntry::encode(&img).map_err(|e| e.to_string())?);
    let f = std::fs::File::create(&icon).map_err(|e| format!("{}: {e}", icon.display()))?;
    dir_ico.write(f).map_err(|e| e.to_string())?;
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(format!("Software\\Classes\\AppUserModelId\\{AUMID}"))
        .map_err(|e| format!("уведомления: {e}"))?;
    key.set_value("DisplayName", &"Domini VPN")
        .map_err(|e| format!("уведомления: {e}"))?;
    key.set_value("IconUri", &icon.display().to_string())
        .map_err(|e| format!("уведомления: {e}"))?;
    Ok(())
}

/// Уведомление Windows со ссылкой (v0.9): кнопка «Открыть» и клик по уведомлению открывают ссылку (только https).
pub fn toast_link(title: &str, body: &str, link: Option<&str>) -> Result<(), String> {
    let Some(url) = link
        .filter(|u| u.starts_with("https://"))
        .map(str::to_string)
    else {
        return toast(title, body);
    };
    let u2 = url.clone();
    tauri_winrt_notification::Toast::new(AUMID)
        .title(title)
        .text1(body)
        .add_button("Открыть", &url)
        .on_activated(move |action| {
            let target = action
                .filter(|a| a.starts_with("https://"))
                .unwrap_or_else(|| u2.clone());
            open_url(&target);
            Ok(())
        })
        .duration(tauri_winrt_notification::Duration::Long)
        .show()
        .map_err(|e| format!("уведомление: {e:?}"))
}

/// Показывает уведомление Windows.
pub fn toast(title: &str, body: &str) -> Result<(), String> {
    tauri_winrt_notification::Toast::new(AUMID)
        .title(title)
        .text1(body)
        .duration(tauri_winrt_notification::Duration::Short)
        .show()
        .map_err(|e| format!("уведомление: {e:?}"))
}
