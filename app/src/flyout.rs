//! Меню у иконки (v0.5): своё окно на WebView2 вместо системного списка строк. Открывается левым (и правым)
//! кликом, как панель Wi‑Fi; прячется, когда теряет фокус. Внутри — страница `/flyout` локального API.
//! Нет WebView2 — трей остаётся со старым системным меню.

use std::time::{Duration, Instant};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebView, WebViewBuilder};

pub const WIDTH: f64 = 372.0;
const START_HEIGHT: f64 = 480.0;

pub struct Flyout {
    window: Window,
    webview: WebView,
    visible: bool,
    hidden_at: Instant,
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
        let webview = WebViewBuilder::new()
            .with_url(url)
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(wrap(req.body().clone()));
            })
            .build(&window)
            .map_err(|e| format!("WebView2: {e}"))?;
        Ok(Self {
            window,
            webview,
            visible: false,
            hidden_at: Instant::now() - Duration::from_secs(10),
        })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
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
        self.place(icon);
        self.window.set_visible(true);
        self.window.set_focus();
        self.visible = true;
        let _ = self
            .webview
            .evaluate_script("window.vpnmon && window.vpnmon.shown()");
    }

    pub fn hide(&mut self) {
        if self.visible {
            self.window.set_visible(false);
            self.visible = false;
            self.hidden_at = Instant::now();
        }
    }

    /// Высота — по содержимому страницы (присылает сама страница), в логических пикселях.
    pub fn resize(&mut self, height: f64) {
        let h = height.clamp(200.0, 760.0);
        self.window.set_inner_size(LogicalSize::new(WIDTH, h));
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
