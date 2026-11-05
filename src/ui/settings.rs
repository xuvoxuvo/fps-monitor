//! The settings window.
//!
//! A plain Win32 window with standard controls, so startup is instant and the
//! UI looks native. State lives in `App`, reachable from the window procedure
//! through `GWLP_USERDATA`.

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, FillRect, FrameRect, InvalidateRect, SetBkMode,
    SetTextColor, UpdateWindow, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    FONT_CHARSET, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY, HBRUSH, HDC, HFONT,
    HGDIOBJ, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::UI::Controls::Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW};
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, SendMessageW,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, CREATESTRUCTW, GWLP_USERDATA, HMENU,
    SET_WINDOW_POS_FLAGS, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_NCCREATE, WM_SETFONT,
};

use crate::app::config::{Config, Metric, POSITIONS};
use crate::app::shared::SharedHandle;
use crate::platform::elevate;
use crate::platform::fonts::installed_families;
use crate::platform::windows::{
    dpi_for_window, get_window_text, invalidate, is_admin, scale, screen_size, set_window_text,
    wide,
};
use crate::ui::overlay::Overlay;

const BN_CLICKED: u16 = 0;
const CBN_SELCHANGE: u16 = 1;
const EN_KILLFOCUS: u16 = 0x0200;

const ID_METRIC_CHECK: i32 = 100;
const ID_METRIC_SWATCH: i32 = 200;
const ID_BG_CHECK: i32 = 300;
const ID_BG_SWATCH: i32 = 301;
const ID_FONT_COMBO: i32 = 400;
const ID_SIZE_EDIT: i32 = 401;
const ID_OPACITY_TRACK: i32 = 402;
const ID_POSITION_COMBO: i32 = 403;
const ID_MARGIN_EDIT: i32 = 404;
const ID_HIDE_BUTTON: i32 = 500;
const ID_QUIT_BUTTON: i32 = 501;
const ID_STATUS: i32 = 502;
const ID_ELEVATE_BUTTON: i32 = 503;
const ID_PREVIEW: i32 = 504;

const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;
const CB_SETDROPPEDWIDTH: u32 = 0x0160;
const TBM_GETPOS: u32 = 0x0400;
const TBM_SETPOS: u32 = 0x0405;
const TBM_SETRANGE: u32 = 0x0406;
const TBM_SETPAGESIZE: u32 = 0x040D;

const WS_CHILD_VISIBLE: u32 = 0x5000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const BS_AUTOCHECKBOX: u32 = 0x0000_0003;
const BS_OWNERDRAW: u32 = 0x0000_000B;
const BS_GROUPBOX: u32 = 0x0000_0007;
const ES_AUTOHSCROLL: u32 = 0x0000_0080;
const ES_NUMBER: u32 = 0x0000_2000;
const CBS_DROPDOWNLIST: u32 = 0x0000_0003;
const CBS_HASSTRINGS: u32 = 0x0000_0200;
const WS_VSCROLL: u32 = 0x0020_0000;
const TBS_NOTICKS: u32 = 0x0000_0010;
const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_OVERLAPPED: u32 = 0x0000_0000;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SS_LEFT: u32 = 0x0000_0000;
const SS_CENTER: u32 = 0x0000_0001;

const CLIENT_WIDTH: i32 = 680;
const CLIENT_HEIGHT: i32 = 560;

pub struct App {
    overlay: Overlay,
    shared: SharedHandle,
    config: Config,
    settings: HWND,
    font: HFONT,
    preview_font: HFONT,
    preview_font_key: Option<(String, i32)>,
    background_brush: HBRUSH,
    dpi: u32,
    admin: bool,
    controls: Vec<HWND>,
    metric_checks: Vec<HWND>,
    metric_swatches: Vec<HWND>,
    bg_check: HWND,
    bg_swatch: HWND,
    font_combo: HWND,
    size_edit: HWND,
    opacity_track: HWND,
    opacity_label: HWND,
    position_combo: HWND,
    margin_edit: HWND,
    hide_button: HWND,
    elevate_button: HWND,
    status_label: HWND,
    preview_label: HWND,
    font_options: Vec<String>,
    position_options: Vec<String>,
    custom_colors: [COLORREF; 16],
}

