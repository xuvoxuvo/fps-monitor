//! The click-through overlay window.
//!
//! A `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`
//! popup is rendered with GDI+ into a 32-bit premultiplied DIB and pushed to
//! the desktop with `UpdateLayeredWindow`. This is deliberate: an OpenGL/egui
//! overlay would itself consume GPU time and pollute the FPS it is measuring,
//! while a layered GDI+ window costs nothing between the once-a-second redraws.

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::Graphics::GdiPlus::{
    FontStyleBold, GdipCreateBitmapFromScan0, GdipCreateFont, GdipCreateFontFamilyFromName,
    GdipCreateSolidFill, GdipCreateStringFormat, GdipDeleteBrush, GdipDeleteFont,
    GdipDeleteFontFamily, GdipDeleteGraphics, GdipDeleteStringFormat, GdipDisposeImage,
    GdipDrawString, GdipFillRectangleI, GdipGetFontHeight, GdipGetGenericFontFamilySansSerif,
    GdipGetImageGraphicsContext, GdipGraphicsClear, GdipMeasureString, GdipSetSolidFillColor,
    GdipSetStringFormatAlign, GdipSetStringFormatFlags, GdipSetTextRenderingHint, GpBitmap, GpFont,
    GpFontFamily, GpGraphics, GpImage, GpSolidFill, GpStringFormat, RectF, StringAlignmentFar,
    StringAlignmentNear, TextRenderingHintAntiAliasGridFit, UnitPixel,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetSystemMetrics, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, UpdateLayeredWindow, GWL_EXSTYLE, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::app::config::{Config, Metric};
use crate::app::shared::{format_value, Stats};

pub const CLASS_NAME: &str = "FpsMonitorOverlay";

/// The overlay never receives input (it is click-through), so the default
/// window procedure is all it needs. The windows crate exposes
/// `DefWindowProcW` as a Rust fn, so a thin `extern "system"` shim is required
/// for class registration.
pub unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, message, wparam, lparam)
}

const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;
const STRING_FORMAT_NO_CLIP: i32 = 0x4000;

pub struct Overlay {
    hwnd: HWND,
    visible: bool,
    glyphs: Option<Glyphs>,
    dpi: u32,
    last_error: Option<String>,
    drawn_pixels: usize,
}

struct Row {
    color: u32,
    label: String,
    value: String,
    /// Widest value this metric can realistically produce. Used only to size
    /// the window so changing numbers never resize it.
    reserved: &'static str,
}

fn reserve_value(metric: Metric) -> &'static str {
    match metric {
        Metric::Fps | Metric::FpsLow | Metric::FpsLow01 => "9999",
        Metric::Cpu | Metric::Gpu => "100 %",
        Metric::GpuTemp => "100°C",
    }
}

struct Glyphs {
    family: *mut GpFontFamily,
    font: *mut GpFont,
    measure_bitmap: *mut GpBitmap,
    measure_graphics: *mut GpGraphics,
    line_height: f32,
    key: (String, i32),
}

impl Drop for Glyphs {
    fn drop(&mut self) {
        unsafe {
            if !self.measure_graphics.is_null() {
                let _ = GdipDeleteGraphics(self.measure_graphics);
            }
            if !self.measure_bitmap.is_null() {
                let _ = GdipDisposeImage(self.measure_bitmap as *mut GpImage);
            }
            if !self.font.is_null() {
                let _ = GdipDeleteFont(self.font);
            }
            if !self.family.is_null() {
                let _ = GdipDeleteFontFamily(self.family);
            }
        }
    }
}

