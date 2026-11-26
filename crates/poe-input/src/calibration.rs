use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const TARGET_ITEM: &str = "item";
pub const TARGET_PANEL: &str = "panel";
pub const TARGET_TRANSMUTE: &str = "transmute";
pub const TARGET_ALT: &str = "alt";
pub const TARGET_AUG: &str = "aug";
pub const TARGET_REGAL: &str = "regal";
pub const TARGET_ALCH: &str = "alch";
pub const TARGET_SCOUR: &str = "scour";
pub const TARGET_CHAOS: &str = "chaos";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormPoint {
    pub x: f32,
    pub y: f32,
}

impl NormPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn is_valid(self) -> bool {
        (0.0..=1.0).contains(&self.x) && (0.0..=1.0).contains(&self.y)
    }

    pub fn to_screen(self, origin_x: i32, origin_y: i32, width: i32, height: i32) -> (i32, i32) {
        let x = origin_x + (self.x * width.max(1) as f32).round() as i32;
        let y = origin_y + (self.y * height.max(1) as f32).round() as i32;
        (x, y)
    }

    pub fn from_screen(
        screen_x: i32,
        screen_y: i32,
        origin_x: i32,
        origin_y: i32,
        width: i32,
        height: i32,
    ) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let x = (screen_x - origin_x) as f32 / width as f32;
        let y = (screen_y - origin_y) as f32 / height as f32;
        let point = Self::new(x, y);
        point.is_valid().then_some(point)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Largest per-channel difference between two colors.
    pub fn distance(self, other: Self) -> u8 {
        self.r
            .abs_diff(other.r)
            .max(self.g.abs_diff(other.g))
            .max(self.b.abs_diff(other.b))
    }

    pub fn from_bgr(value: u32) -> Self {
        Self {
            r: (value & 0xff) as u8,
            g: ((value >> 8) & 0xff) as u8,
            b: ((value >> 16) & 0xff) as u8,
        }
    }
}

pub const DEFAULT_PANEL_TOLERANCE: u8 = 16;

fn default_panel_tolerance() -> u8 {
    DEFAULT_PANEL_TOLERANCE
}

/// Optional pixel probe that must match before any client action. Calibrate it
/// on a stable part of the open stash/inventory panel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelGuard {
    pub point: NormPoint,
    pub color: Color,
    #[serde(default = "default_panel_tolerance")]
    pub tolerance: u8,
}

impl PanelGuard {
    pub fn new(point: NormPoint, color: Color) -> Self {
        Self {
            point,
            color,
            tolerance: DEFAULT_PANEL_TOLERANCE,
        }
    }

    pub fn matches(self, actual: Color) -> bool {
        self.color.distance(actual) <= self.tolerance
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub schema: u32,
    #[serde(default = "default_game_title")]
    pub game_title: String,
    #[serde(default)]
    pub captured_width: Option<i32>,
    #[serde(default)]
    pub captured_height: Option<i32>,
    #[serde(default)]
    pub item: Option<NormPoint>,
    #[serde(default)]
    pub currencies: BTreeMap<String, NormPoint>,
    #[serde(default)]
    pub panel: Option<PanelGuard>,
}

#[derive(Debug, Error)]
pub enum CalibrationError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("toml encode: {0}")]
    TomlEncode(#[from] toml::ser::Error),
    #[error("invalid calibration: {0}")]
    Invalid(String),
}

fn default_game_title() -> String {
    "Path of Exile".to_string()
}

impl Calibration {
    pub fn empty() -> Self {
        Self {
            schema: 1,
            game_title: default_game_title(),
            captured_width: None,
            captured_height: None,
            item: None,
            currencies: BTreeMap::new(),
            panel: None,
        }
    }

    pub fn new(item: NormPoint) -> Self {
        let mut calibration = Self::empty();
        calibration.item = Some(item);
        calibration
    }

