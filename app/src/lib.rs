//! VPN Monitor: логика, общая для всех платформ (пробы, оценка, иконка, обновление, лог).
//! Трей и установка — только Windows, в `main.rs`.

pub mod diag;
pub mod icon;
pub mod log;
pub mod metrics;
pub mod monitor;
pub mod net;
pub mod probe;
pub mod score;
pub mod secrets;
pub mod settings;
pub mod update;

/// Версия этой сборки (из Cargo.toml; CI проверяет, что совпадает с тегом релиза).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
