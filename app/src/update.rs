//! Обновление из GitHub Releases ([ADR 0004]): `latest/latest.version` → версия, которую ставят все.
//! Сравнение на равенство: если последняя версия старше своей — это откат, тоже ставим.
//!
//! Раскладка (контракт со всеми установленными копиями):
//! - `{base}/latest/latest.version` — строка `0.1.2`;
//! - `{base}/v{ver}/settings.json` — настройки версии, в `exe.sha256` — контрольная сумма exe;
//! - `{base}/v{ver}/vpn-monitor.exe`.

use crate::settings::Settings;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const EXE_ASSET: &str = "vpn-monitor.exe";

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

/// Настройки версии `ver` (сырой текст и разобранные).
pub fn fetch_settings(base: &str, ver: &str) -> Result<(String, Settings), String> {
    let text = get_text(&asset_url(base, ver, "settings.json"))?;
    let s = Settings::parse(&text)?;
    Ok((text, s))
}

/// Качает exe версии `ver` в `dest` через `dest.part`, сверяя SHA-256. Возвращает размер.
pub fn download_exe(
    base: &str,
    ver: &str,
    expected_sha256: &str,
    dest: &Path,
) -> Result<u64, String> {
    let url = asset_url(base, ver, EXE_ASSET);
    let agent = crate::net::agent(Duration::from_secs(600));
    let mut resp = agent.get(&url).call().map_err(|e| format!("{url}: {e}"))?;
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
    fn urls() {
        let b = "https://github.com/o/r/releases/download/";
        assert_eq!(
            latest_version_url(b),
            "https://github.com/o/r/releases/download/latest/latest.version"
        );
        assert_eq!(
            asset_url(b, "0.1.0", EXE_ASSET),
            "https://github.com/o/r/releases/download/v0.1.0/vpn-monitor.exe"
        );
    }
}
