//! Сообщения админов (v0.9, ADR 0014): `app/config/announce.json` в репо → ассет релиза `latest`; приложение проверяет
//! раз в 5 минут (как `office.json`). Новое сообщение — плашка с крестиком в меню и окне и одно уведомление Windows.
//! Когда админы дописывают `resolved`, сотрудник, который видел красную плашку, видит вместо неё зелёную «решено» и
//! ещё одно уведомление; кто не видел — ничего (решённое прошлое ему не нужно).
//!
//! JSON обратно совместим: поля только добавляются, незнакомые терпятся; `level` незнакомый — как `info`.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const FILE: &str = "announce.json";
pub const PERIOD: Duration = Duration::from_secs(5 * 60);
/// Сколько держать зелёную «решено», если сотрудник её не закрыл.
pub const RESOLVED_KEEP_MS: i64 = 24 * 3600 * 1000;

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct File {
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Item {
    /// Уникальный идентификатор сообщения: по нему помним «видел» и «закрыл».
    pub id: String,
    /// `info` — к сведению (синяя), `warn` — внимание, `outage` — авария (красная; совет «звонить не нужно»).
    #[serde(default)]
    pub level: String,
    pub title: String,
    #[serde(default)]
    pub text: String,
    /// Ссылка «подробнее» (https).
    #[serde(default)]
    pub link: String,
    /// Затронутые сервисы (`id` целей: `svn`, `jira`…) — для совета; показываем всем.
    #[serde(default)]
    pub targets: Vec<String>,
    /// После этого времени (ISO 8601 или мс UNIX) сообщение не показываем.
    #[serde(default)]
    pub until: Option<serde_json::Value>,
    /// Ситуация разрешена: зелёная плашка вместо красной — у тех, кто видел.
    #[serde(default)]
    pub resolved: Option<Resolved>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Resolved {
    /// Когда решено (ISO 8601 или мс UNIX); нет — с момента, когда приложение это узнало.
    #[serde(default)]
    pub at: Option<serde_json::Value>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
}

/// Что запомнено на машине сотрудника (`user.json` → `announce`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Memory {
    /// Видел активным (показывали красную/синюю плашку) — `id`.
    #[serde(default)]
    pub seen: Vec<String>,
    /// Закрыл крестиком: `id` (активное) или `id:resolved` (зелёное).
    #[serde(default)]
    pub dismissed: Vec<String>,
    /// Когда приложение узнало о «решено» — `id` → мс (если в файле нет `resolved.at`).
    #[serde(default)]
    pub resolved_seen: std::collections::BTreeMap<String, i64>,
}

/// Что показать сейчас.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Shown {
    pub id: String,
    /// `info` / `warn` / `outage` / `resolved`.
    pub level: String,
    pub title: String,
    pub text: String,
    pub link: String,
    pub targets: Vec<String>,
    /// Ключ для крестика: `id` или `id:resolved`.
    pub dismiss_key: String,
}

/// Уведомление Windows, которое надо показать (один раз на ключ).
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub key: String,
    pub title: String,
    pub body: String,
}

pub fn parse(text: &str) -> Result<File, String> {
    serde_json::from_str(text).map_err(|e| format!("{FILE}: {e}"))
}

fn ms(v: &Option<serde_json::Value>) -> Option<i64> {
    match v.as_ref()? {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => parse_iso(s),
        _ => None,
    }
}

/// `2026-11-02T15:00:00Z` (или с `+03:00`) → мс UNIX; без внешних библиотек.
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T').unwrap_or((s, "00:00:00Z"));
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (time, off) = if let Some(t) = rest.strip_suffix('Z') {
        (t, 0)
    } else if let Some(i) = rest.rfind(['+', '-']) {
        let (t, o) = rest.split_at(i);
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let mut p = o[1..].split(':').map(|x| x.parse::<i64>());
        let (oh, om) = (p.next()?.ok()?, p.next().and_then(|x| x.ok()).unwrap_or(0));
        (t, sign * (oh * 3600 + om * 60))
    } else {
        (rest, 0)
    };
    let mut t = time
        .split(':')
        .map(|x| x.split('.').next().unwrap_or("0").parse::<i64>());
    let (hh, mm, ss) = (
        t.next()?.ok()?,
        t.next().and_then(|x| x.ok()).unwrap_or(0),
        t.next().and_then(|x| x.ok()).unwrap_or(0),
    );
    // дни от 1970-01-01 (алгоритм Хиннанта)
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hh * 3600 + mm * 60 + ss - off) * 1000)
}

