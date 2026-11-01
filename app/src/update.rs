//! Обновление из GitHub Releases ([ADR 0004]): `latest/latest.version` → версия, которую ставят все.
//! Сравнение на равенство: если последняя версия старше своей — это откат, тоже ставим.
//!
//! Раскладка (контракт со всеми установленными копиями):
//! - `{base}/latest/latest.version` — строка `0.1.2`;
//! - `{base}/v{ver}/settings.json` — настройки версии, в `exe.sha256` — контрольная сумма exe;
//! - `{base}/v{ver}/domini-vpn-monitor.exe` (с 0.5.4; в релизах есть и `vpn-monitor.exe` — для обновлятелей
//!   версий до 0.5.4, которые знают только это имя; у релизов до 0.5.4 — только `vpn-monitor.exe`).

use crate::settings::ExeInfo;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const EXE_ASSET: &str = "domini-vpn-monitor.exe";
/// Имя exe в релизах до 0.5.4 — запасное: откат на старую версию.
pub const LEGACY_EXE_ASSET: &str = "vpn-monitor.exe";

pub fn latest_version_url(base: &str) -> String {
    format!("{}/latest/latest.version", base.trim_end_matches('/'))
}

fn asset_url(base: &str, ver: &str, name: &str) -> String {
    format!("{}/v{ver}/{name}", base.trim_end_matches('/'))
}

/// `0.1.2` (с `v` или без) → нормализованная строка; иначе ошибка.
pub fn parse_version(s: &str) -> Result<String, String> {
    let v = s
        .trim()
        .trim_start_matches('\u{feff}')
        .trim_start_matches('v');
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        Ok(v.to_string())
    } else {
        Err(format!(
            "latest.version: не версия: {:?}",
            s.chars().take(40).collect::<String>()
        ))
    }
}

/// Нужно ли ставить `latest` вместо `current` (новее или откат).
pub fn needs_update(current: &str, latest: &str) -> bool {
    current.trim_start_matches('v') != latest.trim_start_matches('v')
}

fn get_text(url: &str) -> Result<String, String> {
    let agent = crate::net::agent(Duration::from_secs(30));
    let mut resp = agent
        .get(url)
        .header("Cache-Control", "no-cache")
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let st = resp.status().as_u16();
    if st != 200 {
        return Err(format!("{url}: HTTP {st}"));
    }
    resp.body_mut()
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))
}

pub fn fetch_latest_version(base: &str) -> Result<String, String> {
    parse_version(&get_text(&latest_version_url(base))?)
}

/// Адрес последней версии канала: `latest` (стабильный) — `{base}/latest/latest.version`, `beta` —
/// `{base}/beta/latest.version` (v0.5).
pub fn channel_version_url(base: &str, channel: &str) -> String {
    let ch = if channel == "beta" { "beta" } else { "latest" };
    format!("{}/{ch}/latest.version", base.trim_end_matches('/'))
}

/// Последняя версия канала; если бета-канала ещё нет — стабильная.
pub fn fetch_channel_version(base: &str, channel: &str) -> Result<String, String> {
    if channel == "beta"
        && let Ok(t) = get_text(&channel_version_url(base, "beta"))
        && let Ok(v) = parse_version(&t)
    {
        return Ok(v);
    }
    fetch_latest_version(base)
}

/// Настройки версии `ver` (сырой текст и разобранные).
/// Настройки версии `ver`: сырой текст и данные exe. Целиком их здесь не разбираем — это схема другой версии;
/// нужен только блок `exe` (контракт: все JSON обратно совместимы, ADR 0004).
pub fn fetch_settings(base: &str, ver: &str) -> Result<(String, ExeInfo), String> {
    let text = get_text(&asset_url(base, ver, "settings.json"))?;
    let exe = exe_info(&text)?;
    Ok((text, exe))
}