impl App {
    pub fn new(shared: SharedHandle, overlay: Overlay) -> Self {
        let config = shared.config();
        Self {
            overlay,
            shared,
            config,
            settings: HWND::default(),
            font: HFONT::default(),
            preview_font: HFONT::default(),
            preview_font_key: None,
            background_brush: HBRUSH::default(),
            dpi: 96,
            admin: is_admin(),
            controls: Vec::new(),
            metric_checks: Vec::new(),
            metric_swatches: Vec::new(),
            bg_check: HWND::default(),
            bg_swatch: HWND::default(),
            font_combo: HWND::default(),
            size_edit: HWND::default(),
            opacity_track: HWND::default(),
            opacity_label: HWND::default(),
            position_combo: HWND::default(),
            margin_edit: HWND::default(),
            hide_button: HWND::default(),
            elevate_button: HWND::default(),
            status_label: HWND::default(),
            preview_label: HWND::default(),
            font_options: Vec::new(),
            position_options: POSITIONS.iter().map(|s| s.to_string()).collect(),
            custom_colors: [COLORREF(0x00ff_ffff); 16],
        }
    }

    pub fn attach(&mut self, hwnd: HWND) -> Result<(), String> {
        self.settings = hwnd;
        let actual_dpi = dpi_for_window(hwnd);
        let (_, screen_height) = screen_size();
        let height_limit =
            (96 * (screen_height - 80).max(200) / CLIENT_HEIGHT).clamp(96, 192) as u32;
        self.dpi = actual_dpi.min(height_limit).max(96);

        self.font_options = installed_families();
        if self.font_options.is_empty() {
            self.font_options.push("Segoe UI".into());
        }

        self.resize_window()?;
        self.create_ui_font()?;
        self.build_controls()?;
        self.sync_controls_from_config();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
        }
        self.update_status();
        if !self.admin {
            set_window_text(hwnd, "fps-monitor - run as administrator for FPS");
        }
        Ok(())
    }

    fn s(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }

    pub fn control_count(&self) -> usize {
        self.controls.len()
    }

    pub fn overlay_error(&self) -> Option<String> {
        self.overlay.last_error().map(|e| e.to_string())
    }

    pub fn overlay_drawn_pixels(&self) -> usize {
        self.overlay.drawn_pixels()
    }

    fn resize_window(&self) -> Result<(), String> {
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: self.s(CLIENT_WIDTH),
            bottom: self.s(CLIENT_HEIGHT),
        };
        unsafe {
            AdjustWindowRectExForDpi(
                &mut rect,
                WINDOW_STYLE(style),
                false,
                WINDOW_EX_STYLE(0),
                self.dpi,
            )
            .map_err(|e| format!("AdjustWindowRectExForDpi: {e}"))?;
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let (screen_width, screen_height) = screen_size();
            let x = ((screen_width - width) / 2).max(0);
            let y = ((screen_height - height) / 2).max(0);
            SetWindowPos(
                self.settings,
                None,
                x,
                y,
                width,
                height,
                SET_WINDOW_POS_FLAGS(SWP_NOZORDER | SWP_NOACTIVATE),
            )
            .map_err(|e| format!("SetWindowPos: {e}"))?;
        }
        Ok(())
    }

    fn create_ui_font(&mut self) -> Result<(), String> {
        self.font = self.create_scaled_font("Segoe UI", 9, 400)?;
        Ok(())
    }

    fn create_scaled_font(&self, family: &str, points: i32, weight: i32) -> Result<HFONT, String> {
        // Point size, matching the overlay's own point -> pixel conversion.
        let height = -(((points as f32) * self.dpi as f32 / 72.0).round() as i32).max(1);
        let face = wide(family);
        let font = unsafe {
            CreateFontW(
                height,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                FONT_CHARSET(DEFAULT_CHARSET.0),
                FONT_OUTPUT_PRECISION(OUT_DEFAULT_PRECIS.0),
                FONT_CLIP_PRECISION(CLIP_DEFAULT_PRECIS.0),
                FONT_QUALITY(CLEARTYPE_QUALITY.0),
                0,
                PCWSTR(face.as_ptr()),
            )
        };
        if font.0.is_null() {
            Err(format!("could not create font {family:?}"))
        } else {
            Ok(font)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_control(
        &mut self,
        class: &str,
        text: &str,
        style: u32,
        ex_style: u32,
        id: i32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<HWND, String> {
        let class = wide(class);
        let text = wide(text);
        let (sx, sy, sw, sh) = (self.s(x), self.s(y), self.s(width), self.s(height));
        let id_menu = if id == 0 {
            None
        } else {
            Some(HMENU(id as isize as *mut c_void))
        };
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(ex_style),
                PCWSTR(class.as_ptr()),
                PCWSTR(text.as_ptr()),
                WINDOW_STYLE(style),
                sx,
                sy,
                sw,
                sh,
                Some(self.settings),
                id_menu,
                None,
                None,
            )
        }
        .map_err(|e| format!("create {class:?}: {e}"))?;

        unsafe {
            SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(self.font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        self.controls.push(hwnd);
        Ok(hwnd)
    }

    fn label(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<HWND, String> {
        self.create_control(
            "STATIC",
            text,
            WS_CHILD_VISIBLE | SS_LEFT,
            0,
            0,
            x,
            y,
            width,
            height,
        )
    }

    fn group(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<HWND, String> {
        self.create_control(
            "BUTTON",
            text,
            WS_CHILD_VISIBLE | BS_GROUPBOX,
            0,
            0,
            x,
            y,
            width,
            height,
        )
    }

    fn build_controls(&mut self) -> Result<(), String> {
        self.background_brush = unsafe { CreateSolidBrush(COLORREF(0x00f3_f3f3)) };

        // --- metrics -----------------------------------------------------
        self.group(" Metrics ", 16, 16, 330, 258)?;
        for (index, metric) in Metric::ALL.into_iter().enumerate() {
            let top = 46 + index as i32 * 29;
            let check = self.create_control(
                "BUTTON",
                metric.title(),
                WS_CHILD_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
                0,
                ID_METRIC_CHECK + index as i32,
                32,
                top,
                218,
                23,
            )?;
            self.metric_checks.push(check);
            let swatch = self.create_control(
                "BUTTON",
                "",
                WS_CHILD_VISIBLE | WS_TABSTOP | BS_OWNERDRAW,
                0,
                ID_METRIC_SWATCH + index as i32,
                268,
                top,
                56,
                23,
            )?;
            self.metric_swatches.push(swatch);
        }

        // --- preview -----------------------------------------------------
        self.group(" Preview ", 16, 286, 330, 122)?;
        self.preview_label = self.create_control(
            "STATIC",
            "144 FPS    120    65°C",
            WS_CHILD_VISIBLE | SS_CENTER,
            0,
            ID_PREVIEW,
            32,
            318,
            298,
            74,
        )?;

        // --- appearance --------------------------------------------------
        self.group(" Appearance ", 362, 16, 302, 258)?;
        self.label("Font", 378, 50, 40, 20)?;
        self.font_combo = self.create_control(
            "COMBOBOX",
            "",
            WS_CHILD_VISIBLE | WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST | CBS_HASSTRINGS,
            WS_EX_CLIENTEDGE,
            ID_FONT_COMBO,
            424,
            48,
            224,
            300,
        )?;
        unsafe {
            SendMessageW(
                self.font_combo,
                CB_SETDROPPEDWIDTH,
                Some(WPARAM(self.s(420) as usize)),
                None,
            );
        }
        self.label("Size", 378, 82, 40, 20)?;
        self.size_edit = self.create_control(
            "EDIT",
            "",
            WS_CHILD_VISIBLE | WS_TABSTOP | ES_NUMBER | ES_AUTOHSCROLL,
            WS_EX_CLIENTEDGE,
            ID_SIZE_EDIT,
            424,
            80,
            70,
            23,
        )?;
        self.label("Opacity", 378, 114, 40, 20)?;
        self.opacity_track = self.create_control(
            "msctls_trackbar32",
            "",
            WS_CHILD_VISIBLE | WS_TABSTOP | TBS_NOTICKS,
            0,
            ID_OPACITY_TRACK,
            424,
            110,
            130,
            28,
        )?;
        self.opacity_label = self.label("", 562, 114, 50, 20)?;
        self.bg_check = self.create_control(
            "BUTTON",
            "Background box",
            WS_CHILD_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX,
            0,
            ID_BG_CHECK,
            378,
            150,
            150,
            23,
        )?;
        self.bg_swatch = self.create_control(
            "BUTTON",
            "",
            WS_CHILD_VISIBLE | WS_TABSTOP | BS_OWNERDRAW,
            0,
            ID_BG_SWATCH,
            540,
            150,
            48,
            23,
        )?;

        // --- position ----------------------------------------------------
        self.group(" Position ", 362, 286, 302, 122)?;
        self.label("Screen position", 378, 318, 90, 20)?;
        self.position_combo = self.create_control(
            "COMBOBOX",
            "",
            WS_CHILD_VISIBLE | WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST | CBS_HASSTRINGS,
            WS_EX_CLIENTEDGE,
            ID_POSITION_COMBO,
            470,
            316,
            178,
            200,
        )?;
        self.label("Edge margin", 378, 352, 90, 20)?;
        self.margin_edit = self.create_control(
            "EDIT",
            "",
            WS_CHILD_VISIBLE | WS_TABSTOP | ES_NUMBER | ES_AUTOHSCROLL,
            WS_EX_CLIENTEDGE,
            ID_MARGIN_EDIT,
            470,
            350,
            70,
            23,
        )?;
        self.label("px", 548, 352, 30, 20)?;

        // --- actions + status --------------------------------------------
        self.hide_button = self.create_control(
            "BUTTON",
            "Hide overlay",
            WS_CHILD_VISIBLE | WS_TABSTOP,
            0,
            ID_HIDE_BUTTON,
            16,
            426,
            140,
            30,
        )?;
        self.elevate_button = self.create_control(
            "BUTTON",
            "Run as administrator",
            WS_CHILD_VISIBLE | WS_TABSTOP,
            0,
            ID_ELEVATE_BUTTON,
            168,
            426,
            200,
            30,
        )?;
        self.create_control(
            "BUTTON",
            "Quit",
            WS_CHILD_VISIBLE | WS_TABSTOP,
            0,
            ID_QUIT_BUTTON,
            536,
            426,
            128,
            30,
        )?;
        self.status_label = self.create_control(
            "STATIC",
            "",
            WS_CHILD_VISIBLE | SS_LEFT,
            0,
            ID_STATUS,
            16,
            470,
            648,
            78,
        )?;

        if self.admin {
            unsafe {
                let _ = ShowWindow(self.elevate_button, SW_HIDE);
            }
        }
        self.refresh_preview_font();
        Ok(())
    }

    fn sync_controls_from_config(&mut self) {
        let config = self.config.clone();
        for (index, metric) in Metric::ALL.into_iter().enumerate() {
            let checked = if config.enabled(metric) { 1 } else { 0 };
            unsafe {
                SendMessageW(
                    self.metric_checks[index],
                    BM_SETCHECK,
                    Some(WPARAM(checked)),
                    None,
                );
            }
        }

        if !self
            .font_options
            .iter()
            .any(|font| font == &config.font_family)
        {
            self.font_options.push(config.font_family.clone());
            self.font_options.sort_by_key(|font| font.to_lowercase());
        }
        for font in self.font_options.clone() {
            self.combo_add(self.font_combo, &font);
        }
        if let Some(index) = self
            .font_options
            .iter()
            .position(|font| font == &config.font_family)
        {
            self.combo_select(self.font_combo, index);
        }

        for position in self.position_options.clone() {
            self.combo_add(self.position_combo, &position);
        }
        if let Some(index) = self
            .position_options
            .iter()
            .position(|position| position == &config.position)
        {
            self.combo_select(self.position_combo, index);
        }

        set_window_text(self.size_edit, &config.font_size.to_string());
        set_window_text(self.margin_edit, &config.margin.to_string());
        set_window_text(self.opacity_label, &format!("{}%", config.opacity));

        unsafe {
            SendMessageW(
                self.opacity_track,
                TBM_SETRANGE,
                Some(WPARAM(1)),
                Some(LPARAM((((100u32 << 16) | 20u32) as i32) as isize)),
            );
            SendMessageW(
                self.opacity_track,
                TBM_SETPAGESIZE,
                Some(WPARAM(0)),
                Some(LPARAM(10)),
            );
            SendMessageW(
                self.opacity_track,
                TBM_SETPOS,
                Some(WPARAM(1)),
                Some(LPARAM(config.opacity as isize)),
            );
            SendMessageW(
                self.bg_check,
                BM_SETCHECK,
                Some(WPARAM(if config.show_background { 1 } else { 0 })),
                None,
            );
        }

        for swatch in &self.metric_swatches {
            invalidate(*swatch);
        }
        invalidate(self.bg_swatch);
        self.refresh_preview_font();
    }

    fn combo_add(&self, combo: HWND, text: &str) {
        let text = wide(text);
        unsafe {
            SendMessageW(
                combo,
                CB_ADDSTRING,
                None,
                Some(LPARAM(text.as_ptr() as isize)),
            );
        }
    }

    fn combo_select(&self, combo: HWND, index: usize) {
        unsafe {
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(index)), None);
        }
    }

    fn combo_selection(&self, combo: HWND) -> Option<usize> {
        let selected = unsafe { SendMessageW(combo, CB_GETCURSEL, None, None) }.0;
        (selected >= 0).then_some(selected as usize)
    }

    pub fn on_command(&mut self, id: i32, code: u16) {
        if (ID_METRIC_CHECK..ID_METRIC_CHECK + Metric::ALL.len() as i32).contains(&id) {
            if code == BN_CLICKED {
                let index = (id - ID_METRIC_CHECK) as usize;
                let metric = Metric::ALL[index];
                let checked =
                    unsafe { SendMessageW(self.metric_checks[index], BM_GETCHECK, None, None) }.0
                        == 1;
                self.config.set_enabled(metric, checked);
                self.apply();
            }
            return;
        }
        if (ID_METRIC_SWATCH..ID_METRIC_SWATCH + Metric::ALL.len() as i32).contains(&id) {
            let metric = Metric::ALL[(id - ID_METRIC_SWATCH) as usize];
            self.pick_color(Some(metric));
            return;
        }

        match id {
            ID_BG_CHECK if code == BN_CLICKED => {
                let checked =
                    unsafe { SendMessageW(self.bg_check, BM_GETCHECK, None, None) }.0 == 1;
                self.config.show_background = checked;
                self.apply();
            }
            ID_BG_SWATCH => self.pick_color(None),
            ID_FONT_COMBO if code == CBN_SELCHANGE => {
                if let Some(index) = self.combo_selection(self.font_combo) {
                    if let Some(font) = self.font_options.get(index) {
                        self.config.font_family = font.clone();
                        self.apply();
                    }
                }
            }
            ID_POSITION_COMBO if code == CBN_SELCHANGE => {
                if let Some(index) = self.combo_selection(self.position_combo) {
                    if let Some(position) = self.position_options.get(index) {
                        self.config.position = position.clone();
                        self.apply();
                    }
                }
            }
            ID_SIZE_EDIT if code == EN_KILLFOCUS => {
                if let Ok(size) = get_window_text(self.size_edit).trim().parse::<i32>() {
                    self.config.font_size = size.clamp(6, 72);
                }
                set_window_text(self.size_edit, &self.config.font_size.to_string());
                self.apply();
            }
            ID_MARGIN_EDIT if code == EN_KILLFOCUS => {
                if let Ok(margin) = get_window_text(self.margin_edit).trim().parse::<i32>() {
                    self.config.margin = margin.clamp(0, 300);
                }
                set_window_text(self.margin_edit, &self.config.margin.to_string());
                self.apply();
            }
            ID_HIDE_BUTTON if code == BN_CLICKED => {
                let show = !self.overlay.visible();
                self.overlay.set_visible(show);
                set_window_text(
                    self.hide_button,
                    if show { "Hide overlay" } else { "Show overlay" },
                );
                if show {
                    self.refresh_overlay();
                }
            }
            ID_ELEVATE_BUTTON if code == BN_CLICKED => {
                if elevate::relaunch_elevated() {
                    unsafe {
                        let _ = DestroyWindow(self.settings);
                    }
                } else {
                    set_window_text(
                        self.status_label,
                        "Administrator relaunch was cancelled. CPU temperature and FPS will stay --.",
                    );
                }
            }
            ID_QUIT_BUTTON if code == BN_CLICKED => unsafe {
                let _ = DestroyWindow(self.settings);
            },
            _ => {}
        }
    }

    pub fn on_hscroll(&mut self, lparam: LPARAM) {
        let track = HWND(lparam.0 as *mut c_void);
        if track != self.opacity_track {
            return;
        }
        let position = unsafe { SendMessageW(self.opacity_track, TBM_GETPOS, None, None) }.0 as i32;
        self.config.opacity = position.clamp(20, 100);
        set_window_text(self.opacity_label, &format!("{}%", self.config.opacity));
        self.apply();
    }

    pub fn on_draw_item(&mut self, lparam: LPARAM) {
        if lparam.0 == 0 {
            return;
        }
        let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
        let id = item.CtlID as i32;
        let color = if id == ID_BG_SWATCH {
            self.config.bg_argb()
        } else if (ID_METRIC_SWATCH..ID_METRIC_SWATCH + Metric::ALL.len() as i32).contains(&id) {
            self.config
                .color_argb(Metric::ALL[(id - ID_METRIC_SWATCH) as usize])
        } else {
            return;
        };

        unsafe {
            let brush = CreateSolidBrush(COLORREF(color & 0x00ff_ffff));
            FillRect(item.hDC, &item.rcItem, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
            let border = CreateSolidBrush(COLORREF(0x0080_8080));
            FrameRect(item.hDC, &item.rcItem, border);
            let _ = DeleteObject(HGDIOBJ(border.0));
        }
    }

    /// Colour the preview with the first enabled metric and the status line red
    /// when elevation is missing. Returns the window background brush.
    pub fn on_ctlcolor_static(&mut self, wparam: WPARAM, lparam: LPARAM) -> Option<isize> {
        let control = HWND(lparam.0 as *mut c_void);
        let color = if control == self.preview_label {
            let metric = self
                .config
                .enabled_metrics()
                .first()
                .copied()
                .unwrap_or(Metric::Fps);
            self.config.color_argb(metric)
        } else if control == self.status_label {
            if self.admin {
                0x005a_5a5a
            } else {
                0x00b0_0000
            }
        } else {
            return None;
        };

        let hdc = HDC(wparam.0 as *mut c_void);
        unsafe {
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(color));
        }
        Some(self.background_brush.0 as isize)
    }

    pub fn on_timer(&mut self, timer_id: usize) {
        match timer_id {
            1 => self.refresh_overlay(),
            2 => self.update_status(),
            _ => {}
        }
    }

    pub fn refresh_overlay(&mut self) {
        let stats = self.shared.stats();
        self.overlay.refresh(&self.config, &stats);
    }

    fn update_status(&mut self) {
        let stats = self.shared.stats();
        let admin = if self.admin {
            "yes".to_string()
        } else {
            "NO - FPS needs elevation".to_string()
        };
        let text = format!(
            "Administrator: {admin}\nSensors: {}\nFPS: {}",
            stats.hw_status, stats.fps_status
        );
        set_window_text(self.status_label, &text);
        invalidate(self.status_label);
    }

    fn refresh_preview_font(&mut self) {
        let key = (
            self.config.font_family.clone(),
            self.config.font_size.clamp(8, 28),
        );
        if self.preview_font_key.as_ref() == Some(&key) {
            return;
        }
        let Ok(font) = self.create_scaled_font(&key.0, key.1, 700) else {
            return;
        };
        if !self.preview_font.0.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.preview_font.0));
            }
        }
        self.preview_font = font;
        self.preview_font_key = Some(key);
        unsafe {
            SendMessageW(
                self.preview_label,
                WM_SETFONT,
                Some(WPARAM(self.preview_font.0 as usize)),
                Some(LPARAM(1)),
            );
            let _ = InvalidateRect(Some(self.preview_label), None, true);
        }
    }

    fn pick_color(&mut self, target: Option<Metric>) {
        let current = match target {
            Some(metric) => self.config.color_argb(metric),
            None => self.config.bg_argb(),
        };
        let mut custom = self.custom_colors;
        let mut dialog = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: self.settings,
            rgbResult: COLORREF(current & 0x00ff_ffff),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        let accepted = unsafe { ChooseColorW(&mut dialog).as_bool() };
        self.custom_colors = custom;
        if !accepted {
            return;
        }
        let hex = colorref_to_hex(dialog.rgbResult);
        match target {
            Some(metric) => {
                self.config.set_color(metric, hex);
                if let Some(index) = Metric::ALL.iter().position(|m| *m == metric) {
                    invalidate(self.metric_swatches[index]);
                }
            }
            None => {
                self.config.bg_color = hex;
                invalidate(self.bg_swatch);
            }
        }
        self.apply();
    }

    fn apply(&mut self) {
        self.shared.set_config(self.config.clone());
        self.config.save();
        self.refresh_preview_font();
        invalidate(self.preview_label);
        self.refresh_overlay();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            if !self.font.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(self.font.0));
            }
            if !self.preview_font.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(self.preview_font.0));
            }
            if !self.background_brush.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(self.background_brush.0));
            }
        }
    }
}

pub unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if !app.is_null() {
        if let Some(result) = crate::ui::dispatch(app, hwnd, message, wparam, lparam) {
            return result;
        }
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

pub unsafe fn create_window(instance: HINSTANCE, app_ptr: *const c_void) -> Result<HWND, String> {
    let class = wide(crate::ui::SETTINGS_CLASS);
    let title = wide(crate::ui::SETTINGS_TITLE);
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class.as_ptr()),
        PCWSTR(title.as_ptr()),
        WINDOW_STYLE(style),
        40,
        40,
        720,
        620,
        None,
        None,
        Some(instance),
        Some(app_ptr),
    )
    .map_err(|e| format!("could not create settings window: {e}"))
}

fn colorref_to_hex(color: COLORREF) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        color.0 & 0xff,
        (color.0 >> 8) & 0xff,
        (color.0 >> 16) & 0xff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_to_hex_is_rgb() {
        assert_eq!(colorref_to_hex(COLORREF(0x00332211)), "#112233");
    }
}
