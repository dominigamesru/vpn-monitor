//! Проба: одна попытка достучаться до цели — DNS, задержка (TCP connect), HTTP GET и скорость скачивания.
//! Каждая проба пишет одну подробную строку в лог.

use crate::secrets::Secrets;
use crate::settings::{AuthKind, Internet, Role, Target};
use std::io::Read;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant, SystemTime};

/// Почему проба не удалась — для меню и лога.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    Dns,
    Connect,
    Timeout,
    Auth,
    Http,
    Config,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub target_id: String,
    pub name: String,
    pub role: Option<Role>,
    pub host: String,
    pub at: SystemTime,
    /// Полная проба (большой файл).
    pub full: bool,
    /// Цель отвечает по сети: DNS + TCP + какой-то HTTP-ответ.
    pub reachable: bool,
    /// Всё хорошо: ответ 2xx, доступ есть, данные скачаны.
    pub ok: bool,
    pub resolved: Option<String>,
    pub dns_ms: Option<f64>,
    pub latency_ms: Option<f64>,
    pub http_status: Option<u16>,
    pub bytes: u64,
    pub secs: f64,
    pub mbps: Option<f64>,
    pub fail: Option<Fail>,
    pub error: Option<String>,
    /// Предупреждение без провала (например, нет кредов — проба без авторизации).
    pub note: Option<String>,
}

impl ProbeResult {
    fn new(id: &str, name: &str, role: Option<Role>, host: &str, full: bool) -> Self {
        Self {
            target_id: id.into(),
            name: name.into(),
            role,
            host: host.into(),
            at: SystemTime::now(),
            full,
            reachable: false,
            ok: false,
            resolved: None,
            dns_ms: None,
            latency_ms: None,
            http_status: None,
            bytes: 0,
            secs: 0.0,
            mbps: None,
            fail: None,
            error: None,
            note: None,
        }
    }

    fn failed(mut self, fail: Fail, err: impl Into<String>) -> Self {
        self.fail = Some(fail);
        self.error = Some(err.into());
        self.ok = false;
        self
    }

    /// Строка для лога (без секретов: в ней только хост, коды и числа).
    pub fn log_line(&self) -> String {
        let mut s = format!(
            "{}{} {} host={}",
            self.name,
            if self.full { " (полная)" } else { "" },
            if self.ok {
                "OK"
            } else if self.reachable {
                "ЧАСТИЧНО"
            } else {
                "НЕДОСТУПНА"
            },
            self.host
        );
        if let (Some(ms), Some(ip)) = (self.dns_ms, &self.resolved) {
            s += &format!(" dns={ms:.0}мс→{ip}");
        }
        if let Some(ms) = self.latency_ms {
            s += &format!(" tcp={ms:.0}мс");
        }
        if let Some(c) = self.http_status {
            s += &format!(" http={c}");
        }
        if self.bytes > 0 {
            s += &format!(" скачано={} за {:.2}с", fmt_bytes(self.bytes), self.secs);
        }
        if let Some(m) = self.mbps {
            s += &format!(" скорость={m:.2}Мбит/с");
        }
        if let Some(f) = self.fail {
            s += &format!(" ошибка={f:?}: {}", self.error.as_deref().unwrap_or(""));
        }
        if let Some(n) = &self.note {
            s += &format!(" заметка: {n}");
        }
        s
    }
}

pub fn fmt_bytes(b: u64) -> String {
    if b >= 1024 * 1024 {
        format!("{:.2}МБ", b as f64 / 1048576.0)
    } else {
        format!("{:.1}КБ", b as f64 / 1024.0)
    }
}

/// Креды для цели: заголовки и заметка, если чего-то не хватает.
fn auth_headers(kind: AuthKind, secrets: &Secrets) -> (Vec<(String, String)>, Option<String>) {
    let basic = |u: &str, p: &str| -> Vec<(String, String)> {
        vec![(
            "Authorization".into(),
            format!("Basic {}", base64(format!("{u}:{p}").as_bytes())),
        )]
    };
    match kind {
        AuthKind::None => (vec![], None),
        AuthKind::Confluence => {
            if let Some(t) = secrets.get("CONFLUENCE_TOKEN") {
                (vec![("Authorization".into(), format!("Bearer {t}"))], None)
            } else if let (Some(u), Some(p)) = (
                secrets.get("CONFLUENCE_USER"),
                secrets.get("CONFLUENCE_PASSWORD"),
            ) {
                (basic(&u, &p), None)
            } else {
                (vec![], Some("нет CONFLUENCE_TOKEN (или CONFLUENCE_USER+CONFLUENCE_PASSWORD) в secrets.env".into()))
            }
        }
        AuthKind::Svn => match (secrets.get("SVN_USER"), secrets.get("SVN_PASSWORD")) {
            (Some(u), Some(p)) => (basic(&u, &p), None),
            _ => (
                vec![],
                Some("нет SVN_USER/SVN_PASSWORD в secrets.env".into()),
            ),
        },
        AuthKind::Gitlab => match secrets.get("GITLAB_TOKEN") {
            Some(t) => (vec![("PRIVATE-TOKEN".into(), t)], None),
            None => (vec![], Some("нет GITLAB_TOKEN в secrets.env".into())),
        },
    }
}

