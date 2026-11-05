//! Metric values shared between the worker threads and the UI.
//!
//! A single mutex is plenty: the workers write a handful of fields once a
//! second and the overlay reads them once a second.

use crate::app::config::{Config, Metric};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Clone, Debug)]
pub struct Stats {
    pub fps: Option<f64>,
    pub fps_low: Option<f64>,
    pub fps_low01: Option<f64>,
    pub cpu: Option<f64>,
    pub gpu: Option<f64>,
    pub gpu_temp: Option<f64>,
    pub hw_status: String,
    pub fps_status: String,
    /// PID of the foreground process PresentMon is attached to.
    pub game_pid: Option<u32>,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            fps: None,
            fps_low: None,
            fps_low01: None,
            cpu: None,
            gpu: None,
            gpu_temp: None,
            hw_status: "starting".into(),
            fps_status: "starting".into(),
            game_pid: None,
        }
    }
}

impl Stats {
    pub fn value(&self, metric: Metric) -> Option<f64> {
        match metric {
            Metric::Fps => self.fps,
            Metric::FpsLow => self.fps_low,
            Metric::FpsLow01 => self.fps_low01,
            Metric::Cpu => self.cpu,
            Metric::Gpu => self.gpu,
            Metric::GpuTemp => self.gpu_temp,
        }
    }

    pub fn to_json(&self) -> Value {
        let round = |v: Option<f64>| v.map(|v| (v * 10.0).round() / 10.0);
        json!({
            "fps": round(self.fps),
            "fps_low": round(self.fps_low),
            "fps_low01": round(self.fps_low01),
            "cpu": round(self.cpu),
            "gpu": round(self.gpu),
            "gpu_temp": round(self.gpu_temp),
            "hw_status": self.hw_status,
            "fps_status": self.fps_status,
            "game_pid": self.game_pid,
        })
    }
}

/// State every thread needs: the latest numbers, the live config, the
/// shutdown flag and where the helper binaries live.
pub struct Shared {
    stats: Mutex<Stats>,
    pub config: RwLock<Config>,
    pub running: std::sync::atomic::AtomicBool,
    pub assets: crate::app::assets::Assets,
}

impl Shared {
    pub fn new(config: Config, assets: crate::app::assets::Assets) -> Self {
        Self {
            stats: Mutex::new(Stats::default()),
            config: RwLock::new(config),
            running: std::sync::atomic::AtomicBool::new(true),
            assets,
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Update a few fields without holding the lock longer than necessary.
    pub fn update_stats(&self, f: impl FnOnce(&mut Stats)) {
        if let Ok(mut stats) = self.stats.lock() {
            f(&mut stats);
        }
    }

    pub fn config(&self) -> Config {
        self.config.read().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_config(&self, config: Config) {
        if let Ok(mut slot) = self.config.write() {
            *slot = config;
        }
    }
}

pub type SharedHandle = Arc<Shared>;

/// Render one metric for the overlay (and for the self test).
pub fn format_value(metric: Metric, stats: &Stats) -> String {
    let Some(value) = stats.value(metric) else {
        return "--".to_string();
    };
    match metric {
        Metric::Fps | Metric::FpsLow | Metric::FpsLow01 => format!("{}", value.round() as i64),
        Metric::Cpu | Metric::Gpu => format!("{} %", value.round() as i64),
        Metric::GpuTemp => format!("{}°C", value.round() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_metrics() {
        let mut stats = Stats::default();
        assert_eq!(format_value(Metric::Fps, &stats), "--");
        stats.fps = Some(143.6);
        assert_eq!(format_value(Metric::Fps, &stats), "144");
        stats.cpu = Some(23.2);
        assert_eq!(format_value(Metric::Cpu, &stats), "23 %");
        stats.gpu_temp = Some(61.5);
        assert_eq!(format_value(Metric::GpuTemp, &stats), "62°C");
    }
}
