//! Выбор сотрудника на этой машине (`prefs.json` в папке установки): вид меню, канал обновлений, пауза,
//! уведомления, последняя увиденная версия. JSON обратно совместим: все поля с умолчаниями.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn d_view() -> String {
    "route".into()
}
fn d_channel() -> String {
    "latest".into()
}
fn d_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notify {
    /// «VPN выключился» / «VPN снова работает».
    #[serde(default = "d_true")]
    pub vpn: bool,
    /// «Пароль AD не подошёл».
    #[serde(default = "d_true")]
    pub auth: bool,
    /// «Обновлено до X».
    #[serde(default = "d_true")]
    pub update: bool,
    /// «Медленнее обычного» — по желанию.
    #[serde(default)]
    pub slow: bool,
}

impl Default for Notify {
    fn default() -> Self {
        Self {
            vpn: true,
            auth: true,
            update: true,
            slow: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefs {
    /// Вид меню трея: `route` (по умолчанию), `gauge`, `strip`, `panel`.
    #[serde(default = "d_view")]
    pub view: String,
    /// Канал обновлений: `latest` (стабильный) или `beta`.
    #[serde(default = "d_channel")]
    pub channel: String,
    /// Пауза проверок до этого момента (мс UNIX).
    #[serde(default)]
    pub pause_until_ms: Option<i64>,
    #[serde(default)]
    pub notify: Notify,
    /// Версия, «что нового» которой сотрудник уже видел.
    #[serde(default)]
    pub last_seen_version: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            view: d_view(),
            channel: d_channel(),
            pause_until_ms: None,
            notify: Notify::default(),
            last_seen_version: None,
        }
    }
}

pub const VIEWS: &[&str] = &["route", "gauge", "strip", "panel"];
pub const CHANNELS: &[&str] = &["latest", "beta"];

fn path(dir: &Path) -> PathBuf {
    dir.join("prefs.json")
}

impl Prefs {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(path(dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let p = path(dir);
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// Пауза действует сейчас.
    pub fn paused(&self, now_ms: i64) -> bool {
        self.pause_until_ms.is_some_and(|u| u > now_ms)
    }
}

/// Конец паузы: `minutes` от сейчас или до конца местного дня (`minutes` = 0).
pub fn pause_until(now_ms: i64, minutes: i64) -> i64 {
    if minutes > 0 {
        now_ms + minutes * 60_000
    } else {
        crate::metrics::local_day_start(now_ms) + 86_400_000 - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_and_future_prefs_files_load() {
        let p: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p.view, "route");
        assert_eq!(p.channel, "latest");
        assert!(p.notify.vpn && !p.notify.slow);
        let p: Prefs =
            serde_json::from_str(r#"{"view":"gauge","future":1,"notify":{"vpn":false,"x":2}}"#)
                .unwrap();
        assert_eq!(p.view, "gauge");
        assert!(!p.notify.vpn && p.notify.auth);
    }

    #[test]
    fn pause_window() {
        let now = crate::metrics::now_ms();
        let p = Prefs {
            pause_until_ms: Some(pause_until(now, 30)),
            ..Prefs::default()
        };
        assert!(p.paused(now));
        assert!(!p.paused(now + 31 * 60_000));
        let eod = pause_until(now, 0);
        assert!(eod > now && eod - now <= 86_400_000);
    }
}