/// Адрес пробы с учётом переопределения из env / secrets.env.
pub fn target_url(t: &Target, secrets: &Secrets, full: bool) -> Option<String> {
    if full {
        let env = t.full_url_env.as_deref().and_then(|k| secrets.get(k));
        env.or_else(|| t.full_url.clone())
    } else {
        t.url_env
            .as_deref()
            .and_then(|k| secrets.get(k))
            .or_else(|| Some(t.url.clone()))
    }
}

/// Проба цели. `full` — полная проба большим файлом (если у цели есть `full_url`).
pub fn probe_target(t: &Target, secrets: &Secrets, full: bool) -> ProbeResult {
    let url = match target_url(t, secrets, full) {
        Some(u) => u,
        None => {
            return ProbeResult::new(&t.id, &t.name, Some(t.role), "?", full)
                .failed(Fail::Config, "нет адреса");
        }
    };
    let (headers, note) = auth_headers(t.auth, secrets);
    let mut r = probe_url(
        &t.id,
        &t.name,
        Some(t.role),
        &url,
        &headers,
        Duration::from_secs(t.timeout_sec),
        None,
        full,
    );
    if let Some(n) = note {
        r.note = Some(n);
        if r.ok {
            // скачали без авторизации — вероятно, страница входа; не считаем полноценным успехом для скорости
            if t.role == Role::Speed {
                r.ok = false;
            }
        }
    }
    if t.auth == AuthKind::Confluence
        && r.ok
        && r.error.is_none()
        && r.note.is_none()
        && r.looks_like_login
    {
        r.ok = false;
        r.fail = Some(Fail::Auth);
        r.error = Some("Confluence вернул страницу входа — проверьте CONFLUENCE_TOKEN".into());
    }
    r.into()
}

/// Проба интернета: качаем до `max_seconds`, потом обрываем.
pub fn probe_internet(i: &Internet) -> ProbeResult {
    probe_url(
        "internet",
        &i.name,
        None,
        &i.url,
        &[],
        Duration::from_secs(i.timeout_sec),
        Some(Duration::from_secs(i.max_seconds)),
        false,
    )
    .into()
}

/// Внутренний результат с признаком «похоже на страницу входа».
struct Raw {
    r: ProbeResult,
    looks_like_login: bool,
}
impl std::ops::Deref for Raw {
    type Target = ProbeResult;
    fn deref(&self) -> &ProbeResult {
        &self.r
    }
}
impl std::ops::DerefMut for Raw {
    fn deref_mut(&mut self) -> &mut ProbeResult {
        &mut self.r
    }
}
impl From<Raw> for ProbeResult {
    fn from(r: Raw) -> Self {
        r.r
    }
}

