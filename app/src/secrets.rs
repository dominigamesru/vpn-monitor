//! Секреты: креды целей на машине сотрудника. Порядок: переменная окружения → `secrets.env` (KEY=VALUE, как `.env`) в
//! папке установки → логин и пароль AD из Диспетчера учётных данных (v0.3, ADR 0006) для `*_USER` / `*_PASSWORD`
//! Confluence, Jira и SVN; токен GitLab — `GITLAB_TOKEN`, иначе OAuth-токен, полученный по паролю AD.
//! Значения никогда не пишутся в лог — только «задано / пусто» и откуда.

use std::collections::HashMap;
use std::path::Path;

/// Ключи, о которых знает приложение (для строки «что задано» в логе).
pub const KNOWN_KEYS: &[&str] = &[
    "CONFLUENCE_TOKEN",
    "CONFLUENCE_USER",
    "CONFLUENCE_PASSWORD",
    "SVN_USER",
    "SVN_PASSWORD",
    "JIRA_TOKEN",
    "JIRA_USER",
    "JIRA_PASSWORD",
    "GITLAB_TOKEN",
    "CONFLUENCE_TEST_PAGE_URL",
    "JIRA_PROBE_URL",
    "SVN_TEST_FILE_LITE_PATH",
    "SVN_TEST_FILE_PATH",
    "GITLAB_PROBE_URL",
];

#[derive(Debug, Default, Clone)]
pub struct Secrets {
    file: HashMap<String, String>,
    ad: Option<crate::creds::AdCreds>,
    gitlab_oauth: Option<String>,
    /// Тесты выключают окружение, чтобы не зависеть от переменных машины.
    ignore_env: bool,
}

impl Secrets {
    pub fn load(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let gitlab_oauth = crate::creds::load_gitlab()
            .filter(|t| {
                t.expires_at == 0 || t.expires_at > crate::metrics::now_ms() as u64 / 1000 + 60
            })
            .map(|t| t.access_token);
        Self {
            file: parse_env(&text),
            ad: crate::creds::load_ad(),
            gitlab_oauth,
            ignore_env: false,
        }
    }

    #[cfg(test)]
    pub fn from_text(text: &str) -> Self {
        Self {
            file: parse_env(text),
            ad: None,
            gitlab_oauth: None,
            ignore_env: true,
        }
    }

    #[cfg(test)]
    pub fn with_ad(mut self, login: &str, password: &str) -> Self {
        self.ad = Some(crate::creds::AdCreds {
            login: login.into(),
            password: password.into(),
        });
        self
    }

    /// Логин AD, если сохранён.
    pub fn ad_login(&self) -> Option<&str> {
        self.ad.as_ref().map(|a| a.login.as_str())
    }

    /// OAuth-токен GitLab, полученный по паролю AD (если есть и не истёк).
    pub fn gitlab_oauth(&self) -> Option<&str> {
        self.gitlab_oauth.as_deref()
    }

    /// Откуда значение: `окружение`, `secrets.env`, `AD` — для диагностики (без самого значения).
    pub fn source(&self, key: &str) -> Option<&'static str> {
        if !self.ignore_env && std::env::var(key).is_ok_and(|v| !v.trim().is_empty()) {
            return Some("окружение");
        }
        if self.file.get(key).is_some_and(|v| !v.is_empty()) {
            return Some("secrets.env");
        }
        self.ad_value(key).map(|_| "AD")
    }

    fn ad_value(&self, key: &str) -> Option<String> {
        let ad = self.ad.as_ref()?;
        match key {
            "CONFLUENCE_USER" | "JIRA_USER" | "SVN_USER" => Some(ad.login.clone()),
            "CONFLUENCE_PASSWORD" | "JIRA_PASSWORD" | "SVN_PASSWORD" => Some(ad.password.clone()),
            _ => None,
        }
    }

    /// Непустое значение: сначала окружение, потом файл.
    pub fn get(&self, key: &str) -> Option<String> {
        if !self.ignore_env
            && let Ok(v) = std::env::var(key)
            && !v.trim().is_empty()
        {
            return Some(v.trim().to_string());
        }
        self.file
            .get(key)
            .filter(|v| !v.is_empty())
            .cloned()
            .or_else(|| self.ad_value(key))
    }

    /// «KEY=задано, KEY2=пусто» — без значений, для лога.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = KNOWN_KEYS
            .iter()
            .map(|k| match self.source(k) {
                Some(src) => format!("{k}=задано({src})"),
                None => format!("{k}=пусто"),
            })
            .collect();
        parts.push(format!("логин AD={}", self.ad_login().unwrap_or("нет")));
        parts.push(format!(
            "GitLab OAuth={}",
            if self.gitlab_oauth.is_some() {
                "есть"
            } else {
                "нет"
            }
        ));
        parts.join(", ")
    }
}

fn parse_env(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let mut v = v.trim();
            if v.len() >= 2
                && ((v.starts_with('"') && v.ends_with('"'))
                    || (v.starts_with('\'') && v.ends_with('\'')))
            {
                v = &v[1..v.len() - 1];
            }
            // заглушки из .env.example — как пусто
            if matches!(
                v,
                "paste_token_here" | "paste_password_here" | "your_ad_login"
            ) {
                v = "";
            }
            map.insert(k.trim().to_string(), v.to_string());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_format() {
        let s = Secrets::from_text(
            "# c\nSVN_USER=ivan\nSVN_PASSWORD=\"p=w\"\nGITLAB_TOKEN=paste_token_here\n\nexport X_TEST_KEY_Q='a b'\n",
        );
        assert_eq!(s.get("SVN_USER").as_deref(), Some("ivan"));
        assert_eq!(s.get("SVN_PASSWORD").as_deref(), Some("p=w"));
        assert_eq!(s.get("GITLAB_TOKEN"), None);
        assert_eq!(s.get("X_TEST_KEY_Q").as_deref(), Some("a b"));
    }

    #[test]
    fn ad_fills_user_and_password_but_file_wins() {
        let s = Secrets::from_text("SVN_USER=other\n").with_ad("s.rykov", "adpass");
        assert_eq!(s.get("SVN_USER").as_deref(), Some("other"));
        assert_eq!(s.get("SVN_PASSWORD").as_deref(), Some("adpass"));
        assert_eq!(s.get("JIRA_USER").as_deref(), Some("s.rykov"));
        assert_eq!(s.get("GITLAB_TOKEN"), None);
        assert_eq!(s.source("JIRA_PASSWORD"), Some("AD"));
        assert!(!s.describe().contains("adpass"));
    }

    #[test]
    fn describe_has_no_values() {
        let s = Secrets::from_text("SVN_PASSWORD=supersecret\n");
        let d = s.describe();
        assert!(!d.contains("supersecret"));
        assert!(d.contains("SVN_PASSWORD=задано(secrets.env)"));
    }
}
