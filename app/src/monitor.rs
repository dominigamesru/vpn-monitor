//! Проверка: пробы ко всем целям (параллельно) и итог — вердикт светофора. Общая для трея и `--check`.

use crate::probe::{self, ProbeResult};
use crate::score::{self, History, Verdict};
use crate::secrets::Secrets;
use crate::settings::{Role, Settings};
use crate::{info, warn};
use std::collections::{HashMap, VecDeque};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// Срез: последние результаты и вердикт — то, что видно в меню трея.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub at: SystemTime,
    /// Последний результат по каждой цели (в порядке настроек).
    pub targets: Vec<ProbeResult>,
    pub internet: Option<ProbeResult>,
    /// Интернет мерился в этой проверке (а не взят из прошлой).
    pub internet_fresh: bool,
    /// Клиент и профиль VPN, через который идут цели светофора (v0.4).
    pub client: Option<crate::vpn::Client>,
    /// После сна: провалы первых 2 минут не считаются (сеть поднимается), v0.5.
    pub waking: bool,
    /// Проверка без скачивания и почему («экономия заряда», «лимитная сеть»), v0.5.
    pub lite: Option<String>,
    /// v0.5: канал и туннель — задержка до интернета, надбавка VPN (медиана), доля канала (медиана).
    pub internet_latency_ms: Option<f64>,
    pub overhead_ms: Option<f64>,
    pub share: Option<f64>,
    pub verdict: Verdict,
    /// v0.6: маршрут — звенья по медиане трёх проверок, виновник, каскад; цвет и оценка иконки — из него.
    pub route: Option<crate::route::Route>,
    /// v0.6: была ли полная проба со скачиванием и сколько заняла проверка.
    pub full: bool,
    pub took_ms: u64,
}

pub struct Monitor {
    pub settings: Settings,
    secrets_path: PathBuf,
    history: History,
    last_full: Option<Instant>,
    last_internet_at: Option<Instant>,
    last_internet: Option<ProbeResult>,
    /// Последний результат цели; полная проба перекрывает лёгкую по скорости.
    last: HashMap<String, ProbeResult>,
    weak_internet: bool,
    check_no: u64,
    /// Последние задержки целей светофора и скорости целей скорости — оценка считается по медиане, чтобы один
    /// выброс не ронял светофор.
    latencies: VecDeque<f64>,
    speeds: VecDeque<f64>,
    overheads: VecDeque<f64>,
    shares: VecDeque<f64>,
    /// v0.6: последние три проверки по звеньям маршрута (route.rs).
    r_inet: VecDeque<f64>,
    r_inet_ms: VecDeque<f64>,
    r_vpn: VecDeque<f64>,
    r_svn: VecDeque<f64>,
    /// Своя норма (медианы за неделю) — задаёт трей перед проверкой из файлов метрик (v0.4).
    pub baseline: Option<crate::score::Baseline>,
    /// Клиент и профиль VPN прошлой проверки — по нему трей выбирает норму (v0.5).
    pub last_client: Option<String>,
    /// До этого момента — льготные минуты после сна (ставит трей), v0.5.
    pub grace_until: Option<Instant>,
}

/// Сколько последних проверок сглаживает оценку.
pub const SMOOTH_CHECKS: usize = 5;

fn push_capped(q: &mut VecDeque<f64>, v: f64, cap: usize) {
    q.push_back(v);
    while q.len() > cap {
        q.pop_front();
    }
}

