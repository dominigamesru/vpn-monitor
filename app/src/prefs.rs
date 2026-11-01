//! Выбор сотрудника на этой машине (`user.json` в папке установки (до 0.5.3 — `prefs.json`)): вид меню, канал обновлений, пауза,
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
    /// «Заявка в поддержку сменила статус» (v0.8).
    #[serde(default = "d_true")]
    pub tickets: bool,
}

impl Default for Notify {
    fn default() -> Self {
        Self {
            vpn: true,
            auth: true,
            update: true,
            slow: false,
            tickets: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefs {
    /// Вид меню трея: `route` (по умолчанию), `dash`, `strip` (до v0.6 — ещё `gauge`, `panel`).
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
    /// Тема меню у иконки и окна: `""` — как в системе, `light`, `dark` (v0.6).
    #[serde(default)]
    pub theme: String,
    /// Тема окна в браузере, отдельно от меню (v0.7.1): нет поля — как `theme` (до 0.7.1 тема была общей);
    /// `auto` — как в системе, `light`, `dark`.
    #[serde(default)]
    pub window_theme: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            view: d_view(),
            channel: d_channel(),
            pause_until_ms: None,
            notify: Notify::default(),
            last_seen_version: None,
            theme: String::new(),
            window_theme: None,
        }
    }
}

/// Виды меню. v0.6: `route`, `dash` (приборная панель), `strip`; `gauge` и `panel` — старые, страница показывает
/// их как `dash` (файл может прийти от прошлой версии или откатиться к ней).
pub const VIEWS: &[&str] = &["route", "dash", "strip", "gauge", "panel"];
pub const THEMES: &[&str] = &["", "light", "dark"];
/// Темы окна: `auto` — как в системе (пустая строка занята смыслом «как у меню»).
pub const WINDOW_THEMES: &[&str] = &["auto", "light", "dark"];
pub const CHANNELS: &[&str] = &["latest", "beta"];

fn path(dir: &Path) -> PathBuf {
    dir.join("user.json")
}

/// Имя файла до 0.5.3: при первом запуске переименовываем в `user.json`, выбор сотрудника не теряется.
const LEGACY: &str = "prefs.json";

impl Prefs {
    pub fn load(dir: &Path) -> Self {
        let legacy = dir.join(LEGACY);
        if !path(dir).exists() && legacy.exists() {
            let _ = std::fs::rename(&legacy, path(dir));
        }
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
        assert_eq!(p.window_theme, None); // до 0.7.1 — окно как меню
        let p: Prefs = serde_json::from_str(r#"{"theme":"dark","window_theme":"light"}"#).unwrap();
        assert_eq!(
            (p.theme.as_str(), p.window_theme.as_deref()),
            ("dark", Some("light"))
        );
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

    #[test]
    fn legacy_prefs_json_becomes_user_json() {
        let dir = std::env::temp_dir().join(format!("vpnmon-prefs-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(dir.join("user.json"));
        std::fs::write(
            dir.join("prefs.json"),
            r#"{"view":"strip","channel":"beta"}"#,
        )
        .unwrap();
        let p = Prefs::load(&dir);
        assert_eq!(p.view, "strip");
        assert_eq!(p.channel, "beta");
        assert!(dir.join("user.json").exists() && !dir.join("prefs.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
