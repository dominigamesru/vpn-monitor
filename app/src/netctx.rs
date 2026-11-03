//! Контекст сети (v0.9, `docs/specs/v0.9-context.md`): через что сотрудник в интернете — Wi‑Fi (сигнал, диапазон),
//! кабель, телефон (хотспот, лимит); какая это сеть — для своей нормы у каждой сети; можно ли звонить (разброс
//! задержки и потери); сколько займёт 1 ГБ из SVN. Здесь — чистые функции; чтение Windows — в `win`.

use serde::{Deserialize, Serialize};

/// Сеть сейчас. Имя сети (SSID) остаётся на машине сотрудника — в историю и отчёт для поддержки, никуда больше.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Net {
    /// `wifi` / `cable` / `phone` / `unknown`.
    pub kind: String,
    /// Имя для нормы: SSID для Wi‑Fi и телефона, «кабель» для проводной сети.
    pub name: String,
    /// Wi‑Fi: качество сигнала 0–100 (Windows), диапазон «2,4 ГГц» / «5 ГГц» / «6 ГГц», скорость Wi‑Fi, Мбит/с.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub band: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_mbps: Option<f64>,
    /// Лимитная сеть (Windows) — проверки без скачивания.
    #[serde(default)]
    pub metered: bool,
}

/// Порог слабого сигнала, % (заказчик 2026-11-02: «годится»).
pub const WEAK_SIGNAL: u32 = 40;

/// Wi‑Fi слабый: сигнал ниже порога. Скорость связи с роутером не учитываем (0.9.5, заказчик): Windows показывает
/// согласованную скорость, а не реальную — она «погоды не делает» и сбивала с толку.
pub fn weak_wifi(net: &Net, _inet_mbps: Option<f64>) -> bool {
    net.kind == "wifi" && net.signal.is_some_and(|s| s < WEAK_SIGNAL)
}

/// Диапазон по номеру канала Wi‑Fi: 1–14 — 2,4 ГГц, 32–177 — 5 ГГц (6 ГГц Windows здесь не различает).
pub fn band_of(channel: u32) -> &'static str {
    match channel {
        1..=14 => "2,4 ГГц",
        32..=177 => "5 ГГц",
        _ => "",
    }
}

/// Хотспот телефона: адреса, которые раздают iPhone (172.20.10.x) и Android (192.168.43.x), или имя сети.
pub fn phone_hotspot(ipv4: &[String], ssid: &str) -> bool {
    let s = ssid.to_lowercase();
    ipv4.iter()
        .any(|a| a.starts_with("172.20.10.") || a.starts_with("192.168.43."))
        || s.contains("iphone")
        || s.contains("android")
        || s.contains("galaxy")
        || s.contains("redmi")
        || s.contains("hotspot")
}

/// Звонок по задержкам серии пингов (мс; `None` — не ответил): разброс (средняя разница соседних) и потери, %.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Call {
    pub jitter_ms: f64,
    pub loss_pct: f64,
    /// `good` — звонок пойдёт, `warn` — лучше без видео, `bad` — связь рвётся.
    pub st: String,
}

pub fn call_quality(pings: &[Option<f64>]) -> Option<Call> {
    if pings.len() < 4 {
        return None;
    }
    let ok: Vec<f64> = pings.iter().flatten().copied().collect();
    let loss = 100.0 * (pings.len() - ok.len()) as f64 / pings.len() as f64;
    let jitter = if ok.len() > 1 {
        ok.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (ok.len() - 1) as f64
    } else {
        0.0
    };
    let st = if loss > 10.0 || jitter > 80.0 {
        "bad"
    } else if loss > 2.0 || jitter > 30.0 {
        "warn"
    } else {
        "good"
    };
    Some(Call {
        jitter_ms: (jitter * 10.0).round() / 10.0,
        loss_pct: (loss * 10.0).round() / 10.0,
        st: st.into(),
    })
}

/// Сколько минут займёт 1 ГБ при скорости SVN, Мбит/с (1 ГБ = 8192 Мбит).
pub fn minutes_per_gb(svn_mbps: f64) -> Option<f64> {
    (svn_mbps > 0.05).then(|| 8192.0 / svn_mbps / 60.0)
}

/// Баблы под вердиктом в меню (0.9.11) — те же правила, что в `flyout.html` (`ctxRow`): (ключ, текст, почему).
/// Нужны для лога «повесил / снял и почему».
pub fn bubbles(
    net: &Net,
    call: Option<&Call>,
    svn_mbps: Option<f64>,
    usual_svn_mbps: Option<f64>,
) -> Vec<(&'static str, String, String)> {
    let mut out = vec![];
    if weak_wifi(net, None) {
        out.push((
            "wifi",
            format!("слабый Wi‑Fi {}%", net.signal.unwrap_or(0)),
            format!("сигнал {}% < {WEAK_SIGNAL}%", net.signal.unwrap_or(0)),
        ));
    } else if net.kind == "phone" || net.metered {
        out.push((
            "net",
            if net.kind == "phone" {
                "через телефон"
            } else {
                "лимитная сеть"
            }
            .into(),
            if net.kind == "phone" {
                "адрес или имя сети — хотспот телефона".into()
            } else {
                "Windows отмечает сеть как лимитную".into()
            },
        ));
    }
    if let Some(c) = call.filter(|c| c.st != "good") {
        out.push((
            "call",
            if c.st == "warn" {
                "лучше без видео"
            } else {
                "связь рвётся"
            }
            .into(),
            format!(
                "потери {:.1}%, разброс задержки {:.0} мс (8 TCP-соединений до офиса)",
                c.loss_pct, c.jitter_ms
            ),
        ));
    }
    if let (Some(now), Some(usual)) = (svn_mbps, usual_svn_mbps)
        && let (Some(a), Some(b)) = (minutes_per_gb(now), minutes_per_gb(usual))
        && a > 2.0 * b
    {
        out.push((
            "gb",
            format!("1 ГБ ≈ {a:.0} мин"),
            format!("SVN {now:.1} Мбит/с — вдвое медленнее обычных {usual:.1}"),
        ));
    }
    out
}

