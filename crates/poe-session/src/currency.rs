use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use poe_craft::{Costs, NextAction, OnFail, PlanFile, StepAction};

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
    pub const ALL: [Currency; 7] = [
        Currency::Transmute,
        Currency::Alt,
        Currency::Aug,
        Currency::Regal,
        Currency::Alch,
        Currency::Scour,
        Currency::Chaos,
    ];

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

/// Currencies a schema-2 plan may use. Legacy plans return `None` because their
/// next action is data-dependent.
pub fn plan_currencies(plan: &PlanFile) -> Option<Vec<Currency>> {
    let PlanFile::Steps(plan) = plan else {
        return None;
    };
    let mut currencies = BTreeSet::new();
    let mut reset_may_need_scour = false;
    for step in &plan.steps {
        if step.on_fail == OnFail::Restart {
            reset_may_need_scour = true;
        }
        // Include every currency `BuildRunner::next_action` can return for this
        // action, not only the declared action: rarity can switch to a setup or
        // reset currency.
        match step.action {
            StepAction::Transmute => {
                currencies.insert(Currency::Transmute);
            }
            StepAction::Alt => {
                currencies.insert(Currency::Transmute);
                currencies.insert(Currency::Alt);
                currencies.insert(Currency::Scour);
            }
            StepAction::AltAug => {
                currencies.insert(Currency::Transmute);
                currencies.insert(Currency::Alt);
                currencies.insert(Currency::Aug);
                currencies.insert(Currency::Scour);
            }
            StepAction::Augment => {
                currencies.insert(Currency::Aug);
            }
            StepAction::Regal => {
                currencies.insert(Currency::Regal);
            }
            StepAction::Alch => {
                currencies.insert(Currency::Alch);
            }
            StepAction::Scour => {
                currencies.insert(Currency::Scour);
            }
            StepAction::ScourAlch => {
                currencies.insert(Currency::Scour);
                currencies.insert(Currency::Alch);
            }
            StepAction::Chaos => {
                currencies.insert(Currency::Alch);
                currencies.insert(Currency::Regal);
                currencies.insert(Currency::Chaos);
                currencies.insert(Currency::Scour);
            }
        }
    }
    if reset_may_need_scour {
        currencies.insert(Currency::Scour);
    }
    Some(currencies.into_iter().collect())
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
        NextAction::Scour => vec![Currency::Scour],
        NextAction::ScourAlch => vec![Currency::Scour, Currency::Alch],
        NextAction::ScourTransmute => vec![Currency::Scour, Currency::Transmute],
    }
}
