//! Маршрут v0.6 (`docs/ux/scenarios.md`, прототип `docs/prototypes/flyout-v06.html`): звенья интернет → VPN → офис,
//! статус каждого — по медиане трёх последних проверок (один выпад картину не меняет; в истории — как есть).
//! Виновник — первое звено не в норме; звенья ниже, которые хуже из‑за него, помечаются «из‑за интернета» /
//! «из‑за VPN». Цвет — худшее звено. Пороги — `thresholds` в `settings.json`.

use crate::settings::Thresholds;
use serde::Serialize;

/// Сколько последних проверок сглаживает статус звена.
pub const LAST: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum St {
    Good,
    Warn,
    Bad,
}

fn worst(a: St, b: St) -> St {
    a.max(b)
}

/// Значение звена: медиана последних проверок и сами проверки (для подсказки).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Metric {
    pub value: Option<f64>,
    pub raw: Vec<f64>,
}

impl Metric {
    pub fn from(raw: &[f64]) -> Self {
        let raw: Vec<f64> = raw.iter().rev().take(LAST).rev().copied().collect();
        Self {
            value: median(&raw),
            raw,
        }
    }
}

pub fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let n = s.len();
    Some(if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    })
}

/// Сервис офиса в этой проверке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Svc {
    Up,
    Down,
    AuthFailed,
}

pub struct Input<'a> {
    pub inet_mbps: &'a [f64],
    pub inet_ms: &'a [f64],
    pub vpn_ms: &'a [f64],
    pub svn_mbps: &'a [f64],
    /// Цели офиса отвечают через туннель (VPN включён).
    pub vpn_up: bool,
    /// Сервисы офиса: имя, состояние, меряет ли скорость (SVN).
    pub office: &'a [(String, Svc, bool)],
}

