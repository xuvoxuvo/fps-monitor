//! Configuration, persisted to `%APPDATA%\fps-monitor\config.json`.
//!
//! Missing or invalid values are repaired on load, and unknown keys are
//! ignored so a config from an older build keeps working.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub const APP_NAME: &str = "fps-monitor";

pub const POSITIONS: [&str; 9] = [
    "Top Left",
    "Top Centre",
    "Top Right",
    "Centre Left",
    "Centre",
    "Centre Right",
    "Bottom Left",
    "Bottom Centre",
    "Bottom Right",
];

/// One renderable metric. `ALL` is also the on-screen order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Fps,
    FpsLow,
    FpsLow01,
    Cpu,
    Gpu,
    GpuTemp,
}

impl Metric {
    pub const ALL: [Metric; 6] = [
        Metric::Fps,
        Metric::FpsLow,
        Metric::FpsLow01,
        Metric::Cpu,
        Metric::Gpu,
        Metric::GpuTemp,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Metric::Fps => "fps",
            Metric::FpsLow => "fps_low",
            Metric::FpsLow01 => "fps_low01",
            Metric::Cpu => "cpu",
            Metric::Gpu => "gpu",
            Metric::GpuTemp => "gpu_temp",
        }
    }

    /// Human-readable name used in the settings window.
    pub fn title(self) -> &'static str {
        match self {
            Metric::Fps => "FPS",
            Metric::FpsLow => "1% low FPS",
            Metric::FpsLow01 => "0.1% low FPS",
            Metric::Cpu => "CPU usage",
            Metric::Gpu => "GPU usage",
            Metric::GpuTemp => "GPU temperature",
        }
    }

    pub fn from_id(id: &str) -> Option<Metric> {
        Metric::ALL.into_iter().find(|m| m.id() == id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricConfig {
    pub enabled: bool,
    pub label: String,
    pub color: String,
}

impl Default for MetricConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            label: String::new(),
            color: "#ffffff".into(),
        }
    }
}

impl MetricConfig {
    fn for_metric(metric: Metric) -> Self {
        let (enabled, label, color) = match metric {
            Metric::Fps => (true, "FPS", "#00ff88"),
            Metric::FpsLow => (true, "1% LOW", "#00e0a0"),
            Metric::FpsLow01 => (false, "0.1% LOW", "#7fffd4"),
            Metric::Cpu => (true, "CPU", "#ffcc00"),
            Metric::Gpu => (true, "GPU", "#cc66ff"),
            Metric::GpuTemp => (true, "GPU°", "#ff6699"),
        };
        Self {
            enabled,
            label: label.into(),
            color: color.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub position: String,
    pub margin: i32,
    pub font_family: String,
    pub font_size: i32,
    pub opacity: i32,
    pub show_background: bool,
    pub bg_color: String,
    pub metrics: BTreeMap<String, MetricConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let mut metrics = BTreeMap::new();
        for metric in Metric::ALL {
            metrics.insert(metric.id().to_string(), MetricConfig::for_metric(metric));
        }
        Self {
            position: "Top Right".into(),
            margin: 24,
            font_family: "Consolas".into(),
            font_size: 14,
            opacity: 92,
            show_background: true,
            bg_color: "#101014".into(),
            metrics,
        }
    }
}

impl Config {
    pub fn config_dir() -> PathBuf {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        base.join(APP_NAME)
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    /// Older builds used `%APPDATA%\GameOverlay`; read it once if the new
    /// location does not exist yet, so upgrading does not lose settings.
    fn legacy_config_path() -> Option<PathBuf> {
        Self::config_dir()
            .parent()
            .map(|base| base.join("GameOverlay").join("config.json"))
    }

    /// Load, filling in anything missing or invalid from the defaults.
    pub fn load() -> Self {
        let mut cfg = [Some(Self::config_path()), Self::legacy_config_path()]
            .into_iter()
            .flatten()
            .find_map(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Config>(&text).ok())
            .unwrap_or_default();
        cfg.normalize();
        cfg
    }

    pub fn save(&self) {
        let dir = Self::config_dir();
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join("config.json");
        let Ok(json) = serde_json::to_string_pretty(self) else {
            return;
        };
        let tmp = dir.join("config.json.tmp");
        if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&path);
            let _ = fs::rename(&tmp, &path);
        }
    }

