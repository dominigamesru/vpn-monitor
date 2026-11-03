//! Микроджобы (0.9.9, заказчик 03.11): лёгкие независимые проверки одной части — «вернулся ли SVN», «появился ли
//! интернет», «отвечает ли офис». Без входа и скачивания: `HEAD` к серверу или TCP до проверочных адресов.
//! Джоба живёт, пока часть не ответит; ответила — убирается, может уведомить сотрудника («SVN снова отвечает» — когда
//! он сам нажал «Проверить подключение» или «Обновить» и видел красное) и запустить проверку пошире: только эту часть
//! или всю. Здесь — модель (чистая, с тестами); запуск — в трее (`app.rs`).

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Probe {
    /// `HEAD` к корню сервера (любой HTTP-ответ — сервер отвечает); любой из адресов.
    Ping { urls: Vec<String> },
    /// Интернет: TCP до проверочных адресов (`host:port`).
    Internet { hosts: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "ids", rename_all = "lowercase")]
pub enum Then {
    /// Ничего, только убрать джобу (и уведомить, если надо).
    Nothing,
    /// Проверить только эти цели (по `id`).
    Targets(Vec<String>),
    /// Полная проверка.
    Full,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Job {
    /// Ключ: `svn`, `internet`, `office`; уведомляющие — с приставкой `watch:`.
    pub key: String,
    /// Что показать в уведомлении и логе: «SVN», «Интернет», «Офис».
    pub name: String,
    pub probe: Probe,
    /// Уведомить сотрудника, когда часть ответит.
    pub notify: bool,
    pub then: Then,
    /// Как часто, с.
    pub period_s: u64,
    /// Когда следующий раз, с UNIX.
    pub next_at: u64,
    /// После этого — убрать молча (часть так и не ответила), с UNIX.
    pub until: u64,
}

/// Уведомляющие джобы — раз в минуту, два часа; фоновые — как `light_ping_sec`, пока следующая проверка их не
/// пересоздаст.
pub const WATCH_PERIOD: u64 = 60;
pub const WATCH_FOR: u64 = 2 * 3600;
pub const BACKGROUND_FOR: u64 = 3600;

#[derive(Debug, Default, Clone)]
pub struct Jobs {
    list: Vec<Job>,
}

impl Jobs {
    /// Добавить или заменить по ключу (срок и время следующей попытки — новые).
    pub fn upsert(&mut self, job: Job) {
        self.list.retain(|j| j.key != job.key);
        self.list.push(job);
    }

    pub fn remove(&mut self, key: &str) {
        self.list.retain(|j| j.key != key);
    }

    /// Фоновые (без уведомления) — убрать все: проверка пересоздаст нужные.
    pub fn clear_background(&mut self) {
        self.list.retain(|j| j.notify);
    }

    /// Чей срок подошёл; просроченные убираются.
    pub fn due(&mut self, now: u64) -> Vec<Job> {
        self.list.retain(|j| now < j.until);
        self.list
            .iter()
            .filter(|j| j.next_at <= now)
            .cloned()
            .collect()
    }

    /// Не ответила — следующая попытка через период.
    pub fn again(&mut self, key: &str, now: u64) {
        if let Some(j) = self.list.iter_mut().find(|j| j.key == key) {
            j.next_at = now + j.period_s;
        }
    }

