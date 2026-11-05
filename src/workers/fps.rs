//! FPS capture: drives Intel PresentMon and turns its CSV frame stream into
//! average FPS and the 1% / 0.1% lows.
//!
//! Every CSV row PresentMon emits is one presented frame; its
//! `MsBetweenPresents` column is that frame's time. Average FPS counts frames
//! over a 2 s window. The lows are `1000 / p99` and `1000 / p99.9` of frame
//! times over a 60 s window: the frame rate during your worst 1% (0.1%) of
//! frames, which is what stutter actually feels like.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::app::shared::SharedHandle;
use crate::platform::windows::{
    foreground_pid, hidden_command, kill_child, process_name, sleep_interruptible,
};
use crate::workers::WorkerHandle;

const AVG_WINDOW: Duration = Duration::from_secs(2);
const LOW_WINDOW: Duration = Duration::from_secs(60);
const MIN_LOW_FRAMES: usize = 120;
const MIN_LOW01_FRAMES: usize = 1000;
const MAX_SAMPLES: usize = 300_000;
/// One stable name: the next PresentMon launch uses `--stop_existing_session`
/// to clear a session left behind by a killed process, and shutdown uses
/// `--terminate_existing_session`. Unique names would leak one session per
/// retarget and eventually exhaust ETW resources.
const SESSION_NAME: &str = "FpsMonitorFPS";

static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();

fn init_log() {
    let dir = crate::app::config::Config::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = File::create(dir.join("fps.log")).ok();
    let _ = LOG.set(Mutex::new(file));
}

/// Append one diagnostic line to `%APPDATA%\fps-monitor\fps.log`.
///
/// Deliberately terse (target changes, CSV header, first frame, errors), not
/// one line per frame. It is the first thing to look at when FPS is `--`.
fn log(message: &str) {
    let Some(slot) = LOG.get() else {
        return;
    };
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    let Some(file) = guard.as_mut() else {
        return;
    };
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "{secs} {message}");
    let _ = file.flush();
}

type Samples = Arc<Mutex<VecDeque<(Instant, Option<f64>)>>>;
type Slot = Arc<Mutex<Option<Child>>>;

pub fn start(shared: &SharedHandle) -> WorkerHandle {
    let shared = shared.clone();
    let slot: Slot = Arc::new(Mutex::new(None));
    let stop_slot = slot.clone();
    let stop_exe = shared.assets.path("PresentMon.exe");
    let join = std::thread::spawn(move || run(shared, slot));
    WorkerHandle::new(
        join,
        Box::new(move || {
            if let Ok(mut guard) = stop_slot.lock() {
                if let Some(child) = guard.as_mut() {
                    kill_child(child);
                }
            }
            // Killing PresentMon does not let it clean up its ETW session.
            // `--terminate_existing_session` stops it without capturing.
            if stop_exe.is_file() {
                let _ = hidden_command(&stop_exe)
                    .args([
                        "--terminate_existing_session",
                        "--session_name",
                        SESSION_NAME,
                    ])
                    .output();
            }
        }),
    )
}

