//! Executable step-based build plans (schema 2).

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use poe_data::PobData;

use crate::plan::{
    AugmentMode, BaseSpec, Costs, Goal, Influence, Plan, PlanError, Slot, StartRarity,
};
use crate::pool::{Pool, PoolError};
use crate::state::CraftRarity;

pub const BUILD_SCHEMA: u32 = 2;

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("toml encode error: {0}")]
    TomlEncode(#[from] toml::ser::Error),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Pool(#[from] PoolError),
    #[error("invalid build plan: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildPlan {
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub start: StartRarity,
    pub base: BaseSpec,
    #[serde(default)]
    pub options: BuildOptions,
    pub steps: Vec<PlanStep>,
    #[serde(default)]
    pub costs: Costs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildOptions {
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default = "default_magic_weights")]
    pub magic_mod_weights: [f64; 2],
    #[serde(default = "default_rare_weights")]
    pub rare_mod_weights: [f64; 3],
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            confidence: default_confidence(),
            magic_mod_weights: default_magic_weights(),
            rare_mod_weights: default_rare_weights(),
        }
    }
}

fn default_confidence() -> f64 {
    1.0 - 1.0 / std::f64::consts::E
}

fn default_magic_weights() -> [f64; 2] {
    [1.0, 1.0]
}

