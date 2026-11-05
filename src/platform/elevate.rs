//! Elevation plumbing.
//!
//! Administrator rights are required for FPS capture (ETW session). Declining
//! UAC is not fatal: the app keeps running, FPS shows `--`, and the settings
//! window offers a "Run as administrator" button that retries the relaunch.

use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Quote one argument for the Windows command line.
fn quote_arg(arg: &str) -> String {
    if arg.is_empty() || arg.contains([' ', '\t', '"']) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

/// Start an elevated copy of the exe (`--elevated`). Returns true when the
/// elevated copy was launched; the caller must then quit this instance.
///
/// Command-line arguments are forwarded, otherwise options would be silently
/// lost on the UAC relaunch.
pub fn relaunch_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut params = String::from("--elevated");
    for arg in std::env::args().skip(1) {
        if arg == "--elevated" {
            continue;
        }
        params.push(' ');
        params.push_str(&quote_arg(&arg));
    }

    let verb = crate::platform::windows::wide("runas");
    let file = crate::platform::windows::wide(&exe.to_string_lossy());
    let params = crate::platform::windows::wide(&params);

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a value > 32 on success.
    result.0 as isize > 32
}

/// Prompt at startup unless already elevated or explicitly told not to.
pub fn ensure_admin(args: &[String]) -> bool {
    let admin = crate::platform::windows::is_admin();
    if admin
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--elevated" | "--no-elevate"))
    {
        return admin;
    }
    if relaunch_elevated() {
        std::process::exit(0);
    }

    // The UAC prompt was declined or blocked. Make the consequence explicit
    // instead of silently showing `--` for the two admin-only metrics.
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND,
    };
    let title = HSTRING::from("fps-monitor");
    let body = HSTRING::from(
        "fps-monitor is running without administrator rights.\n\n\
         FPS needs elevation and will show --.\n\
         You can retry later with the \"Run as administrator\" button in the settings window.",
    );
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND,
        );
    }
    false
}
