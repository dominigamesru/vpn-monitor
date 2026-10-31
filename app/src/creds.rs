//! Креды AD и токен GitLab в Диспетчере учётных данных Windows (ADR 0006): шифрование DPAPI, привязка к
//! пользователю Windows; ни в файлы, ни в лог не попадают. Не на Windows — только в памяти процесса (тесты, `--check`).

use serde::{Deserialize, Serialize};

const AD_TARGET: &str = "DominiVPNMonitor/ad";
const GITLAB_TARGET: &str = "DominiVPNMonitor/gitlab-oauth";

#[derive(Clone)]
pub struct AdCreds {
    pub login: String,
    pub password: String,
}

impl std::fmt::Debug for AdCreds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AdCreds {{ login: {:?}, password: *** }}", self.login)
    }
}

/// Токен GitLab, полученный по паролю AD (OAuth, право `read_api`).
#[derive(Clone, Serialize, Deserialize)]
pub struct GitlabToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Секунды UNIX; 0 — без срока.
    #[serde(default)]
    pub expires_at: u64,
}

pub fn load_ad() -> Option<AdCreds> {
    let (user, secret) = store::read(AD_TARGET)?;
    (!user.is_empty() && !secret.is_empty()).then_some(AdCreds {
        login: user,
        password: secret,
    })
}

pub fn save_ad(c: &AdCreds) -> Result<(), String> {
    store::write(AD_TARGET, &c.login, &c.password)
}

pub fn delete_ad() -> Result<(), String> {
    let _ = store::delete(GITLAB_TARGET);
    store::delete(AD_TARGET)
}

pub fn load_gitlab() -> Option<GitlabToken> {
    let (_, secret) = store::read(GITLAB_TARGET)?;
    serde_json::from_str(&secret).ok()
}

pub fn save_gitlab(t: &GitlabToken) -> Result<(), String> {
    let json = serde_json::to_string(t).map_err(|e| e.to_string())?;
    store::write(GITLAB_TARGET, "gitlab", &json)
}

#[cfg(windows)]
mod store {
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        unsafe {
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    }

    pub fn read(target: &str) -> Option<(String, String)> {
        let t = wide(target);
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        unsafe {
            if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 || p.is_null() {
                return None;
            }
            let c = &*p;
            let user = from_wide(c.UserName);
            let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
            let units: Vec<u16> = blob
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b))
                .collect();
            let secret = String::from_utf16_lossy(&units);
            CredFree(p as *const _);
            Some((user, secret))
        }
    }

    pub fn write(target: &str, user: &str, secret: &str) -> Result<(), String> {
        let mut t = wide(target);
        let mut u = wide(user);
        let mut blob: Vec<u8> = secret
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: t.as_mut_ptr(),
            UserName: u.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        let ok = unsafe { CredWriteW(&cred, 0) };
        blob.iter_mut().for_each(|b| *b = 0);
        if ok == 0 {
            Err(format!(
                "Диспетчер учётных данных: не записать ({})",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    }

    pub fn delete(target: &str) -> Result<(), String> {
        let t = wide(target);
        if unsafe { CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(1168) {
                return Ok(()); // ERROR_NOT_FOUND
            }
            return Err(format!("Диспетчер учётных данных: не удалить ({e})"));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod store {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static MEM: Mutex<Option<HashMap<String, (String, String)>>> = Mutex::new(None);

    pub fn read(target: &str) -> Option<(String, String)> {
        MEM.lock().ok()?.as_ref()?.get(target).cloned()
    }
    pub fn write(target: &str, user: &str, secret: &str) -> Result<(), String> {
        let mut g = MEM.lock().map_err(|e| e.to_string())?;
        g.get_or_insert_with(HashMap::new)
            .insert(target.into(), (user.into(), secret.into()));
        Ok(())
    }
    pub fn delete(target: &str) -> Result<(), String> {
        if let Ok(mut g) = MEM.lock()
            && let Some(m) = g.as_mut()
        {
            m.remove(target);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ad_roundtrip_and_debug_hides_password() {
        save_ad(&AdCreds {
            login: "s.rykov".into(),
            password: "p@ss".into(),
        })
        .unwrap();
        let c = load_ad().unwrap();
        assert_eq!(c.login, "s.rykov");
        assert!(!format!("{c:?}").contains("p@ss"));
        delete_ad().unwrap();
        assert!(load_ad().is_none());
    }
}
