//! HTTP-клиент: таймаут, редиректы, проверка сертификатов системой Windows (корпоративные CA).

use std::time::Duration;
use ureq::tls::{RootCerts, TlsConfig};

pub const USER_AGENT: &str = concat!("DominiVPNMonitor/", env!("CARGO_PKG_VERSION"));

pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(timeout.min(Duration::from_secs(10))))
        .http_status_as_error(false)
        .max_redirects(5)
        .user_agent(USER_AGENT)
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

/// Город по внешнему адресу (где нас видит интернет): `ipapi.co`, без ключа. Для шапки меню, v0.6.
pub fn city() -> Result<String, String> {
    let url = "https://ipapi.co/json/";
    let mut r = agent(Duration::from_secs(8))
        .get(url)
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let st = r.status().as_u16();
    if st != 200 {
        return Err(format!("{url}: HTTP {st}"));
    }
    let text = r
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{url}: {e}"))?;
    v["city"]
        .as_str()
        .map(str::trim)
        .filter(|c| !c.is_empty() && c.len() <= 60)
        .map(str::to_string)
        .ok_or_else(|| "в ответе нет города".into())
}