/// Что показать и какие уведомления отправить; обновляет память (видел, когда узнал о «решено»).
pub fn select(file: &File, mem: &mut Memory, now: i64) -> (Vec<Shown>, Vec<Toast>) {
    let mut shown = vec![];
    let mut toasts = vec![];
    for it in &file.items {
        if it.id.is_empty() || it.title.is_empty() {
            continue;
        }
        if ms(&it.until).is_some_and(|u| now > u) {
            continue;
        }
        let link = if it.link.starts_with("https://") {
            it.link.clone()
        } else {
            String::new()
        };
        match &it.resolved {
            None => {
                if !mem.seen.contains(&it.id) {
                    mem.seen.push(it.id.clone());
                    toasts.push(Toast {
                        key: it.id.clone(),
                        title: it.title.clone(),
                        body: it.text.clone(),
                    });
                }
                if !mem.dismissed.contains(&it.id) {
                    let level = match it.level.as_str() {
                        "warn" | "outage" => it.level.clone(),
                        _ => "info".into(),
                    };
                    shown.push(Shown {
                        id: it.id.clone(),
                        level,
                        title: it.title.clone(),
                        text: it.text.clone(),
                        link,
                        targets: it.targets.clone(),
                        dismiss_key: it.id.clone(),
                    });
                }
            }
            Some(r) => {
                // решённое показываем только тем, кто видел его активным
                if !mem.seen.contains(&it.id) {
                    continue;
                }
                let key = format!("{}:resolved", it.id);
                let at = ms(&r.at)
                    .unwrap_or_else(|| *mem.resolved_seen.entry(it.id.clone()).or_insert(now));
                if !mem.resolved_seen.contains_key(&it.id) {
                    mem.resolved_seen.insert(it.id.clone(), now);
                }
                let first = mem.resolved_seen.get(&it.id) == Some(&now);
                let title = if r.title.is_empty() {
                    format!("Решено: {}", it.title)
                } else {
                    r.title.clone()
                };
                if first {
                    toasts.push(Toast {
                        key: key.clone(),
                        title: title.clone(),
                        body: r.text.clone(),
                    });
                }
                if mem.dismissed.contains(&key) || now - at > RESOLVED_KEEP_MS {
                    continue;
                }
                shown.push(Shown {
                    id: it.id.clone(),
                    level: "resolved".into(),
                    title,
                    text: r.text.clone(),
                    link,
                    targets: it.targets.clone(),
                    dismiss_key: key,
                });
            }
        }
    }
    (shown, toasts)
}

/// Хотя бы одна активная авария — совет «звонить в поддержку не нужно».
pub fn outage(shown: &[Shown]) -> bool {
    shown.iter().any(|s| s.level == "outage")
}

pub fn url(base: &str) -> String {
    format!("{}/latest/{FILE}", base.trim_end_matches('/'))
}

/// Скачать `announce.json`: `None` — файла нет (сообщений нет).
pub fn fetch(base: &str) -> Result<Option<String>, String> {
    let u = url(base);
    let mut r = crate::net::agent(Duration::from_secs(20))
        .get(&u)
        .header("Cache-Control", "no-cache")
        .call()
        .map_err(|e| format!("{u}: {e}"))?;
    match r.status().as_u16() {
        200 => r
            .body_mut()
            .read_to_string()
            .map(Some)
            .map_err(|e| format!("{u}: {e}")),
        404 => Ok(None),
        st => Err(format!("{u}: HTTP {st}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_793_620_000_000;

    fn file(resolved: bool) -> File {
        parse(&format!(
            r#"{{"items":[{{"id":"svn-1102","level":"outage","title":"SVN тормозит у всех — чиним","text":"Проблема на VPN-шлюзе.","targets":["svn"],"future":1{}}}]}}"#,
            if resolved { r#","resolved":{"title":"SVN снова работает","text":"Шлюз починили."}"# } else { "" }
        ))
        .unwrap()
    }

    #[test]
    fn active_then_resolved_for_those_who_saw() {
        let mut m = Memory::default();
        let (s, t) = select(&file(false), &mut m, T0);
        assert_eq!((s.len(), s[0].level.as_str(), t.len()), (1, "outage", 1));
        assert!(outage(&s));
        // повторная проверка — без нового уведомления
        assert!(select(&file(false), &mut m, T0 + 300_000).1.is_empty());
        // админы дописали resolved — красная уходит, зелёная приходит, одно уведомление
        let (s, t) = select(&file(true), &mut m, T0 + 600_000);
        assert_eq!(
            (s.len(), s[0].level.as_str(), s[0].title.as_str()),
            (1, "resolved", "SVN снова работает")
        );
        assert_eq!(t.len(), 1);
        assert!(select(&file(true), &mut m, T0 + 900_000).1.is_empty());
        // через сутки зелёная сама уходит
        assert!(
            select(&file(true), &mut m, T0 + 600_000 + RESOLVED_KEEP_MS + 1)
                .0
                .is_empty()
        );
    }

    #[test]
    fn resolved_not_shown_to_who_never_saw_and_dismiss_works() {
        let mut m = Memory::default();
        assert_eq!(select(&file(true), &mut m, T0), (vec![], vec![]));
        let mut m = Memory::default();
        select(&file(false), &mut m, T0);
        m.dismissed.push("svn-1102".into());
        assert!(select(&file(false), &mut m, T0).0.is_empty());
        let (s, _) = select(&file(true), &mut m, T0 + 1);
        assert_eq!(s[0].dismiss_key, "svn-1102:resolved"); // закрыть красную ≠ не увидеть «решено»
    }

    #[test]
    fn until_and_iso_dates() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso("2026-11-02T15:00:00+03:00"),
            parse_iso("2026-11-02T12:00:00Z")
        );
        let f =
            parse(r#"{"items":[{"id":"a","title":"x","until":"2000-01-01T00:00:00Z"}]}"#).unwrap();
        assert!(select(&f, &mut Memory::default(), T0).0.is_empty());
    }
}
