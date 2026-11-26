//! Goal evaluation over an item state.

use serde::{Deserialize, Serialize};

use crate::plan::{Goal, Requirement};
use crate::state::{ItemState, StateMod};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalEvaluation {
    pub satisfied: bool,
    pub counts_ok: bool,
    pub score: i32,
    pub prefixes: u8,
    pub suffixes: u8,
    pub open_prefixes: u8,
    pub open_suffixes: u8,
    pub required_total: usize,
    pub required_matched: usize,
    pub weighted_total: usize,
    pub weighted_matched: usize,
    pub missing_required: Vec<usize>,
    pub matched_weighted: Vec<usize>,
    pub distinct_failure: bool,
}

pub fn evaluate(goal: &Goal, state: &ItemState) -> GoalEvaluation {
    let prefixes = state.prefix_count();
    let suffixes = state.suffix_count();
    let open_prefixes = state.open_prefixes();
    let open_suffixes = state.open_suffixes();

    let counts_ok = goal.min_prefixes.is_none_or(|value| prefixes >= value)
        && goal.max_prefixes.is_none_or(|value| prefixes <= value)
        && goal.min_suffixes.is_none_or(|value| suffixes >= value)
        && goal.max_suffixes.is_none_or(|value| suffixes <= value)
        && goal
            .min_open_prefixes
            .is_none_or(|value| open_prefixes >= value)
        && goal
            .min_open_suffixes
            .is_none_or(|value| open_suffixes >= value);

    let mut missing_required = Vec::new();
    for (index, requirement) in goal.required.iter().enumerate() {
        if !state
            .mods
            .iter()
            .any(|entry| requirement_matches(requirement, entry))
        {
            missing_required.push(index);
        }
    }

    let search = Search {
        required: &goal.required,
        weighted: &goal.weighted,
        mods: &state.mods,
    };
    let full = search.best();
    let required_matched = search.best_required_count();
    let required_ok = required_matched == goal.required.len();
    let (score, matched_weighted) = match &full {
        Some(result) if required_ok => (result.score, result.matched_weighted.clone()),
        _ => (search.best_partial(), Vec::new()),
    };
    let distinct_failure = !required_ok && missing_required.is_empty();

    let satisfied = counts_ok && required_ok && score >= goal.min_score;

    GoalEvaluation {
        satisfied,
        counts_ok,
        score,
        prefixes,
        suffixes,
        open_prefixes,
        open_suffixes,
        required_total: goal.required.len(),
        required_matched,
        weighted_total: goal.weighted.len(),
        weighted_matched: matched_weighted.len(),
        missing_required,
        matched_weighted,
        distinct_failure,
    }
}

pub fn requirement_matches(requirement: &Requirement, entry: &StateMod) -> bool {
    if requirement.slot != crate::plan::Slot::Any && requirement.slot != entry.slot {
        return false;
    }
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
    if let Some(min_tier) = requirement.min_tier {
        if entry.tier.is_none_or(|tier| tier > min_tier) {
            return false;
        }
    }
    if let Some(min_value) = requirement.min_value {
        if entry
            .value(requirement.value_index)
            .is_none_or(|value| value < min_value)
        {
            return false;
        }
    }
    if let Some(min_percentile) = requirement.min_percentile {
        if entry
            .percentile(requirement.value_index)
            .is_none_or(|value| value < min_percentile)
        {
            return false;
        }
    }
    true
}

struct Search<'a> {
    required: &'a [Requirement],
    weighted: &'a [Requirement],
    mods: &'a [StateMod],
}

#[derive(Debug, Clone)]
struct SearchResult {
    score: i32,
    matched_weighted: Vec<usize>,
}

impl<'a> Search<'a> {
    fn best(&self) -> Option<SearchResult> {
        let mut used = vec![false; self.mods.len()];
        self.assign_required(0, &mut used, 0)
    }

    fn best_required_count(&self) -> usize {
        let mut used = vec![false; self.mods.len()];
        self.required_count(0, &mut used)
    }

    fn required_count(&self, index: usize, used: &mut [bool]) -> usize {
        if index == self.required.len() {
            return 0;
        }
        let mut best = self.required_count(index + 1, used);
        let requirement = &self.required[index];
        for mod_index in 0..self.mods.len() {
            if used[mod_index] || !requirement_matches(requirement, &self.mods[mod_index]) {
                continue;
            }
            used[mod_index] = true;
            best = best.max(1 + self.required_count(index + 1, used));
            used[mod_index] = false;
        }
        best
    }

