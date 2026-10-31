//! Секреты: креды целей на машине сотрудника. `secrets.env` (KEY=VALUE, как `.env`) в папке установки;
//! переменная окружения с тем же именем главнее. Значения никогда не пишутся в лог — только «задано / пусто».

use std::collections::HashMap;
use std::path::Path;

/// Ключи, о которых знает приложение (для строки «что задано» в логе).
pub const KNOWN_KEYS: &[&str] = &[
    "CONFLUENCE_TOKEN",
    "CONFLUENCE_USER",
    "CONFLUENCE_PASSWORD",
    "SVN_USER",
    "SVN_PASSWORD",
    "GITLAB_TOKEN",
    "CONFLUENCE_TEST_PAGE_URL",
    "SVN_TEST_FILE_LITE_PATH",
    "SVN_TEST_FILE_PATH",
    "GITLAB_PROBE_URL",
];

#[derive(Debug, Default, Clone)]
pub struct Secrets {
    file: HashMap<String, String>,
    /// Тесты выключают окружение, чтобы не зависеть от переменных машины.
    ignore_env: bool,
}

impl Secrets {
    pub fn load(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        Self {
            file: parse_env(&text),
            ignore_env: false,
        }
    }

    #[cfg(test)]
    pub fn from_text(text: &str) -> Self {
        Self {
            file: parse_env(text),
            ignore_env: true,
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
        self.file.get(key).filter(|v| !v.is_empty()).cloned()
    }

    /// «KEY=задано, KEY2=пусто» — без значений, для лога.
    pub fn describe(&self) -> String {
        KNOWN_KEYS
            .iter()
            .map(|k| {
                format!(
                    "{k}={}",
                    if self.get(k).is_some() {
                        "задано"
                    } else {
                        "пусто"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
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
    fn describe_has_no_values() {
        let s = Secrets::from_text("SVN_PASSWORD=supersecret\n");
        let d = s.describe();
        assert!(!d.contains("supersecret"));
        assert!(d.contains("SVN_PASSWORD=задано"));
    }
}
