use std::path::{Path, PathBuf};
use std::result::Result;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use poe_session::{Currency, GameIo, IoError, WindowInfo};

use crate::calibration::{resolve_relative, Calibration, ItemSource, NormPoint, NormRect};
use crate::vision::{
    best_match, find_match_in_region, match_best_of, CapturedImage, GrayImage, MatchParams,
    MatchResult, VisionError,
};
use crate::win32::{InputError, Win32Game};

const TAB_SCROLL_STEP: i32 = 120;
const MAX_TAB_SCROLL_STEPS: usize = 5;
const TAB_CLICK_ATTEMPTS: usize = 3;
const TAB_SETTLE: u64 = 250;

struct LoadedGuard {
    region: NormRect,
    template: GrayImage,
    params: MatchParams,
}

struct LoadedStrip {
    region: NormRect,
}

struct LoadedTab {
    index: u32,
    selected: GrayImage,
    unselected: Option<GrayImage>,
}

struct VisionEngine {
    guard: Option<LoadedGuard>,
    strips: Vec<LoadedStrip>,
    tabs: Vec<LoadedTab>,
    params: MatchParams,
}

impl VisionEngine {
    fn load(
        calibration: &Calibration,
        calibration_path: Option<&Path>,
    ) -> Result<Self, VisionError> {
        calibration
            .validate()
            .map_err(|error| VisionError::InvalidConfig(error.to_string()))?;
        let base = calibration_path.unwrap_or_else(|| Path::new("calibration.toml"));
        let guard = match &calibration.guard {
            Some(guard) => Some(LoadedGuard {
                region: guard.region,
                template: load_template(base, &guard.reference)?,
                params: guard.params(),
            }),
            None => None,
        };
        let params = calibration
            .guard
            .as_ref()
            .map(|guard| guard.params())
            .unwrap_or_default();
        let mut strips = Vec::new();
        if let Some(strip) = &calibration.strips.horizontal {
            strips.push(LoadedStrip {
                region: strip.region,
            });
        }
        if let Some(strip) = &calibration.strips.vertical {
            strips.push(LoadedStrip {
                region: strip.region,
            });
        }
        let mut tabs = Vec::new();
        for tab in &calibration.tabs {
            tabs.push(LoadedTab {
                index: tab.index,
                selected: load_template(base, &tab.selected)?,
                unselected: tab
                    .unselected
                    .as_deref()
                    .map(|path| load_template(base, path))
                    .transpose()?,
            });
        }
        Ok(Self {
            guard,
            strips,
            tabs,
            params,
        })
    }

    fn has_tabs(&self) -> bool {
        !self.tabs.is_empty()
    }
}

fn load_template(base: &Path, path: &Path) -> Result<GrayImage, VisionError> {
    let resolved = resolve_relative(base, path);
    Ok(GrayImage::load_png(&resolved)?.prepared())
}

/// Windows implementation of the session I/O boundary.
pub struct Win32GameIo {
    pub game: Win32Game,
    pub calibration: Calibration,
    vision: VisionEngine,
    item_anchor: Option<NormPoint>,
    diagnostics_dir: PathBuf,
    verified_source: Option<(Currency, u32, Instant)>,
}

impl Win32GameIo {
    pub fn new(
        game: Win32Game,
        calibration: Calibration,
        calibration_path: Option<PathBuf>,
    ) -> Result<Self, VisionError> {
        let vision = VisionEngine::load(&calibration, calibration_path.as_deref())?;
        Ok(Self {
            game,
            calibration,
            vision,
            item_anchor: None,
            diagnostics_dir: PathBuf::from("logs"),
            verified_source: None,
        })
    }

    /// Use the cursor position captured at session start as the item anchor.
    pub fn set_item_anchor(&mut self, point: NormPoint) {
        self.item_anchor = Some(point);
    }

    pub fn set_diagnostics_dir(&mut self, dir: PathBuf) {
        self.diagnostics_dir = dir;
    }

