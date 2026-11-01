//! Удалённые настройки без релиза (v0.7): `office.json` в релизе `latest` — только те поля, что меняем (пороги,
//! сервисы, адрес поддержки). Приложение проверяет его раз в 5 минут, накладывает поверх своих настроек и
//! применяет без перезапуска; копия лежит в папке установки — для запуска без сети.
//!
//! Наложение: объекты — рекурсивно; `targets` — по `id` (поля цели меняются, новые цели добавляются); остальные
//! массивы заменяются целиком. Результат проходит ту же проверку, что `settings.json`; ошибка — не применяем.

use crate::settings::Settings;
use serde_json::Value;
use std::time::Duration;

pub const FILE: &str = "office.json";
/// Как часто проверять удалённые настройки.
pub const PERIOD: Duration = Duration::from_secs(5 * 60);

pub fn url(base: &str) -> String {
    format!("{}/latest/{FILE}", base.trim_end_matches('/'))
}

fn deep_merge(a: &mut Value, b: &Value, key: &str) {
    match (a, b) {
        (Value::Object(ao), Value::Object(bo)) => {
            for (k, bv) in bo {
                match ao.get_mut(k) {
                    Some(av) => deep_merge(av, bv, k),
                    None => {
                        ao.insert(k.clone(), bv.clone());
                    }
                }
            }
        }
        (Value::Array(aa), Value::Array(ba)) if key == "targets" => {
            for bt in ba {
                let id = bt.get("id").and_then(|x| x.as_str());
                match id.and_then(|id| {
                    aa.iter_mut()
                        .find(|t| t.get("id").and_then(|x| x.as_str()) == Some(id))
                }) {
                    Some(at) => deep_merge(at, bt, ""),
                    None => aa.push(bt.clone()),
                }
            }
        }
        (av, bv) => *av = bv.clone(),
    }
}

/// Накладывает `office.json` на настройки. Возвращает новые настройки и верхние ключи, которые поменялись.
pub fn apply(base: &Settings, office_text: &str) -> Result<(Settings, Vec<String>), String> {
    let office: Value = serde_json::from_str(office_text).map_err(|e| format!("{FILE}: {e}"))?;
    let Value::Object(o) = &office else {
        return Err(format!("{FILE}: ждали объект JSON"));
    };
    let keys: Vec<String> = o.keys().cloned().collect();
    let mut merged = serde_json::to_value(base).map_err(|e| e.to_string())?;
    deep_merge(&mut merged, &office, "");
    let s = Settings::parse(&merged.to_string()).map_err(|e| format!("{FILE}: {e}"))?;
    Ok((s, keys))
}

/// Скачивает `office.json`; `None` — файла нет (404): удалённых настроек нет.
pub fn fetch(base: &str) -> Result<Option<String>, String> {
    let u = url(base);
    let agent = crate::net::agent(Duration::from_secs(20));
    let mut r = agent
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

    #[test]
    fn office_overrides_thresholds_and_targets_by_id() {
        let base = Settings::embedded();
        let (s, keys) = apply(
            &base,
            r#"{"thresholds": {"vpn_good_ms": 400}, "support_url": "https://example/sd",
                "targets": [{"id": "gitlab", "enabled": true}], "future_field": 1}"#,
        )
        .unwrap();
        assert_eq!(s.thresholds.vpn_good_ms, 400.0);
        assert_eq!(
            s.thresholds.svn_good_mbps, base.thresholds.svn_good_mbps,
            "остальные пороги — как были"
        );
        assert_eq!(s.support_url, "https://example/sd");
        assert!(s.targets.iter().find(|t| t.id == "gitlab").unwrap().enabled);
        assert_eq!(
            s.targets.len(),
            base.targets.len(),
            "цель по id меняется, а не добавляется"
        );
        assert!(keys.contains(&"thresholds".to_string()));
    }

    #[test]
    fn broken_office_is_not_applied() {
        let base = Settings::embedded();
        assert!(apply(&base, "не json").is_err());
        assert!(
            apply(&base, r#"{"schema": 2}"#).is_err(),
            "результат проверяется как settings.json"
        );
        assert!(apply(&base, "[1]").is_err());
    }
}
