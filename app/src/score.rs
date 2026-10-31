//! Оценка VPN (01–99) и цвет светофора по срезу и истории проб.
//!
//! Компоненты 0..1: задержка (по целям светофора), скорость (по целям скорости), стабильность (доля успешных
//! проб светофора за окно). Итог — взвешенное среднее геометрическое: плохой любой компонент роняет оценку.

use crate::probe::ProbeResult;
use crate::settings::{Role, Thresholds};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Green,
    Yellow,
    Red,
}

impl Color {
    pub fn ru(self) -> &'static str {
        match self {
            Color::Green => "зелёный",
            Color::Yellow => "жёлтый",
            Color::Red => "красный",
        }
    }
}

/// Что рисовать знаком поверх логотипа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// Ещё не было ни одной проверки.
    Unknown,
    /// Оценка VPN числом.
    Score(u32),
    /// VPN не включён (цель светофора не отвечает) — красный «!».
    NoVpn,
    /// Слабый интернет — полоски 1..=3.
    WeakInternet(u8),
}

#[derive(Debug, Clone)]
pub struct Verdict {
    pub color: Color,
    pub badge: Badge,
    pub score: Option<u32>,
    pub latency: Option<f64>,
    pub speed: Option<f64>,
    pub stability: Option<f64>,
    pub explain: String,
}

/// История проб светофора для стабильности.
#[derive(Debug, Default)]
pub struct History {
    items: VecDeque<(SystemTime, bool)>,
}

impl History {
    pub fn push(&mut self, at: SystemTime, reachable: bool) {
        self.items.push_back((at, reachable));
        let cutoff = at - Duration::from_secs(24 * 3600);
        while self.items.front().is_some_and(|(t, _)| *t < cutoff) {
            self.items.pop_front();
        }
    }

    /// Доля успешных за окно; None — проб нет.
    pub fn stability(&self, now: SystemTime, window: Duration) -> Option<f64> {
        let from = now - window;
        let (ok, all) = self
            .items
            .iter()
            .filter(|(t, _)| *t >= from)
            .fold((0u32, 0u32), |(ok, all), (_, r)| (ok + *r as u32, all + 1));
        (all > 0).then(|| ok as f64 / all as f64)
    }
}

fn lerp_down(v: f64, good: f64, bad: f64) -> f64 {
    ((bad - v) / (bad - good)).clamp(0.0, 1.0)
}

fn lerp_up_log(v: f64, bad: f64, good: f64) -> f64 {
    if v <= 0.0 {
        return 0.0;
    }
    ((v.ln() - bad.ln()) / (good.ln() - bad.ln())).clamp(0.0, 1.0)
}