pub fn median(q: &VecDeque<f64>) -> Option<f64> {
    if q.is_empty() {
        return None;
    }
    let mut v: Vec<f64> = q.iter().copied().collect();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

impl Monitor {
    pub fn new(settings: Settings, secrets_path: PathBuf) -> Self {
        Self {
            settings,
            secrets_path,
            history: History::default(),
            last_full: None,
            last_internet_at: None,
            last_internet: None,
            last: HashMap::new(),
            weak_internet: false,
            check_no: 0,
            latencies: VecDeque::new(),
            speeds: VecDeque::new(),
            overheads: VecDeque::new(),
            shares: VecDeque::new(),
            r_inet: VecDeque::new(),
            r_inet_ms: VecDeque::new(),
            r_vpn: VecDeque::new(),
            r_svn: VecDeque::new(),
            baseline: None,
            last_client: None,
            grace_until: None,
        }
    }

    /// Одна проверка. `manual` — по клику: интернет мерится, только если подошёл его интервал (рейт-лимиты).
    pub fn run_check(&mut self, manual: bool) -> Snapshot {
        self.check_no += 1;
        crate::gitlab::ensure_fresh(self.settings.gitlab_oauth.as_ref());
        crate::probe::set_min_probe_kbps(self.settings.thresholds.min_probe_kbps);
        let secrets = Secrets::load(&self.secrets_path);
        let s = &self.settings;
        let full_due = self
            .last_full
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(s.full_probe_interval_min * 60));
        let lite = crate::diag::lite_reason();
        let waking = self.grace_until.is_some_and(|g| Instant::now() < g);
        let internet_due = lite.is_none()
            && self
                .last_internet_at
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(s.internet_interval_min * 60));
        info!(
            "check",
            "проверка #{} ({}), полная проба: {}, интернет: {}",
            self.check_no,
            if manual {
                "по клику"
            } else {
                "по расписанию"
            },
            if full_due { "да" } else { "нет" },
            if internet_due { "да" } else { "нет" }
        );

        let t0 = Instant::now();
        let (results, internet, online_hint) = std::thread::scope(|sc| {
            let handles: Vec<_> = s
                .targets
                .iter()
                .filter(|t| {
                    t.enabled && (lite.is_none() || matches!(t.role, Role::Light | Role::World))
                })
                .map(|t| {
                    let secrets = &secrets;
                    sc.spawn(move || {
                        let mut out = vec![probe::probe_target(t, secrets, false)];
                        if full_due && t.full_url.is_some() {
                            out.push(probe::probe_target(t, secrets, true));
                        }
                        out
                    })
                })
                .collect();
            let inet = internet_due.then(|| sc.spawn(|| probe::probe_internet(&s.internet)));
            let conn = sc.spawn(|| connectivity(&s.internet.check_hosts));
            let results: Vec<ProbeResult> = handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect();
            let internet = inet.and_then(|h| h.join().ok());
            let online_hint = conn.join().unwrap_or(Some(0.0));
            (results, internet, online_hint)
        });

        let mut results = results;
        let mut internet = internet;
        for r in results.iter_mut().chain(internet.iter_mut()) {
            annotate_route(r, &self.settings);
        }
        let client = detect_client(&results, &self.settings);
        match &client {
            Some(c) => info!(
                "vpn",
                "клиент VPN: {}{} (адаптер «{}»)",
                c.name,
                c.profile
                    .as_deref()
                    .map(|p| format!(", профиль «{p}»"))
                    .unwrap_or_default(),
                c.adapter
            ),
            None => info!("vpn", "клиент VPN не определён"),
        }
        for r in results.iter().chain(internet.iter()) {
            if r.ok {
                info!("probe", "{}", r.log_line());
            } else {
                warn!("probe", "{}", r.log_line());
            }
        }

        // полная проба считается сделанной, только если все большие файлы скачались; иначе — повтор в следующую
        // проверку (например, VPN включили сразу после неудачной пробы)
        if full_due && results.iter().filter(|r| r.full).all(|r| r.ok) {
            self.last_full = Some(Instant::now());
        }
        let internet_fresh = internet.is_some();
        let internet_reachable = internet.as_ref().is_some_and(|i| i.reachable);
        if let Some(i) = internet {
            self.last_internet_at = Some(Instant::now());
            self.last_internet = Some(i);
        }
        let now = SystemTime::now();
        let light_results: Vec<&ProbeResult> = results
            .iter()
            .filter(|r| r.role == Some(Role::Light))
            .collect();
        // нет интернета: не отвечают ни проверочные адреса, ни одна цель — VPN не оцениваем и в историю не пишем
        let internet_latency = online_hint;
        let online =
            online_hint.is_some() || results.iter().any(|r| r.reachable) || internet_reachable;
        if let Some(why) = lite {
            info!("check", "проверка без скачивания: {why}");
        }
        // Wi‑Fi со страницей входа: интернета «нет», но сеть отдаёт свою страницу вместо проверки Windows
        let captive = !online && captive_portal();
        // после сна провалы не считаем: туннель и Wi‑Fi поднимаются не сразу
        let light_down = results
            .iter()
            .filter(|r| r.role == Some(Role::Light))
            .all(|r| !r.reachable);
        let skip_history = waking && (!online || light_down);
        if skip_history {
            info!(
                "check",
                "после сна: провалы не считаю — сеть ещё поднимается"
            );
        }
        if !online {
            warn!(
                "check",
                "нет интернета: проверочные адреса и цели не отвечают — VPN не оцениваю"
            );
        }
        if !light_results.is_empty() && online && !skip_history {
            let reachable = light_results.iter().any(|r| r.reachable);
            self.history.push_check(now, reachable);
            if reachable {
                // худшая задержка среди целей светофора в этой проверке
                if let Some(l) = light_results
                    .iter()
                    .filter_map(|r| r.latency_ms)
                    .reduce(f64::max)
                {
                    push_capped(&mut self.latencies, l, SMOOTH_CHECKS);
                }
            } else {
                // VPN не включён — прошлые замеры относятся к другому подключению
                self.latencies.clear();
                self.speeds.clear();
                self.overheads.clear();
                self.shares.clear();
                self.r_vpn.clear();
                self.r_svn.clear();
            }
        }
        let mut office_speed: Option<f64> = None;
        if let Some(sp) = results
            .iter()
            // «Без ВПН» — это скорость интернета, а не VPN: в оценку VPN не идёт; прерванный медленный замер —
            // идёт (это и есть «очень медленно»), но только когда интернет есть
            .filter(|r| {
                online
                    && r.role == Some(Role::Speed)
                    && (r.ok || r.slow_abort)
                    && r.via_vpn != Some(false)
            })
            .filter_map(|r| r.mbps)
            .reduce(f64::max)
        {
            office_speed = Some(sp);
            push_capped(&mut self.speeds, sp, SMOOTH_CHECKS);
            if !skip_history {
                push_capped(&mut self.r_svn, sp, crate::route::LAST);
            }
        }
        // v0.6: звенья маршрута — интернет (скорость, если мерили; задержка), средняя задержка до офиса через VPN
        if online && !skip_history {
            if let Some(ms) = internet_latency.filter(|m| *m > 0.0) {
                push_capped(&mut self.r_inet_ms, ms, crate::route::LAST);
            }
            if internet_fresh && let Some(m) = self.last_internet.as_ref().and_then(|r| r.mbps) {
                push_capped(&mut self.r_inet, m, crate::route::LAST);
            }
            let office_ms: Vec<f64> = light_results
                .iter()
                .filter(|r| r.reachable && r.via_vpn != Some(false))
                .filter_map(|r| r.latency_ms)
                .collect();
            if let Some(m) = crate::route::median(&office_ms) {
                push_capped(&mut self.r_vpn, m, crate::route::LAST);
            }
        }
        for r in &results {
            // полная проба точнее по скорости — лёгкая не затирает её скорость, но обновляет доступность
            let keep_full_speed = !r.full
                && self.last.get(&r.target_id).is_some_and(|p| {
                    p.full && p.ok && p.at.elapsed().unwrap_or_default() < Duration::from_secs(3600)
                });
            if keep_full_speed && r.ok {
                let mut merged = r.clone();
                merged.mbps = self.last[&r.target_id].mbps;
                self.last.insert(r.target_id.clone(), merged);
            } else {
                self.last.insert(r.target_id.clone(), r.clone());
            }
        }

        let targets: Vec<ProbeResult> = self
            .settings
            .targets
            .iter()
            .filter_map(|t| self.last.get(&t.id).cloned())
            .collect();
        // в оценку — медианы последних проверок (в меню остаются последние значения по целям)
        let smooth_latency = median(&self.latencies);
        let light: Vec<ProbeResult> = targets
            .iter()
            .filter(|r| r.role == Some(Role::Light))
            .cloned()
            .map(|mut r| {
                if r.reachable {
                    r.latency_ms = smooth_latency.or(r.latency_ms);
                }
                r
            })
            .collect();
        let speeds: Vec<f64> = median(&self.speeds).into_iter().collect();
        let baseline = self.baseline.clone();
        let inet_mbps = self.last_internet.as_ref().and_then(|r| r.mbps);
        // туннель (v0.5): надбавка VPN = задержка до офиса − задержка до интернета; доля канала = скорость до офиса /
        // скорость канала, если замер канала свежий
        if online {
            let office_lat = light
                .iter()
                .filter(|r| r.reachable)
                .filter_map(|r| r.latency_ms)
                .reduce(f64::max);
            if let (Some(o), Some(i)) = (office_lat, internet_latency) {
                push_capped(&mut self.overheads, (o - i).max(0.0), SMOOTH_CHECKS);
            }
            let fresh = Duration::from_secs(self.settings.thresholds.share_fresh_min * 60);
            let channel = self
                .last_internet
                .as_ref()
                .filter(|r| r.at.elapsed().unwrap_or_default() <= fresh)
                .and_then(|r| r.mbps)
                .filter(|m| *m > 0.0);
            if let (Some(sp), Some(ch)) = (office_speed, channel) {
                push_capped(&mut self.shares, (sp / ch).min(1.5), SMOOTH_CHECKS);
            }
        }
        self.last_client = client.as_ref().map(crate::vpn::Client::key);
        let tunnel = score::Tunnel {
            overhead_ms: median(&self.overheads),
            share: median(&self.shares),
        };
        let mut verdict = score::evaluate_tunnel(
            &light,
            &speeds,
            inet_mbps,
            self.weak_internet,
            &self.history,
            &self.settings.thresholds,
            now,
            baseline,
            online,
            tunnel,
        );
        if captive {
            verdict.badge = score::Badge::Captive;
            verdict.color = score::Color::Gray;
            verdict.explain = "Wi‑Fi требует входа на своей странице".into();
        } else if skip_history {
            verdict.badge = score::Badge::Unknown;
            verdict.color = score::Color::Gray;
            verdict.explain = "проверяю после сна — сеть ещё поднимается".into();
        }
        // v0.6: маршрут; цвет и оценка иконки — из него (кроме «нет интернета», Wi‑Fi со входом, после сна)
        let route = online.then(|| {
            let office: Vec<(String, crate::route::Svc, bool)> = targets
                .iter()
                .filter(|r| matches!(r.role, Some(Role::Light) | Some(Role::Speed)))
                .map(|r| {
                    let st = if r.fail == Some(crate::probe::Fail::Auth) {
                        crate::route::Svc::AuthFailed
                    } else if r.reachable && r.ok {
                        crate::route::Svc::Up
                    } else {
                        crate::route::Svc::Down
                    };
                    (r.name.clone(), st, r.role == Some(Role::Speed))
                })
                .collect();
            let vpn_up = light.iter().any(|r| r.reachable);
            let (inet, inet_ms, vpn, svn) = (
                Vec::from(self.r_inet.clone()),
                Vec::from(self.r_inet_ms.clone()),
                Vec::from(self.r_vpn.clone()),
                Vec::from(self.r_svn.clone()),
            );
            crate::route::evaluate(
                &crate::route::Input {
                    inet_mbps: &inet,
                    inet_ms: &inet_ms,
                    vpn_ms: &vpn,
                    svn_mbps: &svn,
                    vpn_up,
                    office: &office,
                },
                &self.settings.thresholds,
            )
        });
        if let Some(r) = &route
            && !captive
            && !skip_history
            && matches!(
                verdict.badge,
                score::Badge::Score(_) | score::Badge::WeakInternet(_)
            )
        {
            verdict.color = match r.color {
                crate::route::St::Good => score::Color::Green,
                crate::route::St::Warn => score::Color::Yellow,
                crate::route::St::Bad => score::Color::Red,
            };
            if let Some(n) = r.score {
                verdict.score = Some(n);
                if matches!(verdict.badge, score::Badge::Score(_)) {
                    verdict.badge = score::Badge::Score(n);
                }
            }
            info!(
                "route",
                "маршрут: интернет {:?}/{:?}, VPN {:?}, SVN {:?}, офис {:?}; виновник {:?} ({:?})",
                r.st_inet_speed,
                r.st_inet_lat,
                r.st_vpn,
                r.st_svn,
                r.st_office,
                r.cause,
                r.why
            );
        }
        self.weak_internet = matches!(verdict.badge, score::Badge::WeakInternet(_));
        info!(
            "score",
            "итог: {} знак={:?} оценка={:?} задержка={:?} скорость={:?} стабильность={:?} ({}) за {:.1}с",
            verdict.color.ru(),
            verdict.badge,
            verdict.score,
            verdict.latency.map(|v| v.round()),
            verdict.speed.map(|v| (v * 10.0).round() / 10.0),
            verdict.stability.map(|v| (v * 100.0).round()),
            verdict.explain,
            t0.elapsed().as_secs_f64()
        );
        Snapshot {
            at: now,
            targets,
            internet: self.last_internet.clone(),
            internet_fresh,
            client,
            waking: skip_history,
            lite: lite.map(str::to_string),
            internet_latency_ms: internet_latency,
            overhead_ms: tunnel.overhead_ms,
            share: tunnel.share,
            verdict,
            route,
            full: results.iter().any(|r| r.full),
            took_ms: t0.elapsed().as_millis() as u64,
        }
    }
}

