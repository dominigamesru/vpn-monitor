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
        let from = now_ms() - th.baseline_days as i64 * 86_400_000;
        let all: Vec<&Record> = self
            .records
            .iter()
            .filter(|r| r.t >= from && r.b == "score")
            .collect();
        let mine: Vec<&Record> = client
            .map(|c| {
                all.iter()
                    .copied()
                    .filter(|r| r.vpn.as_deref() == Some(c))
                    .collect()
            })
            .unwrap_or_default();
        let (ok, used_client) = if mine.len() >= th.baseline_min_samples {
            (mine, client.map(str::to_string))
        } else {
            (all, None)
        };
        let med = |mut v: Vec<f64>| -> Option<f64> {
            if v.len() < th.baseline_min_samples {
                return None;
            }
            v.sort_by(|a, b| a.total_cmp(b));
            Some(v[v.len() / 2])
        };
        let lat = med(ok.iter().filter_map(|r| r.lat).collect());
        let spd = med(ok.iter().filter_map(|r| r.spd).collect());
        let ovh = med(ok.iter().filter_map(|r| r.ovh).collect());
        let share = med(ok.iter().filter_map(|r| r.share).collect());
        (lat.is_some() || spd.is_some() || ovh.is_some() || share.is_some()).then_some(
            crate::score::Baseline {
                latency_ms: lat,
                speed_mbps: spd,
                samples: ok.len(),
                overhead_ms: ovh,
                share,
                client: used_client,
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
        }
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
