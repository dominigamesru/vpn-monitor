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
    /// Похож на туннель VPN (tun/tap/openvpn/wireguard/happ… или тип интерфейса «туннель»).
    pub vpn_like: bool,
    /// Тип по Windows словами: «Wi‑Fi», «кабель (Ethernet)», «туннель», … (v0.8.1). Имя адаптера пользователь или
    /// программа VPN может задать любое — тип и описание говорят, что это на самом деле.
    pub kind: String,
}

/// Тип интерфейса Windows (`IfType`, IANA ifType) словами.
pub fn kind_of(if_type: u32) -> &'static str {
    match if_type {
        71 => "Wi‑Fi",
        6 => "кабель (Ethernet)",
        131 => "туннель",
        23 | 512 => "PPP/модем",
        243 | 244 => "мобильная сеть",
        24 => "петля (loopback)",
        53 => "виртуальный адаптер",
        _ => "сетевой адаптер",
    }
}

/// Строка «Маршрут» для диагностики: тип, имя, описание и через VPN ли идёт трафик.
pub fn route_line(a: &Adapter) -> String {
    let desc = if a.description.is_empty() || a.description == a.name {
        String::new()
    } else {
        format!(", {}", a.description)
    };
    format!(
        "через {} «{}»{desc} — {}",
        a.kind,
        a.name,
        if a.vpn_like {
            "это VPN, трафик идёт через туннель"
        } else {
            "не VPN, трафик идёт напрямую"
        }
    )
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
        Some(a) => d.steps.push(step("Маршрут", "ok", route_line(&a))),
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
fn route_adapter(ip: &IpAddr, _adapters: &[Adapter]) -> Option<Adapter> {
    let IpAddr::V4(v4) = ip else { return None };
    let idx = win::best_interface(*v4)?;
    win::adapters()
        .into_iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, a)| a)
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

/// Почему проверять без скачивания: экономия заряда или лимитная сеть (v0.5); `None` — проверять как обычно.
#[cfg(windows)]
pub fn lite_reason() -> Option<&'static str> {
    if win::battery_saver() {
        Some("экономия заряда")
    } else if win::metered() {
        Some("лимитная сеть")
    } else {
        None
    }
}

#[cfg(not(windows))]
pub fn lite_reason() -> Option<&'static str> {
    None
}

#[cfg(not(windows))]
pub fn adapters() -> Vec<Adapter> {
    vec![]
}
#[cfg(not(windows))]
fn route_adapter(_ip: &IpAddr, _adapters: &[Adapter]) -> Option<Adapter> {
    None
}
#[cfg(not(windows))]
fn system_proxy() -> String {
    "не Windows".into()
}

