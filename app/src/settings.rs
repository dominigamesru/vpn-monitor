//! Настройки (`settings.json`): цели, интервалы, пороги. Едут в каждом релизе, секретов не содержат.
//!
//! Откуда берутся: `settings.json` в папке установки (скачан вместе с exe этой версии) → встроенные в exe
//! (`app/config/settings.json` на момент сборки).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Встроенные настройки — те же, что публикуются в релизе.
pub const EMBEDDED: &str = include_str!("../config/settings.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub schema: u32,
    pub release_base_url: String,
    pub install_dir: String,
    pub check_interval_green_sec: u64,
    pub check_interval_alert_sec: u64,
    pub full_probe_interval_min: u64,
    pub internet_interval_min: u64,
    pub update_interval_min: u64,
    pub thresholds: Thresholds,
    pub targets: Vec<Target>,
    pub internet: Internet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gitlab_oauth: Option<GitlabOauth>,
    /// Клиенты VPN в офис и как их узнать (v0.4).
    #[serde(default)]
    pub vpn_clients: Vec<VpnClientRule>,
    /// Адаптеры-туннели, которые не VPN в офис (Tailscale, WSL…).
    #[serde(default)]
    pub ignore_adapters: Vec<String>,
    /// Пишет CI при релизе: контрольные суммы exe этой версии.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<ExeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    pub latency_good_ms: f64,
    pub latency_bad_ms: f64,
    pub speed_good_mbps: f64,
    pub speed_bad_mbps: f64,
    pub stability_window_min: u64,
    pub score_green: u32,
    pub score_yellow: u32,
    pub internet_weak_mbps: f64,
    // --- v0.4: оценка относительно своей нормы (медиана за `baseline_days`) ---
    #[serde(default = "d_baseline_days")]
    pub baseline_days: u64,
    #[serde(default = "d_baseline_min")]
    pub baseline_min_samples: usize,
    /// Задержка во столько раз выше нормы — ещё «хорошо» / уже «плохо».
    #[serde(default = "d_lat_ok")]
    pub latency_ratio_ok: f64,
    #[serde(default = "d_lat_bad")]
    pub latency_ratio_bad: f64,
    /// Скорость — такая доля нормы ещё «хорошо» / уже «плохо».
    #[serde(default = "d_spd_ok")]
    pub speed_ratio_ok: f64,
    #[serde(default = "d_spd_bad")]
    pub speed_ratio_bad: f64,
}

fn d_baseline_days() -> u64 {
    7
}
fn d_baseline_min() -> usize {
    60
}
fn d_lat_ok() -> f64 {
    1.3
}
fn d_lat_bad() -> f64 {
    3.0
}
fn d_spd_ok() -> f64 {
    0.75
}
fn d_spd_bad() -> f64 {
    0.3
}
fn d_true() -> bool {
    true
}

/// Правило «адаптер / процесс → клиент VPN» (R3.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnClientRule {
    pub name: String,
    /// Подстроки имени или описания адаптера (без учёта регистра).
    #[serde(default)]
    pub adapters: Vec<String>,
    /// Имена exe клиента.
    #[serde(default)]
    pub processes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// По цели считается светофор (доступность + задержка).
    Light,
    /// Цель для метрик скорости.
    Speed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    None,
    Confluence,
    Svn,
    Gitlab,
    Jira,
    /// Значение из будущих версий — читаем как «без входа», а не падаем (совместимость, ADR 0004).
    #[serde(other)]
    Unknown,
}

/// Значения `auth`, которые понимают все выпущенные версии (v0.1–v0.2). В `settings.json` поле `auth` — только из
/// этого списка; новые виды входа — в `auth_kind` (старые версии его не читают, но и не падают).
pub const LEGACY_AUTH: &[&str] = &["none", "confluence", "svn", "gitlab"];

/// OAuth-приложение GitLab для входа паролем AD (ADR 0006). Пусто — GitLab только по `GITLAB_TOKEN`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitlabOauth {
    /// Адрес GitLab, например `https://gitlab.dominigames.com`.
    pub url: String,
    /// Application ID (не секрет).
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub url: String,
    /// Имя переменной (env или secrets.env), которая переопределяет `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_env: Option<String>,
    /// Большой файл для полной пробы (реже, см. `full_probe_interval_min`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_url_env: Option<String>,
    /// Вид входа, понятный и старым версиям (см. `LEGACY_AUTH`).
    pub auth: AuthKind,
    /// Вид входа для версий v0.3+; если задан — главнее `auth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_kind: Option<AuthKind>,
    pub timeout_sec: u64,
    /// `false` — цель на паузе: не проверяется (v0.4; старые версии поле не читают и проверяют).
    #[serde(default = "d_true")]
    pub enabled: bool,
}