    /// Best score even when some required constraints are unmet. Used by
    /// strategy decisions such as "regal when a good prefix appears".
    fn best_partial(&self) -> i32 {
        let requirements = self
            .required
            .iter()
            .map(|requirement| (requirement, requirement.weight))
            .chain(
                self.weighted
                    .iter()
                    .map(|requirement| (requirement, requirement.weight)),
            )
            .collect::<Vec<_>>();
        let mut used = vec![false; self.mods.len()];
        self.partial_score(&requirements, 0, &mut used)
    }

    fn partial_score(
        &self,
        requirements: &[(&Requirement, i32)],
        index: usize,
        used: &mut [bool],
    ) -> i32 {
        if index == requirements.len() {
            return 0;
        }
        let mut best = self.partial_score(requirements, index + 1, used);
        let (requirement, weight) = requirements[index];
        for mod_index in 0..self.mods.len() {
            if used[mod_index] || !requirement_matches(requirement, &self.mods[mod_index]) {
                continue;
            }
            used[mod_index] = true;
            let candidate =
                weight.saturating_add(self.partial_score(requirements, index + 1, used));
            used[mod_index] = false;
            best = best.max(candidate);
        }
        best
    }

    fn assign_required(&self, index: usize, used: &mut [bool], score: i32) -> Option<SearchResult> {
        if index == self.required.len() {
            return Some(self.best_weighted(0, used, score, Vec::new()));
        }
        let requirement = &self.required[index];
        let mut best: Option<SearchResult> = None;
        for mod_index in 0..self.mods.len() {
            if used[mod_index] || !requirement_matches(requirement, &self.mods[mod_index]) {
                continue;
            }
            used[mod_index] = true;
            let candidate =
                self.assign_required(index + 1, used, score.saturating_add(requirement.weight));
            used[mod_index] = false;
            if let Some(candidate) = candidate {
                if best
                    .as_ref()
                    .is_none_or(|current| candidate.score > current.score)
                {
                    best = Some(candidate);
                }
            }
        }
        best
    }

