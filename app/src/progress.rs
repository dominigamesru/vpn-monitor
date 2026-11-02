//! Ход проверки (0.9.5): что проверяется прямо сейчас — для меню у иконки (моргает узел маршрута) и полосы
//! микроуведомлений («Проверяю: Мир → ChatGPT…», «✓ Офис → Jira · 120 мс»). Пробы идут параллельно, поэтому
//! «сейчас» — множество узлов. Живёт только в памяти: в `/api/state` → `progress`, пока идёт проверка.

use serde::Serialize;
use std::sync::Mutex;
use std::time::Instant;

/// Узел маршрута, к которому относится шаг: `inet` / `vpn` / `office` / `world`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Step {
    /// Ключ шага (цель, `internet`, `conn`, `vpn`), по нему «начал» сменяется на «закончил».
    pub key: String,
    pub node: String,
    pub text: String,
    /// `run` — идёт, `good` / `warn` / `bad` — закончился.
    pub st: String,
    /// Мс от начала проверки.
    pub t_ms: u64,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct Progress {
    /// Узлы, где сейчас что-то проверяется.
    pub active: Vec<String>,
    /// Шаги по порядку начала; последний изменившийся — `last`.
    pub steps: Vec<Step>,
    /// Индекс шага, который изменился последним (для полосы микроуведомлений).
    pub last: Option<usize>,
}

impl Progress {
    pub fn start(&mut self, key: &str, node: &str, text: String, t_ms: u64) {
        self.steps.retain(|s| s.key != key);
        self.steps.push(Step {
            key: key.into(),
            node: node.into(),
            text,
            st: "run".into(),
            t_ms,
        });
        self.last = Some(self.steps.len() - 1);
        self.recount();
    }

    pub fn done(&mut self, key: &str, st: &str, text: String, t_ms: u64) {
        if let Some(i) = self.steps.iter().position(|s| s.key == key) {
            let s = &mut self.steps[i];
            s.st = st.into();
            s.text = text;
            s.t_ms = t_ms;
            self.last = Some(i);
        }
        self.recount();
    }

    fn recount(&mut self) {
        let mut a: Vec<String> = vec![];
        for s in self.steps.iter().filter(|s| s.st == "run") {
            if !a.contains(&s.node) {
                a.push(s.node.clone());
            }
        }
        self.active = a;
    }
}

/// Узел маршрута по роли цели.
pub fn node_of(role: Option<crate::settings::Role>) -> &'static str {
    use crate::settings::Role;
    match role {
        Some(Role::World) => "world",
        None => "inet",
        _ => "office",
    }
}

/// Строка «закончил» по результату пробы: ✓/✗, узел → имя, главное число.
pub fn done_text(node_name: &str, r: &crate::probe::ProbeResult) -> (String, &'static str) {
    let what = if r.full {
        format!("{node_name} → {} (большой файл)", r.name)
    } else {
        format!("{node_name} → {}", r.name)
    };
    if r.ok {
        let num = match (r.mbps, r.latency_ms) {
            (Some(m), _) if r.role != Some(crate::settings::Role::World) => {
                format!(" · {m:.1} Мбит/с")
            }
            (_, Some(l)) => format!(" · {l:.0} мс"),
            _ => String::new(),
        };
        (format!("✓ {what}{num}"), "good")
    } else {
        let why = r.error.as_deref().unwrap_or("не ответил");
        let short: String = why.chars().take(60).collect();
        (
            format!("✗ {what}: {short}"),
            if r.reachable { "warn" } else { "bad" },
        )
    }
}

pub fn node_name(node: &str) -> &'static str {
    match node {
        "inet" => "Интернет",
        "vpn" => "VPN",
        "world" => "Мир",
        _ => "Офис",
    }
}

static CUR: Mutex<Option<(Instant, Progress)>> = Mutex::new(None);

fn with(f: impl FnOnce(&mut Progress, u64)) {
    let mut g = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t0, p)) = g.as_mut() {
        let t = t0.elapsed().as_millis() as u64;
        f(p, t);
    }
}

/// Проверка началась: прошлый ход забываем.
pub fn begin() {
    *CUR.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), Progress::default()));
}

/// Проверка кончилась: хода нет (меню показывает итог).
pub fn end() {
    *CUR.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn start(key: &str, node: &str, text: String) {
    with(|p, t| p.start(key, node, text, t));
}

pub fn done(key: &str, st: &str, text: String) {
    with(|p, t| p.done(key, st, text, t));
}

/// Шаг пробы: начал — «Проверяю: Офис → SVN», закончил — по результату.
pub fn probe_started(key: &str, role: Option<crate::settings::Role>, name: &str, full: bool) {
    let node = node_of(role);
    let big = if full {
        " (большой файл)"
    } else {
        ""
    };
    start(
        key,
        node,
        format!("Проверяю: {} → {name}{big}…", node_name(node)),
    );
}

pub fn probe_done(key: &str, r: &crate::probe::ProbeResult) {
    let node = node_of(r.role);
    let (text, st) = done_text(node_name(node), r);
    done(key, st, text);
}

/// Снимок для `/api/state`.
pub fn snapshot() -> Option<Progress> {
    CUR.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|(_, p)| p.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Role;

    #[test]
    fn parallel_steps_and_active_nodes() {
        let mut p = Progress::default();
        p.start("svn", "office", "Проверяю: Офис → SVN…".into(), 0);
        p.start("chatgpt", "world", "Проверяю: Мир → ChatGPT…".into(), 5);
        p.start("jira", "office", "Проверяю: Офис → Jira…".into(), 6);
        assert_eq!(p.active, vec!["office", "world"]);
        p.done("chatgpt", "good", "✓ Мир → ChatGPT · 180 мс".into(), 420);
        assert_eq!(p.active, vec!["office"]);
        assert_eq!(p.last, Some(1));
        p.done("svn", "good", "✓".into(), 900);
        p.done("jira", "bad", "✗".into(), 950);
        assert!(p.active.is_empty());
        assert_eq!(p.steps[p.last.unwrap()].key, "jira");
    }

    #[test]
    fn done_text_by_role() {
        let mut r = crate::probe::ProbeResult::new("svn", "SVN", Some(Role::Speed), "svn", false);
        r.ok = true;
        r.reachable = true;
        r.mbps = Some(12.44);
        r.latency_ms = Some(190.0);
        assert_eq!(
            done_text("Офис", &r),
            ("✓ Офис → SVN · 12.4 Мбит/с".into(), "good")
        );
        r.role = Some(Role::World);
        r.name = "ChatGPT".into();
        assert_eq!(done_text("Мир", &r).0, "✓ Мир → ChatGPT · 190 мс");
        r.ok = false;
        r.reachable = false;
        r.error = Some("Timeout".into());
        assert_eq!(
            done_text("Мир", &r),
            ("✗ Мир → ChatGPT: Timeout".into(), "bad")
        );
        assert_eq!(node_of(None), "inet");
        assert_eq!(node_of(Some(Role::Light)), "office");
    }
}
