use std::result::Result;
use std::time::Duration;

use poe_session::{Currency, GameIo, IoError, WindowInfo};

use crate::calibration::Calibration;
use crate::win32::{InputError, Win32Game};

/// Windows implementation of the session I/O boundary.
pub struct Win32GameIo {
    pub game: Win32Game,
    pub calibration: Calibration,
}

impl Win32GameIo {
    pub fn new(game: Win32Game, calibration: Calibration) -> Self {
        Self { game, calibration }
    }

    fn currency_point(&self, currency: Currency) -> Result<crate::NormPoint, IoError> {
        self.calibration
            .point(currency.key())
            .ok_or_else(|| IoError::MissingCalibration(currency.key().to_string()))
    }

    fn item_point(&self) -> Result<crate::NormPoint, IoError> {
        self.calibration
            .item
            .ok_or_else(|| IoError::MissingCalibration("item".to_string()))
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
        if let Some(guard) = self.calibration.panel {
            self.game.verify_panel(guard).map_err(map_input)?;
        }
        Ok(())
    }

    fn read_item(&mut self) -> Result<String, IoError> {
        let item = self.item_point()?;
        self.game.read_item(item).map_err(map_input)
    }

    fn read_currency(&mut self, currency: Currency) -> Result<u32, IoError> {
        let point = self.currency_point(currency)?;
        self.game.read_currency(point).map_err(map_input)
    }

    fn apply_currency(&mut self, currency: Currency) -> Result<(), IoError> {
        self.preflight()?;
        let point = self.currency_point(currency)?;
        let item = self.item_point()?;
        self.game.apply_currency(point, item).map_err(map_input)
    }

    fn begin_currency(&mut self, currency: Currency) -> Result<(), IoError> {
        self.preflight()?;
        let point = self.currency_point(currency)?;
        self.game.begin_hold(point).map_err(map_input)
    }

    fn use_currency(&mut self) -> Result<(), IoError> {
        self.preflight()?;
        let item = self.item_point()?;
        self.game.use_held(item).map_err(map_input)
    }

    fn end_currency(&mut self) -> Result<(), IoError> {
        self.game.end_hold().map_err(map_input)
    }

    fn sleep(&mut self, millis: u64) {
        std::thread::sleep(Duration::from_millis(millis));
    }
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
        InputError::Cancelled => IoError::Cancelled,
    }
}