    /// Capture the live guard and selected tab without clicking. Used by the
    /// UI test button.
    pub fn test_vision(&mut self) -> Result<String, String> {
        let mut out = String::new();
        if let Some(guard) = &self.vision.guard {
            let (_, image, client_width, client_height) = self
                .capture_region(guard.region, guard.params.search_px)
                .map_err(|error| error.to_string())?;
            let candidate = image.to_gray().prepared();
            match best_match(&candidate, &guard.template, &guard.params) {
                Some(result) => out.push_str(&format!(
                    "Guard score {:.3} (threshold {:.3}) at scale {:.2}, client {client_width}x{client_height}\n",
                    result.score, guard.params.threshold, result.scale
                )),
                None => out.push_str("Guard could not be matched\n"),
            }
        } else if self.calibration.panel.is_some() {
            out.push_str("Legacy pixel guard is configured\n");
        } else {
            out.push_str("No panel guard is configured\n");
        }
        if self.vision.has_tabs() {
            match self.current_tab_and_strip() {
                Ok(Some((tab, strip))) => {
                    out.push_str(&format!("Selected tab: {tab} (strip {strip})\n"))
                }
                Ok(None) => out.push_str("Selected tab: not identified\n"),
                Err(error) => out.push_str(&format!("Selected tab error: {error}\n")),
            }
        }
        Ok(out)
    }

    /// Reject an item anchor that sits in a stash tab when the plan needs a
    /// different tab: switching away would hide the item.
    pub fn validate_item_anchor(&self, currencies: &[Currency]) -> Result<(), InputError> {
        let Some(anchor) = self.item_anchor.or(self.calibration.item) else {
            return Ok(());
        };
        let Some(item_tab) = self
            .calibration
            .tabs
            .iter()
            .find(|tab| tab.content.region.contains(anchor))
            .map(|tab| tab.index)
        else {
            return Ok(());
        };
        for currency in currencies {
            if let Some(ItemSource::Tab { tab, .. }) = self.calibration.item_source(currency.key())
            {
                if tab != item_tab {
                    return Err(InputError::Vision(format!(
                        "item anchor is inside tab {item_tab} but `{}` comes from tab {tab}; place the item in the player inventory or keep all sources in the same tab",
                        currency.key()
                    )));
                }
            }
        }
        Ok(())
    }

    fn item_point(&self) -> Result<NormPoint, IoError> {
        self.item_anchor
            .or(self.calibration.item)
            .ok_or_else(|| IoError::MissingCalibration("item".to_string()))
    }

    fn currency_source(&self, currency: Currency) -> Result<(NormPoint, Option<u32>), IoError> {
        let key = currency.key();
        let source = self
            .calibration
            .item_source(key)
            .ok_or_else(|| IoError::MissingCalibration(key.to_string()))?;
        match source {
            ItemSource::Inventory { x, y } => Ok((NormPoint::new(x, y), None)),
            ItemSource::Tab { tab, column, row } => {
                let tab_config = self
                    .calibration
                    .tab(tab)
                    .ok_or_else(|| IoError::MissingCalibration(format!("tab {tab} for `{key}`")))?;
                let point = tab_config.content.cell_center(column, row).ok_or_else(|| {
                    IoError::Input(format!(
                        "item `{key}` cell ({column}, {row}) is outside tab {tab}"
                    ))
                })?;
                Ok((point, Some(tab)))
            }
        }
    }

    fn expected_display_name(&self, currency: Currency) -> String {
        self.calibration
            .display_name(currency.key())
            .map(str::to_string)
            .unwrap_or_else(|| currency.label().to_string())
    }

    fn capture_region(
        &self,
        region: NormRect,
        pad_px: u32,
    ) -> Result<(NormRect, CapturedImage, i32, i32), IoError> {
        let info = self.game.window_info().map_err(map_input)?;
        let padded = pad_region(region, pad_px, info.client_width, info.client_height);
        let image = self.game.capture_rect(padded).map_err(map_input)?;
        Ok((padded, image, info.client_width, info.client_height))
    }

