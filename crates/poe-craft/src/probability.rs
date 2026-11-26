//! Step probability estimates and confidence-attempt math.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::build::{BuildPlan, StepAction};
use crate::plan::{Requirement, StartRarity};
use crate::pool::{Pool, PoolMod};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateSource {
    Analytic,
    Extrapolated,
    MonteCarlo,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepEstimate {
    pub step_index: usize,
    pub p_success: Option<f64>,
    pub source: EstimateSource,
    pub expected_attempts: Option<f64>,
    pub confidence_attempts: Option<u64>,
    pub actions: u64,
    pub accepts: u64,
    pub note: Option<String>,
}

impl StepEstimate {
    pub fn from_probability(
        step_index: usize,
        p_success: f64,
        source: EstimateSource,
        confidence: f64,
        note: Option<String>,
    ) -> Self {
        let p = if p_success.is_finite() {
            p_success.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            step_index,
            p_success: Some(p),
            source,
            expected_attempts: expected_attempts(p),
            confidence_attempts: attempts_for_confidence(p, confidence),
            actions: 0,
            accepts: 0,
            note,
        }
    }
}

pub fn expected_attempts(p_success: f64) -> Option<f64> {
    if p_success <= 0.0 {
        None
    } else {
        Some(1.0 / p_success)
    }
}

/// Number of independent attempts needed for at least `confidence`
/// probability of at least one success.
pub fn attempts_for_confidence(p_success: f64, confidence: f64) -> Option<u64> {
    if p_success <= 0.0 || confidence <= 0.0 || confidence >= 1.0 {
        return None;
    }
    if p_success >= 1.0 {
        return Some(1);
    }
    let attempts = ((1.0 - confidence).ln() / (1.0 - p_success).ln()).ceil();
    if !attempts.is_finite() || attempts < 1.0 {
        None
    } else if attempts > u64::MAX as f64 {
        Some(u64::MAX)
    } else {
        Some(attempts as u64)
    }
}

/// Analytic estimates for steps that are simple enough: first step, one
/// concrete required mod, no weighted/min-score goal, and no conditional
/// incoming state beyond the plan start.
pub fn analytic_estimates(plan: &BuildPlan, pool: &Pool) -> Vec<Option<StepEstimate>> {
    let mut estimates = vec![None; plan.steps.len()];
    let Some(step) = plan.steps.first() else {
        return estimates;
    };
    let Some(goal) = step.goal.as_ref() else {
        return estimates;
    };
    if !goal.weighted.is_empty()
        || goal.min_score != 0
        || goal.required.len() != 1
        || goal.min_prefixes.is_some()
        || goal.max_prefixes.is_some()
        || goal.min_suffixes.is_some()
        || goal.max_suffixes.is_some()
        || goal.min_open_prefixes.is_some()
        || goal.min_open_suffixes.is_some()
    {
        return estimates;
    }
    let Some(requirement) = goal.required.first() else {
        return estimates;
    };
    let counts = match step.action {
        StepAction::Transmute | StepAction::Alt => &plan.options.magic_mod_weights[..],
        StepAction::Alch | StepAction::ScourAlch | StepAction::Chaos => {
            &plan.options.rare_mod_weights[..]
        }
        StepAction::Augment | StepAction::Regal | StepAction::Scour | StepAction::AltAug => {
            return estimates;
        }
    };
    let first_count = match step.action {
        StepAction::Transmute | StepAction::Alt => 1,
        _ => 4,
    };
    if !start_supports(plan.start, step.action) {
        return estimates;
    }
    let Some(target) = target_stats(pool, requirement) else {
        return estimates;
    };
    if target.group_total <= 0.0 || pool.total_weight() <= 0.0 {
        return estimates;
    }
    let mut p = 0.0;
    let total_count_weight = counts.iter().sum::<f64>();
    if total_count_weight <= 0.0 {
        return estimates;
    }
    let mut exact = true;
    for (index, count_weight) in counts.iter().enumerate() {
        if *count_weight <= 0.0 {
            continue;
        }
        let draws = first_count + index;
        let hit =
            target.hit_weight / target.group_total * group_hit_probability(pool, &target, draws);
        if draws > 2 {
            exact = false;
        }
        p += count_weight / total_count_weight * hit;
    }
    estimates[0] = Some(StepEstimate::from_probability(
        0,
        p,
        if exact {
            EstimateSource::Analytic
        } else {
            EstimateSource::Extrapolated
        },
        plan.options.confidence,
        (!exact).then_some(
            "approximate: filler group removal uses an average for 3+ draws".to_string(),
        ),
    ));
    estimates
}

fn start_supports(start: StartRarity, action: StepAction) -> bool {
    match action {
        StepAction::Transmute | StepAction::Alch => start == StartRarity::Normal,
        StepAction::Alt | StepAction::AltAug => start != StartRarity::Rare,
        StepAction::ScourAlch => true,
        StepAction::Chaos => start != StartRarity::Magic,
        StepAction::Augment | StepAction::Regal => start == StartRarity::Magic,
        StepAction::Scour => start != StartRarity::Normal,
    }
}

struct TargetStats {
    hit_weight: f64,
    group: String,
    group_total: f64,
}

fn target_stats(pool: &Pool, requirement: &Requirement) -> Option<TargetStats> {
    let candidates = pool.candidates(requirement.slot);
    let mut hit_weight = 0.0;
    let mut groups = BTreeSet::new();
    for entry in candidates {
        if !matches_group_affix(requirement, entry) {
            continue;
        }
        groups.insert(entry.group.clone());
        hit_weight += entry.weight * tier_success_fraction(entry, requirement);
    }
    if hit_weight <= 0.0 || groups.len() != 1 {
        return None;
    }
    let group = groups.into_iter().next()?;
    let group_total = group_weight(pool, &group);
    Some(TargetStats {
        hit_weight,
        group,
        group_total,
    })
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

fn tier_success_fraction(entry: &PoolMod, requirement: &Requirement) -> f64 {
    if requirement.min_tier.is_some_and(|tier| entry.tier > tier) {
        return 0.0;
    }
    let mut index = 0usize;
    for line in &entry.stat_lines {
        for (min, max) in &line.ranges {
            if index == requirement.value_index {
                if *max <= *min {
                    let value_ok = requirement.min_value.is_none_or(|value| *max >= value);
                    // Fixed ranges always roll percentile 0.5 (see Pool::percentile).
                    let percentile_ok = requirement.min_percentile.is_none_or(|value| 0.5 >= value);
                    return if value_ok && percentile_ok { 1.0 } else { 0.0 };
                }
                let mut threshold = *min;
                if let Some(min_value) = requirement.min_value {
                    threshold = threshold.max(min_value);
                }
                if let Some(percentile) = requirement.min_percentile {
                    threshold = threshold.max(min + (max - min) * percentile);
                }
                return ((max - threshold) / (max - min)).clamp(0.0, 1.0);
            }
            index += 1;
        }
        for flat in &line.flat {
            if index == requirement.value_index {
                let value_ok = requirement.min_value.is_none_or(|value| *flat >= value);
                let percentile_ok = requirement
                    .min_percentile
                    .is_none_or(|percentile| 1.0 >= percentile);
                return if value_ok && percentile_ok { 1.0 } else { 0.0 };
            }
            index += 1;
        }
    }
    0.0
}

fn group_hit_probability(pool: &Pool, target: &TargetStats, draws: usize) -> f64 {
    let total = pool.total_weight();
    if draws == 0 || total <= 0.0 || target.group_total <= 0.0 {
        return 0.0;
    }
    if draws == 1 {
        return (target.group_total / total).clamp(0.0, 1.0);
    }
    if draws == 2 {
        // Exact for one target group and one filler group, ignoring side caps.
        let mut no_target = 0.0;
        for entry in pool.prefixes.iter().chain(pool.suffixes.iter()) {
            if entry.group == target.group {
                continue;
            }
            let conflict = group_weight(pool, &entry.group);
            let remaining = total - conflict;
            if remaining <= 0.0 {
                continue;
            }
            no_target += entry.weight / total * (1.0 - target.group_total / remaining);
        }
        return (1.0 - no_target).clamp(0.0, 1.0);
    }
    // CoE-style extrapolation: remove one average filler group per draw.
    let group_count = pool
        .prefixes
        .iter()
        .chain(pool.suffixes.iter())
        .map(|entry| entry.group.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let filler = if group_count > 1 {
        (total - target.group_total) / (group_count - 1) as f64
    } else {
        0.0
    };
    let mut remaining = total;
    let mut no_target = 1.0;
    for _ in 0..draws {
        if remaining <= 0.0 {
            return 1.0;
        }
        no_target *= (1.0 - target.group_total / remaining).clamp(0.0, 1.0);
        remaining = (remaining - filler).max(0.0);
    }
    (1.0 - no_target).clamp(0.0, 1.0)
}

fn group_weight(pool: &Pool, group: &str) -> f64 {
    pool.prefixes
        .iter()
        .chain(pool.suffixes.iter())
        .filter(|entry| entry.group == group)
        .map(|entry| entry.weight)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_attempts_edge_cases() {
        assert_eq!(attempts_for_confidence(0.0, 0.632), None);
        assert_eq!(attempts_for_confidence(1.0, 0.632), Some(1));
        assert_eq!(attempts_for_confidence(0.5, 1.0), None);
    }

    #[test]
    fn default_confidence_matches_one_over_p_approximately() {
        let confidence = 1.0 - 1.0 / std::f64::consts::E;
        for p in [0.01, 0.05, 0.1, 0.5] {
            let attempts = attempts_for_confidence(p, confidence).unwrap() as f64;
            let expected = 1.0 / p;
            assert!((attempts - expected).abs() <= 1.0);
        }
    }
}