fn run(shared: SharedHandle, slot: Slot) {
    init_log();
    let exe = shared.assets.path("PresentMon.exe");
    let admin = crate::platform::windows::is_admin();
    log(&format!(
        "worker start: version={} admin={admin} perf_log_users={} presentmon={}",
        crate::VERSION,
        crate::platform::windows::in_performance_log_users(),
        exe.display()
    ));
    if !exe.is_file() {
        set_status(&shared, "PresentMon missing");
        log("PresentMon missing");
        return;
    }
    if !admin {
        set_status(
            &shared,
            "PresentMon needs administrator rights (restart as administrator)",
        );
        log("not elevated; not spawning PresentMon");
        return;
    }

    let samples: Samples = Arc::new(Mutex::new(VecDeque::new()));
    let own_pid = std::process::id();
    let mut target_pid: Option<u32> = None;
    let mut target_name: Option<String> = None;
    let mut candidate: Option<(u32, String, Instant)> = None;
    let mut target_since: Option<Instant> = None;
    let mut last_spawn: Option<Instant> = None;
    let mut logged_no_frames = false;

    while shared.is_running() {
        let foreground = foreground_pid();
        let name = foreground.and_then(process_name);

        // Wait for a new foreground window to stay put briefly. Explorer,
        // taskbar and anti-cheat helpers create noisy focus changes; switching
        // PresentMon on every one of them caused the old "FPS comes and goes"
        // behaviour.
        let desired = foreground.filter(|pid| *pid != own_pid);
        let own_focused = foreground == Some(own_pid);
        let mut stable = false;
        if own_focused {
            // Opening the settings window must not blank the FPS: keep the
            // last game/application target alive while the user changes
            // colours or fonts.
            candidate = None;
            if target_pid.is_none() {
                set_status(&shared, "waiting for an application");
            }
        } else if let Some(pid) = desired {
            let pname = name.clone().unwrap_or_default();
            match &candidate {
                Some((candidate_pid, candidate_name, since))
                    if *candidate_pid == pid && *candidate_name == pname =>
                {
                    stable = since.elapsed() >= Duration::from_millis(1500);
                }
                _ => candidate = Some((pid, pname, Instant::now())),
            }
        } else {
            candidate = None;
        }

        if stable {
            let pid = desired.unwrap();
            let pname = name.clone().unwrap_or_default();
            if target_pid != Some(pid) || target_name.as_deref() != Some(pname.as_str()) {
                if let Ok(mut guard) = slot.lock() {
                    if let Some(mut child) = guard.take() {
                        kill_child(&mut child);
                    }
                }
                target_pid = Some(pid);
                target_name = Some(pname.clone());
                target_since = Some(Instant::now());
                last_spawn = None;
                logged_no_frames = false;
                if let Ok(mut guard) = samples.lock() {
                    guard.clear();
                }
                shared.update_stats(|stats| {
                    stats.fps = None;
                    stats.fps_low = None;
                    stats.fps_low01 = None;
                    stats.game_pid = Some(pid);
                });
                log(&format!(
                    "target pid={pid} name={pname:?} filter={}",
                    if is_multiprocess_presenter(&pname) {
                        "process_name"
                    } else {
                        "process_id"
                    }
                ));
                set_status(&shared, &format!("capturing {pname}"));
            }
        } else if desired.is_none() && !own_focused {
            // Our settings window or the desktop: pause instead of measuring
            // the overlay itself.
            if target_pid.take().is_some() {
                if let Ok(mut guard) = slot.lock() {
                    if let Some(mut child) = guard.take() {
                        kill_child(&mut child);
                    }
                }
                target_name = None;
                target_since = None;
                last_spawn = None;
                if let Ok(mut guard) = samples.lock() {
                    guard.clear();
                }
                shared.update_stats(|stats| {
                    stats.fps = None;
                    stats.fps_low = None;
                    stats.fps_low01 = None;
                    stats.game_pid = None;
                });
                set_status(
                    &shared,
                    if foreground == Some(own_pid) {
                        "FPS paused (settings window focused)"
                    } else {
                        "waiting for an application"
                    },
                );
            }
        }

        let mut alive = child_alive(&slot);
        if let (Some(pid), false) = (target_pid, alive) {
            if last_spawn.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
                last_spawn = Some(Instant::now());
                let pname = target_name.clone().unwrap_or_default();
                match spawn_presentmon(
                    &exe,
                    &shared.assets.dir,
                    if pname.is_empty() { None } else { Some(&pname) },
                    pid,
                    SESSION_NAME,
                ) {
                    Ok((child, stdout, stderr)) => {
                        if let Ok(mut guard) = slot.lock() {
                            *guard = Some(child);
                        }
                        spawn_readers(shared.clone(), stdout, stderr, samples.clone());
                        alive = true;
                    }
                    Err(e) => {
                        set_status(&shared, &format!("PresentMon failed: {e}"));
                        log(&format!("PresentMon spawn failed: {e}"));
                    }
                }
            }
        }

        let (fps, low, low01) = if alive && target_pid.is_some() {
            compute(&samples)
        } else {
            (None, None, None)
        };
        if !alive && target_pid.is_some() {
            let status = shared.stats().fps_status;
            if status.starts_with("capturing") {
                set_status(&shared, "PresentMon stopped (see fps.log)");
            }
        }
        if fps.is_none() && alive && target_pid.is_some() {
            if target_since.is_some_and(|at| at.elapsed() > Duration::from_secs(3)) {
                let name = target_name.as_deref().unwrap_or("application");
                set_status(&shared, &format!("capturing {name} (no frames yet)"));
                if !logged_no_frames {
                    log("PresentMon is running but no frames have arrived for this target");
                    logged_no_frames = true;
                }
            }
        } else if fps.is_some() {
            logged_no_frames = false;
        }
        shared.update_stats(|stats| {
            stats.fps = fps;
            stats.fps_low = low;
            stats.fps_low01 = low01;
        });
        sleep_interruptible(&shared, Duration::from_secs(1));
    }

    if let Ok(mut guard) = slot.lock() {
        if let Some(child) = guard.as_mut() {
            kill_child(child);
        }
    }
}

fn is_multiprocess_presenter(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "chrome.exe"
            | "msedge.exe"
            | "firefox.exe"
            | "brave.exe"
            | "opera.exe"
            | "opera_gx.exe"
            | "vivaldi.exe"
            | "code.exe"
            | "discord.exe"
    )
}

