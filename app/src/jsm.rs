//! Заявка в Jira Service Desk из окна (v0.7.2, ADR 0013): тема, описание, приоритет и вложения (сводка, диагностика,
//! лог) — REST API Service Desk под учётной записью AD сотрудника; заявка создаётся от его имени. Портал и тип
//! запроса берутся из `support_url` (`…/servicedesk/customer/portal/<id>/create/<type>`): в Service Desk номер
//! портала — это id service desk. Отправляет только сотрудник кнопкой после просмотра; пароль не пишется в лог.

use serde_json::{Value, json};
use std::time::Duration;

/// Куда создавать заявку: адрес Jira, service desk и тип запроса.
#[derive(Debug, Clone, PartialEq)]
pub struct Portal {
    pub base: String,
    pub desk: String,
    pub request_type: String,
}

/// `https://jira…/servicedesk/customer/portal/1/create/23` → base, desk 1, type 23. Другой адрес — `None`
/// (тогда окно только открывает ссылку, как раньше).
pub fn parse_portal(url: &str) -> Option<Portal> {
    let url = url.trim();
    if !url.starts_with("https://") {
        return None;
    }
    let (base, rest) = url.split_once("/servicedesk/customer/portal/")?;
    let mut parts = rest.split(['/', '?', '#']);
    let desk = parts.next()?;
    if parts.next()? != "create" {
        return None;
    }
    let request_type = parts.next()?;
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    (digits(desk) && digits(request_type)).then(|| Portal {
        base: base.trim_end_matches('/').to_string(),
        desk: desk.into(),
        request_type: request_type.into(),
    })
}

/// Тело `POST /rest/servicedeskapi/request`.
pub fn request_body(p: &Portal, summary: &str, description: &str, priority: Option<&str>) -> Value {
    let mut fields = json!({"summary": summary, "description": description});
    if let Some(pr) = priority.filter(|x| !x.is_empty()) {
        fields["priority"] = json!({"name": pr});
    }
    json!({"serviceDeskId": p.desk, "requestTypeId": p.request_type, "requestFieldValues": fields})
}

/// Одно поле `file` в multipart/form-data.
pub fn multipart(boundary: &str, filename: &str, content: &[u8]) -> Vec<u8> {
    let safe: String = filename
        .chars()
        .filter(|c| !matches!(c, '"' | '\r' | '\n'))
        .collect();
    let mut out = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe}\"\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n"
    )
    .into_bytes();
    out.extend_from_slice(content);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

/// Итог: ключ заявки, ссылка на неё в портале и что не удалось приложить.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Created {
    pub key: String,
    pub url: String,
    pub attach_failed: Vec<String>,
}

fn auth(login: &str, password: &str) -> String {
    format!(
        "Basic {}",
        crate::probe::base64(format!("{login}:{password}").as_bytes())
    )
}

fn post(
    agent: &ureq::Agent,
    url: &str,
    auth: &str,
    ctype: &str,
    body: Vec<u8>,
) -> Result<(u16, String), String> {
    let req = ureq::http::Request::builder()
        .method("POST")
        .uri(url)
        .header("Authorization", auth)
        .header("Accept", "application/json")
        .header("Content-Type", ctype)
        .header("X-Atlassian-Token", "no-check")
        .header("X-ExperimentalApi", "opt-in")
        .body(body)
        .map_err(|e| e.to_string())?;
    let mut resp = agent.run(req).map_err(|e| e.to_string())?;
    let st = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    Ok((st, text))
}

