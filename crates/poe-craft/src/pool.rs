//! Eligible explicit mod pool for a base and item level.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use poe_data::{BaseItem, Mod, ModDomain, ModType, PobData};
use poe_item::normalize_mod_text;
use thiserror::Error;

use crate::plan::{BaseSpec, Plan, Requirement, Slot};
use crate::rng::Rng;
use crate::state::StateMod;

#[derive(Debug, Error)]
pub enum PoolError {
    #[error("base not found: {0}")]
    UnknownBase(String),
    #[error("base {name} is not an item class matching {class}")]
    ClassMismatch { name: String, class: String },
    #[error("no base tags resolved; set base_names or tags")]
    NoBaseTags,
    #[error("no eligible prefix or suffix mods for this base and item level")]
    NoMods,
    #[error("{label} cannot match any eligible mod: {reason}")]
    ImpossibleRequirement { label: String, reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatLine {
    pub template: String,
    pub ranges: Vec<(f64, f64)>,
    pub flat: Vec<f64>,
    pub unscalable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoolMod {
    pub mod_id: String,
    pub slot: Slot,
    pub group: String,
    pub affix: String,
    pub level: i64,
    pub tier: u32,
    pub weight: f64,
    pub stat_lines: Vec<StatLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RolledMod {
    pub mod_id: String,
    pub slot: Slot,
    pub group: String,
    pub affix: String,
    pub tier: u32,
    pub values: Vec<f64>,
    pub percentiles: Vec<f64>,
}

impl RolledMod {
    pub fn to_state(&self) -> StateMod {
        StateMod {
            slot: self.slot,
            group: self.group.clone(),
            affix: self.affix.clone(),
            tier: Some(self.tier),
            values: self.values.clone(),
            percentiles: self.percentiles.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pool {
    pub base_label: String,
    pub base_tags: Vec<String>,
    pub item_level: u32,
    pub prefixes: Vec<PoolMod>,
    pub suffixes: Vec<PoolMod>,
    pub prefix_weight: f64,
    pub suffix_weight: f64,
    prefix_cumulative: Vec<f64>,
    suffix_cumulative: Vec<f64>,
    prefix_group_weight: HashMap<String, f64>,
    suffix_group_weight: HashMap<String, f64>,
}

impl Pool {
    pub fn from_plan(data: &PobData, plan: &Plan) -> Result<Self, PoolError> {
        let (base_label, base_tags) = resolve_base(data, &plan.base)?;
        let item_level = plan.base.item_level;

        let mut candidate_mods: Vec<(ModType, &Mod, f64)> = Vec::new();
        for entry in &data.mods {
            if entry.domain != ModDomain::Explicit || !entry.mod_type.is_affix() {
                continue;
            }
            if entry.level > item_level as i64 || entry.stat_texts.is_empty() {
                continue;
            }
            let weight = weight_for(entry, &base_tags);
            if weight <= 0.0 {
                continue;
            }
            candidate_mods.push((entry.mod_type, entry, weight));
        }

        let mut levels: BTreeMap<(ModType, String), BTreeSet<i64>> = BTreeMap::new();
        for (mod_type, entry, _) in &candidate_mods {
            levels
                .entry((*mod_type, entry.group.clone()))
                .or_default()
                .insert(entry.level);
        }

        let mut prefixes = Vec::new();
        let mut suffixes = Vec::new();
        for (mod_type, entry, weight) in candidate_mods {
            let tier = levels
                .get(&(mod_type, entry.group.clone()))
                .and_then(|levels| {
                    levels
                        .iter()
                        .rev()
                        .position(|level| *level == entry.level)
                        .map(|position| position as u32 + 1)
                })
                .unwrap_or(1);
            let stat_lines = entry
                .stat_texts
                .iter()
                .map(|text| parse_stat_line(text))
                .collect::<Vec<_>>();
            let pool_mod = PoolMod {
                mod_id: entry.id.clone(),
                slot: if mod_type == ModType::Prefix {
                    Slot::Prefix
                } else {
                    Slot::Suffix
                },
                group: entry.group.clone(),
                affix: entry.affix.clone(),
                level: entry.level,
                tier,
                weight,
                stat_lines,
            };
            match mod_type {
                ModType::Prefix => prefixes.push(pool_mod),
                ModType::Suffix => suffixes.push(pool_mod),
                _ => {}
            }
        }

        prefixes.sort_by(|left, right| left.mod_id.cmp(&right.mod_id));
        suffixes.sort_by(|left, right| left.mod_id.cmp(&right.mod_id));

        if prefixes.is_empty() && suffixes.is_empty() {
            return Err(PoolError::NoMods);
        }

        let prefix_weight = prefixes.iter().map(|entry| entry.weight).sum();
        let suffix_weight = suffixes.iter().map(|entry| entry.weight).sum();
        let pool = Self {
            base_label,
            base_tags,
            item_level,
            prefix_cumulative: cumulative(&prefixes),
            suffix_cumulative: cumulative(&suffixes),
            prefix_group_weight: group_weights(&prefixes),
            suffix_group_weight: group_weights(&suffixes),
            prefix_weight,
            suffix_weight,
            prefixes,
            suffixes,
        };

        pool.validate_requirements(plan)?;
        Ok(pool)
    }

    pub fn validate_requirements(&self, plan: &Plan) -> Result<(), PoolError> {
        for (index, requirement) in plan.goal.required.iter().enumerate() {
            self.validate_requirement(requirement, &format!("required[{index}]"))?;
        }
        for (index, requirement) in plan.goal.weighted.iter().enumerate() {
            self.validate_requirement(requirement, &format!("weighted[{index}]"))?;
        }
        self.validate_required_layout(plan)
    }

    fn validate_required_layout(&self, plan: &Plan) -> Result<(), PoolError> {
        let mut options: Vec<Vec<Slot>> = Vec::with_capacity(plan.goal.required.len());
        for (index, requirement) in plan.goal.required.iter().enumerate() {
            let mut slots = Vec::new();
            if matches!(requirement.slot, Slot::Prefix | Slot::Any)
                && self
                    .prefixes
                    .iter()
                    .any(|entry| matches_group_affix(requirement, entry))
            {
                slots.push(Slot::Prefix);
            }
            if matches!(requirement.slot, Slot::Suffix | Slot::Any)
                && self
                    .suffixes
                    .iter()
                    .any(|entry| matches_group_affix(requirement, entry))
            {
                slots.push(Slot::Suffix);
            }
            if slots.is_empty() {
                return Err(PoolError::ImpossibleRequirement {
                    label: format!("required[{index}]"),
                    reason: "no eligible prefix or suffix mod".to_string(),
                });
            }
            options.push(slots);
        }

        let max_prefixes = usize::from(plan.goal.max_prefixes.unwrap_or(3)).min(3);
        let max_suffixes = usize::from(plan.goal.max_suffixes.unwrap_or(3)).min(3);
        let min_open_prefixes = usize::from(plan.goal.min_open_prefixes.unwrap_or(0));
        let min_open_suffixes = usize::from(plan.goal.min_open_suffixes.unwrap_or(0));

        let feasible = feasible_required_layout(
            &options,
            0,
            0,
            0,
            max_prefixes,
            max_suffixes,
            min_open_prefixes,
            min_open_suffixes,
        );
        if !feasible {
            return Err(PoolError::ImpossibleRequirement {
                label: "required layout".to_string(),
                reason: "required affixes cannot fit the slot caps".to_string(),
            });
        }
        Ok(())
    }

    fn validate_requirement(
        &self,
        requirement: &Requirement,
        label: &str,
    ) -> Result<(), PoolError> {
        let candidates = self.candidates(requirement.slot);
        let mut matching = candidates
            .iter()
            .filter(|entry| matches_group_affix(requirement, entry))
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return Err(PoolError::ImpossibleRequirement {
                label: label.to_string(),
                reason: "no mod with this slot/group/affix".to_string(),
            });
        }
        if let Some(min_tier) = requirement.min_tier {
            matching.retain(|entry| entry.tier <= min_tier);
            if matching.is_empty() {
                return Err(PoolError::ImpossibleRequirement {
                    label: label.to_string(),
                    reason: format!("no eligible tier <= {min_tier}"),
                });
            }
        }
        if requirement.min_value.is_some() || requirement.min_percentile.is_some() {
            let mut reachable = false;
            for entry in &matching {
                if value_reachable(entry, requirement.value_index, requirement.min_value) {
                    reachable = true;
                    break;
                }
            }
            if !reachable {
                return Err(PoolError::ImpossibleRequirement {
                    label: label.to_string(),
                    reason: "no eligible roll can reach the requested value".to_string(),
                });
            }
        }
        Ok(())
    }

    pub fn candidates(&self, slot: Slot) -> Vec<&PoolMod> {
        match slot {
            Slot::Prefix => self.prefixes.iter().collect(),
            Slot::Suffix => self.suffixes.iter().collect(),
            Slot::Any => self.prefixes.iter().chain(self.suffixes.iter()).collect(),
        }
    }

    pub fn pick<'s>(
        &'s self,
        slot: Slot,
        rng: &mut Rng,
        used_groups: &[&str],
    ) -> Option<&'s PoolMod> {
        let (candidates, cumulative, total) = match slot {
            Slot::Prefix => (&self.prefixes, &self.prefix_cumulative, self.prefix_weight),
            Slot::Suffix => (&self.suffixes, &self.suffix_cumulative, self.suffix_weight),
            Slot::Any => return None,
        };
        if total <= 0.0 || candidates.is_empty() {
            return None;
        }

        // Rejection sampling is cheap because an item has at most six groups.
        for _ in 0..12 {
            let roll = rng.next_f64() * total;
            let index = cumulative
                .partition_point(|value| *value < roll)
                .min(candidates.len() - 1);
            let entry = &candidates[index];
            if !group_used(entry, used_groups) {
                return Some(entry);
            }
        }

        // Fallback for very restrictive group sets.
        let available = candidates
            .iter()
            .filter(|entry| !group_used(entry, used_groups))
            .map(|entry| entry.weight)
            .sum::<f64>();
        if available <= 0.0 {
            return None;
        }
        let mut roll = rng.next_f64() * available;
        for entry in candidates {
            if group_used(entry, used_groups) {
                continue;
            }
            roll -= entry.weight;
            if roll <= 0.0 {
                return Some(entry);
            }
        }
        None
    }

    pub fn choose_slot(
        &self,
        rng: &mut Rng,
        used_groups: &[&str],
        open_prefixes: u8,
        open_suffixes: u8,
    ) -> Option<Slot> {
        let mut prefix_weight = 0.0;
        if open_prefixes > 0 {
            prefix_weight =
                self.prefix_weight - used_weight(&self.prefix_group_weight, used_groups);
        }
        let mut suffix_weight = 0.0;
        if open_suffixes > 0 {
            suffix_weight =
                self.suffix_weight - used_weight(&self.suffix_group_weight, used_groups);
        }
        let total = prefix_weight.max(0.0) + suffix_weight.max(0.0);
        if total <= 0.0 {
            return None;
        }
        if rng.next_f64() * total < prefix_weight.max(0.0) {
            Some(Slot::Prefix)
        } else {
            Some(Slot::Suffix)
        }
    }
}

fn cumulative(entries: &[PoolMod]) -> Vec<f64> {
    let mut total = 0.0;
    entries
        .iter()
        .map(|entry| {
            total += entry.weight;
            total
        })
        .collect()
}

fn group_weights(entries: &[PoolMod]) -> HashMap<String, f64> {
    let mut weights = HashMap::new();
    for entry in entries {
        if !entry.group.is_empty() {
            *weights.entry(entry.group.clone()).or_insert(0.0) += entry.weight;
        }
    }
    weights
}

fn used_weight(weights: &HashMap<String, f64>, used_groups: &[&str]) -> f64 {
    used_groups
        .iter()
        .filter_map(|group| weights.get(*group))
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn feasible_required_layout(
    options: &[Vec<Slot>],
    index: usize,
    prefixes: usize,
    suffixes: usize,
    max_prefixes: usize,
    max_suffixes: usize,
    min_open_prefixes: usize,
    min_open_suffixes: usize,
) -> bool {
    if index == options.len() {
        return prefixes <= max_prefixes
            && suffixes <= max_suffixes
            && 3usize.saturating_sub(prefixes) >= min_open_prefixes
            && 3usize.saturating_sub(suffixes) >= min_open_suffixes;
    }
    for slot in &options[index] {
        match slot {
            Slot::Prefix if prefixes < max_prefixes => {
                if feasible_required_layout(
                    options,
                    index + 1,
                    prefixes + 1,
                    suffixes,
                    max_prefixes,
                    max_suffixes,
                    min_open_prefixes,
                    min_open_suffixes,
                ) {
                    return true;
                }
            }
            Slot::Suffix if suffixes < max_suffixes => {
                if feasible_required_layout(
                    options,
                    index + 1,
                    prefixes,
                    suffixes + 1,
                    max_prefixes,
                    max_suffixes,
                    min_open_prefixes,
                    min_open_suffixes,
                ) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn group_used(entry: &PoolMod, used_groups: &[&str]) -> bool {
    !entry.group.is_empty() && used_groups.contains(&entry.group.as_str())
}

fn matches_group_affix(requirement: &Requirement, entry: &PoolMod) -> bool {
    if let Some(group) = &requirement.group {
        if &entry.group != group {
            return false;
        }
    }
    if let Some(affix) = &requirement.affix {
        if &entry.affix != affix {
            return false;
        }
    }
    true
}

fn value_reachable(entry: &PoolMod, value_index: usize, min_value: Option<f64>) -> bool {
    let Some(min_value) = min_value else {
        return true;
    };
    let mut index = 0usize;
    for line in &entry.stat_lines {
        for (min, max) in &line.ranges {
            if index == value_index {
                return (*max).max(*min) >= min_value;
            }
            index += 1;
        }
        for flat in &line.flat {
            if index == value_index {
                return *flat >= min_value;
            }
            index += 1;
        }
    }
    false
}

fn parse_stat_line(text: &str) -> StatLine {
    let mut text = text.trim().to_string();
    let mut unscalable = false;
    for marker in ["\u{2014} Unscalable Value", "- Unscalable Value"] {
        if let Some(position) = text.find(marker) {
            text.truncate(position);
            text = text.trim_end().to_string();
            unscalable = true;
            break;
        }
    }
    let normalized = normalize_mod_text(&text);
    StatLine {
        template: normalized.template,
        ranges: normalized.ranges,
        flat: normalized.values,
        unscalable,
    }
}

fn weight_for(entry: &Mod, base_tags: &[String]) -> f64 {
    if !entry.item_types.is_empty() {
        return if entry
            .item_types
            .iter()
            .any(|item_type| base_tags.iter().any(|tag| tag == item_type))
        {
            1000.0
        } else {
            0.0
        };
    }
    for (key, value) in entry.weight_keys.iter().zip(&entry.weight_values) {
        if key == "default" || base_tags.iter().any(|tag| tag == key) {
            return (*value).max(0) as f64;
        }
    }
    0.0
}

fn resolve_base(data: &PobData, spec: &BaseSpec) -> Result<(String, Vec<String>), PoolError> {
    if !spec.base_names.is_empty() {
        let mut tags = BTreeSet::new();
        let mut label = String::new();
        for (index, name) in spec.base_names.iter().enumerate() {
            let base: &BaseItem = data
                .base_by_name(name)
                .ok_or_else(|| PoolError::UnknownBase(name.clone()))?;
            if let Some(class) = &spec.item_class {
                if !class_matches(class, &base.item_class) {
                    return Err(PoolError::ClassMismatch {
                        name: name.clone(),
                        class: class.clone(),
                    });
                }
            }
            if index == 0 {
                label = name.clone();
            }
            tags.extend(base.tags.iter().cloned());
        }
        tags.extend(spec.tags.iter().cloned());
        return Ok((label, tags.into_iter().collect()));
    }

    if !spec.tags.is_empty() {
        let label = spec.tags.join("+");
        return Ok((label, spec.tags.clone()));
    }

    Err(PoolError::NoBaseTags)
}

pub fn class_matches(item_class: &str, base_class: &str) -> bool {
    let left = normalize_class(item_class);
    let right = normalize_class(base_class);
    left == right || left.contains(&right) || right.contains(&left)
}

fn normalize_class(value: &str) -> String {
    let mut value = value.trim().to_ascii_lowercase();
    if value.ends_with('s') {
        value.pop();
    }
    value
}

pub fn roll_value(rng: &mut Rng, min: f64, max: f64) -> f64 {
    let min_rounded = min.round();
    let max_rounded = max.round();
    if (min - min_rounded).abs() < 1e-9 && (max - max_rounded).abs() < 1e-9 {
        rng.range_i64(min_rounded as i64, max_rounded as i64) as f64
    } else {
        rng.range_f64(min, max)
    }
}

pub fn percentile(value: f64, min: f64, max: f64) -> f64 {
    let delta = max - min;
    if delta.abs() < f64::EPSILON {
        0.5
    } else {
        ((value - min) / delta).clamp(0.0, 1.0)
    }
}

impl PoolMod {
    fn roll_values(&self, rng: &mut Rng) -> (Vec<f64>, Vec<f64>) {
        let mut values = Vec::new();
        let mut percentiles = Vec::new();
        for line in &self.stat_lines {
            for (min, max) in &line.ranges {
                let value = roll_value(rng, *min, *max);
                values.push(value);
                percentiles.push(percentile(value, *min, *max));
            }
            for flat in &line.flat {
                values.push(*flat);
                percentiles.push(1.0);
            }
        }
        (values, percentiles)
    }

    pub fn roll(&self, rng: &mut Rng) -> RolledMod {
        let (values, percentiles) = self.roll_values(rng);
        RolledMod {
            mod_id: self.mod_id.clone(),
            slot: self.slot,
            group: self.group.clone(),
            affix: self.affix.clone(),
            tier: self.tier,
            values,
            percentiles,
        }
    }

    pub fn roll_state(&self, rng: &mut Rng) -> StateMod {
        let (values, percentiles) = self.roll_values(rng);
        StateMod {
            slot: self.slot,
            group: self.group.clone(),
            affix: self.affix.clone(),
            tier: Some(self.tier),
            values,
            percentiles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{AugmentMode, Goal, Method, Strategy};
    use crate::state::{CraftRarity, ItemState};
    use poe_data::{BaseItem, DataVersion, ModDomain, ModType, PobData};

    fn base() -> BaseItem {
        BaseItem {
            name: "Test Ring".to_string(),
            item_class: "Ring".to_string(),
            sub_type: None,
            tags: vec!["default".to_string(), "ring".to_string()],
            implicit_mod_types: vec![],
            implicit_ids: vec![],
            level_req: None,
            str_req: None,
            dex_req: None,
            int_req: None,
        }
    }

    fn test_mod(
        id: &str,
        mod_type: ModType,
        group: &str,
        affix: &str,
        level: i64,
        text: &str,
        weight: i32,
    ) -> Mod {
        Mod {
            id: id.to_string(),
            domain: ModDomain::Explicit,
            mod_type,
            affix: affix.to_string(),
            stat_texts: vec![text.to_string()],
            level,
            group: group.to_string(),
            weight_keys: vec!["ring".to_string(), "default".to_string()],
            weight_values: vec![weight, 0],
            tags: vec![],
            trade_hashes: vec![],
            item_types: vec![],
        }
    }

    fn data() -> PobData {
        PobData {
            version: DataVersion::default(),
            mods: vec![
                test_mod(
                    "Life1",
                    ModType::Prefix,
                    "Life",
                    "Hale",
                    10,
                    "+(80-90) to maximum Life",
                    1000,
                ),
                test_mod(
                    "Life2",
                    ModType::Prefix,
                    "Life",
                    "Healthy",
                    1,
                    "+(50-60) to maximum Life",
                    1000,
                ),
                test_mod(
                    "Cold1",
                    ModType::Suffix,
                    "ColdRes",
                    "of Haast",
                    10,
                    "+(40-45)% to Cold Resistance",
                    1000,
                ),
                test_mod(
                    "Zero",
                    ModType::Suffix,
                    "ZeroRes",
                    "of Zero",
                    1,
                    "+(1-2)% to Fire Resistance",
                    0,
                ),
            ],
            bases: vec![base()],
            trade_stats: vec![],
            essences: vec![],
        }
    }

    fn plan() -> Plan {
        Plan {
            schema: 1,
            name: "test".to_string(),
            base: BaseSpec {
                item_class: Some("Ring".to_string()),
                base_names: vec!["Test Ring".to_string()],
                tags: vec![],
                item_level: 86,
            },
            goal: Goal {
                min_prefixes: Some(1),
                required: vec![Requirement {
                    group: Some("Life".to_string()),
                    ..Requirement::default()
                }],
                ..Goal::default()
            },
            strategy: Strategy {
                method: Method::AltAugRegal,
                max_attempts: 1000,
                augment: AugmentMode::IfNeeded,
                regal_when_score: 1,
                magic_two_mod_chance: 1.0,
                chaos_mod_counts: vec![4, 5, 6],
                start: crate::plan::StartRarity::Normal,
            },
            costs: Default::default(),
        }
    }

    #[test]
    fn builds_pool_and_tiers() {
        let pool = Pool::from_plan(&data(), &plan()).unwrap();
        assert_eq!(pool.prefixes.len(), 2);
        assert_eq!(pool.suffixes.len(), 1);
        assert_eq!(
            pool.prefixes
                .iter()
                .find(|entry| entry.mod_id == "Life1")
                .unwrap()
                .tier,
            1
        );
        assert_eq!(
            pool.prefixes
                .iter()
                .find(|entry| entry.mod_id == "Life2")
                .unwrap()
                .tier,
            2
        );
        assert!(pool.suffixes.iter().all(|entry| entry.mod_id != "Zero"));
    }

    #[test]
    fn rolls_values_within_ranges() {
        let pool = Pool::from_plan(&data(), &plan()).unwrap();
        let mut rng = Rng::new(7);
        let life = pool
            .prefixes
            .iter()
            .find(|entry| entry.mod_id == "Life1")
            .unwrap();
        for _ in 0..100 {
            let rolled = life.roll(&mut rng);
            assert!((80.0..=90.0).contains(&rolled.values[0]));
            assert!((0.0..=1.0).contains(&rolled.percentiles[0]));
        }
    }

    #[test]
    fn rejects_impossible_group() {
        let mut plan = plan();
        plan.goal.required[0].group = Some("DoesNotExist".to_string());
        assert!(Pool::from_plan(&data(), &plan).is_err());
    }

    #[test]
    fn rejects_required_layout_that_cannot_fit_slots() {
        let mut plan = plan();
        plan.goal.required = (0..4)
            .map(|_| Requirement {
                slot: Slot::Any,
                group: Some("Life".to_string()),
                ..Requirement::default()
            })
            .collect();
        assert!(Pool::from_plan(&data(), &plan).is_err());

        plan.goal.required = vec![Requirement {
            slot: Slot::Any,
            group: Some("ColdRes".to_string()),
            ..Requirement::default()
        }];
        assert!(Pool::from_plan(&data(), &plan).is_ok());
    }

    #[test]
    fn picks_avoid_used_groups() {
        let pool = Pool::from_plan(&data(), &plan()).unwrap();
        let mut rng = Rng::new(1);
        let picked = pool.pick(Slot::Prefix, &mut rng, &[]).unwrap();
        let used = vec![picked.group.as_str()];
        let next = pool.pick(Slot::Prefix, &mut rng, &used);
        assert!(next.is_none());
    }

    #[test]
    fn state_counts_follow_rarity() {
        let mut state = ItemState::new(CraftRarity::Magic, Some(86));
        assert_eq!(state.open_prefixes(), 1);
        assert_eq!(state.open_suffixes(), 1);
        state.mods.push(
            RolledMod {
                mod_id: "Life1".to_string(),
                slot: Slot::Prefix,
                group: "Life".to_string(),
                affix: "Hale".to_string(),
                tier: 1,
                values: vec![85.0],
                percentiles: vec![0.5],
            }
            .to_state(),
        );
        assert_eq!(state.open_prefixes(), 0);
        assert_eq!(state.open_suffixes(), 1);
    }
}