fn default_rare_weights() -> [f64; 3] {
    [8.0, 3.0, 1.0]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    pub name: String,
    pub action: StepAction,
    #[serde(default)]
    pub goal: Option<Goal>,
    #[serde(default)]
    pub on_fail: OnFail,
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub augment: AugmentMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepAction {
    Transmute,
    Alt,
    AltAug,
    Augment,
    Regal,
    Alch,
    Scour,
    ScourAlch,
    Chaos,
}

impl StepAction {
    pub fn is_loop(self) -> bool {
        matches!(
            self,
            StepAction::Alt | StepAction::AltAug | StepAction::ScourAlch | StepAction::Chaos
        )
    }

    pub fn output_rarity(self) -> CraftRarity {
        match self {
            StepAction::Transmute | StepAction::Alt | StepAction::AltAug | StepAction::Augment => {
                CraftRarity::Magic
            }
            StepAction::Regal | StepAction::Alch | StepAction::ScourAlch | StepAction::Chaos => {
                CraftRarity::Rare
            }
            StepAction::Scour => CraftRarity::Normal,
        }
    }

    pub fn input_allowed(self, rarity: CraftRarity) -> bool {
        match self {
            StepAction::Transmute | StepAction::Alch => rarity == CraftRarity::Normal,
            StepAction::Augment | StepAction::Regal => rarity == CraftRarity::Magic,
            StepAction::Scour => {
                matches!(
                    rarity,
                    CraftRarity::Normal
                        | CraftRarity::Magic
                        | CraftRarity::Rare
                        | CraftRarity::Other
                )
            }
            StepAction::Alt | StepAction::AltAug | StepAction::ScourAlch | StepAction::Chaos => {
                true
            }
        }
    }

    pub fn caps(self) -> (u8, u8) {
        self.output_rarity().caps()
    }

    pub fn label(self) -> &'static str {
        match self {
            StepAction::Transmute => "transmute",
            StepAction::Alt => "alt",
            StepAction::AltAug => "alt + aug",
            StepAction::Augment => "augment",
            StepAction::Regal => "regal",
            StepAction::Alch => "alchemy",
            StepAction::Scour => "scour",
            StepAction::ScourAlch => "scour + alchemy",
            StepAction::Chaos => "chaos",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnFail {
    #[default]
    Restart,
    Continue,
    Abort,
}

impl BuildPlan {
    pub fn from_toml(source: &str) -> Result<Self, BuildError> {
        let plan: BuildPlan = toml::from_str(source)?;
        plan.validate_shape()?;
        Ok(plan)
    }

    pub fn to_toml(&self) -> Result<String, BuildError> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn validate_shape(&self) -> Result<(), BuildError> {
        let mut errors = Vec::new();
        if self.schema != BUILD_SCHEMA {
            errors.push(format!(
                "unsupported schema {} (expected {BUILD_SCHEMA})",
                self.schema
            ));
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
        if self.steps.is_empty() {
            errors.push("steps must not be empty".to_string());
        }
        if !(0.0..1.0).contains(&self.options.confidence) || self.options.confidence <= 0.0 {
            errors.push("options.confidence must be in 0..1".to_string());
        }
        validate_weights(
            "options.magic_mod_weights",
            &self.options.magic_mod_weights,
            &mut errors,
        );
        validate_weights(
            "options.rare_mod_weights",
            &self.options.rare_mod_weights,
            &mut errors,
        );
        validate_costs(&self.costs, &mut errors);

        let mut current = start_rarity(self.start);
        for (index, step) in self.steps.iter().enumerate() {
            let label = format!("steps[{index}]");
            if step.name.trim().is_empty() {
                errors.push(format!("{label}.name must not be empty"));
            }
            if step.max_attempts == Some(0) {
                errors.push(format!("{label}.max_attempts must be > 0"));
            }
            if step.action.is_loop() && step.goal.is_none() {
                errors.push(format!(
                    "{label} uses a loop action and needs an accept goal"
                ));
            }
            if let Some(goal) = &step.goal {
                Plan::validate_goal_shape(goal, &mut errors);
                validate_goal_for_caps(goal, step.action.caps(), &label, &mut errors);
            }
            if !step.action.input_allowed(current) {
                errors.push(format!(
                    "{label} action {} cannot start from {:?} (input {:?})",
                    step.action.label(),
                    current,
                    current
                ));
            }
            current = step.action.output_rarity();
        }
        if let Some(last) = self.steps.last() {
            if last.goal.is_none() {
                errors.push("the last step needs an accept goal".to_string());
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(BuildError::Invalid(errors.join("; ")))
        }
    }

    /// Validate the base, influence set, and every step goal against the pool.
    pub fn validate_pool(&self, data: &PobData) -> Result<Pool, BuildError> {
        let pool = Pool::from_spec(data, &self.base)?;
        for (index, step) in self.steps.iter().enumerate() {
            if let Some(goal) = &step.goal {
                pool.validate_goal(goal, step.action.caps())
                    .map_err(|error| BuildError::Invalid(format!("steps[{index}]: {error}")))?;
            }
        }
        Ok(pool)
    }

    pub fn validate(&self, data: &PobData) -> Result<BuildSummary, BuildError> {
        self.validate_shape()?;
        let pool = self.validate_pool(data)?;
        Ok(BuildSummary {
            base: pool.base_label.clone(),
            base_tags: pool.base_tags.clone(),
            item_level: pool.item_level,
            prefixes: pool.prefixes.len(),
            suffixes: pool.suffixes.len(),
            steps: self.steps.len(),
        })
    }

    pub fn pool(&self, data: &PobData) -> Result<Pool, BuildError> {
        Pool::from_spec(data, &self.base).map_err(BuildError::from)
    }

    pub fn final_goal(&self) -> Option<&Goal> {
        self.steps.last().and_then(|step| step.goal.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSummary {
    pub base: String,
    pub base_tags: Vec<String>,
    pub item_level: u32,
    pub prefixes: usize,
    pub suffixes: usize,
    pub steps: usize,
}

#[derive(Debug, Clone)]
pub enum PlanFile {
    Legacy(Plan),
    Steps(BuildPlan),
}

impl PlanFile {
    pub fn from_toml(source: &str) -> Result<Self, PlanError> {
        let value: toml::Value = toml::from_str(source)?;
        let schema = value
            .get("schema")
            .and_then(toml::Value::as_integer)
            .unwrap_or(0);
        match schema {
            1 => Plan::from_toml(source).map(PlanFile::Legacy),
            2 => BuildPlan::from_toml(source)
                .map(PlanFile::Steps)
                .map_err(|error| PlanError::Invalid(error.to_string())),
            other => Err(PlanError::Invalid(format!(
                "unsupported schema {other} (expected 1 or 2)"
            ))),
        }
    }

    pub fn load(path: &Path) -> Result<Self, PlanError> {
        let source = std::fs::read_to_string(path).map_err(|error| {
            PlanError::Invalid(format!("cannot read {}: {error}", path.display()))
        })?;
        Self::from_toml(&source)
    }

    pub fn costs(&self) -> &Costs {
        match self {
            PlanFile::Legacy(plan) => &plan.costs,
            PlanFile::Steps(plan) => &plan.costs,
        }
    }

    pub fn as_steps(&self) -> Option<&BuildPlan> {
        match self {
            PlanFile::Steps(plan) => Some(plan),
            PlanFile::Legacy(_) => None,
        }
    }
}

fn start_rarity(rarity: StartRarity) -> CraftRarity {
    match rarity {
        StartRarity::Normal => CraftRarity::Normal,
        StartRarity::Magic => CraftRarity::Magic,
        StartRarity::Rare => CraftRarity::Rare,
    }
}

fn validate_weights<const N: usize>(label: &str, weights: &[f64; N], errors: &mut Vec<String>) {
    let mut total = 0.0;
    for (index, weight) in weights.iter().enumerate() {
        if !weight.is_finite() || *weight < 0.0 {
            errors.push(format!("{label}[{index}] must be finite and >= 0"));
        } else {
            total += *weight;
        }
    }
    if total <= 0.0 {
        errors.push(format!("{label} must have a positive total"));
    }
}

fn validate_costs(costs: &Costs, errors: &mut Vec<String>) {
    for (name, value) in [
        ("transmute", costs.transmute),
        ("alt", costs.alt),
        ("aug", costs.aug),
        ("regal", costs.regal),
        ("alch", costs.alch),
        ("scour", costs.scour),
        ("chaos", costs.chaos),
    ] {
        if !value.is_finite() || value < 0.0 {
            errors.push(format!("costs.{name} must be finite and >= 0"));
        }
    }
}

fn validate_goal_for_caps(goal: &Goal, caps: (u8, u8), label: &str, errors: &mut Vec<String>) {
    let (prefix_cap, suffix_cap) = caps;
    if prefix_cap == 0 && suffix_cap == 0 {
        errors.push(format!("{label}.goal cannot match mods on a normal item"));
        return;
    }
    if goal.min_prefixes.unwrap_or(0) > prefix_cap {
        errors.push(format!("{label}.goal.min_prefixes exceeds {prefix_cap}"));
    }
    if goal.max_prefixes.unwrap_or(prefix_cap) > prefix_cap {
        errors.push(format!("{label}.goal.max_prefixes exceeds {prefix_cap}"));
    }
    if goal.min_suffixes.unwrap_or(0) > suffix_cap {
        errors.push(format!("{label}.goal.min_suffixes exceeds {suffix_cap}"));
    }
    if goal.max_suffixes.unwrap_or(suffix_cap) > suffix_cap {
        errors.push(format!("{label}.goal.max_suffixes exceeds {suffix_cap}"));
    }
    if goal.min_open_prefixes.unwrap_or(0) > prefix_cap {
        errors.push(format!(
            "{label}.goal.min_open_prefixes exceeds {prefix_cap}"
        ));
    }
    if goal.min_open_suffixes.unwrap_or(0) > suffix_cap {
        errors.push(format!(
            "{label}.goal.min_open_suffixes exceeds {suffix_cap}"
        ));
    }
    let required_prefixes = goal
        .required
        .iter()
        .filter(|requirement| requirement.slot == Slot::Prefix)
        .count();
    let required_suffixes = goal
        .required
        .iter()
        .filter(|requirement| requirement.slot == Slot::Suffix)
        .count();
    if required_prefixes > usize::from(prefix_cap) {
        errors.push(format!(
            "{label}.goal has {required_prefixes} required prefixes; cap is {prefix_cap}"
        ));
    }
    if required_suffixes > usize::from(suffix_cap) {
        errors.push(format!(
            "{label}.goal has {required_suffixes} required suffixes; cap is {suffix_cap}"
        ));
    }
    if required_prefixes + usize::from(goal.min_open_prefixes.unwrap_or(0))
        > usize::from(prefix_cap)
    {
        errors.push(format!(
            "{label}.goal requires {required_prefixes} prefixes and {} open prefixes; cap is {prefix_cap}",
            goal.min_open_prefixes.unwrap_or(0)
        ));
    }
    if required_suffixes + usize::from(goal.min_open_suffixes.unwrap_or(0))
        > usize::from(suffix_cap)
    {
        errors.push(format!(
            "{label}.goal requires {required_suffixes} suffixes and {} open suffixes; cap is {suffix_cap}",
            goal.min_open_suffixes.unwrap_or(0)
        ));
    }
}

/// Supported base classes for the phase-1 builder.
pub fn supported_classes(data: &PobData) -> Vec<String> {
    let mut classes = BTreeSet::new();
    for base in &data.bases {
        if is_supported_base(base) {
            classes.insert(base.item_class.clone());
        }
    }
    classes.into_iter().collect()
}

pub fn bases_for_class<'a>(data: &'a PobData, item_class: &str) -> Vec<&'a poe_data::BaseItem> {
    data.bases
        .iter()
        .filter(|base| {
            is_supported_base(base) && crate::pool::class_matches(&base.item_class, item_class)
        })
        .collect()
}

pub fn is_supported_base(base: &poe_data::BaseItem) -> bool {
    if base.item_class.is_empty() {
        return false;
    }
    if base.item_class.eq_ignore_ascii_case("Flask")
        || base.item_class.eq_ignore_ascii_case("Tincture")
    {
        return false;
    }
    if base
        .sub_type
        .as_deref()
        .is_some_and(|sub_type| sub_type.eq_ignore_ascii_case("Cluster"))
    {
        return false;
    }
    true
}

/// Influence keys supported by a concrete base.
pub fn base_influences(base: &poe_data::BaseItem) -> Vec<Influence> {
    Influence::ALL
        .into_iter()
        .filter(|influence| base.influence_tags.contains_key(influence.pob_key()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Requirement;

    fn minimal_plan() -> BuildPlan {
        BuildPlan {
            schema: 2,
            name: "test".to_string(),
            start: StartRarity::Normal,
            base: BaseSpec {
                item_class: Some("Ring".to_string()),
                base_names: vec!["Test Ring".to_string()],
                tags: vec![],
                item_level: 86,
                unrestricted: false,
                influences: vec![],
            },
            options: BuildOptions::default(),
            steps: vec![PlanStep {
                name: "life".to_string(),
                action: StepAction::Alt,
                goal: Some(Goal {
                    min_prefixes: Some(1),
                    required: vec![Requirement {
                        slot: Slot::Prefix,
                        group: Some("Life".to_string()),
                        ..Requirement::default()
                    }],
                    ..Goal::default()
                }),
                on_fail: OnFail::Restart,
                max_attempts: Some(1000),
                augment: AugmentMode::Never,
            }],
            costs: Costs::default(),
        }
    }

    #[test]
    fn parses_schema_two_plan() {
        let plan = minimal_plan();
        let text = plan.to_toml().unwrap();
        let parsed = BuildPlan::from_toml(&text).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn rejects_magic_step_that_needs_required_and_open_prefixes() {
        let mut plan = minimal_plan();
        plan.steps[0].goal.as_mut().unwrap().min_open_prefixes = Some(1);
        assert!(plan.validate_shape().is_err());
    }

    #[test]
    fn rejects_magic_step_with_two_prefixes() {
        let mut plan = minimal_plan();
        plan.steps[0]
            .goal
            .as_mut()
            .unwrap()
            .required
            .push(Requirement {
                slot: Slot::Prefix,
                group: Some("Life2".to_string()),
                ..Requirement::default()
            });
        assert!(plan.validate_shape().is_err());
    }

    #[test]
    fn plan_file_detects_schema() {
        let text = minimal_plan().to_toml().unwrap();
        let file = PlanFile::from_toml(&text).unwrap();
        assert!(file.as_steps().is_some());
    }
}
