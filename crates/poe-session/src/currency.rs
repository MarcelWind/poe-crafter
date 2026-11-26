use serde::{Deserialize, Serialize};

use poe_craft::{Costs, NextAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Transmute,
    Alt,
    Aug,
    Regal,
    Alch,
    Scour,
    Chaos,
}

impl std::fmt::Display for Currency {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.key())
    }
}

impl Currency {
    pub fn key(self) -> &'static str {
        match self {
            Currency::Transmute => "transmute",
            Currency::Alt => "alt",
            Currency::Aug => "aug",
            Currency::Regal => "regal",
            Currency::Alch => "alch",
            Currency::Scour => "scour",
            Currency::Chaos => "chaos",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Currency::Transmute => "Orb of Transmutation",
            Currency::Alt => "Orb of Alteration",
            Currency::Aug => "Orb of Augmentation",
            Currency::Regal => "Regal Orb",
            Currency::Alch => "Orb of Alchemy",
            Currency::Scour => "Orb of Scouring",
            Currency::Chaos => "Chaos Orb",
        }
    }

    pub fn cost(self, costs: &Costs) -> f64 {
        match self {
            Currency::Transmute => costs.transmute,
            Currency::Alt => costs.alt,
            Currency::Aug => costs.aug,
            Currency::Regal => costs.regal,
            Currency::Alch => costs.alch,
            Currency::Scour => costs.scour,
            Currency::Chaos => costs.chaos,
        }
    }
}

pub fn currencies_for(action: NextAction) -> Vec<Currency> {
    match action {
        NextAction::Stop => Vec::new(),
        NextAction::Transmute => vec![Currency::Transmute],
        NextAction::Alt => vec![Currency::Alt],
        NextAction::Augment => vec![Currency::Aug],
        NextAction::Regal => vec![Currency::Regal],
        NextAction::Alch => vec![Currency::Alch],
        NextAction::Chaos => vec![Currency::Chaos],
        NextAction::ScourAlch => vec![Currency::Scour, Currency::Alch],
        NextAction::ScourTransmute => vec![Currency::Scour, Currency::Transmute],
    }
}