fn spawn_presentmon(
    exe: &Path,
    assets_dir: &Path,
    process_name: Option<&str>,
    pid: u32,
    session: &str,
) -> std::io::Result<(Child, ChildStdout, std::process::ChildStderr)> {
    let mut command = hidden_command(exe);
    command.args([
        "--output_stdout",
        "--no_console_stats",
        // Anti-cheat and some drivers reject the DxgKrnl/Win32k providers
        // with ETW error 1450. We only need present events, so skip the GPU
        // duration, display-stage and input providers entirely.
        "--no_track_gpu",
        "--no_track_display",
        "--no_track_input",
        "--stop_existing_session",
        "--terminate_on_proc_exit",
    ]);
    // Games present from the foreground PID. Multi-process apps (Chrome,
    // Electron, VS Code, ...) present from a GPU child with the same exe name,
    // so those must be filtered by name instead; adding --process_id there
    // would make PresentMon's consumer drop the child's events.
    match process_name {
        Some(name) if is_multiprocess_presenter(name) => {
            command.args(["--process_name", name]);
        }
        _ => {
            command.args(["--process_id", &pid.to_string()]);
        }
    }
    command
        .args(["--session_name", session])
        .current_dir(assets_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("PresentMon stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("PresentMon stderr unavailable"))?;
    Ok((child, stdout, stderr))
}

fn spawn_readers(
    shared: SharedHandle,
    stdout: ChildStdout,
    stderr: std::process::ChildStderr,
    samples: Samples,
) {
    let frame_shared = shared.clone();
    std::thread::spawn(move || read_frames(frame_shared, stdout, samples));
    std::thread::spawn(move || read_errors(shared, stderr));
}

fn read_frames(shared: SharedHandle, stdout: ChildStdout, samples: Samples) {
    let mut reader = BufReader::new(stdout);
    let mut frame_column: Option<usize> = None;
    let mut logged_first_frame = false;
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if !shared.is_running() {
            break;
        }
        let trimmed = line
            .trim_start_matches('\u{feff}')
            .trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            continue;
        }
        let first = csv_field(trimmed, 0).unwrap_or("").trim();
        if first.eq_ignore_ascii_case("Application") {
            frame_column = frame_time_column(trimmed);
            let header: String = trimmed.chars().take(300).collect();
            log(&format!(
                "CSV header (frame column {frame_column:?}): {header}"
            ));
            if frame_column.is_none() && shared.is_running() {
                set_status(&shared, "PresentMon: frame-time column not found");
                log("no known frame-time column in the header");
            }
            continue;
        }

        let frame_time = frame_column
            .and_then(|column| csv_field(trimmed, column))
            .and_then(|field| field.parse::<f64>().ok());
        if !logged_first_frame && frame_time.is_some() {
            log(&format!(
                "first frame row: {}",
                trimmed.chars().take(200).collect::<String>()
            ));
            logged_first_frame = true;
        }

        if let Ok(mut buffer) = samples.lock() {
            buffer.push_back((Instant::now(), frame_time));
            if buffer.len() > MAX_SAMPLES {
                buffer.pop_front();
            }
        }
    }
}

fn read_errors(shared: SharedHandle, stderr: std::process::ChildStderr) {
    let mut text = String::new();
    let mut reader = BufReader::new(stderr);
    if reader.read_to_string(&mut text).is_err() {
        return;
    }
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        log(&format!(
            "PresentMon stderr: {}",
            trimmed.chars().take(500).collect::<String>()
        ));
    }
    let last = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if !last.is_empty() && shared.is_running() {
        let truncated: String = last.chars().take(120).collect();
        if last.contains("1450") {
            set_status(
                &shared,
                "FPS blocked: ETW 1450 (anti-cheat/reboot; see fps.log)",
            );
        } else {
            set_status(&shared, &format!("PresentMon: {truncated}"));
        }
    }
}

fn compute(samples: &Samples) -> (Option<f64>, Option<f64>, Option<f64>) {
    let now = Instant::now();
    let Ok(mut buffer) = samples.lock() else {
        return (None, None, None);
    };
    while buffer
        .front()
        .is_some_and(|(timestamp, _)| now.duration_since(*timestamp) > LOW_WINDOW)
    {
        buffer.pop_front();
    }

    let recent = buffer
        .iter()
        .filter(|(timestamp, _)| now.duration_since(*timestamp) <= AVG_WINDOW)
        .count();
    if recent == 0 {
        // Nothing rendered lately (menu/alt-tab): blank the lows too, otherwise
        // a stale 1% low sits next to an empty FPS and reads as broken.
        return (None, None, None);
    }

    let mut frame_times: Vec<f64> = buffer
        .iter()
        .filter_map(|(_, frame_time)| *frame_time)
        .filter(|frame_time| *frame_time > 0.0)
        .collect();
    frame_times.sort_unstable_by(f64::total_cmp);

    let fps = Some(recent as f64 / AVG_WINDOW.as_secs_f64());
    let mut fps_low = None;
    let mut fps_low01 = None;
    if frame_times.len() >= MIN_LOW_FRAMES {
        fps_low = percentile(&frame_times, 99.0).map(|p99| 1000.0 / p99);
    }
    if frame_times.len() >= MIN_LOW01_FRAMES {
        fps_low01 = percentile(&frame_times, 99.9).map(|p999| 1000.0 / p999);
    }
    (fps, fps_low, fps_low01)
}

