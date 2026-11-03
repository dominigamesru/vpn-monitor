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
    /// v0.9: сеть (Wi‑Fi, кабель, телефон) и качество звонка до офиса.
    pub net: Option<crate::netctx::Net>,
    pub call: Option<crate::netctx::Call>,
    /// 0.9.7: короткие обрывы туннеля за последний час — офис не ответил, а через 3 с ответил.
    pub drops_hour: u32,
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
    /// 0.9.7: когда были короткие обрывы туннеля (для «обрывов за час»).
    drops: VecDeque<Instant>,
    /// 0.9.11: какие баблы висят (ключ → текст) — в лог «повесил / снял и почему».
    bubbles: HashMap<&'static str, String>,
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
            drops: VecDeque::new(),
            bubbles: HashMap::new(),
        }
    }

    /// Одна проверка. `manual` — по клику: интернет мерится, только если подошёл его интервал (рейт-лимиты).
    pub fn run_check(&mut self, manual: bool) -> Snapshot {
        self.run_check_for(manual, None)
    }

    /// 0.9.9: проверка только этих целей (микроджоба «SVN снова отвечает» → перепроверить SVN): без интернета и
    /// большого файла; остальные цели — из прошлой проверки.
    pub fn run_check_for(&mut self, manual: bool, only: Option<&[String]>) -> Snapshot {
        self.check_no += 1;
        crate::progress::begin();
        crate::gitlab::ensure_fresh(self.settings.gitlab_oauth.as_ref());
        crate::probe::set_min_probe_kbps(self.settings.thresholds.min_probe_kbps);
        let secrets = Secrets::load(&self.secrets_path);
        let s = &self.settings;
        let full_due = only.is_none()
            && self
                .last_full
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(s.full_probe_interval_min * 60));
        let lite = crate::diag::lite_reason();
        let waking = self.grace_until.is_some_and(|g| Instant::now() < g);
        let internet_due = lite.is_none()
            && only.is_none()
            && self
                .last_internet_at
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(s.internet_interval_min * 60));
        info!(
            "check",
            "проверка #{} ({}), полная проба: {}, интернет: {}",
            self.check_no,
            match only {
                Some(o) => format!("только {}", o.join(", ")),
                None if manual => "по клику".into(),
                None => "по расписанию".into(),
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
                    t.enabled
                        && (lite.is_none() || matches!(t.role, Role::Light | Role::World))
                        && only.is_none_or(|o| o.contains(&t.id))
                })
                .map(|t| {
                    let secrets = &secrets;
                    sc.spawn(move || {
                        // ход проверки (0.9.5): меню моргает узлом и пишет шаг в полосу микроуведомлений
                        let step = |full: bool| {
                            let key = format!("{}{}", t.id, if full { ":full" } else { "" });
                            crate::progress::probe_started(&key, Some(t.role), &t.name, full);
                            let r = probe::probe_target(t, secrets, full);
                            crate::progress::probe_done(&key, &r);
                            r
                        };
                        let mut out = vec![step(false)];
                        if full_due && t.full_url.is_some() {
                            out.push(step(true));
                        }
                        out
                    })
                })
                .collect();
            let inet = internet_due.then(|| {
                sc.spawn(|| {
                    crate::progress::probe_started("internet", None, "скорость без VPN", false);
                    let r = probe::probe_internet(&s.internet);
                    crate::progress::probe_done("internet", &r);
                    r
                })
            });
            let conn = sc.spawn(|| {
                crate::progress::start("conn", "inet", "Проверяю: есть ли интернет…".into());
                let c = connectivity(&s.internet.check_hosts);
                crate::progress::done(
                    "conn",
                    if c.is_some() { "good" } else { "bad" },
                    match c {
                        Some(ms) if ms > 0.0 => format!("✓ Интернет есть · {ms:.0} мс"),
                        Some(_) => "✓ Интернет есть".into(),
                        None => "✗ Интернет: проверочные адреса не отвечают".into(),
                    },
                );
                c
            });
            let results: Vec<ProbeResult> = handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect();
            let internet = inet.and_then(|h| h.join().ok());
            let online_hint = conn.join().unwrap_or(Some(0.0));
            (results, internet, online_hint)
        });

        let mut results = results;
        // 0.9.9: любой провал по сети (DNS, соединение, таймаут) — повтор через 3 с; прошёл — берём повтор, в метриках
        // провала нет, только строка в логе (заказчик 03.11: «меньше ложных срабатываний»). Офис молчал целиком, а повтор
        // прошёл — короткий обрыв туннеля (0.9.7).
        let light_ids: Vec<&str> = s
            .targets
            .iter()
            .filter(|t| t.enabled && t.role == Role::Light)
            .map(|t| t.id.as_str())
            .collect();
        let light_silent = !light_ids.is_empty()
            && results
                .iter()
                .filter(|r| light_ids.contains(&r.target_id.as_str()))
                .all(|r| !r.reachable)
            && results
                .iter()
                .any(|r| light_ids.contains(&r.target_id.as_str()));
        let retry: Vec<&crate::settings::Target> = s
            .targets
            .iter()
            .filter(|t| {
                results
                    .iter()
                    .any(|r| r.target_id == t.id && !r.full && crate::jobs::network_fail(r.fail))
            })
            .collect();
        if !retry.is_empty() && online_hint.is_some() && lite.is_none() {
            info!(
                "check",
                "не ответили по сети: {} — повтор через 3 с",
                retry
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            std::thread::sleep(Duration::from_secs(3));
            let again: Vec<ProbeResult> = std::thread::scope(|sc| {
                let hs: Vec<_> = retry
                    .iter()
                    .map(|t| {
                        let secrets = &secrets;
                        sc.spawn(move || {
                            let key = format!("{}:again", t.id);
                            crate::progress::probe_started(&key, Some(t.role), &t.name, false);
                            let r = probe::probe_target(t, secrets, false);
                            crate::progress::probe_done(&key, &r);
                            r
                        })
                    })
                    .collect();
                hs.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            let mut light_back = false;
            for a in again {
                if a.reachable {
                    info!("check", "повтор: {} ответил — провал не считаю", a.name);
                    light_back |= light_ids.contains(&a.target_id.as_str());
                    if let Some(x) = results
                        .iter_mut()
                        .find(|r| r.target_id == a.target_id && !r.full)
                    {
                        *x = a;
                    }
                } else {
                    info!("check", "повтор: {} снова не ответил", a.name);
                }
            }
            if light_silent && light_back {
                self.drops.push_back(Instant::now());
                info!(
                    "check",
                    "короткий обрыв туннеля; обрывов за час: {}",
                    drops_in_hour(&mut self.drops)
                );
            }
        }
        let mut internet = internet;
        for r in results.iter_mut().chain(internet.iter_mut()) {
            annotate_route(r, &self.settings);
        }
        crate::progress::start(
            "vpn",
            "vpn",
            "Определяю клиент VPN по маршруту до офиса…".into(),
        );
        let client = detect_client(&results, &self.settings);
        crate::progress::done(
            "vpn",
            if client.is_some() { "good" } else { "warn" },
            match &client {
                Some(c) => format!(
                    "✓ VPN: {}{} (адаптер «{}»)",
                    c.name,
                    c.place().map(|p| format!(" · {p}")).unwrap_or_default(),
                    c.adapter
                ),
                None => "VPN: клиент не определён".into(),
            },
        );
        match &client {
            Some(c) => info!(
                "vpn",
                "клиент VPN: {}{} (адаптер «{}»)",
                c.name,
                format!(
                    "{}{}",
                    c.profile
                        .as_deref()
                        .map(|p| format!(", профиль «{p}»"))
                        .unwrap_or_default(),
                    c.server
                        .as_deref()
                        .map(|s| format!(", сервер {s}"))
                        .unwrap_or_default()
                ),
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
        // 0.9.8: скорость в оценке — только SVN (GitLab тоже качает пробный файл, но это другой сервер — его скорость
        // видна у самого GitLab и не подменяет «SVN»)
        let svn_ids: Vec<&str> = s
            .targets
            .iter()
            .filter(|t| t.kind() == crate::settings::AuthKind::Svn)
            .map(|t| t.id.as_str())
            .collect();
        if let Some(sp) = results
            .iter()
            .filter(|r| svn_ids.is_empty() || svn_ids.contains(&r.target_id.as_str()))
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
        // v0.9: сеть и звонок — до маршрута: слабый Wi‑Fi уточняет причину «интернет»
        let net = crate::netctx::current();
        let call = if online {
            light.first().and_then(|r| {
                self.settings
                    .targets
                    .iter()
                    .find(|t| t.id == r.target_id)
                    .and_then(|t| call_pings(&t.url))
            })
        } else {
            None
        };
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
        let route = route.map(|mut r| {
            if r.cause == Some("inet") && crate::netctx::weak_wifi(&net, r.inet.value) {
                r.why = Some("wifi");
            }
            // 0.9.7: клиент VPN найден по маршруту до офиса (туннель поднят), а офис молчит — не «выключен», а «не
            // пропускает трафик»
            if r.cause == Some("vpn") && r.why == Some("off") && client.is_some() {
                r.why = Some("stall");
                r.because_office = Some("из‑за VPN");
            }
            r
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
        {
            let svn_now = route.as_ref().and_then(|r| r.svn.value);
            let usual = self.baseline.as_ref().and_then(|b| b.speed_mbps);
            let list = crate::netctx::bubbles(&net, call.as_ref(), svn_now, usual);
            let (sig, c) = (net.signal, call.clone());
            self.log_bubbles(list, &|k| match k {
                "call" => match &c {
                    Some(c) => format!(
                        "звонок в норме — потери {:.1}%, разброс {:.0} мс",
                        c.loss_pct, c.jitter_ms
                    ),
                    None => "звонок не измерялся в этой проверке (нет офиса или подставной адрес туннеля)".into(),
                },
                "wifi" => format!("сигнал {}%", sig.map_or("—".into(), |s| s.to_string())),
                "net" => "сеть сменилась на обычную".into(),
                "gb" => format!(
                    "SVN {}",
                    svn_now.map_or("не мерили".into(), |v| format!("{v:.1} Мбит/с"))
                ),
                _ => String::new(),
            });
        }
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
            net: (!net.kind.is_empty() && net.kind != "unknown").then_some(net),
            call,
            drops_hour: drops_in_hour(&mut self.drops),
        }
    }

    /// Баблы меню: что повесили и что сняли с прошлой проверки — одной строкой с причиной (заказчик 03.11).
    fn log_bubbles(
        &mut self,
        now: Vec<(&'static str, String, String)>,
        why_gone: &dyn Fn(&str) -> String,
    ) {
        for (k, text, why) in &now {
            if self.bubbles.get(k) != Some(text) {
                info!("bubble", "повесил «{text}»: {why}");
            }
        }
        let keys: Vec<&'static str> = now.iter().map(|b| b.0).collect();
        let gone: Vec<(&'static str, String)> = self
            .bubbles
            .iter()
            .filter(|(k, _)| !keys.contains(k))
            .map(|(k, t)| (*k, t.clone()))
            .collect();
        for (k, text) in gone {
            info!("bubble", "снял «{text}»: {}", why_gone(k));
        }
        self.bubbles = now.into_iter().map(|(k, t, _)| (k, t)).collect();
    }
}

impl Monitor {
    /// Адреса целей для микроджоб (0.9.9): (id, имя, роль, адрес лёгкой пробы).
    pub fn target_urls(&self) -> Vec<(String, String, Role, String)> {
        let secrets = Secrets::load(&self.secrets_path);
        self.settings
            .targets
            .iter()
            .filter(|t| t.enabled)
            .filter_map(|t| {
                probe::target_url(t, &secrets, false)
                    .map(|u| (t.id.clone(), t.name.clone(), t.role, u))
            })
            .collect()
    }
}

/// Обрывов за последний час; старые забываются.
fn drops_in_hour(d: &mut VecDeque<Instant>) -> u32 {
    while d
        .front()
        .is_some_and(|t| t.elapsed() > Duration::from_secs(3600))
    {
        d.pop_front();
    }
    d.len() as u32
}

/// Серия TCP-соединений до сервиса офиса (v0.9): 8 штук через 120 мс, без данных — разброс задержки и потери для
/// «можно ли звонить». ~1–2 с; не ответил — потеря.
fn call_pings(url: &str) -> Option<crate::netctx::Call> {
    use std::net::{TcpStream, ToSocketAddrs};
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?']).next()?;
    let (h, port) = match host.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (host, if url.starts_with("https") { 443 } else { 80 }),
    };
    let addr = (h, port).to_socket_addrs().ok()?.next()?;
    // 0.9.11: подставной адрес (Happ, fake-ip) — TCP доходит только до своего туннеля на этом компьютере: задержка ~1 мс
    // и «потери» ничего не говорят о звонке (заказчик 03.11: «связь рвётся» при оценке 99). Не меряем.
    if crate::probe::is_fake_ip(&addr) {
        info!(
            "check",
            "звонок: через подставной адрес туннеля не измерить — пропускаю"
        );
        return None;
    }
    let mut pings = vec![];
    for i in 0..8 {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(120));
        }
        let t = Instant::now();
        pings.push(
            TcpStream::connect_timeout(&addr, Duration::from_secs(1))
                .ok()
                .map(|_| t.elapsed().as_secs_f64() * 1000.0),
        );
    }
    crate::netctx::call_quality(&pings)
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