    pub fn all_targets() -> &'static [&'static str] {
        &[
            TARGET_ITEM,
            TARGET_TRANSMUTE,
            TARGET_ALT,
            TARGET_AUG,
            TARGET_REGAL,
            TARGET_ALCH,
            TARGET_SCOUR,
            TARGET_CHAOS,
        ]
    }

    /// Targets that can be captured, including optional guards.
    pub fn capturable_targets() -> &'static [&'static str] {
        &[
            TARGET_ITEM,
            TARGET_PANEL,
            TARGET_TRANSMUTE,
            TARGET_ALT,
            TARGET_AUG,
            TARGET_REGAL,
            TARGET_ALCH,
            TARGET_SCOUR,
            TARGET_CHAOS,
        ]
    }

    pub fn load(path: &Path) -> Result<Self, CalibrationError> {
        let text = std::fs::read_to_string(path)?;
        let calibration: Calibration = toml::from_str(&text)?;
        calibration.validate()?;
        Ok(calibration)
    }

    pub fn save(&self, path: &Path) -> Result<(), CalibrationError> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), CalibrationError> {
        if self.schema != 1 {
            return Err(CalibrationError::Invalid(format!(
                "unsupported schema {}",
                self.schema
            )));
        }
        if self.item.is_some_and(|item| !item.is_valid()) {
            return Err(CalibrationError::Invalid(
                "item point is outside 0..=1".to_string(),
            ));
        }
        for (target, point) in &self.currencies {
            if !point.is_valid() {
                return Err(CalibrationError::Invalid(format!(
                    "target `{target}` is outside 0..=1"
                )));
            }
        }
        if let Some(panel) = self.panel {
            if !panel.point.is_valid() {
                return Err(CalibrationError::Invalid(
                    "panel point is outside 0..=1".to_string(),
                ));
            }
            if panel.tolerance > 64 {
                return Err(CalibrationError::Invalid(
                    "panel tolerance must be <= 64".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn set_target(&mut self, target: &str, point: NormPoint) -> Result<(), CalibrationError> {
        if !point.is_valid() {
            return Err(CalibrationError::Invalid(format!(
                "target `{target}` is outside 0..=1"
            )));
        }
        if target == TARGET_ITEM {
            self.item = Some(point);
            return Ok(());
        }
        if target == TARGET_PANEL {
            return Err(CalibrationError::Invalid(
                "panel target needs a sampled color; use set_panel".to_string(),
            ));
        }
        if !Self::currency_targets().contains(&target) {
            return Err(CalibrationError::Invalid(format!(
                "unknown target `{target}`"
            )));
        }
        self.currencies.insert(target.to_string(), point);
        Ok(())
    }

    pub fn set_panel(&mut self, point: NormPoint, color: Color) -> Result<(), CalibrationError> {
        if !point.is_valid() {
            return Err(CalibrationError::Invalid(
                "panel point is outside 0..=1".to_string(),
            ));
        }
        self.panel = Some(PanelGuard::new(point, color));
        Ok(())
    }

    pub fn point(&self, target: &str) -> Option<NormPoint> {
        if target == TARGET_ITEM {
            return self.item;
        }
        if target == TARGET_PANEL {
            return self.panel.map(|panel| panel.point);
        }
        self.currencies.get(target).copied()
    }

    pub fn missing(&self, required: &[&str]) -> Vec<String> {
        required
            .iter()
            .filter(|target| self.point(target).is_none())
            .map(|target| (*target).to_string())
            .collect()
    }

    pub fn set_capture_size(&mut self, width: i32, height: i32) {
        self.captured_width = Some(width);
        self.captured_height = Some(height);
    }

    pub fn currency_targets() -> &'static [&'static str] {
        &[
            TARGET_TRANSMUTE,
            TARGET_ALT,
            TARGET_AUG,
            TARGET_REGAL,
            TARGET_ALCH,
            TARGET_SCOUR,
            TARGET_CHAOS,
        ]
    }

    pub fn default_path() -> PathBuf {
        PathBuf::from("calibration.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_is_local() {
        assert_eq!(
            Calibration::default_path(),
            PathBuf::from("calibration.toml")
        );
    }

    #[test]
    fn round_trip_screen_coordinates() {
        let point = NormPoint::new(0.25, 0.75);
        let (x, y) = point.to_screen(100, 50, 1600, 900);
        assert_eq!((x, y), (500, 725));
        assert_eq!(
            NormPoint::from_screen(x, y, 100, 50, 1600, 900),
            Some(point)
        );
    }

    #[test]
    fn rejects_out_of_bounds_targets() {
        let mut calibration = Calibration::new(NormPoint::new(0.5, 0.5));
        assert!(calibration
            .set_target("alt", NormPoint::new(1.2, 0.5))
            .is_err());
        assert!(calibration
            .set_target("unknown", NormPoint::new(0.5, 0.5))
            .is_err());
    }

    #[test]
    fn saves_and_loads() {
        let dir = std::env::temp_dir().join("poe-crafter-calibration-test");
        let path = dir.join("calibration.toml");
        let mut calibration = Calibration::new(NormPoint::new(0.5, 0.5));
        calibration
            .set_target("alt", NormPoint::new(0.1, 0.2))
            .unwrap();
        calibration.set_capture_size(1920, 1080);
        calibration.save(&path).unwrap();
        let loaded = Calibration::load(&path).unwrap();
        assert_eq!(loaded.point("alt"), Some(NormPoint::new(0.1, 0.2)));
        assert_eq!(loaded.captured_width, Some(1920));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn panel_guard_matches_within_tolerance() {
        let guard = PanelGuard::new(NormPoint::new(0.5, 0.5), Color::new(100, 120, 140));
        assert!(guard.matches(Color::new(110, 120, 130)));
        assert!(!guard.matches(Color::new(140, 120, 140)));
    }

    #[test]
    fn saves_and_loads_optional_panel_guard() {
        let dir = std::env::temp_dir().join("poe-crafter-panel-calibration-test");
        let path = dir.join("calibration.toml");
        let mut calibration = Calibration::new(NormPoint::new(0.5, 0.5));
        calibration
            .set_panel(NormPoint::new(0.2, 0.3), Color::new(10, 20, 30))
            .unwrap();
        calibration.save(&path).unwrap();
        let loaded = Calibration::load(&path).unwrap();
        let panel = loaded.panel.expect("panel guard");
        assert_eq!(panel.point, NormPoint::new(0.2, 0.3));
        assert_eq!(panel.color, Color::new(10, 20, 30));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_calibration_without_panel_still_loads() {
        let dir = std::env::temp_dir().join("poe-crafter-old-calibration-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("calibration.toml");
        std::fs::write(
            &path,
            r#"schema = 1
game_title = "Path of Exile"

[item]
x = 0.5
y = 0.5

[currencies.alt]
x = 0.1
y = 0.2
"#,
        )
        .unwrap();
        let loaded = Calibration::load(&path).unwrap();
        assert!(loaded.panel.is_none());
        assert_eq!(loaded.point("alt"), Some(NormPoint::new(0.1, 0.2)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reports_missing_targets() {
        let calibration = Calibration::new(NormPoint::new(0.5, 0.5));
        assert_eq!(
            calibration.missing(&["item", "alt", "chaos"]),
            vec!["alt".to_string(), "chaos".to_string()]
        );
    }
}
