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
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe}\"\r\nContent-Type: {}\r\n\r\n",
        if safe.ends_with(".zip") { "application/zip" } else if safe.ends_with(".json") { "application/json; charset=utf-8" } else { "text/plain; charset=utf-8" }
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
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ticket {
    pub key: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub summary: String,
    /// Когда создана, мс UNIX.
    #[serde(default)]
    pub created_ms: i64,
    /// Статус в Service Desk, как его видит сотрудник («Ожидает поддержку», «Решено»), v0.8.
    #[serde(default)]
    pub status: String,
    /// Категория статуса Jira: `NEW`, `INDETERMINATE`, `DONE` (пусто — ещё не узнавали).
    #[serde(default)]
    pub status_category: String,
    /// Когда статус сменился, мс UNIX.
    #[serde(default)]
    pub status_ms: i64,
}

/// Статус заявки из ответа `GET /rest/servicedeskapi/request/{key}`: (статус, категория, когда сменился).
pub fn parse_status(body: &str) -> Option<(String, String, i64)> {
    let v: Value = serde_json::from_str(body).ok()?;
    let cs = &v["currentStatus"];
    let status = cs["status"].as_str()?.to_string();
    let cat = cs["statusCategory"].as_str().unwrap_or("").to_string();
    let at = cs["statusDate"]["epochMillis"].as_i64().unwrap_or(0);
    Some((status, cat, at))
}

/// Узнать статус заявки (под входом AD сотрудника).
pub fn fetch_status(
    p: &Portal,
    login: &str,
    password: &str,
    key: &str,
) -> Result<(String, String, i64), String> {
    let req = ureq::http::Request::builder()
        .method("GET")
        .uri(format!("{}/rest/servicedeskapi/request/{key}", p.base))
        .header("Authorization", auth(login, password))
        .header("Accept", "application/json")
        .body(())
        .map_err(|e| e.to_string())?;
    let mut resp = crate::net::agent(Duration::from_secs(20))
        .run(req)
        .map_err(|e| e.to_string())?;
    let st = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if st != 200 {
        return Err(explain(st, &text));
    }
    parse_status(&text).ok_or_else(|| "в ответе Jira нет статуса".into())
}

/// Заявки, статус которых стоит спросить: не решённые (и ещё не узнанные).
pub fn open_tickets(list: &[Ticket]) -> Vec<String> {
    list.iter()
        .filter(|t| t.status_category != "DONE")
        .map(|t| t.key.clone())
        .collect()
}

/// Записать новый статус; `Some(старый статус)` — если сменился (для уведомления), `None` — тот же.
pub fn with_status(
    list: &mut [Ticket],
    key: &str,
    status: &str,
    cat: &str,
    at: i64,
) -> Option<String> {
    let t = list.iter_mut().find(|t| t.key == key)?;
    if t.status == status && t.status_category == cat {
        return None;
    }
    let old = std::mem::replace(&mut t.status, status.to_string());
    t.status_category = cat.to_string();
    t.status_ms = at;
    Some(old)
}

/// Текст уведомления о смене статуса; первое узнавание (старого статуса нет) — без уведомления.
pub fn status_toast(t: &Ticket, old: &str) -> Option<(String, String)> {
    if old.is_empty() {
        return None;
    }
    let title = if t.status_category == "DONE" {
        format!("Заявка {} решена", t.key)
    } else {
        format!("Заявка {}: {}", t.key, t.status)
    };
    Some((title, t.summary.clone()))
}

/// Спросить статусы нерешённых заявок и записать; вернуть уведомления о сменах (заголовок, текст).
/// Ошибка сети или входа — пустой список и текст ошибки (файл не трогаем).
pub fn refresh_statuses(
    dir: &std::path::Path,
    p: &Portal,
    login: &str,
    password: &str,
) -> (Vec<(String, String)>, Option<String>) {
    let mut list = load_tickets(dir);
    let mut toasts = vec![];
    let mut err = None;
    let mut changed = false;
    for key in open_tickets(&list) {
        match fetch_status(p, login, password, &key) {
            Ok((st, cat, at)) => {
                if let Some(old) = with_status(&mut list, &key, &st, &cat, at) {
                    changed = true;
                    if let Some(t) = list.iter().find(|t| t.key == key)
                        && let Some(n) = status_toast(t, &old)
                    {
                        toasts.push(n);
                    }
                }
            }
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    if changed && let Err(e) = save_tickets(dir, &list) {
        err = Some(e);
    }
    (toasts, err)
}

/// Как часто спрашивать статусы заявок в фоне.
pub const STATUS_PERIOD: Duration = Duration::from_secs(15 * 60);

pub fn save_tickets(dir: &std::path::Path, list: &[Ticket]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(TICKETS_FILE), text).map_err(|e| e.to_string())
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
    save_tickets(dir, &with_ticket(load_tickets(dir), t))
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
    fn status_parsed_and_changes_notified() {
        // ответ Jira Service Desk Data Center (сокращён)
        let body = r#"{"issueKey":"DEVOPS-1287","currentStatus":{"status":"Ожидает поддержку","statusCategory":"NEW","statusDate":{"epochMillis":1793568000000}}}"#;
        let (st, cat, at) = parse_status(body).unwrap();
        assert_eq!(
            (st.as_str(), cat.as_str(), at),
            ("Ожидает поддержку", "NEW", 1793568000000)
        );
        assert!(parse_status("{}").is_none());
        let mut list = vec![Ticket {
            key: "DEVOPS-1287".into(),
            summary: "VPN".into(),
            ..Default::default()
        }];
        assert_eq!(open_tickets(&list), ["DEVOPS-1287"]);
        let old = with_status(&mut list, "DEVOPS-1287", "Ожидает поддержку", "NEW", 1).unwrap();
        assert!(status_toast(&list[0], &old).is_none()); // первое узнавание — тихо
        assert!(with_status(&mut list, "DEVOPS-1287", "Ожидает поддержку", "NEW", 1).is_none());
        let old = with_status(&mut list, "DEVOPS-1287", "Решено", "DONE", 2).unwrap();
        assert_eq!(
            status_toast(&list[0], &old).unwrap().0,
            "Заявка DEVOPS-1287 решена"
        );
        assert!(open_tickets(&list).is_empty());
    }

    #[test]
    fn tickets_newest_first_no_dupes_and_tolerant() {
        let t = |k: &str| Ticket {
            key: k.into(),
            ..Default::default()
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
