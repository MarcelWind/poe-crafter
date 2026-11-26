//! Item state used by goal evaluation and simulation.

use serde::{Deserialize, Serialize};

use poe_data::ModType;
use poe_item::ResolvedItem;

use crate::plan::Slot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CraftRarity {
    Normal,
    Magic,
    Rare,
    Other,
}

impl CraftRarity {
    pub fn caps(self) -> (u8, u8) {
        match self {
            CraftRarity::Normal | CraftRarity::Other => (0, 0),
            CraftRarity::Magic => (1, 1),
            CraftRarity::Rare => (3, 3),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateMod {
    pub mod_id: String,
    pub slot: Slot,
    pub group: String,
    pub affix: String,
    pub tier: Option<u32>,
    pub values: Vec<f64>,
    pub percentiles: Vec<f64>,
    pub template: String,
}

impl StateMod {
    pub fn value(&self, index: usize) -> Option<f64> {
        self.values.get(index).copied()
    }

    pub fn percentile(&self, index: usize) -> Option<f64> {
        self.percentiles.get(index).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemState {
    pub rarity: CraftRarity,
    pub item_level: Option<u32>,
    pub mods: Vec<StateMod>,
}

impl ItemState {
    pub fn new(rarity: CraftRarity, item_level: Option<u32>) -> Self {
        Self {
            rarity,
            item_level,
            mods: Vec::new(),
        }
    }

    pub fn prefix_count(&self) -> u8 {
        self.mods
            .iter()
            .filter(|entry| entry.slot == Slot::Prefix)
            .count() as u8
    }

    pub fn suffix_count(&self) -> u8 {
        self.mods
            .iter()
            .filter(|entry| entry.slot == Slot::Suffix)
            .count() as u8
    }

    pub fn open_prefixes(&self) -> u8 {
        let (prefix_cap, _) = self.rarity.caps();
        prefix_cap.saturating_sub(self.prefix_count())
    }

    pub fn open_suffixes(&self) -> u8 {
        let (_, suffix_cap) = self.rarity.caps();
        suffix_cap.saturating_sub(self.suffix_count())
    }

    pub fn has_group(&self, group: &str) -> bool {
        !group.is_empty() && self.mods.iter().any(|entry| entry.group == group)
    }

    pub fn from_resolved(resolved: &ResolvedItem) -> Self {
        let rarity = match resolved.item.rarity {
            Some(poe_item::Rarity::Normal) => CraftRarity::Normal,
            Some(poe_item::Rarity::Magic) => CraftRarity::Magic,
            Some(poe_item::Rarity::Rare) => CraftRarity::Rare,
            _ => CraftRarity::Other,
        };

        let mut mods = Vec::new();
        for group in &resolved.groups {
            let Some(matched) = &group.matched else {
                continue;
            };
            let slot = match matched.mod_type {
                ModType::Prefix => Slot::Prefix,
                ModType::Suffix => Slot::Suffix,
                _ => continue,
            };
            let mut values = Vec::new();
            for line in &group.lines {
                if !line.normalized.rolls.is_empty() {
                    values.extend(line.normalized.rolls.iter().map(|roll| roll.value));
                } else {
                    values.extend(line.normalized.values.iter().copied());
                }
            }
            mods.push(StateMod {
                mod_id: matched.mod_id.clone(),
                slot,
                group: matched.group.clone(),
                affix: matched.affix.clone(),
                tier: matched.tier,
                values,
                percentiles: matched.roll_percentiles.clone(),
                template: group
                    .lines
                    .first()
                    .map(|line| line.normalized.template.clone())
                    .unwrap_or_default(),
            });
        }

        Self {
            rarity,
            item_level: resolved.item.item_level,
            mods,
        }
    }
}