impl Target {
    /// Действующий вид входа.
    pub fn kind(&self) -> AuthKind {
        self.auth_kind.unwrap_or(self.auth)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Internet {
    pub name: String,
    pub url: String,
    pub max_seconds: u64,
    pub timeout_sec: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExeInfo {
    pub version: String,
    pub sha256: String,
    #[serde(default)]
    pub crc32: String,
    #[serde(default)]
    pub size: u64,
}

impl Settings {
    pub fn parse(text: &str) -> Result<Self, String> {
        let s: Settings = serde_json::from_str(text).map_err(|e| format!("settings.json: {e}"))?;
        s.validate()?;
        Ok(s)
    }

    pub fn embedded() -> Self {
        Self::parse(EMBEDDED).expect("встроенные настройки валидны (проверяет тест)")
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("settings.json: неизвестная schema {}", self.schema));
        }
        if self.targets.is_empty() {
            return Err("settings.json: нет целей".into());
        }
        if !self.targets.iter().any(|t| t.role == Role::Light) {
            return Err(
                "settings.json: нет цели с role=light — светофор не по чему считать".into(),
            );
        }
        if self.check_interval_alert_sec < 10 || self.check_interval_green_sec < 10 {
            return Err("settings.json: интервал проверки меньше 10 с".into());
        }
        Ok(())
    }

    /// Загружает настройки из папки установки, иначе встроенные. Возвращает и источник для лога.
    pub fn load(dir: &Path) -> (Self, String) {
        let path = dir.join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => match Self::parse(&text) {
                Ok(s) => (s, path.display().to_string()),
                Err(e) => (
                    Self::embedded(),
                    format!("встроенные (ошибка в {}: {e})", path.display()),
                ),
            },
            Err(_) => (Self::embedded(), "встроенные".into()),
        }
    }

    /// Папка установки с раскрытыми переменными `%VAR%`.
    pub fn install_dir(&self) -> PathBuf {
        PathBuf::from(expand_env(&self.install_dir))
    }
}

/// Раскрывает `%VAR%` из окружения; неизвестные остаются как есть.
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_is_valid() {
        let s = Settings::embedded();
        assert!(
            s.targets
                .iter()
                .any(|t| t.id == "confluence" && t.role == Role::Light)
        );
        assert!(s.release_base_url.starts_with("https://github.com/"));
    }

    /// Контракт с установленными копиями (ADR 0004): старый обновлятель (v0.2) разбирает `settings.json` новой
    /// версии своей схемой. Поле `auth` — только из значений, которые он знает; роли — light/speed.
    #[test]
    fn settings_json_readable_by_v0_2() {
        let v: serde_json::Value = serde_json::from_str(EMBEDDED).unwrap();
        for t in v["targets"].as_array().unwrap() {
            let auth = t["auth"].as_str().unwrap();
            assert!(
                LEGACY_AUTH.contains(&auth),
                "auth={auth} не поймёт v0.2 — новый вид входа кладите в auth_kind"
            );
            assert!(matches!(t["role"].as_str(), Some("light" | "speed")));
        }
        assert_eq!(v["schema"], 1);
    }

    #[test]
    fn unknown_auth_kind_does_not_break_parsing() {
        let text = EMBEDDED.replace("\"auth_kind\": \"jira\"", "\"auth_kind\": \"kerberos\"");
        let s = Settings::parse(&text).unwrap();
        assert!(s.targets.iter().any(|t| t.kind() == AuthKind::Unknown));
    }

    #[test]
    fn rejects_unknown_schema() {
        let text = EMBEDDED.replace("\"schema\": 1", "\"schema\": 2");
        assert!(Settings::parse(&text).is_err());
    }

    #[test]
    fn expands_env() {
        unsafe { std::env::set_var("VPNMON_TEST_DIR", "C:\\Users\\x") };
        assert_eq!(expand_env("%VPNMON_TEST_DIR%\\A"), "C:\\Users\\x\\A");
        assert_eq!(expand_env("%NO_SUCH_VAR_123%\\A"), "%NO_SUCH_VAR_123%\\A");
        assert_eq!(expand_env("50%"), "50%");
    }
}
