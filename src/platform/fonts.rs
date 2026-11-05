//! Enumerate installed font families for the font picker.
//!
//! Using `EnumFontFamiliesExW` instead of a hard-coded list means every font
//! the user has installed (including ones added after the app was built) is
//! available to the overlay.

use std::collections::BTreeSet;

use windows::Win32::Foundation::LPARAM;
use windows::Win32::Graphics::Gdi::{
    EnumFontFamiliesExW, GetDC, ReleaseDC, DEFAULT_CHARSET, LOGFONTW, TEXTMETRICW,
};

/// Installed font families, sorted case-insensitively, without the `@...`
/// vertical-writing variants and without duplicates.
pub fn installed_families() -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut logfont = LOGFONTW {
        lfCharSet: DEFAULT_CHARSET,
        ..Default::default()
    };

    unsafe {
        let dc = GetDC(None);
        if !dc.0.is_null() {
            let _ = EnumFontFamiliesExW(
                dc,
                &logfont,
                Some(collect),
                LPARAM(&mut names as *mut BTreeSet<String> as isize),
                0,
            );
            let _ = ReleaseDC(None, dc);
        }
    }

    // `logfont` is only read by the callback; keeping it alive for the whole
    // call is what matters.
    let _ = &mut logfont;
    names.into_iter().collect()
}

unsafe extern "system" fn collect(
    logfont: *const LOGFONTW,
    _textmetric: *const TEXTMETRICW,
    _font_type: u32,
    lparam: LPARAM,
) -> i32 {
    if logfont.is_null() || lparam.0 == 0 {
        return 1;
    }
    let face = (*logfont).lfFaceName;
    let end = face.iter().position(|c| *c == 0).unwrap_or(face.len());
    let name = String::from_utf16_lossy(&face[..end]);
    if !name.is_empty() && !name.starts_with('@') {
        if let Some(set) = (lparam.0 as *mut BTreeSet<String>).as_mut() {
            set.insert(name);
        }
    }
    1 // continue enumeration
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_at_least_one_family() {
        let families = installed_families();
        assert!(!families.is_empty());
        assert!(families.iter().all(|name| !name.starts_with('@')));
    }
}