    /// Force every value into a state the renderer and workers understand.
    pub fn normalize(&mut self) {
        if !POSITIONS.contains(&self.position.as_str()) {
            self.position = Config::default().position;
        }
        self.margin = self.margin.clamp(0, 300);
        if self.font_family.trim().is_empty() {
            self.font_family = Config::default().font_family;
        }
        self.font_size = self.font_size.clamp(6, 72);
        self.opacity = self.opacity.clamp(20, 100);
        if parse_color(&self.bg_color).is_none() {
            self.bg_color = Config::default().bg_color;
        }

        for metric in Metric::ALL {
            let default = MetricConfig::for_metric(metric);
            let entry = self
                .metrics
                .entry(metric.id().to_string())
                .or_insert_with(|| default.clone());
            if entry.label.trim().is_empty() {
                entry.label = default.label;
            }
            if parse_color(&entry.color).is_none() {
                entry.color = default.color;
            }
        }
        self.metrics.retain(|k, _| Metric::from_id(k).is_some());
    }

    pub fn metric(&self, metric: Metric) -> MetricConfig {
        self.metrics
            .get(metric.id())
            .cloned()
            .unwrap_or_else(|| MetricConfig::for_metric(metric))
    }

    pub fn enabled(&self, metric: Metric) -> bool {
        self.metric(metric).enabled
    }

    pub fn set_enabled(&mut self, metric: Metric, enabled: bool) {
        self.metrics
            .entry(metric.id().to_string())
            .or_insert_with(|| MetricConfig::for_metric(metric))
            .enabled = enabled;
    }

    pub fn color_argb(&self, metric: Metric) -> u32 {
        parse_color(&self.metric(metric).color).unwrap_or(0xffff_ffff)
    }

    pub fn set_color(&mut self, metric: Metric, color: String) {
        self.metrics
            .entry(metric.id().to_string())
            .or_insert_with(|| MetricConfig::for_metric(metric))
            .color = color;
    }

    pub fn bg_argb(&self) -> u32 {
        parse_color(&self.bg_color).unwrap_or(0xff10_1014)
    }

    pub fn enabled_metrics(&self) -> Vec<Metric> {
        Metric::ALL
            .into_iter()
            .filter(|m| self.enabled(*m))
            .collect()
    }
}

/// Parse `#rrggbb`, `#rgb` or the same without `#` into `0xAARRGGBB`.
pub fn parse_color(text: &str) -> Option<u32> {
    let hex = text.trim().trim_start_matches('#');
    let expanded;
    let hex = if hex.len() == 3 {
        expanded = hex.chars().flat_map(|c| [c, c]).collect::<String>();
        expanded.as_str()
    } else {
        hex
    };
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(0xff00_0000 | value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_round_trip() {
        assert_eq!(parse_color("#00ff88"), Some(0xff00_ff88));
        assert_eq!(parse_color("00FF88"), Some(0xff00_ff88));
        assert_eq!(parse_color("#0f8"), Some(0xff00_ff88));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn defaults_survive_partial_json() {
        let mut cfg: Config = serde_json::from_str(
            r#"{
                "position": "Centre",
                "font_size": 200,
                "opacity": -5,
                "metrics": { "fps": { "enabled": false } }
            }"#,
        )
        .unwrap();
        cfg.normalize();
        assert_eq!(cfg.position, "Centre");
        assert_eq!(cfg.font_size, 72);
        assert_eq!(cfg.opacity, 20);
        assert!(!cfg.enabled(Metric::Fps));
        assert_eq!(cfg.metric(Metric::Fps).label, "FPS");
        assert!(cfg.enabled(Metric::Gpu));
        assert_eq!(cfg.metrics.len(), 6);
    }

    #[test]
    fn invalid_values_are_repaired() {
        let mut cfg = Config {
            position: "Nowhere".into(),
            bg_color: "not-a-colour".into(),
            metrics: BTreeMap::new(),
            ..Config::default()
        };
        cfg.normalize();
        assert_eq!(cfg.position, "Top Right");
        assert_eq!(cfg.bg_color, "#101014");
        assert_eq!(cfg.metrics.len(), 6);
    }
}