/// Понятная причина отказа Jira (без пароля): 401 — вход, 403 — нет права на портал, остальное — текст ошибки.
pub fn explain(status: u16, body: &str) -> String {
    match status {
        401 => "Jira не приняла вход AD — войдите заново в «Настройках».".into(),
        403 => "У вашей учётной записи нет доступа к этому порталу поддержки.".into(),
        404 => "Портал или тип запроса не найден — проверьте адрес поддержки в настройках.".into(),
        _ => {
            let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let msg = v["errorMessage"]
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    v["errors"].as_object().map(|m| {
                        m.values()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| body.chars().take(200).collect());
            format!("Jira ответила {status}: {msg}")
        }
    }
}

/// Создать заявку и приложить файлы. Вложение, которое не легло, не отменяет заявку — попадает в `attach_failed`.
pub fn create(
    p: &Portal,
    login: &str,
    password: &str,
    summary: &str,
    description: &str,
    priority: Option<&str>,
    files: &[(String, Vec<u8>)],
) -> Result<Created, String> {
    let agent = crate::net::agent(Duration::from_secs(30));
    let auth = auth(login, password);
    let url = format!("{}/rest/servicedeskapi/request", p.base);
    let send = |pr: Option<&str>| {
        post(
            &agent,
            &url,
            &auth,
            "application/json",
            request_body(p, summary, description, pr)
                .to_string()
                .into_bytes(),
        )
    };
    let (mut st, mut text) = send(priority)?;
    // у типа запроса может не быть поля «Приоритет» — тогда без него
    if st == 400 && priority.is_some() && text.to_lowercase().contains("priority") {
        (st, text) = send(None)?;
    }
    if st != 201 && st != 200 {
        return Err(explain(st, &text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("ответ Jira не JSON: {e}"))?;
    let key = v["issueKey"].as_str().unwrap_or_default().to_string();
    if key.is_empty() {
        return Err("Jira не вернула номер заявки".into());
    }
    let web = v["_links"]["web"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}/servicedesk/customer/portal/{}/{key}", p.base, p.desk));
    let mut failed = vec![];
    let mut ids = vec![];
    for (name, content) in files {
        let boundary = format!("vpnmon{:x}", crate::metrics::now_ms());
        let r = post(
            &agent,
            &format!(
                "{}/rest/servicedeskapi/servicedesk/{}/attachTemporaryFile",
                p.base, p.desk
            ),
            &auth,
            &format!("multipart/form-data; boundary={boundary}"),
            multipart(&boundary, name, content),
        );
        let id = r.ok().filter(|(s, _)| *s == 201 || *s == 200).and_then(|(_, t)| {
            serde_json::from_str::<Value>(&t).ok()?["temporaryAttachments"][0]["temporaryAttachmentId"]
                .as_str()
                .map(str::to_string)
        });
        match id {
            Some(i) => ids.push(i),
            None => failed.push(name.clone()),
        }
    }
    if !ids.is_empty() {
        let body = json!({"temporaryAttachmentIds": ids, "public": true}).to_string();
        let ok = post(
            &agent,
            &format!("{}/rest/servicedeskapi/request/{key}/attachment", p.base),
            &auth,
            "application/json",
            body.into_bytes(),
        )
        .is_ok_and(|(s, _)| s == 201 || s == 200);
        if !ok {
            let rest: Vec<String> = files
                .iter()
                .map(|(n, _)| n.clone())
                .filter(|n| !failed.contains(n))
                .collect();
            failed.extend(rest);
        }
    }
    Ok(Created {
        key,
        url: web,
        attach_failed: failed,
    })
}

/// Файл созданных заявок в папке установки: список «что и когда я отправил в поддержку» для «Настроек».
pub const TICKETS_FILE: &str = "jira-tickets.json";
const TICKETS_KEEP: usize = 50;

/// Созданная заявка. JSON обратно совместим: поля только добавляются.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ticket {
    pub key: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub summary: String,
    /// Когда создана, мс UNIX.
    #[serde(default)]
    pub created_ms: i64,
}

/// Заявки из файла, новые первыми; файла нет или он битый — пусто (файл не главный, приложение без него работает).
pub fn load_tickets(dir: &std::path::Path) -> Vec<Ticket> {
    std::fs::read_to_string(dir.join(TICKETS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Ticket>>(&t).ok())
        .unwrap_or_default()
}

/// Новый список: заявка первой, тот же ключ — не дублируется, не больше 50.
pub fn with_ticket(mut list: Vec<Ticket>, t: Ticket) -> Vec<Ticket> {
    list.retain(|x| x.key != t.key);
    list.insert(0, t);
    list.truncate(TICKETS_KEEP);
    list
}

pub fn save_ticket(dir: &std::path::Path, t: Ticket) -> Result<(), String> {
    let list = with_ticket(load_tickets(dir), t);
    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(TICKETS_FILE), text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_from_real_support_url() {
        let p = parse_portal(
            "https://jira.corp.dominigames.com/servicedesk/customer/portal/1/create/23",
        )
        .unwrap();
        assert_eq!(p.base, "https://jira.corp.dominigames.com");
        assert_eq!((p.desk.as_str(), p.request_type.as_str()), ("1", "23"));
        assert!(
            parse_portal("https://jira.corp.dominigames.com/servicedesk/customer/portal/1")
                .is_none()
        );
        assert!(parse_portal("http://jira/servicedesk/customer/portal/1/create/23").is_none());
        assert!(parse_portal("https://t.me/support").is_none());
    }

    #[test]
    fn body_and_multipart() {
        let p = parse_portal("https://j/servicedesk/customer/portal/1/create/23").unwrap();
        let b = request_body(&p, "VPN: SVN медленно", "текст", Some("Low"));
        assert_eq!(b["serviceDeskId"], "1");
        assert_eq!(b["requestTypeId"], "23");
        assert_eq!(b["requestFieldValues"]["priority"]["name"], "Low");
        assert!(
            request_body(&p, "a", "b", None)["requestFieldValues"]
                .get("priority")
                .is_none()
        );
        let m = String::from_utf8(multipart("XX", "vpn\"-monitor.log", b"lines")).unwrap();
        assert!(m.starts_with("--XX\r\n") && m.ends_with("\r\n--XX--\r\n"));
        assert!(m.contains("filename=\"vpn-monitor.log\"") && m.contains("\r\n\r\nlines\r\n"));
    }

    #[test]
    fn tickets_newest_first_no_dupes_and_tolerant() {
        let t = |k: &str| Ticket {
            key: k.into(),
            url: String::new(),
            summary: String::new(),
            created_ms: 0,
        };
        let l = with_ticket(with_ticket(vec![], t("DEVOPS-1")), t("DEVOPS-2"));
        assert_eq!(
            l.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(),
            ["DEVOPS-2", "DEVOPS-1"]
        );
        assert_eq!(with_ticket(l, t("DEVOPS-1"))[0].key, "DEVOPS-1");
        let old: Vec<Ticket> = serde_json::from_str(r#"[{"key":"DEVOPS-7","future":1}]"#).unwrap();
        assert_eq!(old[0].key, "DEVOPS-7");
    }

    #[test]
    fn errors_are_human() {
        assert!(explain(401, "").contains("войдите заново"));
        assert_eq!(
            explain(400, r#"{"errorMessage":"Поле обязательно"}"#),
            "Jira ответила 400: Поле обязательно"
        );
        assert!(explain(400, r#"{"errors":{"summary":"нужна тема"}}"#).contains("нужна тема"));
    }
}
