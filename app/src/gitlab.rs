//! GitLab: токен только на чтение (`read_api`) по паролю AD через OAuth (grant type `password`, ADR 0006).
//! Работает, только если в настройках есть `gitlab_oauth` (Application ID) и в GitLab разрешён такой вход.

use crate::creds::{self, GitlabToken};
use crate::settings::GitlabOauth;
use crate::{info, warn};
use std::time::Duration;

fn now_s() -> u64 {
    crate::metrics::now_ms() as u64 / 1000
}

fn token_request(cfg: &GitlabOauth, form: &[(&str, &str)]) -> Result<GitlabToken, String> {
    let url = format!("{}/oauth/token", cfg.url.trim_end_matches('/'));
    let agent = crate::net::agent(Duration::from_secs(20));
    let mut resp = agent
        .post(&url)
        .send_form(form.iter().copied())
        .map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    if status != 200 {
        // в теле — код ошибки OAuth (invalid_grant, unauthorized_client…), секретов там нет
        let code = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_default();
        return Err(format!("GitLab OAuth: HTTP {status} {code}"));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("GitLab OAuth: {e}"))?;
    let access = v["access_token"]
        .as_str()
        .ok_or("GitLab OAuth: нет access_token")?
        .to_string();
    let expires_in = v["expires_in"].as_u64().unwrap_or(0);
    Ok(GitlabToken {
        access_token: access,
        refresh_token: v["refresh_token"].as_str().map(str::to_string),
        expires_at: if expires_in > 0 {
            now_s() + expires_in
        } else {
            0
        },
    })
}

/// Получить и сохранить токен по логину и паролю AD.
pub fn login(cfg: &GitlabOauth, user: &str, password: &str) -> Result<(), String> {
    let t = token_request(
        cfg,
        &[
            ("grant_type", "password"),
            ("username", user),
            ("password", password),
            ("client_id", &cfg.client_id),
            ("scope", "read_api"),
        ],
    )?;
    creds::save_gitlab(&t)?;
    info!(
        "gitlab",
        "OAuth-токен read_api получен{}",
        if t.expires_at > 0 {
            " (со сроком)"
        } else {
            ""
        }
    );
    Ok(())
}

/// Обновить токен, если до истечения меньше 5 минут. Не вышло обновлением — пробуем паролем AD заново.
pub fn ensure_fresh(cfg: Option<&GitlabOauth>) {
    let Some(cfg) = cfg else { return };
    let Some(ad) = creds::load_ad() else { return };
    let tok = creds::load_gitlab();
    let fresh = tok
        .as_ref()
        .is_some_and(|t| t.expires_at == 0 || t.expires_at > now_s() + 300);
    if fresh {
        return;
    }
    if let Some(rt) = tok.and_then(|t| t.refresh_token) {
        match token_request(
            cfg,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &rt),
                ("client_id", &cfg.client_id),
            ],
        ) {
            Ok(t) => {
                let _ = creds::save_gitlab(&t);
                info!("gitlab", "OAuth-токен обновлён");
                return;
            }
            Err(e) => warn!("gitlab", "обновить токен не вышло: {e} — вхожу паролем AD"),
        }
    }
    if let Err(e) = login(cfg, &ad.login, &ad.password) {
        warn!("gitlab", "{e}");
    }
}
