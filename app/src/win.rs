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