#[cfg(windows)]
pub(crate) mod win {
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
                // туннель по типу интерфейса — VPN, как бы его ни назвали (OpenVPN DCO берёт имя профиля)
                let vpn = vpn_like(&name, &description) || a.IfType == 131;
                out.push((
                    a.Anonymous1.Anonymous.IfIndex,
                    Adapter {
                        name,
                        description,
                        up,
                        ipv4,
                        vpn_like: vpn,
                        kind: crate::diag::kind_of(a.IfType).to_string(),
                    },
                ));
                p = a.Next;
            }
        }
        out
    }

    /// Имена запущенных exe (в нижнем регистре).
    pub fn processes() -> Vec<String> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        let mut out = vec![];
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE || snap.is_null() {
                return out;
            }
            let mut e = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut e) != 0 {
                loop {
                    let n = e
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(e.szExeFile.len());
                    out.push(String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase());
                    if Process32NextW(snap, &mut e) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }

    /// Включена экономия заряда Windows.
    pub fn battery_saver() -> bool {
        use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut st: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
        unsafe { GetSystemPowerStatus(&mut st) != 0 && st.SystemStatusFlag == 1 }
    }

    /// Текущая сеть лимитная (тариф с ограничением, роуминг или превышен лимит) — по данным Windows.
    pub fn metered() -> bool {
        use windows::Networking::Connectivity::{NetworkCostType, NetworkInformation};
        let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() else {
            return false;
        };
        let Ok(cost) = profile.GetConnectionCost() else {
            return false;
        };
        let kind = cost.NetworkCostType().unwrap_or(NetworkCostType::Unknown);
        kind == NetworkCostType::Fixed
            || kind == NetworkCostType::Variable
            || cost.Roaming().unwrap_or(false)
            || cost.OverDataLimit().unwrap_or(false)
    }

    /// Подключённый Wi‑Fi (v0.9): (SSID, качество сигнала 0–100, канал, скорость приёма Мбит/с). WlanAPI — без прав
    /// администратора; Wi‑Fi нет или служба WLAN выключена — `None`.
    pub fn wifi() -> Option<(String, u32, u32, f64)> {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::NetworkManagement::WiFi::{
            WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST,
            WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle,
            WlanQueryInterface, wlan_interface_state_connected, wlan_intf_opcode_channel_number,
            wlan_intf_opcode_current_connection,
        };
        unsafe {
            let mut ver = 0u32;
            let mut h: HANDLE = std::ptr::null_mut();
            if WlanOpenHandle(2, std::ptr::null(), &mut ver, &mut h) != 0 {
                return None;
            }
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            let mut out = None;
            if WlanEnumInterfaces(h, std::ptr::null(), &mut list) == 0 && !list.is_null() {
                let n = (*list).dwNumberOfItems as usize;
                let first = (*list).InterfaceInfo.as_ptr();
                for i in 0..n {
                    let info: &WLAN_INTERFACE_INFO = &*first.add(i);
                    if info.isState != wlan_interface_state_connected {
                        continue;
                    }
                    let (mut size, mut data, mut kind) = (0u32, std::ptr::null_mut(), 0i32);
                    if WlanQueryInterface(
                        h,
                        &info.InterfaceGuid,
                        wlan_intf_opcode_current_connection,
                        std::ptr::null(),
                        &mut size,
                        &mut data,
                        &mut kind,
                    ) != 0
                        || data.is_null()
                    {
                        continue;
                    }
                    let c = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let a = &c.wlanAssociationAttributes;
                    let len = (a.dot11Ssid.uSSIDLength as usize).min(32);
                    let ssid = String::from_utf8_lossy(&a.dot11Ssid.ucSSID[..len]).into_owned();
                    let (signal, rx) = (a.wlanSignalQuality, a.ulRxRate as f64 / 1000.0);
                    WlanFreeMemory(data);
                    let (mut size2, mut data2, mut kind2) = (0u32, std::ptr::null_mut(), 0i32);
                    let mut channel = 0u32;
                    if WlanQueryInterface(
                        h,
                        &info.InterfaceGuid,
                        wlan_intf_opcode_channel_number,
                        std::ptr::null(),
                        &mut size2,
                        &mut data2,
                        &mut kind2,
                    ) == 0
                        && !data2.is_null()
                    {
                        channel = *(data2 as *const u32);
                        WlanFreeMemory(data2);
                    }
                    out = Some((ssid, signal, channel, rx));
                    break;
                }
                WlanFreeMemory(list as *const _);
            }
            WlanCloseHandle(h, std::ptr::null());
            out
        }
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
    fn route_line_says_kind_and_description() {
        // имя адаптера задал не Windows (заказчик: «через Type-C - Local Cable - Sergio», а кабеля нет)
        let a = Adapter {
            name: "Type-C - Local Cable - Sergio".into(),
            description: "Intel(R) Wi-Fi 6 AX201 160MHz".into(),
            up: true,
            ipv4: vec![],
            vpn_like: false,
            kind: kind_of(71).into(),
        };
        assert_eq!(
            route_line(&a),
            "через Wi‑Fi «Type-C - Local Cable - Sergio», Intel(R) Wi-Fi 6 AX201 160MHz — не VPN, трафик идёт напрямую"
        );
        assert_eq!(kind_of(131), "туннель");
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
