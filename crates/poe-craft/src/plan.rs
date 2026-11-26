//! Declarative crafting plan schema and validation.

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_REQUIREMENT_WEIGHT: i32 = 1_000_000;
const MAX_WEIGHTED_REQUIREMENTS: usize = 12;

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid plan: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema: u32,
    pub name: String,
    pub base: BaseSpec,
    pub goal: Goal,
    pub strategy: Strategy,
    #[serde(default)]
    pub costs: Costs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseSpec {
    #[serde(default)]
    pub item_class: Option<String>,
    #[serde(default)]
    pub base_names: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub item_level: u32,
    /// Ignore the item level when building the mod pool.
    #[serde(default)]
    pub unrestricted: bool,
    /// Classic influences. At most two are supported.
    #[serde(default)]
    pub influences: Vec<Influence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Influence {
    Shaper,
    Elder,
    Crusader,
    Redeemer,
    Hunter,
    Warlord,
}

impl Influence {
    pub fn pob_key(self) -> &'static str {
        match self {
            Influence::Shaper => "shaper",
            Influence::Elder => "elder",
            Influence::Crusader => "crusader",
            Influence::Redeemer => "eyrie",
            Influence::Hunter => "basilisk",
            Influence::Warlord => "adjudicator",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Influence::Shaper => "Shaper",
            Influence::Elder => "Elder",
            Influence::Crusader => "Crusader",
            Influence::Redeemer => "Redeemer",
            Influence::Hunter => "Hunter",
            Influence::Warlord => "Warlord",
        }
    }

    pub const ALL: [Influence; 6] = [
        Influence::Shaper,
        Influence::Elder,
        Influence::Crusader,
        Influence::Redeemer,
        Influence::Hunter,
        Influence::Warlord,
    ];
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    #[serde(default)]
    pub min_prefixes: Option<u8>,
    #[serde(default)]
    pub max_prefixes: Option<u8>,
    #[serde(default)]
    pub min_suffixes: Option<u8>,
    #[serde(default)]
    pub max_suffixes: Option<u8>,
    #[serde(default)]
    pub min_open_prefixes: Option<u8>,
    #[serde(default)]
    pub min_open_suffixes: Option<u8>,
    #[serde(default)]
    pub required: Vec<Requirement>,
    #[serde(default)]
    pub weighted: Vec<Requirement>,
    #[serde(default)]
    pub min_score: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    #[serde(default)]
    pub slot: Slot,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub affix: Option<String>,
    #[serde(default)]
    pub min_tier: Option<u32>,
    #[serde(default)]
    pub min_value: Option<f64>,
    #[serde(default)]
    pub min_percentile: Option<f64>,
    #[serde(default)]
    pub value_index: usize,
    #[serde(default = "default_weight")]
    pub weight: i32,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    #[default]
    Any,
    Prefix,
    Suffix,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strategy {
    pub method: Method,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub augment: AugmentMode,
    #[serde(default)]
    pub regal_when_score: i32,
    #[serde(default = "default_two_mod_chance")]
    pub magic_two_mod_chance: f64,
    #[serde(default = "default_chaos_counts")]
    pub chaos_mod_counts: Vec<u8>,
    #[serde(default)]
    pub start: StartRarity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    AltAugRegal,
    AltRegal,
    Chaos,
    AlchScour,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AugmentMode {
    Never,
    #[default]
    IfNeeded,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartRarity {
    #[default]
    Normal,
    Magic,
    Rare,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Costs {
    #[serde(default)]
    pub transmute: f64,
    #[serde(default)]
    pub alt: f64,
    #[serde(default)]
    pub aug: f64,
    #[serde(default)]
    pub regal: f64,
    #[serde(default)]
    pub alch: f64,
    #[serde(default)]
    pub scour: f64,
    #[serde(default)]
    pub chaos: f64,
}

impl Default for Costs {
    fn default() -> Self {
        Self {
            transmute: 0.0,
            alt: 0.0,
            aug: 0.0,
            regal: 0.0,
            alch: 0.0,
            scour: 0.0,
            chaos: 0.0,
        }
    }
}

impl Default for Requirement {
    fn default() -> Self {
        Self {
            slot: Slot::Any,
            group: None,
            affix: None,
            min_tier: None,
            min_value: None,
            min_percentile: None,
            value_index: 0,
            weight: default_weight(),
            label: None,
        }
    }
}

fn default_weight() -> i32 {
    1
}

fn default_max_attempts() -> u32 {
    10_000
}

fn default_two_mod_chance() -> f64 {
    0.5
}

fn default_chaos_counts() -> Vec<u8> {
    vec![4, 5, 6]
}

impl Plan {
    pub fn from_toml(source: &str) -> Result<Self, PlanError> {
        let plan: Plan = toml::from_str(source)?;
        plan.validate_shape()?;
        Ok(plan)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, PlanError> {
        let source = std::fs::read_to_string(path).map_err(|error| {
            PlanError::Invalid(format!("cannot read {}: {error}", path.display()))
        })?;
        Self::from_toml(&source)
    }

    pub fn validate_shape(&self) -> Result<(), PlanError> {
        let mut errors = Vec::new();

        if self.schema != 1 {
            errors.push(format!("unsupported schema {} (expected 1)", self.schema));
        }
        if self.name.trim().is_empty() {
            errors.push("name must not be empty".to_string());
        }
        if !(1..=100).contains(&self.base.item_level) {
            errors.push(format!(
                "item_level {} is outside 1..=100",
                self.base.item_level
            ));
        }
        if self.base.base_names.is_empty() && self.base.tags.is_empty() {
            errors.push("base must set base_names or tags".to_string());
        }
        if self.base.influences.len() > 2 {
            errors.push("base.influences supports at most two influences".to_string());
        }
        for (index, influence) in self.base.influences.iter().enumerate() {
            if self.base.influences[..index].contains(influence) {
                errors.push(format!(
                    "base.influences contains duplicate {}",
                    influence.display()
                ));
            }
        }
        if !self.base.influences.is_empty() && self.base.base_names.is_empty() {
            errors.push("base.influences requires base_names".to_string());
        }

        self.validate_goal(&mut errors);
        self.validate_strategy(&mut errors);
        self.validate_costs(&mut errors);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(PlanError::Invalid(errors.join("; ")))
        }
    }

    fn validate_goal(&self, errors: &mut Vec<String>) {
        Self::validate_goal_shape(&self.goal, errors);
    }

    pub(crate) fn validate_goal_shape(goal: &Goal, errors: &mut Vec<String>) {
        let has_minimum = !goal.required.is_empty()
            || goal.min_score > 0
            || goal.min_prefixes.unwrap_or(0) > 0
            || goal.min_suffixes.unwrap_or(0) > 0
            || goal.min_open_prefixes.unwrap_or(0) > 0
            || goal.min_open_suffixes.unwrap_or(0) > 0;
        if !has_minimum {
            errors.push("goal has no minimum requirement".to_string());
        }

        if goal.min_score < 0 {
            errors.push("goal.min_score must be >= 0".to_string());
        }
        if goal.weighted.len() > MAX_WEIGHTED_REQUIREMENTS {
            errors.push(format!(
                "weighted has {} entries; at most {MAX_WEIGHTED_REQUIREMENTS} are supported",
                goal.weighted.len()
            ));
        }

        for (index, requirement) in goal.required.iter().enumerate() {
            validate_requirement(requirement, &format!("required[{index}]"), errors);
        }
        for (index, requirement) in goal.weighted.iter().enumerate() {
            validate_requirement(requirement, &format!("weighted[{index}]"), errors);
            if requirement.weight <= 0 {
                errors.push(format!("weighted[{index}].weight must be > 0"));
            }
        }

        match max_reachable_score(goal) {
            Some(max_score) if max_score > i64::from(i32::MAX) => {
                errors.push("total requirement weight exceeds i32 score range".to_string());
            }
            Some(max_score) if i64::from(goal.min_score) > max_score => {
                errors.push(format!(
                    "goal.min_score {} exceeds reachable score {}",
                    goal.min_score, max_score
                ));
            }
            None => {
                errors.push("total requirement weight overflows".to_string());
            }
            _ => {}
        }

        if goal.max_prefixes.is_some_and(|value| value > 3) {
            errors.push("goal.max_prefixes exceeds 3".to_string());
        }
        if goal.max_suffixes.is_some_and(|value| value > 3) {
            errors.push("goal.max_suffixes exceeds 3".to_string());
        }

        validate_count_pair(goal.min_prefixes, goal.max_prefixes, "prefixes", errors);
        validate_count_pair(goal.min_suffixes, goal.max_suffixes, "suffixes", errors);

        let min_prefixes = u32::from(goal.min_prefixes.unwrap_or(0));
        let min_suffixes = u32::from(goal.min_suffixes.unwrap_or(0));
        let min_open_prefixes = u32::from(goal.min_open_prefixes.unwrap_or(0));
        let min_open_suffixes = u32::from(goal.min_open_suffixes.unwrap_or(0));

        if min_prefixes > 3 {
            errors.push("goal.min_prefixes exceeds 3".to_string());
        }
        if min_suffixes > 3 {
            errors.push("goal.min_suffixes exceeds 3".to_string());
        }
        if min_prefixes + min_open_prefixes > 3 {
            errors.push("min_prefixes + min_open_prefixes exceeds 3".to_string());
        }
        if min_suffixes + min_open_suffixes > 3 {
            errors.push("min_suffixes + min_open_suffixes exceeds 3".to_string());
        }
        if goal.required.len() > 6 {
            errors.push(format!(
                "required has {} entries; a rare item has at most 6 affixes",
                goal.required.len()
            ));
        }

        let required_prefixes = goal
            .required
            .iter()
            .filter(|requirement| requirement.slot == Slot::Prefix)
            .count() as u32;
        let required_suffixes = goal
            .required
            .iter()
            .filter(|requirement| requirement.slot == Slot::Suffix)
            .count() as u32;
        let max_prefixes = u32::from(goal.max_prefixes.unwrap_or(3));
        let max_suffixes = u32::from(goal.max_suffixes.unwrap_or(3));

        if required_prefixes > 3 {
            errors.push(format!(
                "required has {required_prefixes} prefix entries; a rare item has at most 3"
            ));
        }
        if required_suffixes > 3 {
            errors.push(format!(
                "required has {required_suffixes} suffix entries; a rare item has at most 3"
            ));
        }
        if required_prefixes > max_prefixes {
            errors.push(format!(
                "required prefixes {required_prefixes} exceed max_prefixes {max_prefixes}"
            ));
        }
        if required_suffixes > max_suffixes {
            errors.push(format!(
                "required suffixes {required_suffixes} exceed max_suffixes {max_suffixes}"
            ));
        }
        if required_prefixes + min_open_prefixes > 3 {
            errors.push("required prefixes + min_open_prefixes exceeds 3".to_string());
        }
        if required_suffixes + min_open_suffixes > 3 {
            errors.push("required suffixes + min_open_suffixes exceeds 3".to_string());
        }
    }

    fn validate_strategy(&self, errors: &mut Vec<String>) {
        let strategy = &self.strategy;
        if strategy.max_attempts == 0 {
            errors.push("strategy.max_attempts must be > 0".to_string());
        }
        if !(0.0..=1.0).contains(&strategy.magic_two_mod_chance) {
            errors.push("strategy.magic_two_mod_chance must be in 0..=1".to_string());
        }
        for count in &strategy.chaos_mod_counts {
            if !(1..=6).contains(count) {
                errors.push(format!("chaos_mod_counts value {count} is outside 1..=6"));
            }
        }
        if strategy.chaos_mod_counts.is_empty() {
            errors.push("strategy.chaos_mod_counts must not be empty".to_string());
        }
    }

    fn validate_costs(&self, errors: &mut Vec<String>) {
        let values = [
            ("transmute", self.costs.transmute),
            ("alt", self.costs.alt),
            ("aug", self.costs.aug),
            ("regal", self.costs.regal),
            ("alch", self.costs.alch),
            ("scour", self.costs.scour),
            ("chaos", self.costs.chaos),
        ];
        for (name, value) in values {
            if !value.is_finite() || value < 0.0 {
                errors.push(format!("costs.{name} must be finite and >= 0"));
            }
        }
    }
}

/// Upper bound for a rare item with at most six affixes. Required entries
/// consume slots first; only the highest remaining weighted entries can add
/// score.
fn max_reachable_score(goal: &Goal) -> Option<i64> {
    let required = goal.required.iter().try_fold(0i64, |total, requirement| {
        total.checked_add(i64::from(requirement.weight))
    })?;
    let remaining_slots = 6usize.saturating_sub(goal.required.len());
    let mut weights = goal
        .weighted
        .iter()
        .map(|requirement| requirement.weight)
        .collect::<Vec<_>>();
    weights.sort_unstable_by(|left, right| right.cmp(left));
    let weighted = weights
        .iter()
        .take(remaining_slots)
        .try_fold(0i64, |total, weight| total.checked_add(i64::from(*weight)))?;
    required.checked_add(weighted)
}

fn validate_requirement(requirement: &Requirement, label: &str, errors: &mut Vec<String>) {
    if requirement.group.is_none() && requirement.affix.is_none() {
        errors.push(format!("{label} needs group or affix"));
    }
    if requirement.weight < 0 {
        errors.push(format!("{label}.weight must be >= 0"));
    }
    if requirement.weight > MAX_REQUIREMENT_WEIGHT {
        errors.push(format!(
            "{label}.weight must be <= {MAX_REQUIREMENT_WEIGHT}"
        ));
    }
    if let Some(tier) = requirement.min_tier {
        if tier == 0 {
            errors.push(format!("{label}.min_tier must be >= 1"));
        }
    }
    if let Some(percentile) = requirement.min_percentile {
        if !(0.0..=1.0).contains(&percentile) {
            errors.push(format!("{label}.min_percentile must be in 0..=1"));
        }
    }
    if let Some(value) = requirement.min_value {
        if !value.is_finite() {
            errors.push(format!("{label}.min_value must be finite"));
        }
    }
}

fn validate_count_pair(min: Option<u8>, max: Option<u8>, name: &str, errors: &mut Vec<String>) {
    if let (Some(min), Some(max)) = (min, max) {
        if min > max {
            errors.push(format!("goal.min_{name} exceeds max_{name}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_plan() {
        let plan = Plan::from_toml(
            r#"
schema = 1
name = "boots"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_prefixes = 2

[[goal.required]]
slot = "prefix"
group = "IncreasedLife"

[[goal.weighted]]
slot = "suffix"
group = "ColdResistance"
weight = 2

[strategy]
method = "alt_aug_regal"
regal_when_score = 1

[costs]
alt = 0.05
"#,
        )
        .unwrap();
        assert_eq!(plan.base.item_level, 86);
        assert_eq!(plan.strategy.max_attempts, 10_000);
        assert_eq!(plan.strategy.augment, AugmentMode::IfNeeded);
        assert_eq!(plan.costs.alt, 0.05);
    }

    #[test]
    fn rejects_unknown_fields() {
        let result = Plan::from_toml(
            r#"
schema = 1
name = "x"
unknown = true

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_prefixes = 1

[[goal.required]]
group = "IncreasedLife"

[strategy]
method = "alt_aug_regal"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_impossible_counts() {
        let result = Plan::from_toml(
            r#"
schema = 1
name = "x"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_prefixes = 3
min_open_prefixes = 1

[[goal.required]]
group = "IncreasedLife"

[strategy]
method = "alt_aug_regal"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn required_weights_count_toward_min_score() {
        let plan = Plan::from_toml(
            r#"
schema = 1
name = "required score"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_score = 1

[[goal.required]]
group = "IncreasedLife"

[strategy]
method = "alt_aug_regal"
"#,
        )
        .unwrap();
        assert_eq!(plan.goal.min_score, 1);
    }

    #[test]
    fn rejects_required_prefix_overflow() {
        let result = Plan::from_toml(
            r#"
schema = 1
name = "four prefixes"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_prefixes = 4

[[goal.required]]
slot = "prefix"
group = "IncreasedLife"

[[goal.required]]
slot = "prefix"
group = "LocalEnergyShieldPercent"

[[goal.required]]
slot = "prefix"
group = "LocalEnergyShield"

[[goal.required]]
slot = "prefix"
group = "IncreasedMana"

[strategy]
method = "alt_aug_regal"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_required_suffixes_over_max() {
        let result = Plan::from_toml(
            r#"
schema = 1
name = "too many suffixes"

[base]
base_names = ["Amethyst Ring"]
item_level = 86

[goal]
max_suffixes = 1

[[goal.required]]
slot = "suffix"
group = "ColdResistance"

[[goal.required]]
slot = "suffix"
group = "FireResistance"

[strategy]
method = "chaos"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_too_many_weighted_requirements() {
        let mut source = r#"
schema = 1
name = "too many weighted"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_score = 1

[strategy]
method = "alt_aug_regal"
"#
        .to_string();
        for index in 0..=MAX_WEIGHTED_REQUIREMENTS {
            source.push_str(&format!(
                "\n[[goal.weighted]]\ngroup = \"Group{index}\"\nweight = 1\n"
            ));
        }
        assert!(Plan::from_toml(&source).is_err());
    }

    #[test]
    fn rejects_extreme_counts_without_panicking() {
        let result = Plan::from_toml(
            r#"
schema = 1
name = "extreme counts"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_prefixes = 200
min_open_prefixes = 200

[[goal.required]]
group = "IncreasedLife"

[strategy]
method = "alt_aug_regal"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_extreme_weights_without_panicking() {
        let result = Plan::from_toml(
            r#"
schema = 1
name = "extreme weights"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_score = 1

[[goal.weighted]]
group = "IncreasedLife"
weight = 2000000

[[goal.weighted]]
group = "ColdResistance"
weight = 2000000

[strategy]
method = "alt_aug_regal"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_min_score_above_distinct_affix_capacity() {
        let mut source = r#"
schema = 1
name = "unreachable score"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_score = 100
max_prefixes = 3
max_suffixes = 3
"#
        .to_string();
        for index in 0..6 {
            source.push_str(&format!("\n[[goal.required]]\ngroup = \"Group{index}\"\n"));
        }
        source.push_str(
            "\n[[goal.weighted]]\ngroup = \"Weighted\"\nweight = 100\n\n[strategy]\nmethod = \"alt_aug_regal\"\n",
        );
        assert!(Plan::from_toml(&source).is_err());
    }

    #[test]
    fn rejects_max_counts_above_three() {
        let result = Plan::from_toml(
            r#"
schema = 1
name = "bad max counts"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
max_prefixes = 4

[[goal.required]]
group = "IncreasedLife"

[strategy]
method = "alt_aug_regal"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn example_plans_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plans");
        let entries = std::fs::read_dir(dir).unwrap();
        let mut count = 0;
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("toml") {
                let text = std::fs::read_to_string(&path).unwrap();
                crate::build::PlanFile::from_toml(&text)
                    .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
                count += 1;
            }
        }
        assert!(count > 0);
    }

    #[test]
    fn rejects_missing_goal_minimum() {
        let result = Plan::from_toml(
            r#"
schema = 1
name = "x"

[base]
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
max_prefixes = 1

[strategy]
method = "alt_aug_regal"
"#,
        );
        assert!(result.is_err());
    }
}
