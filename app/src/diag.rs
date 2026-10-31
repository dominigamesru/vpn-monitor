//! Диагностика подключения (вкладка «Подключение»): по каждой цели — все адреса DNS, подставной ли адрес, через
//! какой сетевой адаптер идёт маршрут, TCP, HTTP (код, кто вошёл, куда перенаправило), вывод и подсказка; плюс
//! окружение — адаптеры, прокси системы и окружения, какие секреты заданы. Секретов в выводе нет.

use crate::net;
use crate::probe::target_url;
use crate::secrets::{KNOWN_KEYS, Secrets};
use crate::settings::{AuthKind, Settings};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize)]
pub struct Step {
    pub name: String,
    /// `ok` / `warn` / `fail` / `skip`.
    pub status: &'static str,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct TargetDiag {
    pub id: String,
    pub name: String,
    pub url: String,
    pub auth: String,
    pub steps: Vec<Step>,
    /// Итог одной строкой и что сделать.
    pub verdict: &'static str,
    pub hint: String,
}

#[derive(Debug, Serialize)]
pub struct Adapter {
    pub name: String,
    pub description: String,
    pub up: bool,
    pub ipv4: Vec<String>,
    /// Похож на туннель VPN (tun/tap/openvpn/wireguard/happ…).
    pub vpn_like: bool,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub at_ms: i64,
    pub targets: Vec<TargetDiag>,
    pub adapters: Vec<Adapter>,
    pub system_proxy: String,
    pub env_proxy: String,
    /// Ключ → откуда задан (`окружение` / `secrets.env` / `AD`), `None` — пусто.
    pub secrets: Vec<(String, Option<String>)>,
    pub ad_login: Option<String>,
    pub took_ms: u64,
}

fn step(name: &str, status: &'static str, detail: impl Into<String>) -> Step {
    Step {
        name: name.into(),
        status,
        detail: detail.into(),
    }
}

fn is_fake(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            o == [1, 2, 3, 4] || (o[0] == 198 && (o[1] & 0xfe) == 18)
        }
        _ => false,
    }
}

const VPN_WORDS: &[&str] = &[
    "tun",
    "tap",
    "openvpn",
    "wireguard",
    "wintun",
    "happ",
    "vpn",
    "sing",
    "xray",
    "tailscale",
    "zerotier",
];

pub fn vpn_like(name: &str, desc: &str) -> bool {
    let s = format!("{name} {desc}").to_lowercase();
    VPN_WORDS.iter().any(|w| s.contains(w))
}

/// Полная диагностика. Долгая (до ~30 с при недоступных целях) — звать из фонового потока.
pub fn run(settings: &Settings, secrets: &Secrets) -> Report {
    let t0 = Instant::now();
    let adapters = adapters();
    let mut targets = Vec::new();
    std::thread::scope(|sc| {
        let mut hs = Vec::new();
        for t in &settings.targets {
            for full in [false, true] {
                if full && t.full_url.is_none() {
                    continue;
                }
                let Some(url) = target_url(t, secrets, full) else {
                    continue;
                };
                let name = if full {
                    format!("{} (большой файл)", t.name)
                } else {
                    t.name.clone()
                };
                let (id, auth) = (t.id.clone(), t.kind());
                let adapters = &adapters;
                hs.push(sc.spawn(move || diag_url(&id, &name, &url, auth, secrets, adapters)));
            }
        }
        let i = &settings.internet;
        let adapters = &adapters;
        hs.push(sc.spawn(move || {
            diag_url(
                "internet",
                &i.name,
                &i.url,
                AuthKind::None,
                secrets,
                adapters,
            )
        }));
        for h in hs {
            if let Ok(d) = h.join() {
                targets.push(d);
            }
        }
    });
    let env_proxy = [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok().map(|v| format!("{k}={v}")))
    .collect::<Vec<_>>()
    .join(" ");
    Report {
        at_ms: crate::metrics::now_ms(),
        targets,
        adapters,
        system_proxy: system_proxy(),
        env_proxy: if env_proxy.is_empty() {
            "нет".into()
        } else {
            env_proxy
        },
        secrets: KNOWN_KEYS
            .iter()
            .map(|k| (k.to_string(), secrets.source(k).map(str::to_string)))
            .collect(),
        ad_login: secrets.ad_login().map(str::to_string),
        took_ms: t0.elapsed().as_millis() as u64,
    }
}

