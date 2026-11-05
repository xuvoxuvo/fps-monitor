//! Native Win32 user interface: a transparent layered overlay plus a normal
//! settings window built from standard controls.

pub mod overlay;
pub mod settings;

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::CreateSolidBrush;
use windows::Win32::Graphics::GdiPlus::{GdiplusShutdown, GdiplusStartup, GdiplusStartupInput};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetMessageW, GetWindowRect, IsWindow, IsWindowVisible,
    KillTimer, LoadCursorW, LoadIconW, PostQuitMessage, RegisterClassExW, SetTimer,
    SetWindowLongPtrW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_ARROW,
    IDI_APPLICATION, MSG, WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DRAWITEM,
    WM_HSCROLL, WM_TIMER, WNDCLASSEXW,
};

use crate::app::shared::SharedHandle;

pub const SETTINGS_CLASS: &str = "FpsMonitorSettings";
pub const SETTINGS_TITLE: &str = "fps-monitor Settings";

struct GdiPlus {
    token: usize,
}

impl GdiPlus {
    fn start() -> Result<Self, String> {
        let mut token = 0usize;
        let mut output = Default::default();
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let status = unsafe { GdiplusStartup(&mut token, &input, &mut output) };
        if status.0 != 0 {
            return Err(format!("GDI+ startup failed (status {})", status.0));
        }
        Ok(Self { token })
    }
}

