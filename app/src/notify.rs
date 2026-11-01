//! Когда показывать всплывающее уведомление (v0.5, `docs/ux/scenarios.md` §6). Только логика — показ в трее.
//!
//! Правила против спама:
//! - событие — смена состояния, которая держится 2 проверки подряд; первое наблюдение после запуска — без
//!   уведомления (это не смена, а знакомство);
//! - «снова работает» — только если до этого показали «упал»;
//! - одно и то же событие — не чаще раза в 30 минут; всего — не больше 4 в час;
//! - ничего во время паузы, без интернета, после сна (тогда состояние «неизвестно» и не меняется).

use crate::prefs::Notify as NotifyPrefs;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    OfficeDown,
    OfficeUp,
    WorldDown,
    WorldUp,
    AuthFailed,
    Slow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub kind: Kind,
    pub title: String,
    pub body: String,
}

/// Что видно в этой проверке. `None` — неизвестно (нет интернета, после сна, пауза): состояние не меняем.
#[derive(Debug, Clone, Default)]
pub struct Observation {
    pub office_up: Option<bool>,
    pub world_up: Option<bool>,
    /// Какие мировые сервисы не отвечают (для текста).
    pub world_down_names: Vec<String>,
    pub auth_failed: bool,
    pub slow: bool,
    /// Клиент VPN — для текста («Включите Happ»).
    pub client: Option<String>,
}

/// Подтверждённое состояние с «кандидатом», который должен продержаться 2 проверки.
#[derive(Debug, Default)]
struct Track {
    state: Option<bool>,
    candidate: Option<bool>,
    streak: u32,
    down_shown: bool,
}

impl Track {
    /// Возвращает новое подтверждённое состояние, если оно сменилось.
    fn feed(&mut self, v: Option<bool>) -> Option<bool> {
        let v = v?;
        if self.state.is_none() {
            self.state = Some(v); // знакомство — не событие
            return None;
        }
        if self.state == Some(v) {
            self.candidate = None;
            self.streak = 0;
            return None;
        }
        if self.candidate == Some(v) {
            self.streak += 1;
        } else {
            self.candidate = Some(v);
            self.streak = 1;
        }
        if self.streak >= 2 {
            self.state = Some(v);
            self.candidate = None;
            self.streak = 0;
            return Some(v);
        }
        None
    }
}

#[derive(Debug, Default)]
pub struct Notifier {
    office: Track,
    world: Track,
    auth: Track,
    slow: Track,
    last: HashMap<Kind, Instant>,
    recent: VecDeque<Instant>,
}

pub const SAME_KIND_GAP: Duration = Duration::from_secs(30 * 60);
pub const MAX_PER_HOUR: usize = 4;

