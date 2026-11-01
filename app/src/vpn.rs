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

/// Клиент VPN на этой машине — для сводки поддержке.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Installed {
    pub name: String,
    pub running: bool,
    /// Профиль — то же безопасное название, что в меню (без ключей и ссылок).
    pub profile: Option<String>,
}

/// Какие клиенты из `vpn_clients` установлены (по папкам программы и настроек) и запущены (по процессам).
pub fn installed(rules: &[VpnClientRule]) -> Vec<Installed> {
    let procs: Vec<String> = processes().iter().map(|p| p.to_lowercase()).collect();
    rules
        .iter()
        .filter_map(|r| {
            let lower = r.name.to_lowercase();
            let places: &[(&str, &str)] = if lower.contains("happ") {
                &[("LOCALAPPDATA", "Happ"), ("ProgramFiles", "Happ")]
            } else if lower.contains("connect") {
                &[
                    ("APPDATA", "OpenVPN Connect"),
                    ("ProgramFiles", "OpenVPN Connect"),
                ]
            } else if lower.contains("openvpn") {
                &[
                    ("ProgramFiles", "OpenVPN"),
                    ("USERPROFILE", "OpenVPN\\config"),
                ]
            } else if lower.contains("wireguard") {
                &[("ProgramFiles", "WireGuard")]
            } else {
                &[]
            };
            let running = r
                .processes
                .iter()
                .any(|p| procs.contains(&p.to_lowercase()));
            let present = places
                .iter()
                .any(|(var, rest)| env_path(var, rest).is_some_and(|p| p.exists()));
            (present || running).then(|| Installed {
                name: r.name.clone(),
                running,
                profile: profile(&r.name),
            })
        })
        .collect()
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
    if names.len() != 1 {
        return None;
    }
    let stem = names[0].clone();
    if !lower.contains("connect") {
        return Some(stem);
    }
    // OpenVPN Connect называет файлы профилей числом (внутренний id, «1780314832991»); человеческое название —
    // «vpn.dominigames.com [DG_Rykov_Sergey_main]» — лежит в его JSON. Формат не документирован: ищем объект с этим
    // id и берём поле с названием; не нашли — FRIENDLY_NAME или сервер из .ovpn; число не показываем.
    let base = env_path("APPDATA", "OpenVPN Connect")?;
    if let Some((title, file)) = connect_title(&base, &stem) {
        CONNECT_LOGGED.call_once(|| info!("vpn", "OpenVPN Connect: название профиля из {file}"));
        return Some(title);
    }
    let ovpn = std::fs::read_to_string(dir.join(format!("{stem}.ovpn"))).unwrap_or_default();
    if let Some(t) = title_from_ovpn(&ovpn) {
        CONNECT_LOGGED.call_once(|| info!("vpn", "OpenVPN Connect: название профиля из .ovpn"));
        return Some(t);
    }
    CONNECT_LOGGED.call_once(|| {
        info!(
            "vpn",
            "OpenVPN Connect: название профиля не найдено — показываю только клиента"
        )
    });
    (!looks_like_id(&stem)).then_some(stem)
}

static CONNECT_LOGGED: std::sync::Once = std::sync::Once::new();

/// Номер вместо названия (внутренний id профиля) — показывать незачем.
fn looks_like_id(s: &str) -> bool {
    s.len() >= 6 && s.chars().all(|c| c.is_ascii_digit())
}

fn connect_title(base: &std::path::Path, id: &str) -> Option<(String, String)> {
    let rd = std::fs::read_dir(base).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        if !text.contains(id) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(t) = title_in_json(&v, id) {
            let file = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            return Some((t, file));
        }
    }
    None
}

/// Объект, где встречается `id` (значением или ключом), и в нём поле с названием.
fn title_in_json(v: &serde_json::Value, id: &str) -> Option<String> {
    const NAME_KEYS: &[&str] = &[
        "name",
        "title",
        "profileName",
        "displayName",
        "friendlyName",
        "label",
    ];
    let pick = |o: &serde_json::Map<String, serde_json::Value>| {
        NAME_KEYS
            .iter()
            .filter_map(|k| o.get(*k).and_then(|x| x.as_str()))
            .find(|t| !looks_like_id(t))
            .and_then(safe_label)
    };
    match v {
        serde_json::Value::Object(o) => {
            if o.values().any(|x| x.as_str() == Some(id))
                && let Some(t) = pick(o)
            {
                return Some(t);
            }
            if let Some(serde_json::Value::Object(inner)) = o.get(id)
                && let Some(t) = pick(inner)
            {
                return Some(t);
            }
            o.values().find_map(|x| title_in_json(x, id))
        }
        serde_json::Value::Array(a) => a.iter().find_map(|x| title_in_json(x, id)),
        serde_json::Value::String(s)
            if s.contains(id) && s.trim_start().starts_with(['{', '[']) =>
        {
            // Electron-хранилища кладут JSON строкой внутри JSON
            serde_json::from_str::<serde_json::Value>(s)
                .ok()
                .and_then(|x| title_in_json(&x, id))
        }
        _ => None,
    }
}

/// Из .ovpn — только безопасные строки: `setenv FRIENDLY_NAME "…"` или имя сервера из `remote host port`.
fn title_from_ovpn(text: &str) -> Option<String> {
    for line in text.lines().map(str::trim) {
        let l = line.trim_start_matches(['#', ' ']);
        for prefix in ["setenv FRIENDLY_NAME", "OVPN_ACCESS_SERVER_FRIENDLY_NAME="] {
            if let Some(rest) = l.strip_prefix(prefix) {
                let t = rest.trim().trim_matches('"').trim();
                if !t.is_empty() {
                    return safe_label(t);
                }
            }
        }
    }
    text.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("remote "))
        .and_then(|r| r.split_whitespace().next())
        .filter(|h| {
            h.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        })
        .map(str::to_string)
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

    #[test]
    fn openvpn_connect_title_from_json_and_ovpn() {
        let id = "1780314832991";
        let v: serde_json::Value = serde_json::from_str(&format!(
            r#"{{"profiles":[{{"id":"{id}","name":"vpn.dominigames.com [DG_Rykov_Sergey_main]"}}]}}"#
        ))
        .unwrap();
        assert_eq!(
            title_in_json(&v, id).as_deref(),
            Some("vpn.dominigames.com [DG_Rykov_Sergey_main]")
        );
        // ключом и строкой с JSON внутри (Electron)
        let v: serde_json::Value = serde_json::from_str(&format!(
            r#"{{"store":"{{\"{id}\":{{\"title\":\"office [me]\"}}}}"}}"#
        ))
        .unwrap();
        assert_eq!(title_in_json(&v, id).as_deref(), Some("office [me]"));
        assert_eq!(
            title_from_ovpn("client\nremote vpn.dominigames.com 1194 udp\n<key>\nsecret\n</key>")
                .as_deref(),
            Some("vpn.dominigames.com")
        );
        assert_eq!(
            title_from_ovpn("setenv FRIENDLY_NAME \"DG main\"\nremote x 1").as_deref(),
            Some("DG main")
        );
        assert!(looks_like_id(id) && !looks_like_id("Lisbon"));
    }
}
