use thiserror::Error;

use crate::currency::Currency;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub title: String,
    pub client_width: i32,
    pub client_height: i32,
    pub foreground: bool,
}

#[derive(Debug, Error)]
pub enum IoError {
    #[error("game window not found")]
    WindowNotFound,
    #[error("game window is not foreground")]
    NotForeground,
    #[error("calibration is missing target `{0}`")]
    MissingCalibration(String),
    #[error("clipboard error: {0}")]
    Clipboard(String),
    #[error("input error: {0}")]
    Input(String),
    #[error("timed out reading {0}")]
    Timeout(String),
    #[error("unsupported game state: {0}")]
    Unsupported(String),
    #[error("cancelled")]
    Cancelled,
}

pub trait GameIo {
    fn window_info(&mut self) -> Result<WindowInfo, IoError>;
    fn is_foreground(&mut self) -> Result<bool, IoError>;
    fn emergency_stop(&mut self) -> bool;

    /// Read the item under the calibrated item slot using advanced copy.
    fn read_item(&mut self) -> Result<String, IoError>;

    /// Read the stack size of a calibrated currency slot.
    fn read_currency(&mut self, currency: Currency) -> Result<u32, IoError>;

    /// Apply one currency from its calibrated slot to the calibrated item slot.
    /// Implementations must return the remaining stack to its slot.
    fn apply_currency(&mut self, currency: Currency) -> Result<(), IoError>;

    fn sleep(&mut self, millis: u64);
}