/// Считает вердикт. `speeds` — последние скорости целей скорости (Мбит/с), `internet` — последняя скорость
/// интернета, `weak_before` — был ли интернет слабым (гистерезис: число возвращается выше порога + 8 Мбит/с).
pub fn evaluate(
    light: &[ProbeResult],
    speeds: &[f64],
    internet: Option<f64>,
    weak_before: bool,
    history: &History,
    th: &Thresholds,
    now: SystemTime,
) -> Verdict {
    let light: Vec<&ProbeResult> = light
        .iter()
        .filter(|r| r.role == Some(Role::Light))
        .collect();
    let stability = history.stability(now, Duration::from_secs(th.stability_window_min * 60));
    if light.is_empty() {
        return Verdict {
            color: Color::Red,
            badge: Badge::Unknown,
            score: None,
            latency: None,
            speed: None,
            stability,
            explain: "нет проб светофора".into(),
        };
    }
    if light.iter().all(|r| !r.reachable) {
        return Verdict {
            color: Color::Red,
            badge: Badge::NoVpn,
            score: None,
            latency: None,
            speed: None,
            stability,
            explain: format!(
                "VPN не включён: {}",
                light
                    .iter()
                    .map(|r| format!("{} — {}", r.name, r.error.as_deref().unwrap_or("?")))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
    }

    let latency = light
        .iter()
        .filter_map(|r| r.latency_ms)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    let speed = (!speeds.is_empty()).then(|| speeds.iter().cloned().fold(f64::MIN, f64::max));

    let mut parts: Vec<(f64, f64, &str)> = vec![]; // (значение, вес, имя)
    if let Some(l) = latency {
        parts.push((
            lerp_down(l, th.latency_good_ms, th.latency_bad_ms),
            1.0,
            "задержка",
        ));
    }
    if let Some(s) = speed {
        parts.push((
            lerp_up_log(s, th.speed_bad_mbps, th.speed_good_mbps),
            1.0,
            "скорость",
        ));
    }
    if let Some(st) = stability {
        parts.push((((st - 0.8) / 0.2).clamp(0.0, 1.0), 1.0, "стабильность"));
    }
    let partial = light.iter().any(|r| !r.reachable);
    if partial {
        parts.push((0.3, 1.0, "часть целей недоступна"));
    }
    let wsum: f64 = parts.iter().map(|p| p.1).sum();
    let g = if wsum > 0.0 {
        (parts
            .iter()
            .map(|(v, w, _)| w * v.max(0.01).ln())
            .sum::<f64>()
            / wsum)
            .exp()
    } else {
        0.5
    };
    let score = ((g * 99.0).round() as u32).clamp(1, 99);
    let color = if score >= th.score_green {
        Color::Green
    } else if score >= th.score_yellow {
        Color::Yellow
    } else {
        Color::Red
    };

    let weak = match internet {
        Some(i) if weak_before => i < th.internet_weak_mbps + 8.0,
        Some(i) => i < th.internet_weak_mbps,
        None => weak_before,
    };
    let badge = match (weak, internet) {
        (true, Some(i)) => {
            let w = th.internet_weak_mbps.max(0.1);
            Badge::WeakInternet(if i < w / 3.0 {
                1
            } else if i < w * 2.0 / 3.0 {
                2
            } else {
                3
            })
        }
        _ => Badge::Score(score),
    };
    let explain = parts
        .iter()
        .map(|(v, _, n)| format!("{n}={v:.2}"))
        .collect::<Vec<_>>()
        .join(" ");
    Verdict {
        color,
        badge,
        score: Some(score),
        latency,
        speed,
        stability,
        explain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn light(reachable: bool, lat: f64) -> ProbeResult {
        let mut r = crate::probe::tests_support::result("confluence", Role::Light);
        r.reachable = reachable;
        r.ok = reachable;
        r.latency_ms = reachable.then_some(lat);
        r
    }

    #[test]
    fn no_vpn_when_light_unreachable() {
        let th = Settings::embedded().thresholds;
        let v = evaluate(
            &[light(false, 0.0)],
            &[],
            None,
            false,
            &History::default(),
            &th,
            SystemTime::now(),
        );
        assert_eq!(v.badge, Badge::NoVpn);
        assert_eq!(v.color, Color::Red);
    }

    #[test]
    fn fast_vpn_is_green_slow_is_not() {
        let th = Settings::embedded().thresholds;
        let now = SystemTime::now();
        let mut h = History::default();
        for _ in 0..10 {
            h.push(now, true);
        }
        let good = evaluate(
            &[light(true, 40.0)],
            &[50.0],
            Some(100.0),
            false,
            &h,
            &th,
            now,
        );
        assert_eq!(good.color, Color::Green, "{good:?}");
        assert!(matches!(good.badge, Badge::Score(s) if s >= 90));
        let slow = evaluate(
            &[light(true, 600.0)],
            &[50.0],
            Some(100.0),
            false,
            &h,
            &th,
            now,
        );
        assert_eq!(slow.color, Color::Red, "{slow:?}");
    }

    #[test]
    fn weak_internet_shows_bars_with_hysteresis() {
        let th = Settings::embedded().thresholds; // weak < 5
        let now = SystemTime::now();
        let h = History::default();
        let v = evaluate(&[light(true, 40.0)], &[], Some(1.0), false, &h, &th, now);
        assert_eq!(v.badge, Badge::WeakInternet(1));
        // 7 Мбит/с: выше порога, но ниже порога + 8 → остаются полоски
        let v = evaluate(&[light(true, 40.0)], &[], Some(7.0), true, &h, &th, now);
        assert!(matches!(v.badge, Badge::WeakInternet(_)));
        let v = evaluate(&[light(true, 40.0)], &[], Some(14.0), true, &h, &th, now);
        assert!(matches!(v.badge, Badge::Score(_)));
    }

    #[test]
    fn stability_window() {
        let now = SystemTime::now();
        let mut h = History::default();
        h.push(now - Duration::from_secs(7200), false);
        h.push(now, true);
        assert_eq!(h.stability(now, Duration::from_secs(3600)), Some(1.0));
        assert_eq!(h.stability(now, Duration::from_secs(3 * 3600)), Some(0.5));
    }
}
