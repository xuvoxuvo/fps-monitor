//! `--check` and `--selftest`: diagnostics that can run without a desktop and
//! are safe to use in CI.
//!
//! Output goes to stdout (visible in debug builds and in a console) and to a
//! file under `%APPDATA%\fps-monitor\`, because release builds are windowed and
//! have no console to print to.

use std::fmt::Write as _;
use std::fs;

use crate::app::assets::Assets;
use crate::app::config::{Config, Metric};
use crate::app::shared::{format_value, Shared, SharedHandle};
use crate::platform::windows::is_admin;
use crate::sensors::win_gpu::WinGpu;
use crate::workers::Workers;

pub fn check() -> Result<(), String> {
    let mut report = Report::new("check.txt");
    let assets = environment(&mut report);

    let mut failed = !assets.missing().is_empty();
    if let Err(problems) = assets.verify() {
        failed = true;
        report.line("");
        report.line("asset verification FAILED:");
        for problem in problems {
            report.line(&format!("  - {problem}"));
        }
    } else {
        report.line("");
        report.line("asset checksums: OK");
    }

    report.write_file();
    report.print();
    if failed {
        Err("asset check failed (see the report above or check.txt)".into())
    } else {
        Ok(())
    }
}

pub fn selftest(seconds: u64) -> Result<(), String> {
    let mut report = Report::new("selftest.txt");
    let assets = environment(&mut report);
    let config = Config::load();
    let shared: SharedHandle = std::sync::Arc::new(Shared::new(config, assets));

    let workers = Workers::start(&shared);
    report.line("");
    report.line(&format!("collecting for {seconds} seconds..."));
    std::thread::sleep(std::time::Duration::from_secs(seconds));

    let stats = shared.stats();
    report.line("");
    report.line("stats:");
    let json = serde_json::to_string_pretty(&stats.to_json()).unwrap_or_default();
    for line in json.lines() {
        report.line(&format!("  {line}"));
    }
    report.line("");
    report.line("readable values:");
    for metric in Metric::ALL {
        report.line(&format!(
            "  {:<12} {}",
            metric.id(),
            format_value(metric, &stats)
        ));
    }
    report.line("");
    report.line(&format!("hw_status:   {}", stats.hw_status));
    report.line(&format!("fps_status:  {}", stats.fps_status));
    report.line("");
    report.line(
        "(if FPS is null: run as administrator and keep the game in the foreground; see fps.log)",
    );

    workers.stop(&shared);
    report.write_file();
    report.print();
    Ok(())
}

/// Shared environment section for both diagnostics. Returns the located assets.
fn environment(report: &mut Report) -> Assets {
    report.line(&format!("fps-monitor {}", crate::VERSION));
    report.line(&format!(
        "exe            : {}",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".into())
    ));
    report.line(&format!(
        "admin rights   : {}",
        if is_admin() {
            "YES".to_string()
        } else {
            "NO  <-- FPS capture needs this".to_string()
        }
    ));
    report.line(&format!(
        "Performance Log Users: {}",
        if crate::platform::windows::in_performance_log_users() {
            "yes"
        } else {
            "no (may cause ETW error 1450; add the account and reboot)"
        }
    ));
    report.line(&format!(
        "config         : {}",
        Config::config_path().display()
    ));

    let assets = Assets::locate();
    report.line(&format!("assets         : {}", assets.describe()));
    for (name, _, expected_len) in crate::app::assets::FILE_HASHES {
        let path = assets.path(name);
        let actual = fs::metadata(&path).map(|m| m.len()).ok();
        match actual {
            Some(len) if len == *expected_len => {
                report.line(&format!("  {name:<48} found ({len} bytes)"))
            }
            Some(len) => report.line(&format!(
                "  {name:<48} WRONG SIZE ({len} != {expected_len})"
            )),
            None => report.line(&format!("  {name:<48} MISSING")),
        }
    }

    let mut gpu = WinGpu::new();
    let gpu_name = gpu.name.clone().unwrap_or_else(|| "not detected".into());
    let gpu_temp = gpu.temperature();
    let gpu_load = gpu.load();
    report.line(&format!(
        "gpu (windows)  : {gpu_name} temp={gpu_temp:?} load={gpu_load:?}"
    ));
    report.line(&format!(
        "cpu            : {}",
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".into())
    ));
    assets
}

struct Report {
    text: String,
    filename: &'static str,
}

impl Report {
    fn new(filename: &'static str) -> Self {
        Self {
            text: String::new(),
            filename,
        }
    }

    fn line(&mut self, line: &str) {
        let _ = writeln!(self.text, "{line}");
    }

    /// Print without ever panicking: release builds are windowed and have no
    /// usable stdout handle.
    fn print(&self) {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(self.text.as_bytes());
        let _ = stdout.flush();
    }

    fn write_file(&self) {
        write_report(self.filename, &self.text);
    }
}

/// Write a diagnostic report next to the config, for windowed builds where
/// stdout is not visible.
pub fn write_report(filename: &str, text: &str) {
    let dir = Config::config_dir();
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join(filename), text);
    }
}

/// Persist a startup failure. Release builds are windowed, so this is the only
/// record a user can send along with a bug report after dismissing the box.
pub fn write_error(message: &str) {
    let dir = Config::config_dir();
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join("error.txt"), format!("{message}\n"));
    }
}

#[cfg_attr(debug_assertions, allow(dead_code))]
pub fn show_error(message: &str) {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let title = HSTRING::from("fps-monitor");
    let body = HSTRING::from(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}
