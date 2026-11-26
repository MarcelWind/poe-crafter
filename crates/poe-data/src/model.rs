//! Serialized data model for the Path of Building snapshot.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DataVersion {
    pub source: String,
    pub commit: String,
    pub commit_date: String,
    pub files: usize,
    pub built_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ModDomain {
    Explicit,
    Corrupted,
    Delve,
    Eldritch,
    Master,
    Veiled,
    Tincture,
    Flask,
    Jewel,
    JewelAbyss,
    JewelCharm,
    JewelCluster,
    ItemExclusive,
    Map,
    Unknown,
}

impl ModDomain {
    pub fn is_craftable(self) -> bool {
        matches!(
            self,
            ModDomain::Explicit
                | ModDomain::Delve
                | ModDomain::Master
                | ModDomain::Tincture
                | ModDomain::Flask
                | ModDomain::Jewel
                | ModDomain::JewelAbyss
                | ModDomain::JewelCharm
                | ModDomain::JewelCluster
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ModType {
    Prefix,
    Suffix,
    Implicit,
    Corrupted,
    Enchant,
    Unique,
    Veiled,
    Crucible,
    Unknown,
}

impl ModType {
    pub fn from_pob(value: &str) -> Self {
        match value {
            "Prefix" => ModType::Prefix,
            "Suffix" => ModType::Suffix,
            "Implicit" => ModType::Implicit,
            "Corrupted" => ModType::Corrupted,
            "Enchantment" | "Enchant" => ModType::Enchant,
            "Unique" => ModType::Unique,
            "Veiled" => ModType::Veiled,
            "Crucible" => ModType::Crucible,
            "Exarch" | "Eater" | "DelveImplicit" | "SearingExarch" | "EaterOfWorlds" => {
                ModType::Implicit
            }
            _ => ModType::Unknown,
        }
    }

    pub fn is_affix(self) -> bool {
        matches!(self, ModType::Prefix | ModType::Suffix)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeHash {
    pub hash: u64,
    pub texts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mod {
    pub id: String,
    pub domain: ModDomain,
    pub mod_type: ModType,
    pub affix: String,
    pub stat_texts: Vec<String>,
    pub level: i64,
    pub group: String,
    pub weight_keys: Vec<String>,
    pub weight_values: Vec<i32>,
    pub tags: Vec<String>,
    pub trade_hashes: Vec<TradeHash>,
    /// Item classes this mod applies to, when the data provides `types` instead of weights.
    pub item_types: Vec<String>,
}

impl Mod {
    pub fn is_prefix(&self) -> bool {
        self.mod_type == ModType::Prefix
    }

    pub fn is_suffix(&self) -> bool {
        self.mod_type == ModType::Suffix
    }

    pub fn weight_for(&self, tag: &str) -> Option<i32> {
        self.weight_keys
            .iter()
            .position(|key| key == tag)
            .and_then(|index| self.weight_values.get(index).copied())
    }

    /// Whether this mod has a positive chance to spawn on a base with these tags.
    pub fn spawns_on(&self, tags: &[String]) -> bool {
        if !self.item_types.is_empty() {
            return self
                .item_types
                .iter()
                .any(|item_type| tags.iter().any(|tag| tag == item_type));
        }
        if self.weight_keys.is_empty() {
            return false;
        }
        self.weight_keys.iter().enumerate().any(|(index, key)| {
            let value = self.weight_values.get(index).copied().unwrap_or(0);
            value > 0 && (key == "default" || tags.iter().any(|tag| tag == key))
        })
    }

    pub fn primary_text(&self) -> Option<&str> {
        self.stat_texts.first().map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaseItem {
    pub name: String,
    pub item_class: String,
    pub sub_type: Option<String>,
    pub tags: Vec<String>,
    pub implicit_mod_types: Vec<String>,
    pub implicit_ids: Vec<String>,
    pub level_req: Option<i64>,
    pub str_req: Option<i64>,
    pub dex_req: Option<i64>,
    pub int_req: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeStat {
    pub id: String,
    pub text: String,
    pub stat_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Essence {
    pub metadata: String,
    pub name: String,
    pub type_id: i64,
    pub tier: i64,
    pub mods: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PobData {
    pub version: DataVersion,
    pub mods: Vec<Mod>,
    pub bases: Vec<BaseItem>,
    pub trade_stats: Vec<TradeStat>,
    pub essences: Vec<Essence>,
}

impl PobData {
    pub fn mod_by_id(&self, id: &str) -> Option<&Mod> {
        self.mods.iter().find(|entry| entry.id == id)
    }

    pub fn base_by_name(&self, name: &str) -> Option<&BaseItem> {
        self.bases.iter().find(|entry| entry.name == name)
    }
}