impl Overlay {
    pub fn create(instance: windows::Win32::Foundation::HINSTANCE) -> Result<Self, String> {
        unsafe {
            let class = crate::platform::windows::wide(CLASS_NAME);
            // WS_EX_LAYERED must NOT be present at creation: combining it with
            // WS_EX_TRANSPARENT here makes UpdateLayeredWindow fail with
            // ERROR_GEN_FAILURE. Adding it afterwards is safe (same workaround
            // Qt uses for click-through layered windows).
            let hwnd = CreateWindowExW(
                WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class.as_ptr()),
                PCWSTR(crate::platform::windows::wide("fps-monitor").as_ptr()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )
            .map_err(|e| format!("could not create overlay window: {e}"))?;
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
            let dpi = crate::platform::windows::dpi_for_window(hwnd);
            Ok(Self {
                hwnd,
                visible: true,
                glyphs: None,
                dpi,
                last_error: None,
                drawn_pixels: 0,
            })
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn drawn_pixels(&self) -> usize {
        self.drawn_pixels
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        unsafe {
            if visible {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Rebuild the layered bitmap from the current config/stats and place the
    /// window. Called once a second, and immediately on any settings change.
    pub fn refresh(&mut self, config: &Config, stats: &Stats) {
        if !self.visible {
            return;
        }
        match self.render(config, stats) {
            Ok(()) => self.last_error = None,
            // Rendering must never take the app down; the next tick retries.
            Err(e) => self.last_error = Some(e),
        }
    }

    fn render(&mut self, config: &Config, stats: &Stats) -> Result<(), String> {
        let metrics = config.enabled_metrics();
        let rows: Vec<Row> = metrics
            .iter()
            .map(|metric| Row {
                color: config.color_argb(*metric),
                label: config.metric(*metric).label,
                value: format_value(*metric, stats),
                reserved: reserve_value(*metric),
            })
            .collect();

        let px = ((config.font_size as f32) * self.dpi as f32 / 72.0)
            .round()
            .clamp(6.0, 200.0) as i32;
        self.ensure_glyphs(&config.font_family, px)?;
        let glyphs = self.glyphs.as_ref().ok_or("font unavailable")?;

        let pad_x = ((px as f32 * 0.7).round() as i32).max(6);
        let pad_y = ((px as f32 * 0.4).round() as i32).max(4);
        let gap = ((px as f32 * 0.6).round() as i32).max(8);
        let line_height = glyphs.line_height.max(px as f32);

        // Size the window from the label column and the widest *reserved*
        // value, not from the current numbers. Otherwise "9 %" -> "100 %"
        // would resize the overlay and make the whole thing jump every second.
        let mut label_width = 0.0f32;
        let mut value_width = 0.0f32;
        for row in &rows {
            if let Some((w, _)) = measure(glyphs, &row.label) {
                label_width = label_width.max(w);
            }
            if let Some((w, _)) = measure(glyphs, row.reserved) {
                value_width = value_width.max(w);
            }
        }
        // "--" can be wider than a reserved number in some proportional fonts.
        if let Some((w, _)) = measure(glyphs, "--") {
            value_width = value_width.max(w);
        }

        let width = ((label_width.ceil() + value_width.ceil()) as i32 + pad_x * 2 + gap).max(1);
        let height = ((rows.len() as f32 * line_height).ceil() as i32 + pad_y * 2).max(1);

        unsafe {
            let header = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let info = BITMAPINFO {
                bmiHeader: header,
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)
                .map_err(|e| format!("CreateDIBSection: {e}"))?;
            if bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                return Err("CreateDIBSection: no pixels".into());
            }

            let memory_dc = CreateCompatibleDC(None);

            let mut image: *mut GpBitmap = std::ptr::null_mut();
            let status = GdipCreateBitmapFromScan0(
                width,
                height,
                width * 4,
                PIXEL_FORMAT_32BPP_ARGB,
                Some(bits as *const u8),
                &mut image,
            );
            if status.0 != 0 || image.is_null() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(memory_dc);
                return Err("GDI+ bitmap creation failed".into());
            }

            let mut graphics: *mut GpGraphics = std::ptr::null_mut();
            if GdipGetImageGraphicsContext(image as *mut GpImage, &mut graphics).0 != 0 {
                let _ = GdipDisposeImage(image as *mut GpImage);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(memory_dc);
                return Err("GDI+ graphics creation failed".into());
            }

            let _ = GdipSetTextRenderingHint(graphics, TextRenderingHintAntiAliasGridFit);
            let _ = GdipGraphicsClear(graphics, 0);

            if config.show_background {
                let mut background: *mut GpSolidFill = std::ptr::null_mut();
                if GdipCreateSolidFill(config.bg_argb(), &mut background).0 == 0 {
                    let _ = GdipFillRectangleI(graphics, background as *mut _, 0, 0, width, height);
                    let _ = GdipDeleteBrush(background as *mut _);
                }
            }

            let mut label_format: *mut GpStringFormat = std::ptr::null_mut();
            let mut value_format: *mut GpStringFormat = std::ptr::null_mut();
            let _ = GdipCreateStringFormat(0, 0, &mut label_format);
            let _ = GdipCreateStringFormat(0, 0, &mut value_format);
            let _ = GdipSetStringFormatAlign(label_format, StringAlignmentNear);
            let _ = GdipSetStringFormatAlign(value_format, StringAlignmentFar);
            let _ = GdipSetStringFormatFlags(label_format, STRING_FORMAT_NO_CLIP);
            let _ = GdipSetStringFormatFlags(value_format, STRING_FORMAT_NO_CLIP);

            let value_left = pad_x + label_width.ceil() as i32 + gap;
            let value_rect_width = (width - pad_x - value_left).max(1) as f32;

            let mut brush: *mut GpSolidFill = std::ptr::null_mut();
            if GdipCreateSolidFill(0xffff_ffff, &mut brush).0 == 0 {
                for (index, row) in rows.iter().enumerate() {
                    let _ = GdipSetSolidFillColor(brush, row.color);
                    let y = pad_y as f32 + index as f32 * line_height;
                    let label_rect = RectF {
                        X: pad_x as f32,
                        Y: y,
                        Width: label_width + 2.0,
                        Height: line_height * 1.5,
                    };
                    let value_rect = RectF {
                        X: value_left as f32,
                        Y: y,
                        Width: value_rect_width,
                        Height: line_height * 1.5,
                    };
                    let label = crate::platform::windows::wide(&row.label);
                    let value = crate::platform::windows::wide(&row.value);
                    let _ = GdipDrawString(
                        graphics,
                        PCWSTR(label.as_ptr()),
                        -1,
                        glyphs.font,
                        &label_rect,
                        label_format,
                        brush as *mut _,
                    );
                    let _ = GdipDrawString(
                        graphics,
                        PCWSTR(value.as_ptr()),
                        -1,
                        glyphs.font,
                        &value_rect,
                        value_format,
                        brush as *mut _,
                    );
                }
                let _ = GdipDeleteBrush(brush as *mut _);
            }
            if !label_format.is_null() {
                let _ = GdipDeleteStringFormat(label_format);
            }
            if !value_format.is_null() {
                let _ = GdipDeleteStringFormat(value_format);
            }

            let _ = GdipDeleteGraphics(graphics);
            let _ = GdipDisposeImage(image as *mut GpImage);
            let _ = GdiFlush();
            let old = SelectObject(memory_dc, HGDIOBJ(bitmap.0));

            // GDI+ writes straight ARGB; UpdateLayeredWindow wants premultiplied.
            let pixels =
                std::slice::from_raw_parts_mut(bits as *mut u8, (width * height * 4) as usize);
            premultiply(pixels);
            self.drawn_pixels = pixels.chunks_exact(4).filter(|pixel| pixel[3] != 0).count();

            let (screen_width, screen_height) =
                { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
            let margin = config.margin;
            let position = config.position.as_str();
            let x = if position.contains("Left") {
                margin
            } else if position.contains("Right") {
                screen_width - width - margin
            } else {
                (screen_width - width) / 2
            };
            let y = if position.contains("Top") {
                margin
            } else if position.contains("Bottom") {
                screen_height - height - margin
            } else {
                (screen_height - height) / 2
            };

            let alpha = ((config.opacity.clamp(20, 100) as u32 * 255) / 100) as u8;
            let blend = BLENDFUNCTION {
                BlendOp: 0, // AC_SRC_OVER
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: 1, // AC_SRC_ALPHA
            };
            let destination = POINT { x, y };
            let size = SIZE {
                cx: width,
                cy: height,
            };
            let source = POINT { x: 0, y: 0 };
            let screen_dc = GetDC(None);
            let result = UpdateLayeredWindow(
                self.hwnd,
                Some(screen_dc),
                Some(&destination),
                Some(&size),
                Some(memory_dc),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );

            let _ = SelectObject(memory_dc, old);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory_dc);
            let _ = ReleaseDC(None, screen_dc);

            result.map_err(|e| format!("UpdateLayeredWindow({width}x{height}): {e}"))?;
            // Re-assert topmost every refresh: some games steal the z-order.
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            Ok(())
        }
    }

    fn ensure_glyphs(&mut self, family_name: &str, px: i32) -> Result<(), String> {
        let key = (family_name.to_string(), px);
        if self.glyphs.as_ref().is_some_and(|glyphs| glyphs.key == key) {
            return Ok(());
        }
        self.glyphs = None;

        unsafe {
            let mut family: *mut GpFontFamily = std::ptr::null_mut();
            let name = crate::platform::windows::wide(family_name);
            let mut status = GdipCreateFontFamilyFromName(
                PCWSTR(name.as_ptr()),
                std::ptr::null_mut(),
                &mut family,
            );
            if status.0 != 0 || family.is_null() {
                status = GdipGetGenericFontFamilySansSerif(&mut family);
            }
            if status.0 != 0 || family.is_null() {
                return Err(format!("font family {family_name:?} unavailable"));
            }

            let mut font: *mut GpFont = std::ptr::null_mut();
            if GdipCreateFont(family, px as f32, FontStyleBold.0, UnitPixel, &mut font).0 != 0
                || font.is_null()
            {
                let _ = GdipDeleteFontFamily(family);
                return Err("could not create overlay font".into());
            }

            let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
            if GdipCreateBitmapFromScan0(1, 1, 4, PIXEL_FORMAT_32BPP_ARGB, None, &mut bitmap).0 != 0
            {
                let _ = GdipDeleteFont(font);
                let _ = GdipDeleteFontFamily(family);
                return Err("could not create measurement surface".into());
            }
            let mut graphics: *mut GpGraphics = std::ptr::null_mut();
            if GdipGetImageGraphicsContext(bitmap as *mut GpImage, &mut graphics).0 != 0 {
                let _ = GdipDisposeImage(bitmap as *mut GpImage);
                let _ = GdipDeleteFont(font);
                let _ = GdipDeleteFontFamily(family);
                return Err("could not create measurement graphics".into());
            }

            let mut line_height = px as f32;
            let mut measured = 0.0f32;
            if GdipGetFontHeight(font, graphics, &mut measured).0 == 0 {
                line_height = measured;
            }

            self.glyphs = Some(Glyphs {
                family,
                font,
                measure_bitmap: bitmap,
                measure_graphics: graphics,
                line_height,
                key,
            });
            Ok(())
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.glyphs = None;
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
    }
}

fn measure(glyphs: &Glyphs, text: &str) -> Option<(f32, f32)> {
    unsafe {
        let mut format: *mut GpStringFormat = std::ptr::null_mut();
        if GdipCreateStringFormat(0, 0, &mut format).0 != 0 {
            return None;
        }
        let _ = GdipSetStringFormatFlags(format, STRING_FORMAT_NO_CLIP);
        let rect = RectF {
            X: 0.0,
            Y: 0.0,
            Width: 4000.0,
            Height: glyphs.line_height * 2.0,
        };
        let text = crate::platform::windows::wide(text);
        let mut bounds = RectF::default();
        let status = GdipMeasureString(
            glyphs.measure_graphics,
            PCWSTR(text.as_ptr()),
            -1,
            glyphs.font,
            &rect,
            format,
            &mut bounds,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        let _ = GdipDeleteStringFormat(format);
        (status.0 == 0).then_some((bounds.Width, bounds.Height))
    }
}

/// Convert straight ARGB pixels to premultiplied ARGB in place.
fn premultiply(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }
        if alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((*channel as u32 * alpha + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplies_alpha() {
        let mut pixels = [255u8, 128, 0, 128, 9, 9, 9, 0];
        premultiply(&mut pixels);
        assert_eq!(&pixels[..4], &[128, 64, 0, 128]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 0]);
    }
}