impl Drop for GdiPlus {
    fn drop(&mut self) {
        unsafe { GdiplusShutdown(self.token) };
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RectInfo {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl RectInfo {
    fn from_rect(rect: windows::Win32::Foundation::RECT) -> Self {
        Self {
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UiReport {
    pub settings: RectInfo,
    pub overlay: RectInfo,
    pub overlay_visible: bool,
    pub controls: usize,
    pub overlay_error: Option<String>,
    pub drawn_pixels: usize,
    /// Smoke test only: overlay width before and after a large value change.
    /// They must match, otherwise the overlay would jump while numbers change.
    pub width_before: i32,
    pub width_after: i32,
}

pub fn run(shared: SharedHandle) -> Result<(), String> {
    run_inner(shared, None).map(|_| ())
}

/// Create both windows and pump the message loop for a couple of seconds, then
/// report their geometry. Used by `--ui-smoke` so the rendering path can be
/// exercised on a machine without anyone clicking anything.
pub fn smoke(shared: SharedHandle, seconds: u64) -> Result<UiReport, String> {
    run_inner(shared, Some(seconds.max(1)))
}

fn run_inner(shared: SharedHandle, auto_quit_seconds: Option<u64>) -> Result<UiReport, String> {
    unsafe {
        let _gdiplus = GdiPlus::start()?;

        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_STANDARD_CLASSES,
        };
        // Not fatal: on a few locked-down systems this reports FALSE even
        // though the trackbar class is already registered. The settings window
        // will surface a real failure if the control cannot be created.
        let _ = InitCommonControlsEx(&controls);

        let module = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
        let instance = HINSTANCE(module.0);

        register_classes(instance)?;

        let overlay = overlay::Overlay::create(instance)?;
        let overlay_hwnd = overlay.hwnd();
        let stats_source = shared.clone();
        let mut app = Box::new(settings::App::new(shared, overlay));
        let app_ptr = &mut *app as *mut settings::App as *const c_void;

        let hwnd = settings::create_window(instance, app_ptr)?;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app_ptr as isize);
        app.attach(hwnd)?;
        app.refresh_overlay();

        let mut before = Default::default();
        GetWindowRect(overlay_hwnd, &mut before).ok();
        let width_before = before.right - before.left;

        if auto_quit_seconds.is_some() {
            // Exercise the "numbers change" path: the overlay must keep the
            // same width because values are right-aligned in a reserved column.
            stats_source.update_stats(|stats| {
                stats.fps = Some(9999.0);
                stats.fps_low = Some(9999.0);
                stats.fps_low01 = Some(9999.0);
                stats.cpu = Some(100.0);
                stats.gpu = Some(100.0);
                stats.gpu_temp = Some(123.0);
            });
            app.refresh_overlay();
        }
        let mut after = Default::default();
        GetWindowRect(overlay_hwnd, &mut after).ok();
        let width_after = after.right - after.left;

        SetTimer(Some(hwnd), 1, 1_000, None);
        SetTimer(Some(hwnd), 2, 2_000, None);
        let timer_quit = auto_quit_seconds
            .map(|seconds| SetTimer(Some(hwnd), 99, seconds as u32 * 1_000, None))
            .unwrap_or(0);

        message_loop();

        for timer in [1, 2] {
            let _ = KillTimer(Some(hwnd), timer);
        }
        if timer_quit != 0 {
            let _ = KillTimer(Some(hwnd), 99);
        }

        let mut settings_rect = Default::default();
        let mut overlay_rect = Default::default();
        GetWindowRect(hwnd, &mut settings_rect).ok();
        GetWindowRect(overlay_hwnd, &mut overlay_rect).ok();
        let report = UiReport {
            settings: RectInfo::from_rect(settings_rect),
            overlay: RectInfo::from_rect(overlay_rect),
            overlay_visible: IsWindowVisible(overlay_hwnd).as_bool(),
            controls: app.control_count(),
            overlay_error: app.overlay_error(),
            drawn_pixels: app.overlay_drawn_pixels(),
            width_before,
            width_after,
        };

        if IsWindow(Some(hwnd)).as_bool() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(hwnd);
        }
        drop(app);
        Ok(report)
    }
}

fn message_loop() {
    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn register_classes(instance: HINSTANCE) -> Result<(), String> {
    unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW).map_err(|e| format!("cursor: {e}"))?;
        let icon = LoadIconW(Some(instance), PCWSTR(1 as *const u16))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default();
        let brush = CreateSolidBrush(COLORREF(0x00f3_f3f3));

        let overlay_class = crate::platform::windows::wide(overlay::CLASS_NAME);
        let overlay_window = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay::window_proc),
            hInstance: instance,
            hIcon: icon,
            hCursor: cursor,
            lpszClassName: PCWSTR(overlay_class.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&overlay_window) == 0 {
            return Err("could not register the overlay window class".into());
        }

        let settings_class = crate::platform::windows::wide(SETTINGS_CLASS);
        let settings_window = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(settings::window_proc),
            hInstance: instance,
            hIcon: icon,
            hCursor: cursor,
            hbrBackground: brush,
            lpszClassName: PCWSTR(settings_class.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&settings_window) == 0 {
            return Err("could not register the settings window class".into());
        }
        Ok(())
    }
}

/// Generic entry point used by the settings window procedure.
pub unsafe fn dispatch(
    app: *mut settings::App,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<windows::Win32::Foundation::LRESULT> {
    match message {
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let code = ((wparam.0 >> 16) & 0xffff) as u16;
            (*app).on_command(id, code);
            Some(windows::Win32::Foundation::LRESULT(0))
        }
        WM_HSCROLL => {
            (*app).on_hscroll(lparam);
            Some(windows::Win32::Foundation::LRESULT(0))
        }
        WM_DRAWITEM => {
            (*app).on_draw_item(lparam);
            Some(windows::Win32::Foundation::LRESULT(0))
        }
        WM_CTLCOLORSTATIC => (*app)
            .on_ctlcolor_static(wparam, lparam)
            .map(windows::Win32::Foundation::LRESULT),
        WM_TIMER => {
            if wparam.0 == 99 {
                PostQuitMessage(0);
            } else {
                (*app).on_timer(wparam.0);
            }
            Some(windows::Win32::Foundation::LRESULT(0))
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            Some(windows::Win32::Foundation::LRESULT(0))
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            Some(windows::Win32::Foundation::LRESULT(0))
        }
        _ => None,
    }
}
