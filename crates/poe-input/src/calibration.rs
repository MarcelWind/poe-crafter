use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SCHEMA: u32 = 2;
pub const LEGACY_SCHEMA: u32 = 1;

pub const TARGET_ITEM: &str = "item";
pub const TARGET_PANEL: &str = "panel";
pub const TARGET_TRANSMUTE: &str = "transmute";
pub const TARGET_ALT: &str = "alt";
pub const TARGET_AUG: &str = "aug";
pub const TARGET_REGAL: &str = "regal";
pub const TARGET_ALCH: &str = "alch";
pub const TARGET_SCOUR: &str = "scour";
pub const TARGET_CHAOS: &str = "chaos";

pub const DEFAULT_GUARD_THRESHOLD: f32 = 0.90;
pub const DEFAULT_GUARD_MARGIN: f32 = 0.05;
pub const DEFAULT_GUARD_SEARCH_PX: u32 = 4;

fn default_guard_threshold() -> f32 {
    DEFAULT_GUARD_THRESHOLD
}

fn default_guard_margin() -> f32 {
    DEFAULT_GUARD_MARGIN
}

fn default_guard_search_px() -> u32 {
    DEFAULT_GUARD_SEARCH_PX
}

fn default_guard_scales() -> Vec<f32> {
    vec![0.98, 1.0, 1.02]
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Normalized rectangle in client space. `x`/`y` are the top-left corner and
/// `w`/`h` are the size, all in `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NormRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_valid(self) -> bool {
        (0.0..=1.0).contains(&self.x)
            && (0.0..=1.0).contains(&self.y)
            && self.w > 0.0
            && self.h > 0.0
            && self.x + self.w <= 1.0 + f32::EPSILON
            && self.y + self.h <= 1.0 + f32::EPSILON
    }

    pub fn center(self) -> NormPoint {
        NormPoint::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(self, point: NormPoint) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.w
            && point.y >= self.y
            && point.y <= self.y + self.h
    }

    pub fn to_pixels(
        self,
        origin_x: i32,
        origin_y: i32,
        client_width: i32,
        client_height: i32,
    ) -> (i32, i32, i32, i32) {
        let width = client_width.max(1) as f32;
        let height = client_height.max(1) as f32;
        let x = origin_x + (self.x * width).round() as i32;
        let y = origin_y + (self.y * height).round() as i32;
        let w = (self.w * width).round().max(1.0) as i32;
        let h = (self.h * height).round().max(1.0) as i32;
        let w = w.min(client_width - (x - origin_x)).max(1);
        let h = h.min(client_height - (y - origin_y)).max(1);
        (x, y, w, h)
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

/// Legacy single-pixel probe. Kept for existing calibrations; the image guard
/// in [`ImageGuard`] takes precedence when present.
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

/// Template-based guard that proves the stash or inventory panel is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageGuard {
    pub region: NormRect,
    pub reference: PathBuf,
    #[serde(default = "default_guard_threshold")]
    pub threshold: f32,
    #[serde(default = "default_guard_margin")]
    pub margin: f32,
    #[serde(default = "default_guard_search_px")]
    pub search_px: u32,
    #[serde(default = "default_guard_scales")]
    pub scales: Vec<f32>,
}

impl ImageGuard {
    pub fn new(region: NormRect, reference: PathBuf) -> Self {
        Self {
            region,
            reference,
            threshold: DEFAULT_GUARD_THRESHOLD,
            margin: DEFAULT_GUARD_MARGIN,
            search_px: DEFAULT_GUARD_SEARCH_PX,
            scales: default_guard_scales(),
        }
    }

