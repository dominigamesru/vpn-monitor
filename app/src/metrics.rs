//! Метрики в файлах папки установки (R5.2–R5.3, ADR 0002): `metrics_1d.json` — календарные сутки,
//! `metrics_7d.json` — текущий день и 7 полных дней до него. Одна запись — одна проверка.
//! Метаданные (компьютер, пользователь, версия) — один раз в шапке файла (R5.4). Архив по неделям — позже.

use crate::monitor::Snapshot;
use crate::score::{Badge, Color};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetRec {
    /// Цель ответила по сети.
    pub r: bool,
    /// Проба прошла полностью.
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mbps: Option<f64>,
    /// Причина провала (`Dns`, `Connect`, `Timeout`, `Auth`, `Http`, `Config`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub f: Option<String>,
    /// `true` — трафик к цели идёт мимо VPN («Без ВПН», v0.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nv: Option<bool>,
}

/// Одна проверка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// Время, мс UNIX.
    pub t: i64,
    /// Цвет светофора: `green` / `yellow` / `red`.
    pub c: String,
    /// Знак: `score` / `novpn` / `weak` / `unknown`.
    pub b: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s: Option<u32>,
    /// Задержка в оценке (медиана), мс.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    /// Скорость в оценке (медиана), Мбит/с.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spd: Option<f64>,
    /// Интернет, Мбит/с — только в проверке, где он мерился.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inet: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stab: Option<f64>,
    #[serde(default)]
    pub tg: BTreeMap<String, TargetRec>,
    /// Клиент VPN (и профиль), v0.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vpn: Option<String>,
    /// v0.5: задержка до интернета, мс; надбавка VPN, мс; доля канала, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ilat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ovh: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<f64>,
    /// v0.9: сеть (SSID или «кабель») — своя норма для каждой сети; тип (`wifi`/`cable`/`phone`); сигнал Wi‑Fi, %;
    /// разброс задержки до офиса, мс (звонок).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jit: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub computer: String,
    pub user: String,
    pub version: String,
    /// Логин AD (R5.4); пусто — не вошли.
    #[serde(default)]
    pub login: String,
    /// Позже — клиент и профиль VPN, город.
    #[serde(default)]
    pub period: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct File {
    meta: Meta,
    records: Vec<Record>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Record {
    pub fn from_snapshot(s: &Snapshot, internet_measured_now: bool) -> Self {
        let v = &s.verdict;
        let c = match v.color {
            Color::Green => "green",
            Color::Yellow => "yellow",
            Color::Red => "red",
            Color::Gray => "gray",
        };
        let b = match v.badge {
            Badge::Score(_) => "score",
            Badge::NoVpn => "novpn",
            Badge::WeakInternet(_) => "weak",
            Badge::Unknown => "unknown",
            Badge::NoInternet => "offline",
            Badge::Captive => "captive",
            Badge::Paused => "paused",
        };
        let tg = s
            .targets
            .iter()
            .map(|r| {
                (
                    r.target_id.clone(),
                    TargetRec {
                        r: r.reachable,
                        ok: r.ok,
                        lat: r.latency_ms.map(round1),
                        mbps: r.mbps.map(round2),
                        f: r.fail.map(|f| format!("{f:?}")),
                        nv: r.via_vpn.map(|v| !v).filter(|nv| *nv),
                    },
                )
            })
            .collect();
        Record {
            t: s.at
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            c: c.into(),
            b: b.into(),
            s: v.score,
            lat: v.latency.map(round1),
            spd: v.speed.map(round2),
            inet: if internet_measured_now {
                s.internet.as_ref().and_then(|i| i.mbps).map(round2)
            } else {
                None
            },
            stab: v.stability.map(round2),
            tg,
            vpn: s.client.as_ref().map(crate::vpn::Client::key),
            ilat: s.internet_latency_ms.map(round1),
            ovh: s.overhead_ms.map(round1),
            share: s.share.map(round2),
            net: s
                .net
                .as_ref()
                .map(|n| n.name.clone())
                .filter(|n| !n.is_empty()),
            nk: s.net.as_ref().map(|n| n.kind.clone()),
            sig: s.net.as_ref().and_then(|n| n.signal),
            jit: s.call.as_ref().map(|c| c.jitter_ms),
        }
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// История проверок в памяти (текущий день + 7 полных) и запись файлов.
pub struct Store {
    dir: PathBuf,
    meta: Meta,
    pub records: Vec<Record>,
}

/// Начало местных суток (мс UNIX), в которые попадает `t_ms`.
pub fn local_day_start(t_ms: i64) -> i64 {
    let off = crate::log::local_offset_secs() * 1000;
    let day = 86_400_000;
    (t_ms + off).div_euclid(day) * day - off
}

impl Store {
    /// Загружает историю из `metrics_7d.json` (если есть).
    pub fn open(dir: &Path, meta: Meta) -> Self {
        let records = std::fs::read_to_string(dir.join("metrics_7d.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<File>(&t).ok())
            .map(|f| f.records)
            .unwrap_or_default();
        let mut s = Store {
            dir: dir.to_path_buf(),
            meta,
            records,
        };
        s.trim(now_ms());
        s
    }

    fn trim(&mut self, now: i64) {
        let from = local_day_start(now) - 7 * 86_400_000;
        self.records.retain(|r| r.t >= from);
    }

    /// Добавляет проверку и переписывает оба файла. Ошибка записи не роняет приложение — уходит в лог.
    pub fn push(&mut self, rec: Record) -> Result<(), String> {
        let now = rec.t;
        self.records.push(rec);
        self.trim(now);
        let today = local_day_start(now);
        let day: Vec<Record> = self
            .records
            .iter()
            .filter(|r| r.t >= today)
            .cloned()
            .collect();
        self.write("metrics_1d.json", "сутки", day)?;
        self.write(
            "metrics_7d.json",
            "сегодня и 7 полных дней",
            self.records.clone(),
        )
    }

    fn write(&self, name: &str, period: &str, records: Vec<Record>) -> Result<(), String> {
        let mut meta = self.meta.clone();
        meta.period = period.into();
        let text = serde_json::to_string(&File { meta, records }).map_err(|e| e.to_string())?;
        let path = self.dir.join(name);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Логин AD для шапки файлов (после входа или выхода в окне).
    pub fn set_login(&mut self, login: &str) {
        self.meta.login = login.into();
    }

    /// Своя норма (v0.4): медианы задержки и скорости за `baseline_days` по проверкам, когда VPN работал. Пока
    /// проверок меньше `baseline_min_samples` — нормы нет, оценка идёт по абсолютным порогам.
    /// Норма своя для каждого клиента и профиля VPN (`client` — «Happ / Lisbon»); пока по нему мало проверок —
    /// общая.
    pub fn baseline(
        &self,
        th: &crate::settings::Thresholds,
        client: Option<&str>,
    ) -> Option<crate::score::Baseline> {
        self.baseline_for(th, client, None, &BTreeMap::new())
    }

    /// Своя норма (v0.9): сначала по сети и клиенту VPN, потом по сети, по клиенту, по всем — что набрало
    /// `baseline_min_samples`. «Забытая» сеть (`forgot`: сеть → мс) не учитывает свои записи до забывания.
    pub fn baseline_for(
        &self,
        th: &crate::settings::Thresholds,
        client: Option<&str>,
        net: Option<&str>,
        forgot: &BTreeMap<String, i64>,
    ) -> Option<crate::score::Baseline> {
        let from = now_ms() - th.baseline_days as i64 * 86_400_000;
        let all: Vec<&Record> = self
            .records
            .iter()
            .filter(|r| r.t >= from && r.b == "score")
            .filter(|r| {
                match (
                    r.net.as_deref(),
                    r.net.as_deref().and_then(|n| forgot.get(n)),
                ) {
                    (Some(_), Some(at)) => r.t > *at,
                    _ => true,
                }
            })
            .collect();
        let of_net: Vec<&Record> = net
            .map(|n| {
                all.iter()
                    .copied()
                    .filter(|r| r.net.as_deref() == Some(n))
                    .collect()
            })
            .unwrap_or_default();
        let net_samples = of_net.len();
        fn by_client<'a>(v: &[&'a Record], client: Option<&str>) -> Vec<&'a Record> {
            client
                .map(|c| {
                    v.iter()
                        .copied()
                        .filter(|r| r.vpn.as_deref() == Some(c))
                        .collect()
                })
                .unwrap_or_default()
        }
        let enough = |v: &Vec<&Record>| v.len() >= th.baseline_min_samples;
        let (ok, used_client, used_net) = {
            let nc = by_client(&of_net, client);
            if enough(&nc) {
                (nc, client.map(str::to_string), net.map(str::to_string))
            } else if enough(&of_net) {
                (of_net, None, net.map(str::to_string))
            } else {
                let c = by_client(&all, client);
                if enough(&c) {
                    (c, client.map(str::to_string), None)
                } else {
                    (all, None, None)
                }
            }
        };
        let med = |mut v: Vec<f64>, min: usize| -> Option<f64> {
            if v.len() < min {
                return None;
            }
            v.sort_by(|a, b| a.total_cmp(b));
            Some(v[v.len() / 2])
        };
        let n = th.baseline_min_samples;
        let lat = med(ok.iter().filter_map(|r| r.lat).collect(), n);
        let spd = med(ok.iter().filter_map(|r| r.spd).collect(), n);
        let ovh = med(ok.iter().filter_map(|r| r.ovh).collect(), n);
        let share = med(ok.iter().filter_map(|r| r.share).collect(), n);
        // интернет мерится реже — хватит 5 замеров
        let inet = med(ok.iter().filter_map(|r| r.inet).collect(), 5);
        (lat.is_some() || spd.is_some() || ovh.is_some() || share.is_some()).then_some(
            crate::score::Baseline {
                latency_ms: lat,
                speed_mbps: spd,
                samples: ok.len(),
                overhead_ms: ovh,
                share,
                client: used_client,
                net: used_net,
                net_samples,
                inet_mbps: inet,
            },
        )
    }

    /// Записи с момента `from_ms`.
    pub fn since(&self, from_ms: i64) -> Vec<Record> {
        self.records
            .iter()
            .filter(|r| r.t >= from_ms)
            .cloned()
            .collect()
    }
}

/// Статистика по сети за неделю (v0.9) — раздел «Ваши сети» и описание запроса в Jira.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NetStat {
    pub name: String,
    pub kind: String,
    pub checks: usize,
    pub last_ms: i64,
    pub inet_mbps: Option<f64>,
    pub vpn_ms: Option<f64>,
    pub svn_mbps: Option<f64>,
    pub signal: Option<u32>,
    /// Доля проверок «плохо» (красный), 0..1.
    pub bad: f64,
}

/// Сети из записей: по каждой — сколько проверок, медианы, доля «плохо»; свежие первыми.
pub fn networks(records: &[Record]) -> Vec<NetStat> {
    let mut by: BTreeMap<String, Vec<&Record>> = BTreeMap::new();
    for r in records {
        if let Some(n) = r.net.as_deref().filter(|n| !n.is_empty()) {
            by.entry(n.to_string()).or_default().push(r);
        }
    }
    let med = |mut v: Vec<f64>| -> Option<f64> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    };
    let mut out: Vec<NetStat> = by
        .into_iter()
        .map(|(name, rs)| NetStat {
            kind: rs
                .iter()
                .rev()
                .find_map(|r| r.nk.clone())
                .unwrap_or_default(),
            checks: rs.len(),
            last_ms: rs.iter().map(|r| r.t).max().unwrap_or(0),
            inet_mbps: med(rs.iter().filter_map(|r| r.inet).collect()),
            vpn_ms: med(rs.iter().filter_map(|r| r.lat).collect()),
            svn_mbps: med(rs.iter().filter_map(|r| r.spd).collect()),
            signal: med(rs.iter().filter_map(|r| r.sig.map(f64::from)).collect()).map(|v| v as u32),
            bad: rs.iter().filter(|r| r.c == "red").count() as f64 / rs.len() as f64,
            name,
        })
        .collect();
    out.sort_by_key(|n| std::cmp::Reverse(n.last_ms));
    out
}

/// Те же сети текстом — для описания запроса в Jira.
pub fn networks_text(list: &[NetStat]) -> String {
    let f = |v: Option<f64>, u: &str| {
        v.map(|x| format!("{x:.1} {u}"))
            .unwrap_or_else(|| "—".into())
    };
    let mut out = vec![
        "Сети за неделю (сеть, тип, проверок, интернет, VPN, SVN, сигнал, плохо):".to_string(),
    ];
    for n in list {
        out.push(format!(
            "- {} ({}): {}, интернет {}, VPN {}, SVN {}, сигнал {}, плохо {:.0}%",
            n.name,
            if n.kind.is_empty() { "?" } else { &n.kind },
            n.checks,
            f(n.inet_mbps, "Мбит/с"),
            f(n.vpn_ms, "мс"),
            f(n.svn_mbps, "Мбит/с"),
            n.signal
                .map(|s| format!("{s}%"))
                .unwrap_or_else(|| "—".into()),
            n.bad * 100.0
        ));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(t: i64) -> Record {
        Record {
            t,
            c: "green".into(),
            b: "score".into(),
            s: Some(80),
            lat: Some(180.0),
            spd: Some(3.0),
            inet: None,
            stab: None,
            tg: BTreeMap::new(),
            vpn: None,
            ilat: None,
            ovh: None,
            share: None,
            net: None,
            nk: None,
            sig: None,
            jit: None,
        }
    }

    #[test]
    fn baseline_per_network_and_forget() {
        let th = crate::settings::Settings::embedded().thresholds;
        let now = now_ms();
        let meta = Meta {
            computer: "PC".into(),
            user: "u".into(),
            version: "0".into(),
            login: String::new(),
            period: String::new(),
        };
        let dir = std::env::temp_dir().join(format!("vpnmon-net-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = Store::open(&dir, meta);
        for i in 0..80 {
            let mut r = rec(now - i * 60_000);
            r.net = Some("Rykov_5G".into());
            r.spd = Some(12.0);
            st.records.push(r);
            let mut r = rec(now - i * 60_000 - 30_000);
            r.net = Some("Rykov_2G".into());
            r.spd = Some(3.0);
            st.records.push(r);
        }
        let b = st
            .baseline_for(&th, None, Some("Rykov_5G"), &BTreeMap::new())
            .unwrap();
        assert_eq!(
            (b.speed_mbps, b.net.as_deref(), b.net_samples),
            (Some(12.0), Some("Rykov_5G"), 80)
        );
        // новая сеть — норма общая, проверок в сети 0 («здесь вы впервые»)
        let b = st
            .baseline_for(&th, None, Some("Hotel_Lisboa"), &BTreeMap::new())
            .unwrap();
        assert_eq!((b.net, b.net_samples), (None, 0));
        // «Забыть сеть» — её старые записи не в счёт
        let forgot = BTreeMap::from([("Rykov_5G".to_string(), now + 1)]);
        assert_eq!(
            st.baseline_for(&th, None, Some("Rykov_5G"), &forgot)
                .unwrap()
                .net_samples,
            0
        );
    }

    #[test]
    fn networks_stats() {
        let now = now_ms();
        let mut v = vec![];
        for i in 0..10 {
            let mut r = rec(now - i * 60_000);
            r.net = Some("Rykov_2G".into());
            r.nk = Some("wifi".into());
            r.sig = Some(30 + i as u32);
            if i < 4 {
                r.c = "red".into();
            }
            v.push(r);
        }
        let mut r = rec(now - 86_400_000);
        r.net = Some("кабель".into());
        v.push(r);
        let n = networks(&v);
        assert_eq!(
            (n[0].name.as_str(), n[0].checks, n[0].kind.as_str()),
            ("Rykov_2G", 10, "wifi")
        );
        assert!((n[0].bad - 0.4).abs() < 1e-9);
        assert_eq!(n[1].name, "кабель");
        assert!(networks_text(&n).contains("Rykov_2G (wifi): 10"));
    }

    #[test]
    fn baseline_needs_enough_samples() {
        let meta = Meta {
            computer: "PC".into(),
            user: "u".into(),
            version: "0".into(),
            login: String::new(),
            period: String::new(),
        };
        let dir = std::env::temp_dir().join(format!("vpnmon-base-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = Store::open(&dir, meta);
        let th = crate::settings::Settings::embedded().thresholds;
        let now = now_ms();
        for i in 0..10 {
            st.records.push(rec(now - i * 60_000));
        }
        assert!(st.baseline(&th, None).is_none(), "10 проверок — мало");
        for i in 10..100 {
            st.records.push(rec(now - i * 60_000));
        }
        let b = st.baseline(&th, None).unwrap();
        assert_eq!(b.latency_ms, Some(180.0));
        assert_eq!(b.speed_mbps, Some(3.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn writes_day_and_week_and_trims() {
        let dir = std::env::temp_dir().join(format!("vpnmon-metrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let meta = Meta {
            computer: "PC".into(),
            user: "u".into(),
            version: "0".into(),
            login: String::new(),
            period: String::new(),
        };
        let mut st = Store::open(&dir, meta.clone());
        let now = now_ms();
        st.records.push(rec(now - 10 * 86_400_000)); // старше окна — уйдёт
        st.records.push(rec(now - 2 * 86_400_000));
        st.push(rec(now)).unwrap();
        assert_eq!(st.records.len(), 2);
        let st2 = Store::open(&dir, meta);
        assert_eq!(st2.records.len(), 2, "история читается из metrics_7d.json");
        let day: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("metrics_1d.json")).unwrap())
                .unwrap();
        assert_eq!(day["records"].as_array().unwrap().len(), 1);
        assert_eq!(day["meta"]["computer"], "PC");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Все JSON обратно совместимы: файл старой версии (без новых полей) и файл будущей (с незнакомыми) читаются.
    #[test]
    fn metrics_files_are_backward_and_forward_compatible() {
        let old = r#"{"meta":{"computer":"PC","user":"u","version":"0.2.0","period":"сутки"},
            "records":[{"t":1,"c":"green","b":"score","s":80,"tg":{"svn":{"r":true,"ok":true}}}]}"#;
        let f: File = serde_json::from_str(old).unwrap();
        assert_eq!(f.meta.login, "");
        let future = r#"{"meta":{"computer":"PC","user":"u","version":"9.0.0","login":"x","city":"Lisbon"},
            "records":[{"t":1,"c":"green","b":"newbadge","vpn_client":"Happ","tg":{}}],"extra":1}"#;
        assert!(serde_json::from_str::<File>(future).is_ok());
    }

    #[test]
    fn day_start_is_before_or_equal() {
        let t = now_ms();
        let d = local_day_start(t);
        assert!(d <= t && t - d < 86_400_000);
    }
}