fn diag_url(
    id: &str,
    name: &str,
    url: &str,
    auth: AuthKind,
    secrets: &Secrets,
    adapters: &[Adapter],
) -> TargetDiag {
    let a = crate::probe::auth(auth, secrets);
    let (headers, auth_name, missing) = (a.headers, a.label, a.missing);
    let mut d = TargetDiag {
        id: id.into(),
        name: name.into(),
        url: url.into(),
        auth: auth_name.clone(),
        steps: vec![],
        verdict: "fail",
        hint: String::new(),
    };
    if let Some(m) = &missing {
        d.steps.push(step("Креды", "warn", m.clone()));
    } else if auth != AuthKind::None {
        d.steps
            .push(step("Креды", "ok", format!("заданы, вход: {auth_name}")));
    }

    let uri: ureq::http::Uri = match url.parse() {
        Ok(u) => u,
        Err(e) => {
            d.steps
                .push(step("Адрес", "fail", format!("плохой адрес: {e}")));
            d.hint = "Проверьте адрес в настройках или в secrets.env.".into();
            return d;
        }
    };
    let host = uri.host().unwrap_or_default().to_string();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("http") {
            80
        } else {
            443
        });

    // DNS
    let t = Instant::now();
    let addrs: Vec<SocketAddr> = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(e) => {
            d.steps.push(step("DNS", "fail", format!("{host}: {e}")));
            d.hint = "Имя не разрешается: VPN выключен или его DNS не отвечает. Включите VPN и повторите.".into();
            return d;
        }
    };
    let ips: Vec<String> = addrs.iter().map(|a| a.ip().to_string()).collect();
    let fake = addrs.iter().any(|a| is_fake(&a.ip()));
    d.steps.push(step(
        "DNS",
        if fake { "warn" } else { "ok" },
        format!(
            "{host} → {} за {} мс{}",
            ips.join(", "),
            t.elapsed().as_millis(),
            if fake {
                " — подставной адрес клиента VPN (Happ / fake-ip)"
            } else {
                ""
            }
        ),
    ));
    let addr = *addrs.iter().find(|a| a.is_ipv4()).unwrap_or(&addrs[0]);

    // Маршрут
    match route_adapter(&addr.ip(), adapters) {
        Some((a, vpn)) => d.steps.push(step(
            "Маршрут",
            "ok",
            format!(
                "через адаптер «{a}»{}",
                if vpn {
                    " (похож на VPN)"
                } else {
                    " (не VPN — трафик идёт напрямую)"
                }
            ),
        )),
        None => d.steps.push(step("Маршрут", "skip", "не определён")),
    }

    // TCP
    let t = Instant::now();
    match TcpStream::connect_timeout(&addr, Duration::from_secs(8)) {
        Ok(_) => {
            let ms = t.elapsed().as_millis();
            d.steps.push(step(
                "TCP",
                if ms > 500 { "warn" } else { "ok" },
                format!("{addr} за {ms} мс"),
            ));
        }
        Err(e) => {
            d.steps.push(step("TCP", "fail", format!("{addr}: {e}")));
            d.hint = if fake {
                "Адрес подставной, но туннель не пропускает трафик: перезапустите клиент VPN или проверьте, что \
                 этот сервис входит в его правила."
                    .into()
            } else {
                "Соединение не устанавливается: VPN не пропускает трафик до сервиса или сервис недоступен.".into()
            };
            return d;
        }
    }

    // HTTP
    let agent = net::agent(Duration::from_secs(20));
    let mut req = agent.get(url);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let t = Instant::now();
    match req.call() {
        Ok(resp) => {
            let ms = t.elapsed().as_millis();
            let status = resp.status().as_u16();
            let final_uri = ureq::ResponseExt::get_uri(&resp).to_string();
            let h = |n: &str| {
                resp.headers()
                    .get(n)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            };
            let who = h("X-AUSERNAME").filter(|u| u != "anonymous");
            let mut info = format!("HTTP {status} за {ms} мс");
            if final_uri != url {
                info += &format!(", перенаправило на {final_uri}");
            }
            if let Some(ct) = h("Content-Type") {
                info += &format!(", {ct}");
            }
            if let Some(srv) = h("Server") {
                info += &format!(", сервер {srv}");
            }
            if let Some(reason) = h("X-Seraph-LoginReason") {
                info += &format!(", вход: {reason}");
            }
            let login_page = final_uri.contains("login.action") || final_uri.contains("/login");
            let ok = (200..300).contains(&status) && !login_page;
            d.steps
                .push(step("HTTP", if ok { "ok" } else { "fail" }, info));
            if let Some(u) = &who {
                d.steps.push(step(
                    "Вход",
                    "ok",
                    format!("сервер узнал пользователя «{u}»"),
                ));
            } else if matches!(auth, AuthKind::Confluence | AuthKind::Jira) && ok {
                d.steps.push(step(
                    "Вход",
                    "warn",
                    "сервер не назвал пользователя — возможно, страница открыта анонимно",
                ));
            }
            if ok {
                d.verdict = if missing.is_some() { "warn" } else { "ok" };
                d.hint = if missing.is_some() {
                    "Сервис отвечает, но без кредов — скорость и доступ проверяются не полностью."
                        .into()
                } else {
                    "Всё в порядке.".into()
                };
            } else {
                d.hint = match status {
                    401 | 403 => "Нет доступа: проверьте логин, пароль или токен в secrets.env.".into(),
                    404 if auth == AuthKind::Gitlab => {
                        "404: файла нет по адресу или у GITLAB_TOKEN нет прав на проект (GitLab прячет закрытое за 404).".into()
                    }
                    _ if login_page => "Сервер перенаправил на страницу входа: креды не приняты.".into(),
                    _ => format!("Сервер ответил {status}."),
                };
            }
        }
        Err(e) => {
            d.steps.push(step("HTTP", "fail", e.to_string()));
            d.hint =
                "TCP есть, но HTTP-запрос не прошёл: TLS, прокси или сервер не отвечает.".into();
        }
    }
    d
}

