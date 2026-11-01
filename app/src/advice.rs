//! Советы по известным причинам (v0.7, «меньше обращений в поддержку»): по результатам проверки — что сделать
//! самому, до звонка в поддержку. Узоры — из реальных логов: подставной DNS клиента VPN, 401, закрытый для страны
//! сервис, VPN режет скорость при нормальной задержке.

use crate::probe::{Fail, ProbeResult};
use crate::route::Route;
use crate::settings::Role;

/// Советы по порядку важности; пусто — советовать нечего.
pub fn advice(
    route: Option<&Route>,
    targets: &[ProbeResult],
    client: &str,
    captive: bool,
) -> Vec<String> {
    let mut out = vec![];
    if captive {
        out.push("Откройте браузер и войдите на странице Wi‑Fi (отель, кафе) — до этого VPN не подключится.".into());
        return out;
    }
    let office: Vec<&ProbeResult> = targets
        .iter()
        .filter(|t| matches!(t.role, Some(Role::Light) | Some(Role::Speed)))
        .collect();
    if office.iter().any(|t| {
        t.error
            .as_deref()
            .is_some_and(|e| e.contains("подставной адрес DNS"))
    }) {
        out.push(format!(
            "{client} отдаёт подставной адрес DNS — туннель не пропускает трафик. Отключите и снова подключите профиль."
        ));
    }
    let auth: Vec<&str> = office
        .iter()
        .filter(|t| t.fail == Some(Fail::Auth))
        .map(|t| t.name.as_str())
        .collect();
    if !auth.is_empty() {
        out.push(format!(
            "{} не принимает вход — возможно, сменился пароль AD. Войдите заново в «Настройках».",
            auth.join(", ")
        ));
    }
    if let Some(r) = route {
        if r.cause == Some("vpn") && (r.why == Some("speed") || r.why == Some("lat")) {
            // вывод уже говорит «переподключите» — совет о следующем шаге
            out.push(
                "Не помогло переподключение — выберите другой сервер VPN или создайте запрос в поддержку \
                 (вкладка «Настройки»)."
                    .into(),
            );
        }
        if r.cause == Some("inet") {
            out.push(
                "Причина — интернет: подойдите ближе к роутеру, смените сеть или подождите.".into(),
            );
        }
    }
    let closed: Vec<&str> = targets
        .iter()
        .filter(|t| t.role == Some(Role::World) && !t.ok && t.via_vpn == Some(false))
        .filter(|t| {
            t.error
                .as_deref()
                .is_some_and(|e| e.contains("закрыт для вашей страны"))
        })
        .map(|t| t.name.as_str())
        .collect();
    if !closed.is_empty() {
        out.push(format!(
            "{} закрыт напрямую — включите в {client} доступ в мир (весь трафик через VPN).",
            closed.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(
        name: &str,
        role: Role,
        ok: bool,
        fail: Option<Fail>,
        err: Option<&str>,
        via: Option<bool>,
    ) -> ProbeResult {
        let mut r = crate::probe::tests_support::result(&name.to_lowercase(), role);
        r.name = name.into();
        r.ok = ok;
        r.reachable = ok;
        r.fail = fail;
        r.error = err.map(str::to_string);
        r.via_vpn = via;
        r
    }

    #[test]
    fn fake_dns_auth_and_closed_world() {
        let targets = vec![
            t(
                "Confluence",
                Role::Light,
                false,
                Some(Fail::Timeout),
                Some(
                    "TCP 1.2.3.4:443 (подставной адрес DNS клиента VPN вроде Happ — туннель не пропускает трафик)",
                ),
                None,
            ),
            t(
                "Jira",
                Role::Light,
                false,
                Some(Fail::Auth),
                Some("401"),
                Some(true),
            ),
            t(
                "Claude",
                Role::World,
                false,
                Some(Fail::Http),
                Some("закрыт для вашей страны (HTTP 403) — нужен VPN в мир"),
                Some(false),
            ),
        ];
        let a = advice(None, &targets, "Happ", false);
        assert!(a[0].contains("подставной адрес DNS") && a[0].contains("Happ"));
        assert!(a.iter().any(|x| x.contains("Jira не принимает вход")));
        assert!(a.iter().any(|x| x.contains("Claude закрыт напрямую")));
    }

    #[test]
    fn captive_first_and_alone() {
        assert_eq!(advice(None, &[], "VPN", true).len(), 1);
    }
}