    fn current_tab_and_strip(&self) -> Result<Option<(u32, usize)>, IoError> {
        if self.vision.tabs.is_empty() {
            return Ok(None);
        }
        let templates: Vec<&GrayImage> = self.vision.tabs.iter().map(|tab| &tab.selected).collect();
        let mut best: Option<(u32, usize, MatchResult)> = None;
        for (strip_index, strip) in self.vision.strips.iter().enumerate() {
            let (_, image, _, _) =
                self.capture_region(strip.region, self.vision.params.search_px)?;
            let candidate = image.to_gray().prepared();
            let Some((matched, _)) = match_best_of(&candidate, &templates, &self.vision.params)
            else {
                continue;
            };
            if !matched.result.passes(&self.vision.params) {
                continue;
            }
            let Some(tab) = self.vision.tabs.get(matched.index) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(_, _, current)| matched.result.score > current.score)
            {
                best = Some((tab.index, strip_index, matched.result));
            }
        }
        Ok(best.map(|(tab, strip, _)| (tab, strip)))
    }

    fn current_selected_tab(&self) -> Result<Option<u32>, IoError> {
        Ok(self.current_tab_and_strip()?.map(|(tab, _)| tab))
    }

    fn find_tab(
        &self,
        tab_index: u32,
        strip_filter: Option<usize>,
    ) -> Result<Option<(NormPoint, MatchResult)>, IoError> {
        let Some(tab) = self.vision.tabs.iter().find(|tab| tab.index == tab_index) else {
            return Ok(None);
        };
        let mut templates: Vec<&GrayImage> = Vec::new();
        if let Some(unselected) = &tab.unselected {
            templates.push(unselected);
        }
        templates.push(&tab.selected);
        let mut best: Option<(NormPoint, MatchResult)> = None;
        for (strip_index, strip) in self.vision.strips.iter().enumerate() {
            if strip_filter.is_some_and(|filter| filter != strip_index) {
                continue;
            }
            let (padded, image, client_width, client_height) =
                self.capture_region(strip.region, self.vision.params.search_px)?;
            let candidate = image.to_gray().prepared();
            for template in &templates {
                let Some(result) = find_match_in_region(&candidate, template, &self.vision.params)
                else {
                    continue;
                };
                if result.score < self.vision.params.threshold {
                    continue;
                }
                let (center_x, center_y) = result.center_native;
                let point = NormPoint::new(
                    padded.x + center_x / client_width.max(1) as f32,
                    padded.y + center_y / client_height.max(1) as f32,
                );
                if !point.is_valid() {
                    continue;
                }
                if best
                    .as_ref()
                    .is_none_or(|(_, current)| result.score > current.score)
                {
                    best = Some((point, result));
                }
            }
        }
        Ok(best)
    }

    fn try_select_tab(
        &mut self,
        tab_index: u32,
        strip_filter: Option<usize>,
    ) -> Result<bool, IoError> {
        let Some((point, _)) = self.find_tab(tab_index, strip_filter)? else {
            return Ok(false);
        };
        self.check_action_gate()?;
        self.game.click_point(point, true).map_err(map_input)?;
        for _ in 0..TAB_CLICK_ATTEMPTS {
            self.sleep(TAB_SETTLE);
            if self.current_selected_tab()? == Some(tab_index) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn check_action_gate(&mut self) -> Result<(), IoError> {
        if self.game.emergency_stop() {
            return Err(IoError::Cancelled);
        }
        if !self.game.is_foreground() {
            return Err(IoError::NotForeground);
        }
        self.preflight()
    }

    fn save_region_failure(&self, region: NormRect, label: &str) {
        if let Ok((_, image, _, _)) = self.capture_region(region, self.vision.params.search_px) {
            self.save_vision_failure(&image, label, None);
        }
    }

    fn save_vision_failure(&self, image: &CapturedImage, label: &str, score: Option<f32>) {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let path = self
            .diagnostics_dir
            .join(format!("vision-{epoch}-{label}.png"));
        if image.save_png(&path).is_ok() {
            let details = format!(
                "{{\"label\":\"{label}\",\"score\":{},\"width\":{},\"height\":{}}}\n",
                score
                    .map(|value| format!("{value:.4}"))
                    .unwrap_or_else(|| "null".to_string()),
                image.width,
                image.height
            );
            let _ = std::fs::write(path.with_extension("json"), details);
        }
    }

    fn read_verified_stack(
        &mut self,
        currency: Currency,
        point: NormPoint,
    ) -> Result<u32, IoError> {
        let read = self.game.read_currency(point).map_err(map_input)?;
        if read.count > 0 {
            let expected = self.expected_display_name(currency);
            if !names_match(&read.display_name, &expected) {
                self.save_region_failure(
                    rect_around(point, 0.03),
                    &format!("source-{}", currency.key()),
                );
                return Err(IoError::Preflight(format!(
                    "source slot holds `{}`, expected `{expected}`",
                    read.display_name
                )));
            }
        }
        Ok(read.count)
    }

    fn prepare_source_tab(&mut self, tab_index: u32) -> Result<(), IoError> {
        self.preflight()?;
        let identified = self.current_tab_and_strip()?;
        if identified.is_some_and(|(tab, _)| tab == tab_index) {
            return Ok(());
        }
        if self.try_select_tab(tab_index, identified.map(|(_, strip)| strip))? {
            return Ok(());
        }
        let Some((_, strip_index)) = identified else {
            if let Some(strip) = self.vision.strips.first() {
                self.save_region_failure(strip.region, &format!("tab-{tab_index}"));
            }
            return Err(IoError::Preflight(format!(
                "target stash tab {tab_index} was not found; show the tab strip and retry"
            )));
        };
        // The first sweep moves forward; the reverse sweep must first undo that
        // movement and then continue past the starting position, otherwise the
        // opposite side of the strip is never searched.
        for (delta, steps) in [
            (TAB_SCROLL_STEP, MAX_TAB_SCROLL_STEPS),
            (-TAB_SCROLL_STEP, MAX_TAB_SCROLL_STEPS * 2),
        ] {
            for _ in 0..steps {
                self.check_action_gate()?;
                let Some(strip) = self.vision.strips.get(strip_index) else {
                    break;
                };
                let center = strip.region.center();
                self.game.scroll_point(center, delta).map_err(map_input)?;
                self.sleep(120);
                if self.try_select_tab(tab_index, Some(strip_index))? {
                    return Ok(());
                }
            }
        }
        if let Some(strip) = self.vision.strips.get(strip_index) {
            self.save_region_failure(strip.region, &format!("tab-{tab_index}"));
        }
        Err(IoError::Preflight(format!(
            "target stash tab {tab_index} was not found; show or scroll the tab strip and retry"
        )))
    }
}

impl GameIo for Win32GameIo {
    fn window_info(&mut self) -> Result<WindowInfo, IoError> {
        let info = self.game.window_info().map_err(map_input)?;
        Ok(WindowInfo {
            title: info.title,
            client_width: info.client_width,
            client_height: info.client_height,
            foreground: info.foreground,
        })
    }

    fn is_foreground(&mut self) -> Result<bool, IoError> {
        Ok(self.game.is_foreground())
    }

    fn emergency_stop(&mut self) -> bool {
        self.game.emergency_stop()
    }

    fn preflight(&mut self) -> Result<(), IoError> {
        if let Some(guard) = &self.vision.guard {
            let (_, image, _, _) = self.capture_region(guard.region, guard.params.search_px)?;
            let candidate = image.to_gray().prepared();
            let result = best_match(&candidate, &guard.template, &guard.params);
            match result {
                Some(result) if result.score >= guard.params.threshold => Ok(()),
                Some(result) => {
                    self.save_vision_failure(&image, "guard", Some(result.score));
                    Err(IoError::Preflight(format!(
                        "panel guard score {:.3} is below {:.3}; is the stash/inventory open?",
                        result.score, guard.params.threshold
                    )))
                }
                None => {
                    self.save_vision_failure(&image, "guard", None);
                    Err(IoError::Preflight(
                        "panel guard could not be matched; check the guard template and client size"
                            .to_string(),
                    ))
                }
            }
        } else if let Some(guard) = self.calibration.panel {
            self.game.verify_panel(guard).map_err(map_input)
        } else {
            Ok(())
        }
    }

    fn prepare_source(&mut self, currency: Currency) -> Result<(), IoError> {
        let (point, Some(tab_index)) = self.currency_source(currency)? else {
            return Ok(());
        };
        if !self.vision.has_tabs() {
            return Ok(());
        }
        self.prepare_source_tab(tab_index)?;
        // Verify the slot even when stack-count verification is disabled: a
        // wrong grid cell or tab must not be clicked.
        let count = self.read_verified_stack(currency, point)?;
        if count == 0 {
            return Err(IoError::Preflight(format!(
                "the configured tab slot for `{}` is empty",
                currency.key()
            )));
        }
        // The session reads the count immediately after preparation; reuse it
        // instead of running another clipboard copy/poll cycle.
        self.verified_source = Some((currency, count, Instant::now()));
        Ok(())
    }

    fn read_item(&mut self) -> Result<String, IoError> {
        let item = self.item_point()?;
        self.game.read_item(item).map_err(map_input)
    }

    fn read_currency(&mut self, currency: Currency) -> Result<u32, IoError> {
        if let Some((cached_currency, count, at)) = self.verified_source.take() {
            if cached_currency == currency && at.elapsed() <= Duration::from_millis(500) {
                return Ok(count);
            }
        }
        let (point, _) = self.currency_source(currency)?;
        self.read_verified_stack(currency, point)
    }

    fn apply_currency(&mut self, currency: Currency) -> Result<(), IoError> {
        self.verified_source = None;
        self.preflight()?;
        let (point, _) = self.currency_source(currency)?;
        let item = self.item_point()?;
        self.game.apply_currency(point, item).map_err(map_input)
    }

    fn begin_currency(&mut self, currency: Currency) -> Result<(), IoError> {
        self.verified_source = None;
        self.preflight()?;
        let (point, _) = self.currency_source(currency)?;
        self.game.begin_hold(point).map_err(map_input)
    }

    fn use_currency(&mut self) -> Result<(), IoError> {
        self.verified_source = None;
        self.preflight()?;
        let item = self.item_point()?;
        self.game.use_held(item).map_err(map_input)
    }

    fn end_currency(&mut self) -> Result<(), IoError> {
        self.verified_source = None;
        self.game.end_hold().map_err(map_input)
    }

    fn sleep(&mut self, millis: u64) {
        std::thread::sleep(Duration::from_millis(millis));
    }
}

fn names_match(actual: &str, expected: &str) -> bool {
    normalize_name(actual) == normalize_name(expected)
}

fn normalize_name(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn rect_around(point: NormPoint, half_size: f32) -> NormRect {
    let x = (point.x - half_size).max(0.0);
    let y = (point.y - half_size).max(0.0);
    let right = (point.x + half_size).min(1.0);
    let bottom = (point.y + half_size).min(1.0);
    NormRect::new(x, y, (right - x).max(0.0001), (bottom - y).max(0.0001))
}

fn pad_region(region: NormRect, pad_px: u32, client_width: i32, client_height: i32) -> NormRect {
    let pad_x = pad_px as f32 / client_width.max(1) as f32;
    let pad_y = pad_px as f32 / client_height.max(1) as f32;
    let x = (region.x - pad_x).max(0.0);
    let y = (region.y - pad_y).max(0.0);
    let right = (region.x + region.w + pad_x).min(1.0);
    let bottom = (region.y + region.h + pad_y).min(1.0);
    NormRect::new(x, y, (right - x).max(0.0001), (bottom - y).max(0.0001))
}

pub fn map_input(error: InputError) -> IoError {
    match error {
        InputError::WindowNotFound => IoError::WindowNotFound,
        InputError::NotForeground => IoError::NotForeground,
        InputError::EmptyClientArea => IoError::Input("window client area is empty".to_string()),
        InputError::PointOutsideWindow => {
            IoError::Input("calibration point is outside the window".to_string())
        }
        InputError::MissingCalibration(target) => IoError::MissingCalibration(target),
        InputError::Clipboard(message) => IoError::Clipboard(message),
        InputError::Input(message) => IoError::Input(message),
        InputError::Timeout(message) => IoError::Timeout(message),
        InputError::PanelGuard { expected, actual } => IoError::Preflight(format!(
            "panel guard expected {expected:?}, sampled {actual:?}; is the stash/inventory open?"
        )),
        InputError::Vision(message) => IoError::Preflight(message),
        InputError::Cancelled => IoError::Cancelled,
    }
}
