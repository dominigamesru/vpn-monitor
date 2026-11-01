//! Меню у иконки (v0.5): своё окно на WebView2 вместо системного списка строк. Открывается левым (и правым)
//! кликом, как панель Wi‑Fi; прячется, когда теряет фокус. Внутри — страница `/flyout` локального API.
//! Нет WebView2 — трей остаётся со старым системным меню.

use std::time::{Duration, Instant};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};
use wry::{WebView, WebViewBuilder};

pub const WIDTH: f64 = 372.0;
const START_HEIGHT: f64 = 480.0;

pub struct Flyout {
    window: Window,
    webview: WebView,
    visible: bool,
    hidden_at: Instant,
    /// Где иконка — чтобы после смены высоты окно осталось у неё, а не уехало за край экрана.
    icon: Option<tray_icon::Rect>,
    height: f64,
    shown_at: Instant,
    /// Были ли мы на переднем плане после показа: если Windows не отдала фокус, по чужому окну не прячем.
    had_foreground: std::cell::Cell<bool>,
}

impl Flyout {
    pub fn new<T: 'static, F>(
        target: &EventLoopWindowTarget<T>,
        url: &str,
        proxy: EventLoopProxy<T>,
        wrap: F,
    ) -> Result<Self, String>
    where
        F: Fn(String) -> T + 'static,
    {
        let window = WindowBuilder::new()
            .with_title("Domini VPN")
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top(true)
            .with_skip_taskbar(true)
            .with_visible(false)
            .with_inner_size(LogicalSize::new(WIDTH, START_HEIGHT))
            .build(target)
            .map_err(|e| format!("окно меню: {e}"))?;
        round_corners(window.hwnd());
        let wrap = std::rc::Rc::new(wrap);
        let (load_proxy, load_wrap) = (proxy.clone(), wrap.clone());
        let webview = WebViewBuilder::new()
            .with_url(url)
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(wrap(req.body().clone()));
            })
            // диагностика: грузится ли страница меню вообще (в логе заказчика от страницы — тишина)
            .with_on_page_load_handler(move |ev, url| {
                let what = match ev {
                    wry::PageLoadEvent::Started => "начала грузиться",
                    wry::PageLoadEvent::Finished => "загрузилась",
                };
                vpn_monitor::info!("flyout", "страница меню {what}: {url}");
                if matches!(ev, wry::PageLoadEvent::Finished) {
                    let _ = load_proxy.send_event(load_wrap("loaded".into()));
                }
            })
            .build(&window)
            .map_err(|e| format!("WebView2: {e}"))?;
        Ok(Self {
            window,
            webview,
            visible: false,
            hidden_at: Instant::now() - Duration::from_secs(10),
            icon: None,
            height: START_HEIGHT,
            shown_at: Instant::now(),
            had_foreground: std::cell::Cell::new(false),
        })
    }

    /// Что на странице на самом деле — в лог (ответ приходит из WebView2, минуя IPC страницы).
    pub fn probe(&self) {
        const JS: &str = "(() => { try { const r = document.getElementById('root'); return [document.readyState, 'url=' + location.href, 'tippy=' + typeof window.tippy, 'ipc=' + typeof window.ipc, 'chrome.webview=' + typeof (window.chrome && window.chrome.webview), 'root=' + (r ? r.children.length : 'нет'), 'vis=' + document.visibilityState, 'h=' + document.body.getBoundingClientRect().height].join(' '); } catch (e) { return 'ошибка: ' + e; } })()";
        let res = self.webview.evaluate_script_with_callback(JS, |r| {
            vpn_monitor::info!("flyout", "состояние страницы меню: {r}");
        });
        if let Err(e) = res {
            vpn_monitor::warn!("flyout", "не прочитать состояние страницы: {e}");
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Ушли ли в другое окно. Фокус внутри WebView2 (его дочернее окно) — не уход: корень у него наш.
    /// Первые полсекунды после показа не проверяем — клик по иконке ещё «держит» панель задач.
    pub fn lost_to_other_window(&self) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GA_ROOT, GetAncestor, GetForegroundWindow,
        };
        if !self.visible || self.shown_at.elapsed() < Duration::from_millis(500) {
            return false;
        }
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() {
            return false;
        }
        let ours = unsafe { GetAncestor(fg, GA_ROOT) } as isize == self.window.hwnd();
        if ours {
            self.had_foreground.set(true);
        }
        !ours && self.had_foreground.get()
    }

    /// Показать у иконки или спрятать. Клик по иконке сразу после того, как меню спряталось из-за потери
    /// фокуса (этим же кликом), меню не открывает заново.
    pub fn toggle(&mut self, icon: tray_icon::Rect) {
        if self.visible {
            self.hide();
            return;
        }
        if self.hidden_at.elapsed() < Duration::from_millis(350) {
            return;
        }
        self.icon = Some(icon);
        self.place(icon);
        self.window.set_visible(true);
        // Окно создано скрытым: WebView2 сам может не ожить (старый кадр, клики не доходят) — включаем явно.
        let _ = self.webview.set_visible(true);
        self.window.set_focus();
        let _ = self.webview.focus();
        self.visible = true;
        self.shown_at = Instant::now();
        self.had_foreground.set(false);
        let size = self.window.inner_size();
        let bounds = self
            .webview
            .bounds()
            .map(|b| format!("{:?}", b.size))
            .unwrap_or_else(|e| e.to_string());
        vpn_monitor::info!(
            "flyout",
            "меню открыто: окно {}×{}, WebView {bounds}",
            size.width,
            size.height
        );
        let _ = self
            .webview
            .evaluate_script("window.vpnmon && window.vpnmon.shown()");
    }

    pub fn hide(&mut self) {
        if self.visible {
            let _ = self.webview.set_visible(false);
            self.window.set_visible(false);
            self.visible = false;
            self.hidden_at = Instant::now();
        }
    }

    /// Высота — по содержимому страницы (присылает сама страница), в логических пикселях.
    /// Повтор той же высоты не трогает окно (без петли «окно → страница → окно»).
    pub fn resize(&mut self, height: f64) {
        let h = height.clamp(160.0, 760.0).round();
        if (h - self.height).abs() < 2.0 {
            return;
        }
        self.height = h;
        self.window.set_inner_size(LogicalSize::new(WIDTH, h));
        if let Some(icon) = self.icon {
            self.place(icon);
        }
    }

    /// Над иконкой, если панель задач снизу; под ней — если сверху; не вылезая за край экрана.
    fn place(&self, icon: tray_icon::Rect) {
        let scale = self.window.scale_factor();
        let size = self.window.outer_size();
        let (w, h) = (size.width as f64, size.height as f64);
        let (ix, iy) = (icon.position.x, icon.position.y);
        let (iw, ih) = (icon.size.width as f64, icon.size.height as f64);
        let gap = 10.0 * scale;
        let (mx, my, mw, mh) = self
            .window
            .current_monitor()
            .or_else(|| self.window.primary_monitor())
            .map(|m| {
                let p = m.position();
                let s = m.size();
                (p.x as f64, p.y as f64, s.width as f64, s.height as f64)
            })
            .unwrap_or((0.0, 0.0, 1920.0, 1080.0));
        let x = (ix + iw / 2.0 - w / 2.0).clamp(mx + gap, mx + mw - w - gap);
        let y = if iy > my + mh / 2.0 {
            iy - h - gap
        } else {
            iy + ih + gap
        };
        self.window
            .set_outer_position(PhysicalPosition::new(x, y.max(my + gap)));
    }
}

/// Скруглённые углы окна, как у панелей Windows 11 (на Windows 10 — без эффекта).
fn round_corners(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    let pref = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}
