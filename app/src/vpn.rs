//! Клиент и профиль VPN, «Без ВПН» (R3.3, R3.6): маршрут до цели → сетевой адаптер → клиент (правила «адаптер /
//! процесс → клиент» из настроек) → профиль (чтение конфигов клиента, только имя). Tailscale, WSL и т. п. — не VPN в
//! офис (`ignore_adapters`).

use crate::settings::VpnClientRule;
use crate::{info, warn};
use serde::Serialize;
use std::net::IpAddr;
use std::path::PathBuf;

/// Через какой адаптер идёт трафик до цели.
#[derive(Debug, Clone, Serialize)]
pub struct Route {
    pub adapter: String,
    pub description: String,
    /// Адаптер — VPN в офис (по правилам клиентов или похож на туннель и не в списке исключений).
    pub via_vpn: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Client {
    /// «Happ», «OpenVPN Connect»…
    pub name: String,
    /// Профиль (подключение внутри клиента), если удалось прочитать.
    pub profile: Option<String>,
    pub adapter: String,
}

fn matches_any(text: &str, patterns: &[String]) -> bool {
    let t = text.to_lowercase();
    patterns
        .iter()
        .any(|p| !p.is_empty() && t.contains(&p.to_lowercase()))
}

impl Client {
    /// Ключ нормы и подпись: «Happ / Lisbon» или «Happ».
    pub fn key(&self) -> String {
        match &self.profile {
            Some(p) => format!("{} / {p}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Маршрут до адреса: адаптер и «через VPN ли».
pub fn route(ip: &IpAddr, rules: &[VpnClientRule], ignore: &[String]) -> Option<Route> {
    let (name, description, vpn_like) = platform::route(ip)?;
    let both = format!("{name} {description}");
    let by_rule = rules.iter().any(|r| matches_any(&both, &r.adapters));
    let via_vpn = by_rule || (vpn_like && !matches_any(&both, ignore));
    Some(Route {
        adapter: name,
        description,
        via_vpn,
    })
}

/// Какой клиент поднял туннель на `adapter`: сначала по адаптеру, при нескольких подходящих — по запущенным процессам.
pub fn detect(
    adapter: &str,
    description: &str,
    rules: &[VpnClientRule],
    processes: &[String],
) -> Option<Client> {
    let both = format!("{adapter} {description}");
    let by_adapter: Vec<&VpnClientRule> = rules
        .iter()
        .filter(|r| matches_any(&both, &r.adapters))
        .collect();
    let running = |r: &VpnClientRule| {
        r.processes
            .iter()
            .any(|p| processes.iter().any(|q| q.eq_ignore_ascii_case(p)))
    };
    let rule = match by_adapter.len() {
        0 => rules.iter().find(|r| running(r))?,
        1 => by_adapter[0],
        _ => by_adapter
            .iter()
            .copied()
            .find(|r| running(r))
            .unwrap_or(by_adapter[0]),
    };
    Some(Client {
        name: rule.name.clone(),
        profile: profile(&rule.name),
        adapter: adapter.into(),
    })
}

pub fn processes() -> Vec<String> {
    platform::processes()
}

fn env_path(var: &str, rest: &str) -> Option<PathBuf> {
    std::env::var(var).ok().map(|v| PathBuf::from(v).join(rest))
}

/// Имя профиля — только то, что безопасно показать: имя файла или подпись сервера (после `#` в ссылке).
fn profile(client: &str) -> Option<String> {
    let lower = client.to_lowercase();
    if lower.contains("happ") {
        return happ_profile();
    }
    let dir = if lower.contains("connect") {
        env_path("APPDATA", "OpenVPN Connect\\profiles")
    } else if lower.contains("openvpn") {
        env_path("USERPROFILE", "OpenVPN\\config")
    } else {
        None
    }?;
    let names: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_str()?.to_lowercase();
            matches!(ext.as_str(), "ovpn" | "json")
                .then(|| p.file_stem()?.to_str().map(str::to_string))
                .flatten()
        })
        .collect();
    (names.len() == 1).then(|| names[0].clone())
}

static HAPP_KEYS_LOGGED: std::sync::Once = std::sync::Once::new();

fn happ_profile() -> Option<String> {
    let path = env_path("LOCALAPPDATA", "Happ\\config.json")?;
    let text = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    // формат конфига Happ не документирован: один раз пишем в лог имена полей верхнего уровня (без значений),
    // чтобы уточнить чтение профиля
    HAPP_KEYS_LOGGED.call_once(|| {
        let keys: Vec<&str> = v
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        info!("vpn", "Happ config.json: поля {keys:?}");
    });
    for key in [
        "selectedServerName",
        "currentServerName",
        "selectedServer",
        "currentServer",
        "selected",
        "remarks",
        "name",
    ] {
        if let Some(safe) = v.get(key).and_then(|x| x.as_str()).and_then(safe_label) {
            return Some(safe);
        }
    }
    None
}

/// Подпись, которую можно показать: из ссылки `vless://…#Подпись` — только подпись; ссылку целиком — никогда
/// (в ней ключи). Длинное — обрезается.
pub fn safe_label(s: &str) -> Option<String> {
    let s = s.trim();
    let label = if s.contains("://") {
        let frag = s.rsplit_once('#')?.1;
        percent_decode(frag)
    } else {
        s.to_string()
    };
    let label = label.trim().to_string();
    if label.is_empty() || label.len() > 80 || label.contains("://") {
        if label.len() > 80 {
            warn!("vpn", "подпись профиля длиннее 80 символов — не показываю");
        }
        return None;
    }
    Some(label)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(windows)]
mod platform {
    use std::net::IpAddr;

    pub fn route(ip: &IpAddr) -> Option<(String, String, bool)> {
        let IpAddr::V4(v4) = ip else { return None };
        let idx = crate::diag::win::best_interface(*v4)?;
        crate::diag::win::adapters()
            .into_iter()
            .find(|(i, _)| *i == idx)
            .map(|(_, a)| (a.name, a.description, a.vpn_like))
    }

    pub fn processes() -> Vec<String> {
        crate::diag::win::processes()
    }
}

#[cfg(not(windows))]
mod platform {
    use std::net::IpAddr;
    pub fn route(_ip: &IpAddr) -> Option<(String, String, bool)> {
        None
    }
    pub fn processes() -> Vec<String> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<VpnClientRule> {
        crate::settings::Settings::embedded().vpn_clients
    }

    #[test]
    fn detects_clients_by_adapter_and_process() {
        let r = rules();
        let c = detect("happ-tun", "sing-tun", &r, &[]).unwrap();
        assert_eq!(c.name, "Happ");
        let c = detect(
            "Ethernet 3",
            "TAP-Windows Adapter V9 for OpenVPN Connect",
            &r,
            &[],
        )
        .unwrap();
        assert_eq!(c.name, "OpenVPN Connect");
        // DCO у обоих OpenVPN — решает процесс
        let c = detect(
            "OpenVPN Data Channel Offload",
            "OpenVPN Data Channel Offload",
            &r,
            &["openvpn-gui.exe".into()],
        )
        .unwrap();
        assert_eq!(c.name, "OpenVPN GUI");
        assert!(detect("Wi-Fi", "Intel Wi-Fi 6", &r, &[]).is_none());
    }

    #[test]
    fn safe_label_never_leaks_links() {
        assert_eq!(
            safe_label("vless://uuid@host:443?x=1#Lisbon%20VPN").as_deref(),
            Some("Lisbon VPN")
        );
        assert_eq!(safe_label("vless://uuid@host:443?x=1"), None);
        assert_eq!(safe_label("Офис Москва").as_deref(), Some("Офис Москва"));
    }
}