pub fn exe_info(settings_text: &str) -> Result<ExeInfo, String> {
    let v: serde_json::Value =
        serde_json::from_str(settings_text).map_err(|e| format!("settings.json: {e}"))?;
    serde_json::from_value::<ExeInfo>(
        v.get("exe")
            .cloned()
            .ok_or("в settings.json релиза нет блока exe")?,
    )
    .map_err(|e| format!("settings.json, блок exe: {e}"))
}

/// Качает exe версии `ver` в `dest` через `dest.part`, сверяя SHA-256. Возвращает размер.
pub fn download_exe(
    base: &str,
    ver: &str,
    expected_sha256: &str,
    dest: &Path,
) -> Result<u64, String> {
    let agent = crate::net::agent(Duration::from_secs(600));
    let mut url = asset_url(base, ver, EXE_ASSET);
    let mut resp = agent.get(&url).call().map_err(|e| format!("{url}: {e}"))?;
    if resp.status().as_u16() == 404 {
        url = asset_url(base, ver, LEGACY_EXE_ASSET);
        resp = agent.get(&url).call().map_err(|e| format!("{url}: {e}"))?;
    }
    let st = resp.status().as_u16();
    if st != 200 {
        return Err(format!("{url}: HTTP {st}"));
    }
    let part: PathBuf = dest.with_extension("exe.part");
    let mut f = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("{url}: обрыв после {total} байт: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        f.write_all(&buf[..n])
            .map_err(|e| format!("{}: {e}", part.display()))?;
        total += n as u64;
    }
    drop(f);
    let got = hex(&hasher.finalize());
    if !got.eq_ignore_ascii_case(expected_sha256.trim()) {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "SHA-256 не совпал: ждали {expected_sha256}, получили {got}"
        ));
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(&part, dest)
        .map_err(|e| format!("{} → {}: {e}", part.display(), dest.display()))?;
    Ok(total)
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Имя exe версии в папке установки.
pub fn exe_name(ver: &str) -> String {
    format!("vpn-monitor-{ver}.exe")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.1.2\n").unwrap(), "0.1.2");
        assert_eq!(parse_version("\u{feff}1.0.0").unwrap(), "1.0.0");
        assert!(parse_version("latest").is_err());
        assert!(parse_version("<html>").is_err());
        assert!(needs_update("0.2.0", "0.1.0"), "откат тоже обновление");
        assert!(needs_update("0.1.0", "0.2.0"));
        assert!(!needs_update("0.1.0", "v0.1.0"));
    }

    #[test]
    fn exe_info_ignores_unknown_settings() {
        // настройки будущей версии с незнакомыми полями и значениями — блок exe всё равно читается
        let text = r#"{"schema": 1, "targets": [{"auth": "kerberos"}], "new_field": {"x": 1},
            "exe": {"version": "9.9.9", "sha256": "ab", "crc32": "01", "size": 3, "future": true}}"#;
        let e = exe_info(text).unwrap();
        assert_eq!(e.version, "9.9.9");
        assert!(exe_info(r#"{"schema": 1}"#).is_err());
    }

    #[test]
    fn channel_urls() {
        let b = "https://github.com/o/r/releases/download";
        assert_eq!(
            channel_version_url(b, "beta"),
            format!("{b}/beta/latest.version")
        );
        assert_eq!(
            channel_version_url(b, "latest"),
            format!("{b}/latest/latest.version")
        );
        assert_eq!(
            channel_version_url(b, "что-то"),
            format!("{b}/latest/latest.version")
        );
    }

    #[test]
    fn urls() {
        let b = "https://github.com/o/r/releases/download/";
        assert_eq!(
            latest_version_url(b),
            "https://github.com/o/r/releases/download/latest/latest.version"
        );
        assert_eq!(
            asset_url(b, "0.1.0", EXE_ASSET),
            "https://github.com/o/r/releases/download/v0.1.0/domini-vpn-monitor.exe"
        );
    }
}