    fn best_weighted(
        &self,
        index: usize,
        used: &mut [bool],
        score: i32,
        matched: Vec<usize>,
    ) -> SearchResult {
        if index == self.weighted.len() {
            return SearchResult {
                score,
                matched_weighted: matched,
            };
        }
        let requirement = &self.weighted[index];
        let mut best = self.best_weighted(index + 1, used, score, matched.clone());
        for mod_index in 0..self.mods.len() {
            if used[mod_index] || !requirement_matches(requirement, &self.mods[mod_index]) {
                continue;
            }
            used[mod_index] = true;
            let mut next_matched = matched.clone();
            next_matched.push(index);
            let candidate = self.best_weighted(
                index + 1,
                used,
                score.saturating_add(requirement.weight),
                next_matched,
            );
            used[mod_index] = false;
            if candidate.score > best.score {
                best = candidate;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Goal, Requirement, Slot};
    use crate::state::{CraftRarity, ItemState, StateMod};

    fn entry(group: &str, slot: Slot, tier: u32, value: f64) -> StateMod {
        StateMod {
            mod_id: group.to_string(),
            slot,
            group: group.to_string(),
            affix: String::new(),
            tier: Some(tier),
            values: vec![value],
            percentiles: vec![0.5],
            template: String::new(),
        }
    }

    fn state(rarity: CraftRarity, mods: Vec<StateMod>) -> ItemState {
        ItemState {
            rarity,
            item_level: Some(86),
            mods,
        }
    }

    #[test]
    fn required_and_counts() {
        let goal = Goal {
            min_prefixes: Some(2),
            required: vec![Requirement {
                slot: Slot::Prefix,
                group: Some("IncreasedLife".to_string()),
                affix: None,
                min_tier: Some(2),
                min_value: Some(80.0),
                min_percentile: None,
                value_index: 0,
                weight: 1,
                label: None,
            }],
            weighted: vec![],
            min_score: 0,
            ..Goal::default()
        };
        let item = state(
            CraftRarity::Magic,
            vec![entry("IncreasedLife", Slot::Prefix, 1, 90.0)],
        );
        let result = evaluate(&goal, &item);
        assert!(!result.satisfied);
        assert!(!result.counts_ok);
        assert_eq!(result.required_matched, 1);
        assert_eq!(result.score, 1);

        let item = state(
            CraftRarity::Rare,
            vec![
                entry("IncreasedLife", Slot::Prefix, 1, 90.0),
                entry("Other", Slot::Prefix, 1, 1.0),
            ],
        );
        let result = evaluate(&goal, &item);
        assert!(result.satisfied);
    }

    #[test]
    fn partial_score_does_not_require_all_required() {
        let goal = Goal {
            required: vec![
                Requirement {
                    group: Some("Life".to_string()),
                    ..Requirement::default()
                },
                Requirement {
                    group: Some("EnergyShield".to_string()),
                    ..Requirement::default()
                },
            ],
            ..Goal::default()
        };
        let item = state(
            CraftRarity::Magic,
            vec![entry("Life", Slot::Prefix, 1, 10.0)],
        );
        let result = evaluate(&goal, &item);
        assert!(!result.satisfied);
        assert_eq!(result.required_matched, 1);
        assert_eq!(result.score, 1);
        assert!(!result.missing_required.is_empty());
    }

    #[test]
    fn required_and_weighted_cannot_share_a_mod() {
        let goal = Goal {
            required: vec![Requirement {
                slot: Slot::Prefix,
                group: Some("Life".to_string()),
                min_value: Some(50.0),
                weight: 1,
                ..Requirement::default()
            }],
            weighted: vec![Requirement {
                slot: Slot::Prefix,
                group: Some("Life".to_string()),
                min_value: Some(80.0),
                weight: 10,
                ..Requirement::default()
            }],
            min_score: 10,
            ..Goal::default()
        };
        let item = state(
            CraftRarity::Rare,
            vec![entry("Life", Slot::Prefix, 1, 80.0)],
        );
        let result = evaluate(&goal, &item);
        assert!(!result.satisfied);
        assert!(result.counts_ok);
        assert_eq!(result.required_matched, 1);
        assert_eq!(result.score, 1);
    }

    #[test]
    fn handles_more_than_32_mods() {
        let goal = Goal {
            required: vec![Requirement {
                slot: Slot::Any,
                group: Some("Life".to_string()),
                ..Requirement::default()
            }],
            ..Goal::default()
        };
        let mut mods = (0..39)
            .map(|index| entry(&format!("Group{index}"), Slot::Prefix, 1, 1.0))
            .collect::<Vec<_>>();
        mods.push(entry("Life", Slot::Prefix, 1, 10.0));
        let item = state(CraftRarity::Rare, mods);
        let result = evaluate(&goal, &item);
        assert!(result.satisfied);
        assert_eq!(result.required_matched, 1);
    }

    #[test]
    fn weighted_score_with_distinct_mods() {
        let goal = Goal {
            required: vec![],
            weighted: vec![
                Requirement {
                    slot: Slot::Prefix,
                    group: Some("Life".to_string()),
                    weight: 2,
                    ..Requirement::default()
                },
                Requirement {
                    slot: Slot::Prefix,
                    group: Some("EnergyShield".to_string()),
                    weight: 2,
                    ..Requirement::default()
                },
            ],
            min_score: 3,
            ..Goal::default()
        };
        let item = state(
            CraftRarity::Rare,
            vec![entry("Life", Slot::Prefix, 1, 10.0)],
        );
        let result = evaluate(&goal, &item);
        assert!(!result.satisfied);
        assert_eq!(result.score, 2);

        let item = state(
            CraftRarity::Rare,
            vec![
                entry("Life", Slot::Prefix, 1, 10.0),
                entry("EnergyShield", Slot::Prefix, 1, 10.0),
            ],
        );
        let result = evaluate(&goal, &item);
        assert!(result.satisfied);
        assert_eq!(result.score, 4);
        assert_eq!(result.weighted_matched, 2);
    }

    #[test]
    fn distinctness_is_enforced() {
        let goal = Goal {
            required: vec![
                Requirement {
                    group: Some("Life".to_string()),
                    ..Requirement::default()
                },
                Requirement {
                    group: Some("Life".to_string()),
                    ..Requirement::default()
                },
            ],
            ..Goal::default()
        };
        let item = state(
            CraftRarity::Rare,
            vec![entry("Life", Slot::Prefix, 1, 10.0)],
        );
        let result = evaluate(&goal, &item);
        assert!(!result.satisfied);
        assert_eq!(result.required_matched, 1);
        assert!(result.distinct_failure);
    }
}
