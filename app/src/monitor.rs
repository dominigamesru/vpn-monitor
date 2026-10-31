//! Проверка: пробы ко всем целям (параллельно) и итог — вердикт светофора. Общая для трея и `--check`.

use crate::probe::{self, ProbeResult};
use crate::score::{self, History, Verdict};
use crate::secrets::Secrets;
use crate::settings::{Role, Settings};
use crate::{info, warn};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// Срез: последние результаты и вердикт — то, что видно в меню трея.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub at: SystemTime,
    /// Последний результат по каждой цели (в порядке настроек).
    pub targets: Vec<ProbeResult>,
    pub internet: Option<ProbeResult>,
    pub verdict: Verdict,
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
        }
    }

    /// Одна проверка. `manual` — по клику: интернет мерится, только если подошёл его интервал (рейт-лимиты).
    pub fn run_check(&mut self, manual: bool) -> Snapshot {
        self.check_no += 1;
        let secrets = Secrets::load(&self.secrets_path);
        let s = &self.settings;
        let full_due = self
            .last_full
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(s.full_probe_interval_min * 60));
        let internet_due = self
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
        let (results, internet) = std::thread::scope(|sc| {
            let handles: Vec<_> = s
                .targets
                .iter()
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
            let results: Vec<ProbeResult> = handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect();
            let internet = inet.and_then(|h| h.join().ok());
            (results, internet)
        });

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
        if let Some(i) = internet {
            self.last_internet_at = Some(Instant::now());
            self.last_internet = Some(i);
        }
        let now = SystemTime::now();
        let light_results: Vec<&ProbeResult> = results
            .iter()
            .filter(|r| r.role == Some(Role::Light))
            .collect();
        if !light_results.is_empty() {
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
            }
        }
        if let Some(sp) = results
            .iter()
            .filter(|r| r.role == Some(Role::Speed) && r.ok)
            .filter_map(|r| r.mbps)
            .reduce(f64::max)
        {
            push_capped(&mut self.speeds, sp, SMOOTH_CHECKS);
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
        let inet_mbps = self.last_internet.as_ref().and_then(|r| r.mbps);
        let verdict = score::evaluate(
            &light,
            &speeds,
            inet_mbps,
            self.weak_internet,
            &self.history,
            &self.settings.thresholds,
            now,
        );
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
            verdict,
        }
    }
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
