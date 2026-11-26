pub mod calibration;
pub mod config;
pub mod game_io;
pub mod vision;
pub mod win32;

pub use calibration::{
    resolve_relative, Calibration, CalibrationError, Color, GridConfig, ImageGuard, ItemConfig,
    ItemSource, NormPoint, NormRect, PanelGuard, StripConfig, Strips, TabConfig, TARGET_ALCH,
    TARGET_ALT, TARGET_AUG, TARGET_CHAOS, TARGET_ITEM, TARGET_PANEL, TARGET_REGAL, TARGET_SCOUR,
    TARGET_TRANSMUTE,
};
pub use config::{AppConfig, ConfigError, HotkeyConfig, ParsedHotkeys, Shortcut, UpdateConfig};
pub use game_io::Win32GameIo;
pub use vision::{
    best_match, find_match_in_region, match_best_of, CapturedImage, GrayImage, MatchParams,
    MatchResult, TemplateMatch, VisionError,
};
pub use win32::{global_cursor_position, InputError, StackRead, Win32Game, WindowInfo};
