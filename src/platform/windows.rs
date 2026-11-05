//! Small Win32 helpers shared by the UI and the workers: process lookups,
//! window text, DPI maths, CPU load and child-process control.

use std::ffi::OsStr;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::Threading::{
    GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::IsUserAnAdmin;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowTextW, GetWindowThreadProcessId,
    SetWindowTextW, SM_CXSCREEN, SM_CYSCREEN,
};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// NUL-terminated UTF-16 for the wide Win32 APIs.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

/// Whether the current token carries the built-in "Performance Log Users"
/// group (SID S-1-5-32-559). PresentMon can capture ETW with this group even
/// when the process is not elevated, and the group is also part of the fix for
/// graphics-provider error 1450.
pub fn in_performance_log_users() -> bool {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let whoami = format!("{root}\\System32\\whoami.exe");
    let output = match hidden_command(&whoami).arg("/groups").output() {
        Ok(output) => output,
        Err(_) => return false,
    };
    String::from_utf8_lossy(&output.stdout).contains("S-1-5-32-559")
}

pub fn foreground_pid() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

/// Name of a running process, lower-cased (`foo.exe`), for the foreground
/// ignore-list. Uses the query-limited handle so it works for normal apps.
pub fn process_name(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Path::new(&full)
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
    }
}

/// `GetSystemTimes` deltas -> percent busy (Task Manager style).
#[derive(Default)]
pub struct CpuLoad {
    previous: Option<(u64, u64, u64)>,
}
impl CpuLoad {
    pub fn sample(&mut self) -> Option<f64> {
        unsafe {
            let mut idle = Default::default();
            let mut kernel = Default::default();
            let mut user = Default::default();
            GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()?;
            let idle = filetime(idle);
            let kernel = filetime(kernel);
            let user = filetime(user);

            let previous = self.previous.replace((idle, kernel, user))?;
            let idle_delta = idle.saturating_sub(previous.0) as f64;
            let total_delta =
                (kernel.saturating_sub(previous.1) + user.saturating_sub(previous.2)) as f64;
            if total_delta <= 0.0 {
                return None;
            }
            Some(((total_delta - idle_delta) / total_delta * 100.0).clamp(0.0, 100.0))
        }
    }
}

fn filetime(ft: windows::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

pub fn screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

pub fn dpi_for_window(hwnd: HWND) -> u32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        96
    } else {
        dpi
    }
}

/// Scale a 96-DPI design unit to the current monitor.
pub fn scale(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi as i64) / 96) as i32
}

pub fn get_window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

pub fn set_window_text(hwnd: HWND, text: &str) {
    let wide = wide(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
    }
}

pub fn invalidate(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

pub fn sleep_interruptible(shared: &crate::app::shared::Shared, duration: Duration) {
    let deadline = Instant::now() + duration;
    while shared.is_running() && Instant::now() < deadline {
        std::thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

pub fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

pub fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_maths() {
        assert_eq!(scale(100, 96), 100);
        assert_eq!(scale(100, 144), 150);
        assert_eq!(scale(7, 120), 8);
    }
}
