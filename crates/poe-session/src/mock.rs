use std::collections::{BTreeMap, VecDeque};

use crate::currency::Currency;
use crate::io::{GameIo, IoError, WindowInfo};

#[derive(Debug, Clone)]
pub struct MockGame {
    pub current: String,
    pub next_items: VecDeque<String>,
    pub currencies: BTreeMap<Currency, u32>,
    pub foreground: bool,
    pub emergency: bool,
    pub consume_currency: bool,
    pub currency_drop: u32,
    pub lose_foreground_after: Option<usize>,
    pub emergency_after: Option<usize>,
    pub applied: Vec<Currency>,
    pub prepared: Vec<Currency>,
    pub prepared_while_held: Vec<Currency>,
    held_currency: Option<Currency>,
}

impl MockGame {
    pub fn new(
        current: impl Into<String>,
        next_items: impl IntoIterator<Item = String>,
        currencies: BTreeMap<Currency, u32>,
    ) -> Self {
        Self {
            current: current.into(),
            next_items: next_items.into_iter().collect(),
            currencies,
            foreground: true,
            emergency: false,
            consume_currency: true,
            currency_drop: 1,
            lose_foreground_after: None,
            emergency_after: None,
            applied: Vec::new(),
            prepared: Vec::new(),
            prepared_while_held: Vec::new(),
            held_currency: None,
        }
    }
}

impl GameIo for MockGame {
    fn window_info(&mut self) -> Result<WindowInfo, IoError> {
        Ok(WindowInfo {
            title: "Mock Path of Exile".to_string(),
            client_width: 1920,
            client_height: 1080,
            foreground: self.foreground,
        })
    }

    fn is_foreground(&mut self) -> Result<bool, IoError> {
        Ok(self.foreground)
    }

    fn emergency_stop(&mut self) -> bool {
        self.emergency
    }

    fn prepare_source(&mut self, currency: Currency) -> Result<(), IoError> {
        self.prepared.push(currency);
        if self.held_currency.is_some() {
            self.prepared_while_held.push(currency);
        }
        Ok(())
    }

    fn read_item(&mut self) -> Result<String, IoError> {
        Ok(self.current.clone())
    }

    fn read_currency(&mut self, currency: Currency) -> Result<u32, IoError> {
        Ok(self.currencies.get(&currency).copied().unwrap_or(0))
    }

    fn apply_currency(&mut self, currency: Currency) -> Result<(), IoError> {
        self.applied.push(currency);
        if self.consume_currency {
            if let Some(count) = self.currencies.get_mut(&currency) {
                *count = count.saturating_sub(self.currency_drop);
            }
            if let Some(next) = self.next_items.pop_front() {
                self.current = next;
            }
        }
        if self
            .lose_foreground_after
            .is_some_and(|limit| self.applied.len() >= limit)
        {
            self.foreground = false;
        }
        if self
            .emergency_after
            .is_some_and(|limit| self.applied.len() >= limit)
        {
            self.emergency = true;
        }
        Ok(())
    }

    fn begin_currency(&mut self, currency: Currency) -> Result<(), IoError> {
        self.held_currency = Some(currency);
        Ok(())
    }

    fn use_currency(&mut self) -> Result<(), IoError> {
        let Some(currency) = self.held_currency else {
            return Ok(());
        };
        self.apply_currency(currency)
    }

    fn end_currency(&mut self) -> Result<(), IoError> {
        self.held_currency = None;
        Ok(())
    }

    fn sleep(&mut self, _millis: u64) {}
}