/// Nearest-rank percentile of an ascending-sorted slice.
fn percentile(sorted: &[f64], percent: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (percent / 100.0 * sorted.len() as f64).ceil() as isize - 1;
    let index = rank.clamp(0, sorted.len() as isize - 1) as usize;
    sorted.get(index).copied()
}

fn child_alive(slot: &Slot) -> bool {
    let Ok(mut guard) = slot.lock() else {
        return false;
    };
    let Some(child) = guard.as_mut() else {
        return false;
    };
    match child.try_wait() {
        Ok(Some(_)) => {
            guard.take();
            false
        }
        Ok(None) => true,
        Err(_) => false,
    }
}

fn set_status(shared: &SharedHandle, text: &str) {
    let text = text.to_string();
    shared.update_stats(|stats| stats.fps_status = text);
}

/// PresentMon's CSV moves a few columns between metric sets. Prefer the
/// classic `MsBetweenPresents`, then the 2.x names.
fn frame_time_column(header: &str) -> Option<usize> {
    [
        "MsBetweenPresents",
        "FrameTime",
        "MsBetweenDisplayChange",
        "DisplayedTime",
    ]
    .into_iter()
    .find_map(|name| header_column(header, name))
}

/// Index of `name` in a PresentMon CSV header, respecting quoted fields.
fn header_column(header: &str, name: &str) -> Option<usize> {
    let mut index = 0;
    let mut start = 0;
    let mut quoted = false;
    let bytes = header.as_bytes();
    let mut i = 0;
    while i <= bytes.len() {
        let at_end = i == bytes.len();
        let byte = if at_end { b',' } else { bytes[i] };
        match byte {
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                let field = header[start..i].trim().trim_matches('"');
                if field == name {
                    return Some(index);
                }
                index += 1;
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Nth CSV field of `line`, quote-aware.
fn csv_field(line: &str, index: usize) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut field = 0;
    let mut start = 0;
    let mut quoted = false;
    let mut i = 0;
    while i <= bytes.len() {
        let at_end = i == bytes.len();
        let byte = if at_end { b',' } else { bytes[i] };
        match byte {
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                if field == index {
                    return Some(line[start..i].trim().trim_matches('"'));
                }
                field += 1;
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&values, 99.0), Some(5.0));
        assert_eq!(percentile(&values, 50.0), Some(3.0));
        assert_eq!(percentile(&[], 99.0), None);
    }

    #[test]
    fn finds_frame_time_column() {
        let header = "Application,ProcessID,SwapChainAddress,Runtime,SyncInterval,PresentFlags,AllowsTearing,PresentMode,MsBetweenPresents,MsInPresentAPI";
        assert_eq!(frame_time_column(header), Some(8));
        assert_eq!(header_column(header, "ProcessID"), Some(1));
        assert_eq!(header_column(header, "Nope"), None);

        let v2 = "Application,ProcessID,FrameTime,CPUBusy,GPUTime";
        assert_eq!(frame_time_column(v2), Some(2));

        let quoted = "\"Application\",\"ProcessID\",\"MsBetweenPresents\"";
        assert_eq!(csv_field(quoted, 0), Some("Application"));
        assert_eq!(frame_time_column(quoted), Some(2));
    }

    #[test]
    fn csv_parser_handles_quoted_commas() {
        let line = r#""Game, The",1234,5.5,7.25"#;
        assert_eq!(csv_field(line, 0), Some("Game, The"));
        assert_eq!(csv_field(line, 1), Some("1234"));
        assert_eq!(csv_field(line, 3), Some("7.25"));
        assert_eq!(csv_field(line, 9), None);
    }

    #[test]
    fn frame_times_produce_lows() {
        let samples: Samples = Arc::new(Mutex::new(VecDeque::new()));
        let now = Instant::now();
        {
            let mut buffer = samples.lock().unwrap();
            for i in 0..1000 {
                buffer.push_back((now, Some(10.0 + (i % 10) as f64)));
            }
            buffer.push_back((now, Some(100.0))); // one terrible frame
        }
        let (fps, low, _) = compute(&samples);
        assert!(fps.unwrap() > 490.0);
        assert!(low.unwrap() > 0.0);
    }
}