    pub fn list(&self) -> &[Job] {
        &self.list
    }
}

/// Уведомляющая джоба «сообщить, когда вернётся» — для цели, которую сотрудник сам проверил и увидел красной.
pub fn watch(id: &str, name: &str, url: &str, now: u64, sec: u32) -> Job {
    Job {
        key: format!("watch:{id}"),
        name: name.into(),
        probe: Probe::Ping {
            urls: vec![url.into()],
        },
        notify: true,
        then: Then::Targets(vec![id.into()]),
        period_s: WATCH_PERIOD,
        next_at: now + WATCH_PERIOD + (sec as u64 % 15),
        until: now + WATCH_FOR,
    }
}

/// Фоновая джоба после обычной проверки (без уведомления).
pub fn background(key: &str, name: &str, probe: Probe, then: Then, period: u64, now: u64) -> Job {
    Job {
        key: key.into(),
        name: name.into(),
        probe,
        notify: false,
        then,
        period_s: period.max(30),
        next_at: now + period.max(30),
        until: now + BACKGROUND_FOR,
    }
}

/// Что сторожить после обычной проверки (без уведомлений): нет интернета — ждать интернет и проверить всё; офис
/// молчит целиком — пинговать офис и проверить всё; иначе — каждую цель, не ответившую по сети, пинговать и
/// перепроверить только её. `failed` — (id, имя, адрес).
pub fn plan(
    no_internet: bool,
    office_silent: bool,
    hosts: &[String],
    light_urls: &[String],
    failed: &[(String, String, String)],
    period: u64,
    now: u64,
) -> Vec<Job> {
    if period == 0 {
        return vec![];
    }
    if no_internet {
        return vec![background(
            "internet",
            "Интернет",
            Probe::Internet {
                hosts: hosts.to_vec(),
            },
            Then::Full,
            period,
            now,
        )];
    }
    if office_silent {
        return vec![background(
            "office",
            "Офис",
            Probe::Ping {
                urls: light_urls.to_vec(),
            },
            Then::Full,
            period,
            now,
        )];
    }
    failed
        .iter()
        .map(|(id, name, url)| {
            background(
                id,
                name,
                Probe::Ping {
                    urls: vec![url.clone()],
                },
                Then::Targets(vec![id.clone()]),
                period,
                now,
            )
        })
        .collect()
}

/// Провал по сети (не ответил вовсе) — его стоит повторить и сторожить джобой. Вход не принят, 404 и т. п. — ответ
/// сервера: повтор и пинг ничего не изменят (пинг «пройдёт» и соврёт, что всё хорошо).
pub fn network_fail(f: Option<crate::probe::Fail>) -> bool {
    use crate::probe::Fail;
    matches!(f, Some(Fail::Dns | Fail::Connect | Fail::Timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_lives_until_ok_or_expiry() {
        let mut j = Jobs::default();
        j.upsert(watch("svn", "SVN", "http://svn/x", 1000, 7));
        assert!(j.due(1000).is_empty());
        let d = j.due(1000 + 60 + 7);
        assert_eq!(d.len(), 1);
        assert!(d[0].notify);
        assert_eq!(d[0].then, Then::Targets(vec!["svn".into()]));
        j.again("watch:svn", 1067);
        assert!(j.due(1100).is_empty());
        assert_eq!(j.due(1127).len(), 1);
        // два часа без ответа — убирается молча
        assert!(j.due(1000 + WATCH_FOR).is_empty());
        assert!(j.list().is_empty());
    }

    #[test]
    fn background_replaced_by_next_check_watch_kept() {
        let mut j = Jobs::default();
        j.upsert(watch("svn", "SVN", "http://svn/x", 0, 0));
        j.upsert(background(
            "internet",
            "Интернет",
            Probe::Internet {
                hosts: vec!["1.1.1.1:443".into()],
            },
            Then::Full,
            120,
            0,
        ));
        j.upsert(background(
            "svn",
            "SVN",
            Probe::Ping {
                urls: vec!["http://svn/x".into()],
            },
            Then::Targets(vec!["svn".into()]),
            10,
            0,
        ));
        assert_eq!(j.list().len(), 3);
        assert_eq!(j.list()[2].period_s, 30); // не чаще раза в 30 с
        j.clear_background();
        assert_eq!(j.list().len(), 1);
        assert_eq!(j.list()[0].key, "watch:svn");
    }

    #[test]
    fn plan_by_situation() {
        let hosts = vec!["1.1.1.1:443".to_string()];
        let light = vec![
            "https://jira/".to_string(),
            "https://confluence/".to_string(),
        ];
        let svn = vec![(
            "svn".to_string(),
            "SVN".to_string(),
            "http://svn/f".to_string(),
        )];
        // интернета нет — одна джоба на интернет, проверка целиком
        let p = plan(true, true, &hosts, &light, &svn, 120, 0);
        assert_eq!(
            (p.len(), p[0].key.as_str(), &p[0].then),
            (1, "internet", &Then::Full)
        );
        // офис молчит — пинг офиса, проверка целиком
        let p = plan(false, true, &hosts, &light, &svn, 120, 0);
        assert_eq!((p.len(), p[0].key.as_str()), (1, "office"));
        // VPN есть, SVN не ответил (лог заказчика 03.11: «коннект таймаут») — пинг SVN, перепроверка только SVN
        let p = plan(false, false, &hosts, &light, &svn, 120, 0);
        assert_eq!(p[0].then, Then::Targets(vec!["svn".into()]));
        assert!(!p[0].notify);
        // выключено в настройках
        assert!(plan(true, false, &hosts, &light, &svn, 0, 0).is_empty());
    }

    #[test]
    fn only_network_failures_are_watched() {
        use crate::probe::Fail;
        assert!(network_fail(Some(Fail::Timeout)));
        assert!(network_fail(Some(Fail::Connect)));
        assert!(network_fail(Some(Fail::Dns)));
        assert!(!network_fail(Some(Fail::Auth)));
        assert!(!network_fail(Some(Fail::Http)));
        assert!(!network_fail(None));
    }
}