#[allow(clippy::too_many_arguments)]
fn probe_url(
    id: &str,
    name: &str,
    role: Option<Role>,
    url: &str,
    headers: &[(String, String)],
    timeout: Duration,
    max_body_time: Option<Duration>,
    full: bool,
) -> Raw {
    let (host, port) = match host_port(url) {
        Ok(hp) => hp,
        Err(e) => {
            return Raw {
                r: ProbeResult::new(id, name, role, url, full).failed(Fail::Config, e),
                looks_like_login: false,
            };
        }
    };
    let mut r = ProbeResult::new(id, name, role, &host, full);

    // DNS
    let t0 = Instant::now();
    let addrs: Vec<SocketAddr> = match (host.as_str(), port).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(e) => {
            return Raw {
                r: r.failed(Fail::Dns, format!("DNS: {e}")),
                looks_like_login: false,
            };
        }
    };
    r.dns_ms = Some(ms(t0.elapsed()));
    let Some(addr) = addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
    else {
        return Raw {
            r: r.failed(Fail::Dns, "DNS: нет адресов"),
            looks_like_login: false,
        };
    };
    r.resolved = Some(addr.ip().to_string());

    // Задержка: установка TCP-соединения
    let t1 = Instant::now();
    match TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(10))) {
        Ok(s) => {
            r.latency_ms = Some(ms(t1.elapsed()));
            drop(s);
        }
        Err(e) => {
            let fail = if e.kind() == std::io::ErrorKind::TimedOut {
                Fail::Timeout
            } else {
                Fail::Connect
            };
            return Raw {
                r: r.failed(fail, format!("TCP {addr}: {e}")),
                looks_like_login: false,
            };
        }
    }

    // HTTP GET и скорость
    let agent = crate::net::agent(timeout);
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let t2 = Instant::now();
    let mut resp = match req.call() {
        Ok(resp) => resp,
        Err(e) => {
            let fail = if matches!(e, ureq::Error::Timeout(_)) {
                Fail::Timeout
            } else {
                Fail::Http
            };
            return Raw {
                r: r.failed(fail, format!("HTTP: {e}")),
                looks_like_login: false,
            };
        }
    };
    let status = resp.status().as_u16();
    r.http_status = Some(status);
    r.reachable = true;
    let final_uri = ureq::ResponseExt::get_uri(&resp).to_string();
    let looks_like_login = final_uri.contains("login.action") || final_uri.contains("/login");

    let mut buf = vec![0u8; 64 * 1024];
    let mut reader = resp.body_mut().as_reader();
    let mut read_err = None;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                r.bytes += n as u64;
                if max_body_time.is_some_and(|max| t2.elapsed() >= max) {
                    break;
                }
            }
            Err(e) => {
                read_err = Some(e);
                break;
            }
        }
    }
    r.secs = t2.elapsed().as_secs_f64();
    if r.bytes >= 64 * 1024 && r.secs > 0.0 {
        r.mbps = Some(r.bytes as f64 * 8.0 / r.secs / 1_000_000.0);
    }

    r.ok = (200..300).contains(&status);
    if status == 401 || status == 403 {
        r = r.failed(
            Fail::Auth,
            format!("нет доступа (HTTP {status}) — проверьте креды в secrets.env"),
        );
    } else if !r.ok {
        r = r.failed(Fail::Http, format!("HTTP {status} ({final_uri})"));
    } else if let Some(e) = read_err {
        let fail = if e.kind() == std::io::ErrorKind::TimedOut {
            Fail::Timeout
        } else {
            Fail::Http
        };
        let got = fmt_bytes(r.bytes);
        r = r.failed(fail, format!("обрыв при скачивании после {got}: {e}"));
    }
    Raw {
        r,
        looks_like_login,
    }
}

fn host_port(url: &str) -> Result<(String, u16), String> {
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|e| format!("плохой адрес {url}: {e}"))?;
    let host = uri
        .host()
        .ok_or_else(|| format!("нет хоста в адресе {url}"))?
        .to_string();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("http") {
            80
        } else {
            443
        });
    Ok((host, port))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"user:p@ss"), "dXNlcjpwQHNz");
    }

    #[test]
    fn parses_host_port() {
        assert_eq!(
            host_port("http://svn.corp.x.com/svn/a").unwrap(),
            ("svn.corp.x.com".into(), 80)
        );
        assert_eq!(
            host_port("https://g.x.com/api?x=1").unwrap(),
            ("g.x.com".into(), 443)
        );
        assert_eq!(
            host_port("https://g.x.com:8443/").unwrap(),
            ("g.x.com".into(), 8443)
        );
    }

    #[test]
    fn missing_creds_give_note_not_secret() {
        let s = Secrets::from_text("");
        let (h, note) = auth_headers(AuthKind::Svn, &s);
        assert!(h.is_empty());
        assert!(note.unwrap().contains("SVN_USER"));
    }

    #[test]
    fn unreachable_host_fails_fast_with_reason() {
        let r = probe_url(
            "x",
            "X",
            None,
            "http://nonexistent.invalid/",
            &[],
            Duration::from_secs(3),
            None,
            false,
        );
        assert!(!r.reachable);
        assert_eq!(r.fail, Some(Fail::Dns));
    }
}

#[cfg(test)]
pub mod tests_support {
    use super::*;
    pub fn result(id: &str, role: Role) -> ProbeResult {
        ProbeResult::new(id, id, Some(role), "host", false)
    }
}
