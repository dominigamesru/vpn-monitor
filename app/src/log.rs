//! Лог в файл `vpn-monitor.log` (папка установки). Ротация: > 5 МБ → `vpn-monitor.log.1`.
//! Формат строки: `2026-11-01 12:00:00.123 INFO  [probe] сообщение`. Секреты сюда не попадают.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SIZE: u64 = 5 * 1024 * 1024;

struct Logger {
    path: PathBuf,
    file: Option<File>,
    echo: bool,
}

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

/// Включает запись в файл. `echo` — дублировать в stdout (режим `--check`).
pub fn init(path: &Path, echo: bool) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = OpenOptions::new().create(true).append(true).open(path).ok();
    *LOGGER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Logger {
        path: path.to_path_buf(),
        file,
        echo,
    });
}

pub fn path() -> Option<PathBuf> {
    LOGGER.lock().ok()?.as_ref().map(|l| l.path.clone())
}

pub fn write(level: &str, module: &str, msg: &str) {
    let line = format!("{} {level:<5} [{module}] {msg}\n", now_local_string());
    let mut guard = LOGGER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(l) = guard.as_mut() else {
        eprint!("{line}");
        return;
    };
    if l.echo {
        print!("{line}");
    }
    if let Some(f) = l.file.as_mut() {
        let _ = f.write_all(line.as_bytes());
        if f.metadata().map(|m| m.len() > MAX_SIZE).unwrap_or(false) {
            let old = l.path.with_extension("log.1");
            let _ = std::fs::remove_file(&old);
            l.file = None;
            let _ = std::fs::rename(&l.path, &old);
            l.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&l.path)
                .ok();
        }
    }
}

#[macro_export]
macro_rules! info { ($m:expr, $($a:tt)*) => { $crate::log::write("INFO", $m, &format!($($a)*)) } }
#[macro_export]
macro_rules! warn { ($m:expr, $($a:tt)*) => { $crate::log::write("WARN", $m, &format!($($a)*)) } }
#[macro_export]
macro_rules! error { ($m:expr, $($a:tt)*) => { $crate::log::write("ERROR", $m, &format!($($a)*)) } }

/// Местное время строкой. Смещение часового пояса берётся у ОС на Windows; иначе UTC.
pub fn now_local_string() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64 + local_offset_secs();
    let (y, mo, d, h, mi, s) = civil(secs);
    format!(
        "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{:03}",
        now.subsec_millis()
    )
}

/// «ЧЧ:ММ» местного времени — для меню.
pub fn hhmm(t: SystemTime) -> String {
    let secs =
        t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64 + local_offset_secs();
    let (_, _, _, h, mi, _) = civil(secs);
    format!("{h:02}:{mi:02}")
}

#[cfg(windows)]
fn local_offset_secs() -> i64 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    unsafe {
        let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tz);
        let bias = match r {
            2 => tz.Bias + tz.DaylightBias, // TIME_ZONE_ID_DAYLIGHT
            _ => tz.Bias + tz.StandardBias,
        };
        -(bias as i64) * 60
    }
}

#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

/// Секунды UNIX → (год, месяц, день, час, минута, секунда), алгоритм Howard Hinnant.
fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_known_date() {
        // 2026-11-01 00:00:00 UTC
        assert_eq!(super::civil(1_793_491_200), (2026, 11, 1, 0, 0, 0));
    }
}