/// Сеть сейчас — по Windows: Wi‑Fi (WlanAPI), хотспот телефона, кабель; лимитная — NetworkCost.
#[cfg(windows)]
pub fn current() -> Net {
    let adapters = crate::diag::adapters();
    let ipv4: Vec<String> = adapters
        .iter()
        .filter(|a| a.up && !a.vpn_like)
        .flat_map(|a| a.ipv4.clone())
        .collect();
    let metered = crate::diag::win::metered();
    if let Some((ssid, signal, channel, rx)) = crate::diag::win::wifi() {
        let phone = phone_hotspot(&ipv4, &ssid);
        return Net {
            kind: if phone { "phone" } else { "wifi" }.into(),
            name: ssid,
            signal: Some(signal),
            band: band_of(channel).into(),
            link_mbps: (rx > 0.0).then_some(rx),
            metered,
        };
    }
    let cable = adapters
        .iter()
        .any(|a| a.up && !a.vpn_like && a.kind.starts_with("кабель"));
    Net {
        kind: if phone_hotspot(&ipv4, "") {
            "phone"
        } else if cable {
            "cable"
        } else {
            "unknown"
        }
        .into(),
        name: if cable {
            "кабель".into()
        } else {
            String::new()
        },
        metered,
        ..Default::default()
    }
}

#[cfg(not(windows))]
pub fn current() -> Net {
    Net::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_wifi_by_signal_or_link() {
        let mut n = Net {
            kind: "wifi".into(),
            name: "Rykov_2G".into(),
            signal: Some(32),
            band: "2,4 ГГц".into(),
            link_mbps: Some(26.0),
            metered: false,
        };
        assert!(weak_wifi(&n, Some(3.1)));
        n.signal = Some(82);
        n.link_mbps = Some(433.0);
        assert!(!weak_wifi(&n, Some(15.0)));
        n.link_mbps = Some(20.0); // скорость связи с роутером не в счёт — сигнал хороший
        assert!(!weak_wifi(&n, Some(15.0)));
        n.signal = Some(32);
        n.kind = "cable".into();
        assert!(!weak_wifi(&n, Some(15.0)));
        assert_eq!(band_of(6), "2,4 ГГц");
        assert_eq!(band_of(44), "5 ГГц");
    }

    #[test]
    fn bubbles_with_reasons() {
        // лог заказчика 03.11: одна потеря из 8 TCP — бабл «связь рвётся» и почему
        let call = call_quality(&[
            Some(1.0),
            Some(2.0),
            None,
            Some(1.0),
            Some(1.0),
            Some(2.0),
            Some(1.0),
            Some(1.0),
        ])
        .unwrap();
        let wifi = Net {
            kind: "wifi".into(),
            name: "Home".into(),
            signal: Some(80),
            ..Default::default()
        };
        let b = bubbles(&wifi, Some(&call), Some(10.0), Some(10.0));
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].0, "call");
        assert_eq!(b[0].1, "связь рвётся");
        assert!(b[0].2.contains("потери 12.5%"));
        let weak = Net {
            signal: Some(30),
            ..wifi
        };
        let b = bubbles(&weak, None, Some(2.0), Some(10.0));
        assert_eq!(
            b.iter().map(|x| x.0).collect::<Vec<_>>(),
            vec!["wifi", "gb"]
        );
    }

    #[test]
    fn hotspot_and_call_and_forecast() {
        assert!(phone_hotspot(&["172.20.10.3".into()], "Home"));
        assert!(phone_hotspot(&[], "iPhone Сергея"));
        assert!(!phone_hotspot(&["192.168.1.5".into()], "Rykov_5G"));
        let good =
            call_quality(&[Some(50.0), Some(52.0), Some(49.0), Some(51.0), Some(50.0)]).unwrap();
        assert_eq!(good.st, "good");
        let bad = call_quality(&[Some(50.0), Some(180.0), None, Some(40.0), Some(200.0)]).unwrap();
        assert_eq!(bad.st, "bad");
        assert!(call_quality(&[Some(1.0)]).is_none());
        // SVN 12,4 Мбит/с — 1 ГБ ≈ 11 мин; 1,9 — ≈ 72 мин (лог заказчика)
        assert_eq!(minutes_per_gb(12.4).unwrap().round(), 11.0);
        assert_eq!(minutes_per_gb(1.9).unwrap().round(), 72.0);
    }
}