impl Notifier {
    pub fn observe(&mut self, o: &Observation, prefs: &NotifyPrefs, now: Instant) -> Vec<Notice> {
        let mut out = vec![];
        let client = o.client.clone().unwrap_or_else(|| "VPN".into());
        if let Some(up) = self.office.feed(o.office_up) {
            if !up {
                self.office.down_shown = prefs.vpn;
                out.push(Notice {
                    kind: Kind::OfficeDown,
                    title: "VPN в офис не работает".into(),
                    body: format!("Confluence, Jira и SVN недоступны. Проверьте {client}."),
                });
            } else if std::mem::take(&mut self.office.down_shown) {
                out.push(Notice {
                    kind: Kind::OfficeUp,
                    title: "VPN в офис снова работает".into(),
                    body: "Confluence, Jira и SVN доступны.".into(),
                });
            }
        }
        if let Some(up) = self.world.feed(o.world_up) {
            if !up {
                self.world.down_shown = prefs.vpn;
                let names = if o.world_down_names.is_empty() {
                    "ChatGPT, Claude и Luma".into()
                } else {
                    o.world_down_names.join(", ")
                };
                out.push(Notice {
                    kind: Kind::WorldDown,
                    title: "Нет доступа к сервисам в мире".into(),
                    body: format!("{names} не открываются. Проверьте {client}."),
                });
            } else if std::mem::take(&mut self.world.down_shown) {
                out.push(Notice {
                    kind: Kind::WorldUp,
                    title: "Доступ к сервисам в мире вернулся".into(),
                    body: "ChatGPT, Claude и Luma открываются.".into(),
                });
            }
        }
        if self.auth.feed(Some(o.auth_failed)) == Some(true) {
            out.push(Notice {
                kind: Kind::AuthFailed,
                title: "Пароль AD не подошёл".into(),
                body: "Возможно, его сменили. Войдите заново в окне VPN Monitor.".into(),
            });
        }
        if self.slow.feed(o.office_up.and(Some(o.slow))) == Some(true) {
            out.push(Notice {
                kind: Kind::Slow,
                title: "VPN медленнее обычного".into(),
                body: "Задержка до офиса выше обычной.".into(),
            });
        }

        // настройки сотрудника и лимиты
        out.retain(|n| match n.kind {
            Kind::OfficeDown | Kind::OfficeUp | Kind::WorldDown | Kind::WorldUp => prefs.vpn,
            Kind::AuthFailed => prefs.auth,
            Kind::Slow => prefs.slow,
        });
        while self
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600))
        {
            self.recent.pop_front();
        }
        let mut sent = vec![];
        for n in out {
            if self
                .last
                .get(&n.kind)
                .is_some_and(|t| now.duration_since(*t) < SAME_KIND_GAP)
            {
                continue;
            }
            if self.recent.len() >= MAX_PER_HOUR {
                break;
            }
            self.last.insert(n.kind, now);
            self.recent.push_back(now);
            sent.push(n);
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(office: Option<bool>, world: Option<bool>) -> Observation {
        Observation {
            office_up: office,
            world_up: world,
            ..Default::default()
        }
    }

    #[test]
    fn needs_two_checks_and_no_alert_on_startup() {
        let p = NotifyPrefs::default();
        let mut n = Notifier::default();
        let t = Instant::now();
        assert!(
            n.observe(&obs(Some(false), None), &p, t).is_empty(),
            "первое наблюдение — не событие"
        );
        let mut n = Notifier::default();
        n.observe(&obs(Some(true), None), &p, t);
        assert!(
            n.observe(&obs(Some(false), None), &p, t).is_empty(),
            "одна проверка — мало"
        );
        let v = n.observe(&obs(Some(false), None), &p, t);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, Kind::OfficeDown);
        // вернулся — «снова работает», раз «упал» показывали
        n.observe(&obs(Some(true), None), &p, t + Duration::from_secs(60));
        let v = n.observe(&obs(Some(true), None), &p, t + Duration::from_secs(120));
        assert_eq!(v[0].kind, Kind::OfficeUp);
    }

    #[test]
    fn unknown_does_not_change_state() {
        let p = NotifyPrefs::default();
        let mut n = Notifier::default();
        let t = Instant::now();
        n.observe(&obs(Some(true), Some(true)), &p, t);
        for _ in 0..5 {
            assert!(
                n.observe(&obs(None, None), &p, t).is_empty(),
                "нет интернета / после сна — молчим"
            );
        }
    }

    #[test]
    fn flapping_is_rate_limited() {
        let p = NotifyPrefs::default();
        let mut n = Notifier::default();
        let mut t = Instant::now();
        n.observe(&obs(Some(true), Some(true)), &p, t);
        let mut total = 0;
        // VPN мигает каждые 2 проверки в течение часа
        for i in 0..60 {
            t += Duration::from_secs(60);
            let up = (i / 2) % 2 == 0;
            total += n.observe(&obs(Some(up), Some(up)), &p, t).len();
        }
        assert!(total <= MAX_PER_HOUR, "за час показали {total}");
    }

    #[test]
    fn respects_prefs() {
        let p = NotifyPrefs {
            vpn: false,
            ..NotifyPrefs::default()
        };
        let mut n = Notifier::default();
        let t = Instant::now();
        n.observe(&obs(Some(true), None), &p, t);
        n.observe(&obs(Some(false), None), &p, t);
        assert!(n.observe(&obs(Some(false), None), &p, t).is_empty());
    }
}
