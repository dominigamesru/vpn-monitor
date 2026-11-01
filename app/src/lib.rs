//! VPN Monitor: логика, общая для всех платформ (пробы, оценка, иконка, обновление, лог).
//! Трей и установка — только Windows, в `main.rs`.

pub mod advice;
pub mod creds;
pub mod diag;
pub mod gitlab;
pub mod icon;
pub mod jsm;
pub mod log;
pub mod login;
pub mod mcp;
pub mod metrics;
pub mod monitor;
pub mod net;
pub mod notify;
pub mod office;
pub mod prefs;
pub mod probe;
pub mod route;
pub mod score;
pub mod secrets;
pub mod settings;
pub mod update;
pub mod vpn;

/// Версия этой сборки (из Cargo.toml; CI проверяет, что совпадает с тегом релиза).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
