//! Вход логином AD (v0.3, ADR 0006): проверить логин и пароль на Jira, Confluence и SVN, при успехе — сохранить в
//! Диспетчер учётных данных и получить токен GitLab. Пароль не пишется ни в лог, ни в ответы API.

use crate::creds::{self, AdCreds};
use crate::probe::base64;
use crate::settings::{AuthKind, Settings};
use crate::{info, warn};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct ServiceCheck {
    pub service: String,
    /// `ok` — вход принят; `denied` — логин или пароль не подошли; `unreachable` — сервис не ответил.
    pub result: &'static str,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResult {
    pub saved: bool,
    pub login: String,
    pub checks: Vec<ServiceCheck>,
    pub message: String,
}

fn origin(url: &str) -> Option<String> {
    let u: ureq::http::Uri = url.parse().ok()?;
    Some(format!("{}://{}", u.scheme_str()?, u.authority()?))
}

fn check(
    service: &str,
    url: &str,
    user: &str,
    pass: &str,
    user_field: Option<&str>,
) -> ServiceCheck {
    let agent = crate::net::agent(Duration::from_secs(15));
    let auth = format!("Basic {}", base64(format!("{user}:{pass}").as_bytes()));
    let r = agent.get(url).header("Authorization", auth.as_str()).call();
    let (result, detail) = match r {
        Err(e) => ("unreachable", format!("не ответил: {e}")),
        Ok(mut resp) => {
            let st = resp.status().as_u16();
            let final_uri = ureq::ResponseExt::get_uri(&resp).to_string();
            let who = resp
                .headers()
                .get("X-AUSERNAME")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let body = if user_field.is_some() {
                resp.body_mut().read_to_string().unwrap_or_default()
            } else {
                String::new()
            };
            let from_json = user_field.and_then(|f| {
                serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v[f].as_str().map(str::to_string))
            });
            let name = who.or(from_json).filter(|n| n != "anonymous");
            match st {
                200..=299 if final_uri.contains("login") => {
                    ("denied", "перенаправил на страницу входа".into())
                }
                200..=299 if user_field.is_some() && name.is_none() => {
                    ("denied", "вход не принят (анонимно)".into())
                }
                200..=299 => (
                    "ok",
                    name.map(|n| format!("вошли как «{n}»"))
                        .unwrap_or_else(|| "вход принят".into()),
                ),
                401 | 403 => ("denied", format!("логин или пароль не подошли (HTTP {st})")),
                _ => ("unreachable", format!("HTTP {st}")),
            }
        }
    };
    ServiceCheck {
        service: service.into(),
        result,
        detail,
    }
}

/// Проверяет логин и пароль на всех сервисах; хоть один принял — сохраняет.
pub fn login(settings: &Settings, user: &str, password: &str) -> LoginResult {
    let user = user.trim();
    let url_of = |kind: AuthKind| {
        settings
            .targets
            .iter()
            .find(|t| t.auth == kind)
            .map(|t| t.url.clone())
    };
    let mut jobs: Vec<(String, String, Option<&str>)> = vec![];
    if let Some(o) = url_of(AuthKind::Jira).as_deref().and_then(origin) {
        jobs.push((
            "Jira".into(),
            format!("{o}/rest/api/2/myself"),
            Some("name"),
        ));
    }
    if let Some(o) = url_of(AuthKind::Confluence).as_deref().and_then(origin) {
        jobs.push((
            "Confluence".into(),
            format!("{o}/rest/api/user/current"),
            Some("username"),
        ));
    }
    if let Some(u) = url_of(AuthKind::Svn) {
        jobs.push(("SVN".into(), u, None));
    }
    let checks: Vec<ServiceCheck> = std::thread::scope(|sc| {
        let hs: Vec<_> = jobs
            .iter()
            .map(|(s, u, f)| sc.spawn(move || check(s, u, user, password, *f)))
            .collect();
        hs.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    for c in &checks {
        info!(
            "login",
            "проверка логина AD «{user}» в {}: {} — {}", c.service, c.result, c.detail
        );
    }
    let any_ok = checks.iter().any(|c| c.result == "ok");
    let all_unreachable = checks.iter().all(|c| c.result == "unreachable");
    if !any_ok {
        let message = if all_unreachable {
            "Сервисы не ответили — включите VPN и попробуйте снова. Ничего не сохранено.".into()
        } else {
            "Логин или пароль не подошли. Ничего не сохранено.".into()
        };
        return LoginResult {
            saved: false,
            login: user.into(),
            checks,
            message,
        };
    }
    if let Err(e) = creds::save_ad(&AdCreds {
        login: user.into(),
        password: password.into(),
    }) {
        return LoginResult {
            saved: false,
            login: user.into(),
            checks,
            message: e,
        };
    }
    info!(
        "login",
        "логин AD «{user}» сохранён в Диспетчере учётных данных"
    );
    let mut message =
        "Вход сохранён в Диспетчере учётных данных Windows. Проверки идут с ним.".to_string();
    if let Some(cfg) = &settings.gitlab_oauth {
        match crate::gitlab::login(cfg, user, password) {
            Ok(()) => message += " GitLab: получен токен только на чтение.",
            Err(e) => {
                warn!("login", "GitLab по паролю AD: {e}");
                message += &format!(" GitLab по паролю AD не вышел ({e}) — нужен GITLAB_TOKEN.");
            }
        }
    }
    LoginResult {
        saved: true,
        login: user.into(),
        checks,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_of_urls() {
        assert_eq!(
            origin("https://jira.corp.x.com/rest/api/2/myself").as_deref(),
            Some("https://jira.corp.x.com")
        );
        assert_eq!(
            origin("http://svn.corp.x.com:8080/svn/a").as_deref(),
            Some("http://svn.corp.x.com:8080")
        );
    }

    #[test]
    fn nothing_saved_when_unreachable() {
        let mut s = Settings::embedded();
        for t in &mut s.targets {
            t.url = "http://127.0.0.1:9/x".into(); // порт discard: соединение отклонено
        }
        let r = login(&s, "u", "p");
        assert!(!r.saved);
        assert!(r.message.contains("VPN"));
    }
}