#[derive(Debug, Clone, Serialize)]
pub struct Service {
    pub name: String,
    pub st: St,
    /// Словами: «отвечает», «вход AD не принят», «не отвечает», «качает 1,9 Мбит/с — медленно».
    pub what: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Route {
    pub inet: Metric,
    pub inet_ms: Metric,
    pub vpn: Metric,
    pub svn: Metric,
    pub st_inet_speed: St,
    pub st_inet_lat: St,
    pub st_inet: St,
    /// VPN: по задержке; если VPN режет скорость при нормальной задержке — по скорости SVN.
    pub st_vpn: St,
    pub st_svn: St,
    pub st_office: St,
    pub services: Vec<Service>,
    /// Первое звено не в норме: `inet` / `vpn` / `office`; `None` — всё в норме.
    pub cause: Option<&'static str>,
    /// Почему: `speed` / `lat` / `off` / `auth` / `svc`.
    pub why: Option<&'static str>,
    pub because_vpn: Option<&'static str>,
    pub because_office: Option<&'static str>,
    pub color: St,
    pub score: Option<u32>,
    /// Сервисы, не принявшие вход AD.
    pub auth_failed: Vec<String>,
    /// Точка у учётной записи: все приняли вход / один не принял / два и больше.
    pub st_user: St,
}

fn by_min(v: Option<f64>, good: f64, bad: f64) -> St {
    match v {
        None => St::Good, // не мерили — не виним
        Some(x) if x >= good => St::Good,
        Some(x) if x >= bad => St::Warn,
        Some(_) => St::Bad,
    }
}

fn by_max(v: Option<f64>, good: f64, bad: f64) -> St {
    match v {
        None => St::Good,
        Some(x) if x <= good => St::Good,
        Some(x) if x <= bad => St::Warn,
        Some(_) => St::Bad,
    }
}

fn rank(s: St) -> f64 {
    match s {
        St::Good => 0.0,
        St::Warn => 1.0,
        St::Bad => 2.0,
    }
}

pub fn evaluate(i: &Input, th: &Thresholds) -> Route {
    let inet = Metric::from(i.inet_mbps);
    let inet_ms = Metric::from(i.inet_ms);
    let vpn = if i.vpn_up {
        Metric::from(i.vpn_ms)
    } else {
        Metric::default()
    };
    let svn = if i.vpn_up {
        Metric::from(i.svn_mbps)
    } else {
        Metric::default()
    };
    let st_inet_speed = by_min(inet.value, th.inet_good_mbps, th.inet_bad_mbps);
    let st_inet_lat = by_max(inet_ms.value, th.inet_lat_warn_ms, th.inet_lat_bad_ms);
    let st_inet = worst(st_inet_speed, st_inet_lat);
    let st_vpn_lat = if i.vpn_up {
        by_max(vpn.value, th.vpn_good_ms, th.vpn_bad_ms)
    } else {
        St::Bad
    };
    let st_svn = if i.vpn_up {
        by_min(svn.value, th.svn_good_mbps, th.svn_bad_mbps)
    } else {
        St::Bad
    };

    let auth_failed: Vec<String> = i
        .office
        .iter()
        .filter(|(_, s, _)| *s == Svc::AuthFailed)
        .map(|(n, _, _)| n.clone())
        .collect();
    let services: Vec<Service> = i
        .office
        .iter()
        .map(|(name, s, speed)| {
            let (st, what) = match s {
                Svc::AuthFailed => (St::Warn, "вход AD не принят".to_string()),
                Svc::Down if !i.vpn_up => (St::Bad, "недоступен без VPN".into()),
                Svc::Down => (St::Bad, "не отвечает".into()),
                Svc::Up if *speed && st_svn != St::Good => (
                    st_svn,
                    format!(
                        "качает {} Мбит/с — медленно",
                        fmt1(svn.value.unwrap_or_default())
                    ),
                ),
                Svc::Up => (St::Good, "отвечает".into()),
            };
            Service {
                name: name.clone(),
                st,
                what,
            }
        })
        .collect();
    let up = i.office.iter().filter(|(_, s, _)| *s == Svc::Up).count();
    let st_svc = if up == i.office.len() {
        St::Good
    } else if up > 0 || !auth_failed.is_empty() {
        St::Warn
    } else {
        St::Bad
    };
    let st_office = worst(st_svn, st_svc);

    let (cause, why) = if st_inet != St::Good {
        (
            Some("inet"),
            Some(if st_inet_lat != St::Good {
                "lat"
            } else {
                "speed"
            }),
        )
    } else if !i.vpn_up {
        (Some("vpn"), Some("off"))
    } else if st_vpn_lat != St::Good {
        (Some("vpn"), Some("lat"))
    } else if st_svn != St::Good {
        (Some("vpn"), Some("speed"))
    } else if auth_failed.len() >= 2 {
        (Some("office"), Some("auth"))
    } else if st_office != St::Good {
        (Some("office"), Some("svc"))
    } else {
        (None, None)
    };
    let st_vpn = if why == Some("speed") && cause == Some("vpn") {
        worst(st_vpn_lat, st_svn)
    } else {
        st_vpn_lat
    };
    let (because_vpn, because_office) = match cause {
        Some("inet") => (
            (st_vpn != St::Good).then_some("из‑за интернета"),
            (st_office != St::Good).then_some("из‑за интернета"),
        ),
        Some("vpn") => (
            None,
            (st_office != St::Good).then_some(if why == Some("off") {
                "без VPN"
            } else {
                "из‑за VPN"
            }),
        ),
        _ => (None, None),
    };
    let color = worst(st_inet, worst(st_vpn, st_office));
    let score = (i.vpn_up).then(|| {
        let v = vpn.value.unwrap_or(0.0);
        let s = svn.value.unwrap_or(th.svn_good_mbps);
        let raw = 99.0
            - (rank(st_inet) * 18.0 + rank(st_vpn) * 20.0 + rank(st_office) * 20.0)
            - ((v - th.vpn_good_ms * 0.66) / 20.0).max(0.0)
            - ((th.svn_good_mbps - s) * 2.5).max(0.0);
        raw.clamp(1.0, 99.0).round() as u32
    });
    let st_user = match auth_failed.len() {
        0 => St::Good,
        1 => St::Warn,
        _ => St::Bad,
    };
    Route {
        inet,
        inet_ms,
        vpn,
        svn,
        st_inet_speed,
        st_inet_lat,
        st_inet,
        st_vpn,
        st_svn,
        st_office,
        services,
        cause,
        why,
        because_vpn,
        because_office,
        color,
        score,
        auth_failed,
        st_user,
    }
}

/// Вывод одной строкой — для подсказки иконки в трее (та же логика, что в меню).
pub fn headline(r: &Route, client: &str) -> String {
    match (r.cause, r.why) {
        (Some("inet"), Some("lat")) => "Интернет отвечает с задержкой — VPN не виноват".into(),
        (Some("inet"), _) => "Медленный интернет — VPN в порядке".into(),
        (Some("vpn"), Some("off")) => format!("VPN выключен — включите {client}"),
        (Some("vpn"), Some("lat")) => format!("VPN медленный — переподключите {client}"),
        (Some("vpn"), _) => "SVN качает медленно — режет VPN".into(),
        (Some("office"), Some("auth")) => "Пароль AD не подошёл — войдите заново".into(),
        (Some("office"), _) => {
            let bad: Vec<&str> = r
                .services
                .iter()
                .filter(|s| s.st != St::Good)
                .map(|s| s.name.as_str())
                .collect();
            format!("{} не отвечает — остальное работает", bad.join(", "))
        }
        _ => "Можно работать".into(),
    }
}

/// Цифры звеньев одной строкой: «SVN 1,9 Мбит/с · VPN 240 мс · канал 15 Мбит/с» (канал — интернет без VPN).
pub fn numbers(r: &Route) -> String {
    let mut parts = vec![];
    if let Some(s) = r.svn.value {
        parts.push(format!("SVN {} Мбит/с", fmt1(s)));
    }
    if let Some(v) = r.vpn.value {
        parts.push(format!("VPN {v:.0} мс"));
    }
    if let Some(i) = r.inet.value {
        parts.push(format!(
            "канал {} Мбит/с",
            if i < 10.0 { fmt1(i) } else { format!("{i:.0}") }
        ));
    }
    parts.join(" · ")
}

/// Подсказка иконки в трее: несколько строк, не длиннее 127 символов (предел Windows).
pub fn tooltip(lines: &[String]) -> String {
    let mut out = String::new();
    for l in lines.iter().filter(|l| !l.is_empty()) {
        let next = if out.is_empty() {
            l.clone()
        } else {
            format!("{out}\n{l}")
        };
        if next.chars().count() > 127 {
            break;
        }
        out = next;
    }
    out
}

fn fmt1(x: f64) -> String {
    format!("{x:.1}").replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th() -> Thresholds {
        crate::settings::Settings::embedded().thresholds
    }
    fn office(svn: Svc) -> Vec<(String, Svc, bool)> {
        vec![
            ("Confluence".into(), Svc::Up, false),
            ("Jira".into(), Svc::Up, false),
            ("SVN".into(), svn, true),
        ]
    }

    #[test]
    fn median_of_three_ignores_one_spike() {
        // выпад в последней проверке не меняет картину (регрессия по трём точкам его бы усилила)
        assert_eq!(Metric::from(&[12.1, 11.8, 1.1]).value, Some(11.8));
        assert_eq!(
            Metric::from(&[9.0, 14.0, 9.0, 4.0]).raw,
            vec![14.0, 9.0, 4.0]
        );
    }

    #[test]
    fn slow_svn_with_good_internet_blames_vpn() {
        // лог заказчика 1 ноября: интернет 15 Мбит/с, SVN 1,3–3,4
        let o = office(Svc::Up);
        let r = evaluate(
            &Input {
                inet_mbps: &[15.6, 14.2, 15.0],
                inet_ms: &[94.0, 223.0, 120.0],
                vpn_ms: &[348.0, 223.0, 240.0],
                svn_mbps: &[1.9, 3.4, 1.3],
                vpn_up: true,
                office: &o,
            },
            &th(),
        );
        assert_eq!((r.cause, r.why), (Some("vpn"), Some("speed")));
        assert_eq!(r.st_svn, St::Bad);
        assert_eq!(r.st_vpn, St::Bad, "звено VPN — цветом того, что оно ломает");
        assert_eq!(r.services[2].st, St::Bad, "у SVN не зелёная точка");
        assert_eq!(r.services[0].st, St::Good);
        assert_eq!(r.because_office, Some("из‑за VPN"));
    }

    #[test]
    fn slow_internet_is_the_cause_and_cascades() {
        let o = office(Svc::Up);
        let r = evaluate(
            &Input {
                inet_mbps: &[4.1, 3.8, 4.4],
                inet_ms: &[70.0, 85.0, 80.0],
                vpn_ms: &[260.0, 280.0, 270.0],
                svn_mbps: &[3.5, 3.2, 3.6],
                vpn_up: true,
                office: &o,
            },
            &th(),
        );
        assert_eq!((r.cause, r.why), (Some("inet"), Some("speed")));
        assert_eq!(r.st_vpn, St::Good, "VPN не виноват");
        assert_eq!(r.because_office, Some("из‑за интернета"));
        assert_eq!(r.color, St::Warn);
    }

    #[test]
    fn internet_latency_over_a_second_is_orange() {
        let o = office(Svc::Up);
        let r = evaluate(
            &Input {
                inet_mbps: &[20.0],
                inet_ms: &[1200.0, 1350.0, 1100.0],
                vpn_ms: &[1450.0],
                svn_mbps: &[9.0],
                vpn_up: true,
                office: &o,
            },
            &th(),
        );
        assert_eq!(r.st_inet_lat, St::Warn);
        assert_eq!((r.cause, r.why), (Some("inet"), Some("lat")));
        assert_eq!(r.because_vpn, Some("из‑за интернета"));
    }

    #[test]
    fn vpn_off_and_auth() {
        let o = vec![
            ("Confluence".to_string(), Svc::Down, false),
            ("Jira".into(), Svc::Down, false),
            ("SVN".into(), Svc::Down, true),
        ];
        let r = evaluate(
            &Input {
                inet_mbps: &[48.0],
                inet_ms: &[40.0],
                vpn_ms: &[200.0],
                svn_mbps: &[12.0],
                vpn_up: false,
                office: &o,
            },
            &th(),
        );
        assert_eq!((r.cause, r.why), (Some("vpn"), Some("off")));
        assert_eq!(r.color, St::Bad);
        assert!(r.score.is_none());
        let o = vec![
            ("Confluence".to_string(), Svc::AuthFailed, false),
            ("Jira".into(), Svc::AuthFailed, false),
            ("SVN".into(), Svc::Up, true),
        ];
        let r = evaluate(
            &Input {
                inet_mbps: &[48.0],
                inet_ms: &[40.0],
                vpn_ms: &[200.0],
                svn_mbps: &[12.0],
                vpn_up: true,
                office: &o,
            },
            &th(),
        );
        assert_eq!(r.st_user, St::Bad);
        assert_eq!((r.cause, r.why), (Some("office"), Some("auth")));
    }

    #[test]
    fn all_good() {
        let o = office(Svc::Up);
        let r = evaluate(
            &Input {
                inet_mbps: &[46.0, 52.0, 48.0],
                inet_ms: &[38.0, 41.0, 40.0],
                vpn_ms: &[192.0, 205.0, 198.0],
                svn_mbps: &[13.9, 12.8, 13.5],
                vpn_up: true,
                office: &o,
            },
            &th(),
        );
        assert_eq!(r.cause, None);
        assert_eq!(r.color, St::Good);
        assert!(r.score.unwrap() >= 90);
    }

    #[test]
    fn tray_tooltip_lines_fit_windows_limit() {
        let o = office(Svc::Up);
        let r = evaluate(
            &Input {
                inet_mbps: &[15.0],
                inet_ms: &[100.0],
                vpn_ms: &[240.0],
                svn_mbps: &[1.9],
                vpn_up: true,
                office: &o,
            },
            &th(),
        );
        assert_eq!(
            headline(&r, "OpenVPN Connect"),
            "SVN качает медленно — режет VPN"
        );
        assert_eq!(numbers(&r), "SVN 1,9 Мбит/с · VPN 240 мс · канал 15 Мбит/с");
        let t = tooltip(&[
            "Domini VPN Monitor — 42/99".into(),
            headline(&r, "OpenVPN Connect"),
            numbers(&r),
            "Проверено в 19:36".into(),
            "очень длинная строка, которая уже не влезет в предел Windows и должна быть отброшена целиком".into(),
        ]);
        assert!(t.chars().count() <= 127);
        assert_eq!(t.lines().count(), 4);
    }
}
