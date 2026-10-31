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