    pub fn params(&self) -> crate::vision::MatchParams {
        crate::vision::MatchParams {
            threshold: self.threshold,
            margin: self.margin,
            search_px: self.search_px,
            scales: self.scales.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StripConfig {
    pub region: NormRect,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strips {
    #[serde(default)]
    pub horizontal: Option<StripConfig>,
    #[serde(default)]
    pub vertical: Option<StripConfig>,
}

impl Strips {
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &StripConfig)> {
        [
            ("horizontal", self.horizontal.as_ref()),
            ("vertical", self.vertical.as_ref()),
        ]
        .into_iter()
        .filter_map(|(name, strip)| strip.map(|strip| (name, strip)))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridConfig {
    pub region: NormRect,
    pub columns: u32,
    pub rows: u32,
}

impl GridConfig {
    pub fn cell_center(&self, column: u32, row: u32) -> Option<NormPoint> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        let x = self.region.x + self.region.w * (column as f32 + 0.5) / self.columns as f32;
        let y = self.region.y + self.region.h * (row as f32 + 0.5) / self.rows as f32;
        Some(NormPoint::new(x, y))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabConfig {
    pub index: u32,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    pub selected: PathBuf,
    #[serde(default)]
    pub unselected: Option<PathBuf>,
    pub content: GridConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemSource {
    Inventory { x: f32, y: f32 },
    Tab { tab: u32, column: u32, row: u32 },
}

impl ItemSource {
    pub fn inventory(point: NormPoint) -> Self {
        Self::Inventory {
            x: point.x,
            y: point.y,
        }
    }

    pub fn point(&self) -> Option<NormPoint> {
        match self {
            Self::Inventory { x, y } => Some(NormPoint::new(*x, *y)),
            Self::Tab { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemConfig {
    pub display_name: String,
    pub source: ItemSource,
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
    #[serde(default)]
    pub guard: Option<ImageGuard>,
    #[serde(default)]
    pub strips: Strips,
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
    #[serde(default)]
    pub items: BTreeMap<String, ItemConfig>,
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
            schema: SCHEMA,
            game_title: default_game_title(),
            captured_width: None,
            captured_height: None,
            item: None,
            currencies: BTreeMap::new(),
            panel: None,
            guard: None,
            strips: Strips::default(),
            tabs: Vec::new(),
            items: BTreeMap::new(),
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
        let mut calibration: Calibration = toml::from_str(&text)?;
        calibration.migrate()?;
        calibration.validate()?;
        Ok(calibration)
    }

    pub fn save(&self, path: &Path) -> Result<(), CalibrationError> {
        let mut encoded = self.clone();
        encoded.schema = SCHEMA;
        encoded.migrate()?;
        encoded.validate()?;
        encoded.validate_template_files(path)?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let text = toml::to_string_pretty(&encoded)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Bring legacy schema-1 fields into the schema-2 model in memory.
    pub fn migrate(&mut self) -> Result<(), CalibrationError> {
        if self.schema != LEGACY_SCHEMA && self.schema != SCHEMA {
            return Err(CalibrationError::Invalid(format!(
                "unsupported schema {}",
                self.schema
            )));
        }
        self.schema = SCHEMA;
        for (key, point) in &self.currencies {
            self.items.entry(key.clone()).or_insert_with(|| ItemConfig {
                display_name: default_display_name(key),
                source: ItemSource::inventory(*point),
            });
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), CalibrationError> {
        if self.schema != LEGACY_SCHEMA && self.schema != SCHEMA {
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
        if let Some(guard) = &self.guard {
            validate_rect("guard", guard.region)?;
            if guard.reference.as_os_str().is_empty() {
                return Err(CalibrationError::Invalid(
                    "guard reference image is empty".to_string(),
                ));
            }
            if !(0.0..=1.0).contains(&guard.threshold) {
                return Err(CalibrationError::Invalid(
                    "guard threshold must be between 0 and 1".to_string(),
                ));
            }
            if !(0.0..=1.0).contains(&guard.margin) {
                return Err(CalibrationError::Invalid(
                    "guard margin must be between 0 and 1".to_string(),
                ));
            }
            if guard.search_px > 64 {
                return Err(CalibrationError::Invalid(
                    "guard search_px must be <= 64".to_string(),
                ));
            }
            if guard.scales.is_empty()
                || guard.scales.len() > 8
                || guard
                    .scales
                    .iter()
                    .any(|scale| !(0.5..=2.0).contains(scale))
            {
                return Err(CalibrationError::Invalid(
                    "guard scales must be 1 to 8 values between 0.5 and 2.0".to_string(),
                ));
            }
            validate_template_path("guard", &guard.reference)?;
        }
        for (name, strip) in self.strips.iter() {
            validate_rect(&format!("{name} strip"), strip.region)?;
        }
        let mut indices = std::collections::BTreeSet::new();
        for tab in &self.tabs {
            if !indices.insert(tab.index) {
                return Err(CalibrationError::Invalid(format!(
                    "duplicate tab index {}",
                    tab.index
                )));
            }
            validate_rect(&format!("tab {} content", tab.index), tab.content.region)?;
            if tab.content.columns == 0 || tab.content.rows == 0 {
                return Err(CalibrationError::Invalid(format!(
                    "tab {} grid must have at least one row and column",
                    tab.index
                )));
            }
            if tab.content.columns > 64 || tab.content.rows > 64 {
                return Err(CalibrationError::Invalid(format!(
                    "tab {} grid is unreasonably large",
                    tab.index
                )));
            }
            validate_template_path(&format!("tab {} selected", tab.index), &tab.selected)?;
            if let Some(unselected) = &tab.unselected {
                validate_template_path(&format!("tab {} unselected", tab.index), unselected)?;
            }
        }
        let has_tab_source = self
            .items
            .values()
            .any(|item| matches!(item.source, ItemSource::Tab { .. }));
        if has_tab_source && self.strips.horizontal.is_none() && self.strips.vertical.is_none() {
            return Err(CalibrationError::Invalid(
                "tab item sources are configured but no tab strip region is calibrated".to_string(),
            ));
        }
        for (key, item) in &self.items {
            if item.display_name.trim().is_empty() {
                return Err(CalibrationError::Invalid(format!(
                    "item `{key}` display_name is empty"
                )));
            }
            match &item.source {
                ItemSource::Inventory { x, y } => {
                    if !NormPoint::new(*x, *y).is_valid() {
                        return Err(CalibrationError::Invalid(format!(
                            "item `{key}` inventory point is outside 0..=1"
                        )));
                    }
                }
                ItemSource::Tab { tab, column, row } => {
                    let Some(tab) = self.tab(*tab) else {
                        return Err(CalibrationError::Invalid(format!(
                            "item `{key}` references unknown tab {tab}"
                        )));
                    };
                    if tab.content.cell_center(*column, *row).is_none() {
                        return Err(CalibrationError::Invalid(format!(
                            "item `{key}` cell ({column}, {row}) is outside tab {} grid",
                            tab.index
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_template_files(&self, base_file: &Path) -> Result<(), CalibrationError> {
        let check = |name: &str, template: &Path| {
            let resolved = resolve_relative(base_file, template);
            if !resolved.is_file() {
                return Err(CalibrationError::Invalid(format!(
                    "{name} template is missing: {}",
                    resolved.display()
                )));
            }
            Ok(())
        };
        if let Some(guard) = &self.guard {
            check("guard", &guard.reference)?;
        }
        for tab in &self.tabs {
            check(&format!("tab {} selected", tab.index), &tab.selected)?;
            if let Some(unselected) = &tab.unselected {
                check(&format!("tab {} unselected", tab.index), unselected)?;
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
        if let Some(item) = self.items.get(target) {
            if matches!(item.source, ItemSource::Tab { .. }) {
                return Err(CalibrationError::Invalid(format!(
                    "target `{target}` is configured as a tab source; remove the tab mapping before calibrating a point"
                )));
            }
        }
        self.currencies.insert(target.to_string(), point);
        match self.items.get_mut(target) {
            Some(item) => item.source = ItemSource::inventory(point),
            None => {
                self.items.insert(
                    target.to_string(),
                    ItemConfig {
                        display_name: default_display_name(target),
                        source: ItemSource::inventory(point),
                    },
                );
            }
        }
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

    pub fn set_guard(&mut self, guard: ImageGuard) -> Result<(), CalibrationError> {
        validate_rect("guard", guard.region)?;
        self.guard = Some(guard);
        Ok(())
    }

    pub fn set_strip(
        &mut self,
        orientation: &str,
        region: NormRect,
    ) -> Result<(), CalibrationError> {
        validate_rect(&format!("{orientation} strip"), region)?;
        let strip = Some(StripConfig { region });
        match orientation {
            "horizontal" => self.strips.horizontal = strip,
            "vertical" => self.strips.vertical = strip,
            other => {
                return Err(CalibrationError::Invalid(format!(
                    "unknown strip orientation `{other}`"
                )))
            }
        }
        Ok(())
    }

    pub fn upsert_tab(&mut self, tab: TabConfig) -> Result<(), CalibrationError> {
        validate_rect(&format!("tab {} content", tab.index), tab.content.region)?;
        if tab.content.columns == 0 || tab.content.rows == 0 {
            return Err(CalibrationError::Invalid(
                "tab grid must have at least one row and column".to_string(),
            ));
        }
        match self
            .tabs
            .iter_mut()
            .find(|existing| existing.index == tab.index)
        {
            Some(existing) => *existing = tab,
            None => self.tabs.push(tab),
        }
        self.tabs.sort_by_key(|tab| tab.index);
        Ok(())
    }

    pub fn set_item(&mut self, key: &str, item: ItemConfig) {
        self.items.insert(key.to_string(), item);
    }

    pub fn tab(&self, index: u32) -> Option<&TabConfig> {
        self.tabs.iter().find(|tab| tab.index == index)
    }

    pub fn item_source(&self, key: &str) -> Option<ItemSource> {
        if let Some(item) = self.items.get(key) {
            return Some(item.source.clone());
        }
        self.currencies.get(key).copied().map(ItemSource::inventory)
    }

    pub fn display_name(&self, key: &str) -> Option<&str> {
        self.items.get(key).map(|item| item.display_name.as_str())
    }

    pub fn point(&self, target: &str) -> Option<NormPoint> {
        if target == TARGET_ITEM {
            return self.item;
        }
        if target == TARGET_PANEL {
            return self.panel.map(|panel| panel.point);
        }
        if let Some(item) = self.items.get(target) {
            return match &item.source {
                ItemSource::Inventory { x, y } => Some(NormPoint::new(*x, *y)),
                ItemSource::Tab { tab, column, row } => self
                    .tab(*tab)
                    .and_then(|tab| tab.content.cell_center(*column, *row)),
            };
        }
        self.currencies.get(target).copied()
    }

    pub fn missing(&self, required: &[&str]) -> Vec<String> {
        required
            .iter()
            .filter(|target| {
                let target: &str = target;
                if target == TARGET_ITEM {
                    self.item.is_none()
                } else if target == TARGET_PANEL {
                    self.panel.is_none()
                } else {
                    self.item_source(target).is_none()
                }
            })
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

fn validate_template_path(name: &str, path: &Path) -> Result<(), CalibrationError> {
    if path.as_os_str().is_empty() {
        return Err(CalibrationError::Invalid(format!(
            "{name} template path is empty"
        )));
    }
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return Err(CalibrationError::Invalid(format!(
            "{name} template path must stay inside the calibration directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_rect(name: &str, rect: NormRect) -> Result<(), CalibrationError> {
    if !rect.is_valid() {
        return Err(CalibrationError::Invalid(format!(
            "{name} rectangle is outside 0..=1"
        )));
    }
    Ok(())
}

fn default_display_name(key: &str) -> String {
    match key {
        TARGET_TRANSMUTE => "Orb of Transmutation".to_string(),
        TARGET_ALT => "Orb of Alteration".to_string(),
        TARGET_AUG => "Orb of Augmentation".to_string(),
        TARGET_REGAL => "Regal Orb".to_string(),
        TARGET_ALCH => "Orb of Alchemy".to_string(),
        TARGET_SCOUR => "Orb of Scouring".to_string(),
        TARGET_CHAOS => "Chaos Orb".to_string(),
        other => other.to_string(),
    }
}

/// Resolve a template path relative to the calibration file's directory.
pub fn resolve_relative(base_file: &Path, relative: &Path) -> PathBuf {
    if relative.is_absolute() {
        return relative.to_path_buf();
    }
    base_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(relative))
        .unwrap_or_else(|| relative.to_path_buf())
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
    fn rect_bounds_and_center() {
        let rect = NormRect::new(0.1, 0.2, 0.4, 0.4);
        assert!(rect.is_valid());
        assert!(rect.contains(NormPoint::new(0.3, 0.4)));
        assert!(!rect.contains(NormPoint::new(0.6, 0.4)));
        assert_eq!(rect.center(), NormPoint::new(0.3, 0.4));
        assert!(!NormRect::new(0.8, 0.2, 0.4, 0.4).is_valid());
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
        assert_eq!(loaded.schema, SCHEMA);
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
        assert_eq!(loaded.schema, SCHEMA);
        assert!(loaded.panel.is_none());
        assert_eq!(loaded.point("alt"), Some(NormPoint::new(0.1, 0.2)));
        assert_eq!(
            loaded.item_source("alt"),
            Some(ItemSource::inventory(NormPoint::new(0.1, 0.2)))
        );
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

    #[test]
    fn grid_cell_centers() {
        let grid = GridConfig {
            region: NormRect::new(0.1, 0.2, 0.6, 0.4),
            columns: 3,
            rows: 2,
        };
        let center = grid.cell_center(0, 0).unwrap();
        assert!((center.x - 0.2).abs() < 1e-6);
        assert!((center.y - 0.3).abs() < 1e-6);
        assert!(grid.cell_center(3, 0).is_none());
        assert!(grid.cell_center(0, 2).is_none());
    }

    #[test]
    fn tab_and_item_validation() {
        let mut calibration = Calibration::empty();
        calibration
            .set_strip("horizontal", NormRect::new(0.0, 0.0, 0.5, 0.05))
            .unwrap();
        let tab = TabConfig {
            index: 0,
            kind: "currency".to_string(),
            name: "Currency".to_string(),
            selected: PathBuf::from("tab0-selected.png"),
            unselected: None,
            content: GridConfig {
                region: NormRect::new(0.1, 0.1, 0.5, 0.5),
                columns: 12,
                rows: 12,
            },
        };
        calibration.upsert_tab(tab).unwrap();
        calibration.set_item(
            "chaos",
            ItemConfig {
                display_name: "Chaos Orb".to_string(),
                source: ItemSource::Tab {
                    tab: 0,
                    column: 4,
                    row: 1,
                },
            },
        );
        calibration.validate().unwrap();
        calibration.tabs[0].content.columns = 0;
        assert!(calibration.validate().is_err());
    }

    #[test]
    fn tab_items_reject_unknown_tab_and_cell() {
        let mut calibration = Calibration::empty();
        calibration
            .set_strip("horizontal", NormRect::new(0.0, 0.0, 0.5, 0.05))
            .unwrap();
        calibration
            .upsert_tab(TabConfig {
                index: 0,
                kind: "currency".to_string(),
                name: "Currency".to_string(),
                selected: PathBuf::from("tab0-selected.png"),
                unselected: None,
                content: GridConfig {
                    region: NormRect::new(0.1, 0.1, 0.5, 0.5),
                    columns: 2,
                    rows: 2,
                },
            })
            .unwrap();
        calibration.set_item(
            "chaos",
            ItemConfig {
                display_name: "Chaos Orb".to_string(),
                source: ItemSource::Tab {
                    tab: 9,
                    column: 0,
                    row: 0,
                },
            },
        );
        assert!(calibration.validate().is_err());
        calibration.set_item(
            "chaos",
            ItemConfig {
                display_name: "Chaos Orb".to_string(),
                source: ItemSource::Tab {
                    tab: 0,
                    column: 2,
                    row: 0,
                },
            },
        );
        assert!(calibration.validate().is_err());
    }

    #[test]
    fn schema_two_round_trip_with_vision_and_tabs() {
        let dir = std::env::temp_dir().join("poe-crafter-schema-two-test");
        let path = dir.join("calibration.toml");
        let templates = dir.join("calibration-templates");
        std::fs::create_dir_all(&templates).unwrap();
        for name in ["guard.png", "tab0-selected.png", "tab0-unselected.png"] {
            std::fs::write(templates.join(name), b"").unwrap();
        }
        let mut calibration = Calibration::empty();
        calibration.set_capture_size(1920, 1080);
        calibration
            .set_guard(ImageGuard::new(
                NormRect::new(0.0, 0.0, 0.2, 0.1),
                PathBuf::from("calibration-templates/guard.png"),
            ))
            .unwrap();
        calibration
            .set_strip("horizontal", NormRect::new(0.0, 0.02, 0.6, 0.03))
            .unwrap();
        calibration
            .set_strip("vertical", NormRect::new(0.84, 0.1, 0.03, 0.6))
            .unwrap();
        calibration
            .upsert_tab(TabConfig {
                index: 0,
                kind: "currency".to_string(),
                name: "Currency".to_string(),
                selected: PathBuf::from("calibration-templates/tab0-selected.png"),
                unselected: Some(PathBuf::from("calibration-templates/tab0-unselected.png")),
                content: GridConfig {
                    region: NormRect::new(0.1, 0.1, 0.5, 0.5),
                    columns: 12,
                    rows: 12,
                },
            })
            .unwrap();
        calibration.set_item(
            "chaos",
            ItemConfig {
                display_name: "Chaos Orb".to_string(),
                source: ItemSource::Tab {
                    tab: 0,
                    column: 4,
                    row: 1,
                },
            },
        );
        calibration.set_item(
            "alt",
            ItemConfig {
                display_name: "Orb of Alteration".to_string(),
                source: ItemSource::Inventory { x: 0.1, y: 0.2 },
            },
        );
        calibration.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("schema = 2"));
        assert!(text.contains("[[tabs]]"));
        assert!(text.contains("kind = \"tab\""));
        let loaded = Calibration::load(&path).unwrap();
        assert_eq!(loaded.schema, SCHEMA);
        assert_eq!(loaded.guard, calibration.guard);
        assert_eq!(loaded.strips, calibration.strips);
        assert_eq!(loaded.tabs, calibration.tabs);
        assert_eq!(loaded.items, calibration.items);
        assert_eq!(
            loaded.item_source("chaos"),
            Some(ItemSource::Tab {
                tab: 0,
                column: 4,
                row: 1,
            })
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn sample_tab(selected: PathBuf) -> TabConfig {
        TabConfig {
            index: 0,
            kind: "currency".to_string(),
            name: "Currency".to_string(),
            selected,
            unselected: None,
            content: GridConfig {
                region: NormRect::new(0.1, 0.1, 0.5, 0.5),
                columns: 4,
                rows: 4,
            },
        }
    }

    #[test]
    fn rejects_unsafe_template_paths() {
        let mut calibration = Calibration::empty();
        calibration
            .set_strip("horizontal", NormRect::new(0.0, 0.0, 0.5, 0.05))
            .unwrap();
        calibration
            .upsert_tab(sample_tab(PathBuf::from("/absolute/selected.png")))
            .unwrap();
        assert!(calibration.validate().is_err());

        calibration.tabs[0].selected = PathBuf::from("../escape.png");
        assert!(calibration.validate().is_err());

        calibration.tabs[0].selected = PathBuf::from("templates/selected.png");
        calibration
            .set_guard(ImageGuard::new(
                NormRect::new(0.0, 0.0, 0.2, 0.1),
                PathBuf::from("/absolute/guard.png"),
            ))
            .unwrap();
        assert!(calibration.validate().is_err());
    }

    #[test]
    fn rejects_unbounded_guard_scales() {
        let mut calibration = Calibration::empty();
        let mut guard = ImageGuard::new(
            NormRect::new(0.0, 0.0, 0.2, 0.1),
            PathBuf::from("guard.png"),
        );
        guard.scales = vec![1.0; 9];
        calibration.set_guard(guard).unwrap();
        assert!(calibration.validate().is_err());
    }

    #[test]
    fn tab_point_uses_grid_center() {
        let mut calibration = Calibration::empty();
        calibration
            .set_strip("horizontal", NormRect::new(0.0, 0.0, 0.5, 0.05))
            .unwrap();
        calibration
            .upsert_tab(sample_tab(PathBuf::from("selected.png")))
            .unwrap();
        calibration.set_item(
            "chaos",
            ItemConfig {
                display_name: "Chaos Orb".to_string(),
                source: ItemSource::Tab {
                    tab: 0,
                    column: 1,
                    row: 2,
                },
            },
        );
        let expected = calibration.tabs[0].content.cell_center(1, 2).unwrap();
        assert_eq!(calibration.point("chaos"), Some(expected));
        assert_eq!(
            calibration.item_source("chaos"),
            Some(ItemSource::Tab {
                tab: 0,
                column: 1,
                row: 2,
            })
        );
    }

    #[test]
    fn set_target_rejects_tab_source() {
        let mut calibration = Calibration::empty();
        calibration.set_item(
            "chaos",
            ItemConfig {
                display_name: "Chaos Orb".to_string(),
                source: ItemSource::Tab {
                    tab: 0,
                    column: 1,
                    row: 2,
                },
            },
        );
        assert!(calibration
            .set_target("chaos", NormPoint::new(0.1, 0.2))
            .is_err());
        assert!(matches!(
            calibration.item_source("chaos"),
            Some(ItemSource::Tab { .. })
        ));
    }

    #[test]
    fn save_rejects_missing_template_files() {
        let dir = std::env::temp_dir().join("poe-crafter-missing-template-test");
        let path = dir.join("calibration.toml");
        let mut calibration = Calibration::empty();
        calibration
            .set_guard(ImageGuard::new(
                NormRect::new(0.0, 0.0, 0.2, 0.1),
                PathBuf::from("calibration-templates/missing.png"),
            ))
            .unwrap();
        assert!(calibration.save(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn relative_templates_resolve_against_calibration_file() {
        let base = PathBuf::from("profiles/calibration.toml");
        assert_eq!(
            resolve_relative(&base, Path::new("calibration-templates/guard.png")),
            PathBuf::from("profiles/calibration-templates/guard.png")
        );
        assert_eq!(
            resolve_relative(&base, Path::new("C:/abs/guard.png")),
            PathBuf::from("C:/abs/guard.png")
        );
    }
}