/// Wi‑Fi со страницей входа: проверка Windows (`connecttest.txt`) вернула не свой текст или перенаправила.
fn captive_portal() -> bool {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    match agent
        .get("http://www.msftconnecttest.com/connecttest.txt")
        .call()
    {
        Ok(mut r) => {
            let st = r.status().as_u16();
            let body = r.body_mut().read_to_string().unwrap_or_default();
            (300..400).contains(&st) || (st == 200 && body.trim() != "Microsoft Connect Test")
        }
        Err(_) => false,
    }
}

/// Есть ли интернет и какая до него задержка: лучшее время TCP до проверочных адресов, `None` — ни один не ответил.
fn connectivity(hosts: &[String]) -> Option<f64> {
    if hosts.is_empty() {
        return Some(0.0);
    }
    std::thread::scope(|sc| {
        let hs: Vec<_> = hosts
            .iter()
            .map(|h| {
                sc.spawn(move || {
                    let a = h.to_socket_addrs().ok()?.next()?;
                    let t = Instant::now();
                    TcpStream::connect_timeout(&a, Duration::from_secs(3)).ok()?;
                    Some(t.elapsed().as_secs_f64() * 1000.0)
                })
            })
            .collect();
        hs.into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .reduce(f64::min)
    })
}

/// Маршрут до цели: адаптер и «через VPN ли» (по правилам клиентов из настроек).
fn annotate_route(r: &mut ProbeResult, s: &Settings) {
    let Some(ip) = r
        .resolved
        .as_deref()
        .and_then(|x| x.parse::<std::net::IpAddr>().ok())
    else {
        return;
    };
    if let Some(route) = crate::vpn::route(&ip, &s.vpn_clients, &s.ignore_adapters) {
        r.adapter = Some(route.adapter);
        r.adapter_desc = Some(route.description);
        // «Без ВПН» имеет смысл для целей офиса; интернет всегда мимо VPN
        r.via_vpn = Some(route.via_vpn);
    }
}

/// Клиент VPN — по адаптеру, через который идут цели светофора (если они идут через VPN).
fn detect_client(results: &[ProbeResult], s: &Settings) -> Option<crate::vpn::Client> {
    let r = results
        .iter()
        .find(|r| r.role == Some(Role::Light) && r.via_vpn == Some(true))?;
    let procs = crate::vpn::processes();
    crate::vpn::detect(
        r.adapter.as_deref()?,
        r.adapter_desc.as_deref().unwrap_or(""),
        &s.vpn_clients,
        &procs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_ignores_single_spike() {
        let q: VecDeque<f64> = [171.0, 165.0, 1133.0, 176.0, 160.0].into_iter().collect();
        assert_eq!(median(&q), Some(171.0));
        let q: VecDeque<f64> = [2.0, 4.0].into_iter().collect();
        assert_eq!(median(&q), Some(3.0));
        assert_eq!(median(&VecDeque::new()), None);
    }
}