// ---------- платформенная часть ----------

#[cfg(windows)]
pub fn adapters() -> Vec<Adapter> {
    win::adapters().into_iter().map(|(_, a)| a).collect()
}

#[cfg(windows)]
fn route_adapter(ip: &IpAddr, _adapters: &[Adapter]) -> Option<(String, bool)> {
    let IpAddr::V4(v4) = ip else { return None };
    let idx = win::best_interface(*v4)?;
    win::adapters()
        .into_iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, a)| (a.name.clone(), a.vpn_like))
}

#[cfg(windows)]
fn system_proxy() -> String {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let Ok(k) = hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
    else {
        return "не прочитать".into();
    };
    let enabled: u32 = k.get_value("ProxyEnable").unwrap_or(0);
    let server: String = k.get_value("ProxyServer").unwrap_or_default();
    let pac: String = k.get_value("AutoConfigURL").unwrap_or_default();
    let mut parts = vec![];
    if enabled == 1 && !server.is_empty() {
        parts.push(format!("прокси {server}"));
    }
    if !pac.is_empty() {
        parts.push(format!("PAC {pac}"));
    }
    if parts.is_empty() {
        "нет".into()
    } else {
        parts.join(", ")
    }
}

#[cfg(not(windows))]
pub fn adapters() -> Vec<Adapter> {
    vec![]
}
#[cfg(not(windows))]
fn route_adapter(_ip: &IpAddr, _adapters: &[Adapter]) -> Option<(String, bool)> {
    None
}
#[cfg(not(windows))]
fn system_proxy() -> String {
    "не Windows".into()
}

#[cfg(windows)]
mod win {
    use super::{Adapter, vpn_like};
    use std::net::Ipv4Addr;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, GetBestInterface, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};

    unsafe fn wstr(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        unsafe {
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    }

    /// (индекс интерфейса, адаптер).
    pub fn adapters() -> Vec<(u32, Adapter)> {
        let mut size: u32 = 32 * 1024;
        let mut buf: Vec<u64> = Vec::new();
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        for _ in 0..3 {
            buf = vec![0u64; (size as usize).div_ceil(8)];
            let r = unsafe {
                GetAdaptersAddresses(
                    AF_UNSPEC as u32,
                    flags,
                    std::ptr::null(),
                    buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                    &mut size,
                )
            };
            if r == 0 {
                break;
            }
            if r != 111 {
                // не ERROR_BUFFER_OVERFLOW
                return vec![];
            }
        }
        let mut out = vec![];
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        unsafe {
            while !p.is_null() {
                let a = &*p;
                let name = wstr(a.FriendlyName);
                let description = wstr(a.Description);
                let mut ipv4 = vec![];
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    let sa = (*u).Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let b = sin.sin_addr.S_un.S_addr.to_ne_bytes();
                        ipv4.push(Ipv4Addr::new(b[0], b[1], b[2], b[3]).to_string());
                    }
                    u = (*u).Next;
                }
                let up = a.OperStatus == 1; // IfOperStatusUp
                let vpn = vpn_like(&name, &description);
                out.push((
                    a.Anonymous1.Anonymous.IfIndex,
                    Adapter {
                        name,
                        description,
                        up,
                        ipv4,
                        vpn_like: vpn,
                    },
                ));
                p = a.Next;
            }
        }
        out
    }

    pub fn best_interface(ip: Ipv4Addr) -> Option<u32> {
        let mut idx = 0u32;
        let dest = u32::from_ne_bytes(ip.octets());
        let r = unsafe { GetBestInterface(dest, &mut idx) };
        (r == 0).then_some(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_vpn_adapters() {
        assert!(vpn_like("happ-tun", "sing-tun"));
        assert!(vpn_like(
            "Ethernet 3",
            "TAP-Windows Adapter V9 for OpenVPN Connect"
        ));
        assert!(!vpn_like("Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz"));
    }

    #[test]
    fn diag_reports_dns_failure_with_hint() {
        let s = Secrets::default();
        let d = diag_url(
            "x",
            "X",
            "http://nonexistent.invalid/",
            AuthKind::None,
            &s,
            &[],
        );
        assert_eq!(d.verdict, "fail");
        assert_eq!(d.steps.last().unwrap().name, "DNS");
        assert!(!d.hint.is_empty());
    }
}
