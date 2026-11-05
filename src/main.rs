//! fps-monitor - a lightweight FPS overlay for Windows.
//!
//! PresentMon supplies frame times and the overlay draws them through a
//! click-through layered window. CPU/GPU usage and GPU temperature come from
//! native Windows APIs; there is no helper service, DLL or driver.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod diagnostics;
mod platform;
mod sensors;
mod ui;
mod workers;

use std::sync::Arc;

use app::{assets, config, shared};
use platform::elevate;
use windows::Win32::System::LibraryLoader::{
    SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

mod build_info {
    include!(concat!(env!("OUT_DIR"), "/version.rs"));
}
pub use build_info::VERSION;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        safe_stdout(&format!("fps-monitor {VERSION}\n"));
        return;
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return;
    }
    if args.iter().any(|arg| arg == "--check") {
        match diagnostics::check() {
            Ok(()) => return,
            Err(message) => {
                safe_stderr(&format!("fps-monitor: {message}\n"));
                std::process::exit(2);
            }
        }
    }
    if args.iter().any(|arg| arg == "--selftest") {
        let seconds = arg_value(&args, "--seconds")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(8);
        if let Err(message) = diagnostics::selftest(seconds) {
            safe_stderr(&format!("fps-monitor: {message}\n"));
            std::process::exit(1);
        }
        return;
    }

    if args.iter().any(|arg| arg == "--ui-smoke") {
        let seconds = arg_value(&args, "--seconds")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(3);
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let assets = assets::Assets::locate();
        let config = config::Config::load();
        let shared: shared::SharedHandle = Arc::new(shared::Shared::new(config, assets));
        return match ui::smoke(shared, seconds) {
            Ok(report) => {
                let text = format!(
                    "settings: {}x{} at ({}, {})
overlay : {}x{} at ({}, {}) visible={}
controls: {}
overlay error: {}
drawn pixels: {}
width before/after value change: {}/{}
",
                    report.settings.width,
                    report.settings.height,
                    report.settings.x,
                    report.settings.y,
                    report.overlay.width,
                    report.overlay.height,
                    report.overlay.x,
                    report.overlay.y,
                    report.overlay_visible,
                    report.controls,
                    report.overlay_error.as_deref().unwrap_or("none"),
                    report.drawn_pixels,
                    report.width_before,
                    report.width_after
                );
                safe_stdout(&text);
                diagnostics::write_report("ui-smoke.txt", &text);
            }
            Err(message) => {
                safe_stderr(&format!("fps-monitor: {message}\n"));
                std::process::exit(1);
            }
        };
    }

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Restrict DLL resolution to the application directory, System32 and
        // any directories we add explicitly. The current working directory is
        // excluded, so a game (or a shortcut's "Start in" folder) cannot plant
        // a gdiplus.dll / pdh.dll next to the overlay and get it loaded.
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
    }

    // The elevated copy runs with --elevated so it does not prompt again.
    elevate::ensure_admin(&args);

    if let Err(message) = run() {
        safe_stderr(&format!("fps-monitor: {message}\n"));
        diagnostics::write_error(&message);
        // Release builds are windowed, so a message box is the only way to be
        // seen. Debug builds have a console (and are easier to script).
        #[cfg(not(debug_assertions))]
        diagnostics::show_error(&message);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let assets = assets::Assets::locate();
    let config = config::Config::load();
    let shared: shared::SharedHandle = Arc::new(shared::Shared::new(config, assets));
    let workers = workers::Workers::start(&shared);

    let result = ui::run(shared.clone());
    workers.stop(&shared);
    result
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn print_help() {
    let help = format!(
        "fps-monitor {VERSION}

Usage: fps-monitor [options]

Options:
  --check              Verify the bundled PresentMon/sensor files and exit.
  --selftest           Print a full diagnostic (also written to
                       %APPDATA%\\fps-monitor\\selftest.txt).
  --ui-smoke           Create both windows for N seconds (layout smoke test).
  --seconds N          Duration for --selftest / --ui-smoke (default 8 / 3).
  --no-elevate         Do not ask for administrator rights.
  --elevated           Internal: marks the already-elevated copy.
  -V, --version        Print the version.
  -h, --help           Print this help.

FPS capture needs administrator rights.
Decline UAC and those metrics show --.
"
    );
    safe_stdout(&help);
}

/// Never panic when there is no console (release builds are windowed).
fn safe_stdout(message: &str) {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(message.as_bytes());
    let _ = stdout.flush();
}

/// Never panic when there is no console (release builds are windowed).
fn safe_stderr(message: &str) {
    use std::io::Write;
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(message.as_bytes());
    let _ = stderr.flush();
}
