//! Crafting strategy state machine and Monte Carlo simulation.

use std::collections::BTreeMap;

use serde::Serialize;
use thiserror::Error;

use poe_data::PobData;

use crate::build::{BuildError, BuildPlan, OnFail, StepAction};
use crate::eval::evaluate;
use crate::eval::GoalEvaluation;
use crate::plan::{AugmentMode, Costs, Goal, Method, Plan, StartRarity};
use crate::pool::{Pool, PoolError};
use crate::probability::{
    attempts_for_confidence, expected_attempts, EstimateSource, StepEstimate,
};
use crate::state::{CraftRarity, ItemState, StateMod};
use poe_timing::Rng;

#[derive(Debug, Error)]
pub enum SimError {
    #[error(transparent)]
    Pool(#[from] PoolError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("invalid simulation: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CurrencyCounts {
    pub transmute: u64,
    pub alt: u64,
    pub aug: u64,
    pub regal: u64,
    pub alch: u64,
    pub scour: u64,
    pub chaos: u64,
}

impl CurrencyCounts {
    pub fn cost(&self, costs: &Costs) -> f64 {
        self.transmute as f64 * costs.transmute
            + self.alt as f64 * costs.alt
            + self.aug as f64 * costs.aug
            + self.regal as f64 * costs.regal
            + self.alch as f64 * costs.alch
            + self.scour as f64 * costs.scour
            + self.chaos as f64 * costs.chaos
    }

    pub fn to_map(&self) -> BTreeMap<String, f64> {
        let mut map = BTreeMap::new();
        for (name, value) in [
            ("transmute", self.transmute),
            ("alt", self.alt),
            ("aug", self.aug),
            ("regal", self.regal),
            ("alch", self.alch),
            ("scour", self.scour),
            ("chaos", self.chaos),
        ] {
            if value > 0 {
                map.insert(name.to_string(), value as f64);
            }
        }
        map
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunOutcome {
    pub success: bool,
    pub attempts: u32,
    pub no_mods: bool,
    pub currency: CurrencyCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextAction {
    Stop,
    Transmute,
    Alt,
    Augment,
    Regal,
    Alch,
    Chaos,
    Scour,
    ScourAlch,
    ScourTransmute,
}

impl std::fmt::Display for NextAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            NextAction::Stop => "stop (goal reached)",
            NextAction::Transmute => "transmute",
            NextAction::Alt => "alteration",
            NextAction::Augment => "augment",
            NextAction::Regal => "regal",
            NextAction::Alch => "alchemy",
            NextAction::Chaos => "chaos",
            NextAction::Scour => "scour",
            NextAction::ScourAlch => "scour + alchemy",
            NextAction::ScourTransmute => "scour + transmute",
        };
        formatter.write_str(text)
    }
}

pub fn next_action(plan: &Plan, state: &ItemState) -> NextAction {
    let evaluation = evaluate(&plan.goal, state);
    if evaluation.satisfied {
        return NextAction::Stop;
    }

    match plan.strategy.method {
        Method::AltAugRegal => match state.rarity {
            CraftRarity::Normal => NextAction::Transmute,
            CraftRarity::Magic => {
                let can_augment = state.open_prefixes() > 0 || state.open_suffixes() > 0;
                let should_augment = match plan.strategy.augment {
                    AugmentMode::Never => false,
                    AugmentMode::Always => can_augment,
                    AugmentMode::IfNeeded => can_augment && evaluation.score > 0,
                };
                if should_augment {
                    NextAction::Augment
                } else if evaluation.score >= plan.strategy.regal_when_score {
                    NextAction::Regal
                } else {
                    NextAction::Alt
                }
            }
            CraftRarity::Rare | CraftRarity::Other => NextAction::ScourTransmute,
        },
        Method::AltRegal => match state.rarity {
            CraftRarity::Normal => NextAction::Transmute,
            CraftRarity::Magic => {
                if evaluation.score >= plan.strategy.regal_when_score {
                    NextAction::Regal
                } else {
                    NextAction::Alt
                }
            }
            CraftRarity::Rare | CraftRarity::Other => NextAction::ScourTransmute,
        },
        Method::Chaos => match state.rarity {
            CraftRarity::Normal | CraftRarity::Other => NextAction::Alch,
            CraftRarity::Magic => NextAction::Regal,
            CraftRarity::Rare => NextAction::Chaos,
        },
        Method::AlchScour => match state.rarity {
            CraftRarity::Normal => NextAction::Alch,
            CraftRarity::Magic | CraftRarity::Rare | CraftRarity::Other => NextAction::ScourAlch,
        },
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SimulationReport {
    pub plan_name: String,
    pub base: String,
    pub item_level: u32,
    pub seed: u64,
    pub runs: u64,
    pub completed_runs: u64,
    pub truncated: bool,
    pub attempt_budget: Option<u64>,
    pub total_attempts: u64,
    pub successes: u64,
    pub success_rate: f64,
    pub max_attempts_reached: u64,
    pub failed_no_mods: u64,
    pub max_attempts: u32,
    pub mean_attempts: f64,
    pub mean_success_attempts: Option<f64>,
    pub p50_success_attempts: Option<u64>,
    pub p90_success_attempts: Option<u64>,
    pub p99_success_attempts: Option<u64>,
    pub mean_cost: f64,
    pub mean_cost_success: Option<f64>,
    pub mean_currency: BTreeMap<String, f64>,
    pub mean_currency_success: Option<BTreeMap<String, f64>>,
}

/// Shared weighted roller used by legacy simulation, step simulation, and tests.
pub struct PoolRoller<'a> {
    pub pool: &'a Pool,
}

impl<'a> PoolRoller<'a> {
    pub fn new(pool: &'a Pool) -> Self {
        Self { pool }
    }

    pub fn roll_magic(&self, rng: &mut Rng, two_mod_chance: f64) -> Option<ItemState> {
        let count = if rng.chance(two_mod_chance) { 2 } else { 1 };
        self.roll_magic_count(rng, count)
    }

    pub fn roll_magic_count(&self, rng: &mut Rng, count: usize) -> Option<ItemState> {
        let mut state = ItemState::new(CraftRarity::Magic, Some(self.pool.item_level));
        let mut used = [""; 6];
        let mut used_count = 0usize;
        let mut produced = 0usize;
        for _ in 0..count {
            let Some(slot) = self.pool.choose_slot(
                rng,
                &used[..used_count],
                state.open_prefixes(),
                state.open_suffixes(),
            ) else {
                break;
            };
            let Some(pool_mod) = self.pool.pick(slot, rng, &used[..used_count]) else {
                break;
            };
            if used_count < used.len() {
                used[used_count] = pool_mod.group.as_str();
                used_count += 1;
            }
            state.mods.push(pool_mod.roll_state(rng));
            produced += 1;
        }
        if produced == 0 {
            None
        } else {
            Some(state)
        }
    }

    pub fn roll_rare_count(&self, rng: &mut Rng, count: usize) -> ItemState {
        let mut state = ItemState::new(CraftRarity::Rare, Some(self.pool.item_level));
        let mut used = [""; 6];
        let mut used_count = 0usize;
        for _ in 0..count {
            let Some(slot) = self.pool.choose_slot(
                rng,
                &used[..used_count],
                state.open_prefixes(),
                state.open_suffixes(),
            ) else {
                break;
            };
            let Some(pool_mod) = self.pool.pick(slot, rng, &used[..used_count]) else {
                break;
            };
            if used_count < used.len() {
                used[used_count] = pool_mod.group.as_str();
                used_count += 1;
            }
            state.mods.push(pool_mod.roll_state(rng));
        }
        state
    }

    pub fn add_mod(&self, rng: &mut Rng, state: &mut ItemState) -> Option<StateMod> {
        let (used, used_count) = used_groups(state);
        let slot = self.pool.choose_slot(
            rng,
            &used[..used_count],
            state.open_prefixes(),
            state.open_suffixes(),
        )?;
        let pool_mod = self.pool.pick(slot, rng, &used[..used_count])?;
        Some(pool_mod.roll_state(rng))
    }

    pub fn regal(&self, rng: &mut Rng, state: &mut ItemState) -> Option<StateMod> {
        let (used, used_count) = used_groups(state);
        let open_prefixes = 3u8.saturating_sub(state.prefix_count());
        let open_suffixes = 3u8.saturating_sub(state.suffix_count());
        let slot = self
            .pool
            .choose_slot(rng, &used[..used_count], open_prefixes, open_suffixes)?;
        let pool_mod = self.pool.pick(slot, rng, &used[..used_count])?;
        state.rarity = CraftRarity::Rare;
        Some(pool_mod.roll_state(rng))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepTransition {
    Stayed,
    Advanced,
    Finished,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct StepRunner {
    pub current_step: usize,
    pub attempts_in_step: u32,
    pub pending_reset: bool,
    pub finished: bool,
    pub failed: Option<String>,
}

impl StepRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn target_goal<'a>(&self, plan: &'a BuildPlan) -> Option<&'a Goal> {
        plan.steps
            .get(self.current_step)
            .and_then(|step| step.goal.as_ref())
    }

    pub fn observe(&mut self, plan: &BuildPlan, state: &ItemState) {
        while !self.finished && self.failed.is_none() {
            if self.pending_reset && state.rarity == CraftRarity::Normal {
                self.pending_reset = false;
                self.current_step = 0;
                self.attempts_in_step = 0;
            }
            let Some(step) = plan.steps.get(self.current_step) else {
                self.finished = true;
                return;
            };
            if step
                .goal
                .as_ref()
                .is_some_and(|goal| evaluate(goal, state).satisfied)
            {
                self.advance();
            } else {
                return;
            }
        }
    }

    fn advance(&mut self) {
        self.current_step += 1;
        self.attempts_in_step = 0;
    }

    fn apply_on_fail(&mut self, plan: &BuildPlan, state: &ItemState, on_fail: OnFail) {
        match on_fail {
            OnFail::Restart => {
                self.current_step = 0;
                self.attempts_in_step = 0;
                if state.rarity != CraftRarity::Normal {
                    self.pending_reset = true;
                }
            }
            OnFail::Continue => {
                self.advance();
                if self.current_step >= plan.steps.len() {
                    let name = plan
                        .steps
                        .get(self.current_step.saturating_sub(1))
                        .map(|step| step.name.as_str())
                        .unwrap_or("final step");
                    self.failed = Some(format!(
                        "final step `{name}` did not meet its goal after on_fail=continue"
                    ));
                }
            }
            OnFail::Abort => {
                let name = plan
                    .steps
                    .get(self.current_step)
                    .map(|step| step.name.as_str())
                    .unwrap_or("step");
                self.failed = Some(format!("step `{name}` failed"));
            }
        }
    }

    /// Advance/fail after a currency action has been applied.
    pub fn after_action(
        &mut self,
        plan: &BuildPlan,
        state: &ItemState,
        primary: bool,
    ) -> StepTransition {
        if primary {
            self.attempts_in_step = self.attempts_in_step.saturating_add(1);
        }
        if self.failed.is_some() {
            return StepTransition::Failed;
        }
        if self.pending_reset && state.rarity == CraftRarity::Normal {
            self.pending_reset = false;
            self.current_step = 0;
            self.attempts_in_step = 0;
        }
        let Some(step) = plan.steps.get(self.current_step) else {
            self.finished = true;
            return StepTransition::Finished;
        };
        let accepted = step.goal.is_none()
            || step
                .goal
                .as_ref()
                .is_some_and(|goal| evaluate(goal, state).satisfied);
        if accepted {
            self.advance();
            if self.current_step >= plan.steps.len() {
                self.finished = true;
                StepTransition::Finished
            } else {
                StepTransition::Advanced
            }
        } else if !step.action.is_loop()
            || step
                .max_attempts
                .is_some_and(|limit| self.attempts_in_step >= limit)
        {
            let on_fail = step.on_fail;
            self.apply_on_fail(plan, state, on_fail);
            if self.failed.is_some() {
                StepTransition::Failed
            } else {
                StepTransition::Stayed
            }
        } else {
            StepTransition::Stayed
        }
    }

    /// Return the next currency action for the current item state.
    pub fn next_action(&mut self, plan: &BuildPlan, state: &ItemState) -> NextAction {
        let guard = plan.steps.len().saturating_add(4);
        for _ in 0..guard {
            self.observe(plan, state);
            if self.finished || self.failed.is_some() {
                return NextAction::Stop;
            }
            if self.pending_reset {
                if state.rarity == CraftRarity::Normal {
                    self.pending_reset = false;
                    self.current_step = 0;
                    self.attempts_in_step = 0;
                } else {
                    return NextAction::Scour;
                }
            }
            let Some(step) = plan.steps.get(self.current_step) else {
                self.finished = true;
                return NextAction::Stop;
            };
            let action = step.action;
            let on_fail = step.on_fail;
            let augment = step.augment;
            let decision = match action {
                StepAction::Transmute => {
                    (state.rarity == CraftRarity::Normal).then_some(NextAction::Transmute)
                }
                StepAction::Alt => match state.rarity {
                    CraftRarity::Normal => Some(NextAction::Transmute),
                    CraftRarity::Magic => Some(NextAction::Alt),
                    _ => Some(NextAction::Scour),
                },
                StepAction::AltAug => match state.rarity {
                    CraftRarity::Normal => Some(NextAction::Transmute),
                    CraftRarity::Magic => {
                        let can_augment = state.open_prefixes() > 0 || state.open_suffixes() > 0;
                        let should_augment = can_augment
                            && match augment {
                                AugmentMode::Never => false,
                                AugmentMode::Always => true,
                                AugmentMode::IfNeeded => step
                                    .goal
                                    .as_ref()
                                    .is_some_and(|goal| evaluate(goal, state).score > 0),
                            };
                        Some(if should_augment {
                            NextAction::Augment
                        } else {
                            NextAction::Alt
                        })
                    }
                    _ => Some(NextAction::Scour),
                },
                StepAction::Augment => {
                    let ok = state.rarity == CraftRarity::Magic
                        && (state.open_prefixes() > 0 || state.open_suffixes() > 0);
                    ok.then_some(NextAction::Augment)
                }
                StepAction::Regal => {
                    (state.rarity == CraftRarity::Magic).then_some(NextAction::Regal)
                }
                StepAction::Alch => {
                    (state.rarity == CraftRarity::Normal).then_some(NextAction::Alch)
                }
                StepAction::Scour => {
                    if state.rarity == CraftRarity::Normal {
                        if step.goal.is_none() {
                            self.advance();
                            if self.current_step >= plan.steps.len() {
                                self.finished = true;
                                return NextAction::Stop;
                            }
                            continue;
                        }
                        None
                    } else {
                        Some(NextAction::Scour)
                    }
                }
                StepAction::ScourAlch => match state.rarity {
                    CraftRarity::Normal => Some(NextAction::Alch),
                    _ => Some(NextAction::ScourAlch),
                },
                StepAction::Chaos => match state.rarity {
                    CraftRarity::Normal => Some(NextAction::Alch),
                    CraftRarity::Magic => Some(NextAction::Regal),
                    CraftRarity::Rare => Some(NextAction::Chaos),
                    CraftRarity::Other => Some(NextAction::ScourAlch),
                },
            };
            if let Some(next) = decision {
                return next;
            }
            self.apply_on_fail(plan, state, on_fail);
            if self.failed.is_some() {
                return NextAction::Stop;
            }
        }
        NextAction::Stop
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildRunOutcome {
    pub success: bool,
    pub attempts: u32,
    pub currency: CurrencyCounts,
    pub step_actions: Vec<u64>,
    pub step_accepts: Vec<u64>,
}

pub struct BuildSimulator {
    pub plan: BuildPlan,
    pub pool: Pool,
}

impl BuildSimulator {
    pub fn new(data: &PobData, plan: BuildPlan) -> Result<Self, SimError> {
        plan.validate_shape()?;
        let pool = plan.validate_pool(data)?;
        Ok(Self { plan, pool })
    }

    pub fn roller(&self) -> PoolRoller<'_> {
        PoolRoller::new(&self.pool)
    }

    pub fn initial_state(&self, rng: &mut Rng) -> ItemState {
        match self.plan.start {
            StartRarity::Normal => ItemState::new(CraftRarity::Normal, Some(self.pool.item_level)),
            StartRarity::Magic => {
                let count = weighted_count(rng, &self.plan.options.magic_mod_weights, 1);
                self.roller()
                    .roll_magic_count(rng, count)
                    .unwrap_or_else(|| {
                        ItemState::new(CraftRarity::Magic, Some(self.pool.item_level))
                    })
            }
            StartRarity::Rare => {
                let count = weighted_count(rng, &self.plan.options.rare_mod_weights, 4);
                self.roller().roll_rare_count(rng, count)
            }
        }
    }

    pub fn run_once(&self, rng: &mut Rng, max_actions: u32) -> BuildRunOutcome {
        let mut state = self.initial_state(rng);
        let mut runner = StepRunner::new();
        let mut currency = CurrencyCounts::default();
        let mut attempts = 0u32;
        let mut actions = 0u32;
        let mut step_actions = vec![0u64; self.plan.steps.len()];
        let mut step_accepts = vec![0u64; self.plan.steps.len()];

        loop {
            runner.observe(&self.plan, &state);
            if runner.finished {
                return BuildRunOutcome {
                    success: true,
                    attempts,
                    currency,
                    step_actions,
                    step_accepts,
                };
            }
            if runner.failed.is_some() {
                return BuildRunOutcome {
                    success: false,
                    attempts,
                    currency,
                    step_actions,
                    step_accepts,
                };
            }
            if actions >= max_actions {
                return BuildRunOutcome {
                    success: false,
                    attempts,
                    currency,
                    step_actions,
                    step_accepts,
                };
            }
            let action = runner.next_action(&self.plan, &state);
            if action == NextAction::Stop {
                let success = runner.finished && runner.failed.is_none();
                return BuildRunOutcome {
                    success,
                    attempts,
                    currency,
                    step_actions,
                    step_accepts,
                };
            }
            let step_index = runner.current_step;
            let primary = step_action_is_primary(
                self.plan
                    .steps
                    .get(step_index)
                    .map(|step| step.action)
                    .unwrap_or(StepAction::Alt),
                action,
            );
            self.apply_action(rng, &mut state, action, &mut currency);
            actions += 1;
            if primary {
                attempts += 1;
                if let Some(value) = step_actions.get_mut(step_index) {
                    *value += 1;
                }
            }
            let transition = runner.after_action(&self.plan, &state, primary);
            if matches!(
                transition,
                StepTransition::Advanced | StepTransition::Finished
            ) && self
                .plan
                .steps
                .get(step_index)
                .is_some_and(|step| step.goal.is_some())
            {
                if let Some(value) = step_accepts.get_mut(step_index) {
                    *value += 1;
                }
            }
            if transition == StepTransition::Failed {
                return BuildRunOutcome {
                    success: false,
                    attempts,
                    currency,
                    step_actions,
                    step_accepts,
                };
            }
        }
    }

    fn apply_action(
        &self,
        rng: &mut Rng,
        state: &mut ItemState,
        action: NextAction,
        currency: &mut CurrencyCounts,
    ) {
        match action {
            NextAction::Stop => {}
            NextAction::Transmute => {
                let count = weighted_count(rng, &self.plan.options.magic_mod_weights, 1);
                if let Some(next) = self.roller().roll_magic_count(rng, count) {
                    *state = next;
                }
                currency.transmute += 1;
            }
            NextAction::Alt => {
                let count = weighted_count(rng, &self.plan.options.magic_mod_weights, 1);
                if let Some(next) = self.roller().roll_magic_count(rng, count) {
                    *state = next;
                }
                currency.alt += 1;
            }
            NextAction::Augment => {
                if let Some(rolled) = self.roller().add_mod(rng, state) {
                    state.mods.push(rolled);
                    currency.aug += 1;
                }
            }
            NextAction::Regal => {
                if let Some(rolled) = self.roller().regal(rng, state) {
                    state.mods.push(rolled);
                    currency.regal += 1;
                }
            }
            NextAction::Alch => {
                let count = weighted_count(rng, &self.plan.options.rare_mod_weights, 4);
                *state = self.roller().roll_rare_count(rng, count);
                currency.alch += 1;
            }
            NextAction::Chaos => {
                let count = weighted_count(rng, &self.plan.options.rare_mod_weights, 4);
                *state = self.roller().roll_rare_count(rng, count);
                currency.chaos += 1;
            }
            NextAction::Scour => {
                if state.rarity != CraftRarity::Normal {
                    currency.scour += 1;
                }
                *state = ItemState::new(CraftRarity::Normal, Some(self.pool.item_level));
            }
            NextAction::ScourAlch => {
                if state.rarity != CraftRarity::Normal {
                    currency.scour += 1;
                }
                let count = weighted_count(rng, &self.plan.options.rare_mod_weights, 4);
                *state = self.roller().roll_rare_count(rng, count);
                currency.alch += 1;
            }
            NextAction::ScourTransmute => {
                if state.rarity != CraftRarity::Normal {
                    currency.scour += 1;
                }
                let count = weighted_count(rng, &self.plan.options.magic_mod_weights, 1);
                if let Some(next) = self.roller().roll_magic_count(rng, count) {
                    *state = next;
                }
                currency.transmute += 1;
            }
        }
    }
}

pub fn step_action_is_primary(step: StepAction, action: NextAction) -> bool {
    if step == StepAction::Scour {
        return action == NextAction::Scour;
    }
    // Every action that can produce or improve the current item counts as an
    // attempt. Scour is reset overhead and Stop never applies.
    !matches!(action, NextAction::Stop | NextAction::Scour)
}

pub fn weighted_count(rng: &mut Rng, weights: &[f64], first: usize) -> usize {
    let total = weights
        .iter()
        .copied()
        .filter(|weight| *weight > 0.0)
        .sum::<f64>();
    if total <= 0.0 {
        return first;
    }
    let mut roll = rng.next_f64() * total;
    for (index, weight) in weights.iter().enumerate() {
        if *weight <= 0.0 {
            continue;
        }
        if roll < *weight {
            return first + index;
        }
        roll -= *weight;
    }
    first + weights.len().saturating_sub(1)
}

pub struct Simulator {
    pub plan: Plan,
    pub pool: Pool,
}

impl Simulator {
    pub fn new(data: &PobData, plan: Plan) -> Result<Self, SimError> {
        let pool = Pool::from_plan(data, &plan)?;
        Ok(Self { plan, pool })
    }

    pub fn evaluate_state(&self, state: &ItemState) -> GoalEvaluation {
        evaluate(&self.plan.goal, state)
    }

    pub fn run_once(&self, rng: &mut Rng) -> RunOutcome {
        match self.plan.strategy.method {
            Method::AltAugRegal | Method::AltRegal => self.run_alt_loop(rng),
            Method::Chaos => self.run_chaos(rng),
            Method::AlchScour => self.run_alch_scour(rng),
        }
    }

    fn run_alt_loop(&self, rng: &mut Rng) -> RunOutcome {
        let mut state = self.initial_state(rng);
        let mut currency = CurrencyCounts::default();
        let mut attempts = 0u32;
        let mut need_roll = false;

        loop {
            if state.rarity != CraftRarity::Magic {
                if state.rarity == CraftRarity::Rare {
                    currency.scour += 1;
                }
                let Some(magic) = self.roller().roll_magic(rng, 0.0) else {
                    return RunOutcome {
                        success: false,
                        attempts,
                        no_mods: true,
                        currency,
                    };
                };
                state = magic;
                currency.transmute += 1;
                need_roll = false;
            }

            if need_roll {
                let Some(magic) = self
                    .roller()
                    .roll_magic(rng, self.plan.strategy.magic_two_mod_chance)
                else {
                    return RunOutcome {
                        success: false,
                        attempts,
                        no_mods: true,
                        currency,
                    };
                };
                state = magic;
                currency.alt += 1;
            }
            need_roll = true;
            attempts += 1;

            let mut evaluation = self.evaluate_state(&state);
            if evaluation.satisfied {
                return RunOutcome {
                    success: true,
                    attempts,
                    no_mods: false,
                    currency,
                };
            }

            if self.plan.strategy.method == Method::AltAugRegal
                && state.rarity == CraftRarity::Magic
                && (state.open_prefixes() > 0 || state.open_suffixes() > 0)
            {
                let should_augment = match self.plan.strategy.augment {
                    AugmentMode::Never => false,
                    AugmentMode::Always => true,
                    AugmentMode::IfNeeded => evaluation.score > 0,
                };
                if should_augment {
                    if let Some(rolled) = self.roller().add_mod(rng, &mut state) {
                        currency.aug += 1;
                        state.mods.push(rolled);
                        evaluation = self.evaluate_state(&state);
                        if evaluation.satisfied {
                            return RunOutcome {
                                success: true,
                                attempts,
                                no_mods: false,
                                currency,
                            };
                        }
                    }
                }
            }

            if state.rarity == CraftRarity::Magic
                && evaluation.score >= self.plan.strategy.regal_when_score
            {
                if let Some(rolled) = self.roller().regal(rng, &mut state) {
                    currency.regal += 1;
                    state.mods.push(rolled);
                    if self.evaluate_state(&state).satisfied {
                        return RunOutcome {
                            success: true,
                            attempts,
                            no_mods: false,
                            currency,
                        };
                    }
                    state.rarity = CraftRarity::Normal;
                    state.mods.clear();
                    currency.scour += 1;
                    need_roll = false;
                }
            }

            if attempts >= self.plan.strategy.max_attempts {
                return RunOutcome {
                    success: false,
                    attempts,
                    no_mods: false,
                    currency,
                };
            }
        }
    }

    fn run_chaos(&self, rng: &mut Rng) -> RunOutcome {
        let mut state = self.initial_state(rng);
        let mut currency = CurrencyCounts::default();
        let mut attempts = 0u32;

        if state.rarity == CraftRarity::Normal {
            state = self.roller().roll_rare_count(rng, 4);
            currency.alch += 1;
        } else if state.rarity == CraftRarity::Magic {
            if let Some(rolled) = self.roller().regal(rng, &mut state) {
                currency.regal += 1;
                state.mods.push(rolled);
            }
        }

        loop {
            attempts += 1;
            if self.evaluate_state(&state).satisfied {
                return RunOutcome {
                    success: true,
                    attempts,
                    no_mods: false,
                    currency,
                };
            }
            if attempts >= self.plan.strategy.max_attempts {
                return RunOutcome {
                    success: false,
                    attempts,
                    no_mods: false,
                    currency,
                };
            }
            let count = legacy_rare_count(&self.plan, rng);
            state = self.roller().roll_rare_count(rng, count);
            currency.chaos += 1;
        }
    }

    fn run_alch_scour(&self, rng: &mut Rng) -> RunOutcome {
        let mut currency = CurrencyCounts::default();
        let mut attempts = 0u32;
        let initial = self.initial_state(rng);
        if initial.rarity != CraftRarity::Normal {
            currency.scour += 1;
        }

        loop {
            let state = self.roller().roll_rare_count(rng, 4);
            currency.alch += 1;
            attempts += 1;
            if self.evaluate_state(&state).satisfied {
                return RunOutcome {
                    success: true,
                    attempts,
                    no_mods: false,
                    currency,
                };
            }
            if attempts >= self.plan.strategy.max_attempts {
                return RunOutcome {
                    success: false,
                    attempts,
                    no_mods: false,
                    currency,
                };
            }
            currency.scour += 1;
        }
    }

    fn initial_state(&self, rng: &mut Rng) -> ItemState {
        match self.plan.strategy.start {
            StartRarity::Normal => ItemState::new(CraftRarity::Normal, Some(self.pool.item_level)),
            StartRarity::Magic => self
                .roller()
                .roll_magic(rng, self.plan.strategy.magic_two_mod_chance)
                .unwrap_or_else(|| ItemState::new(CraftRarity::Normal, Some(self.pool.item_level))),
            StartRarity::Rare => {
                let count = legacy_rare_count(&self.plan, rng);
                self.roller().roll_rare_count(rng, count)
            }
        }
    }

    fn roller(&self) -> PoolRoller<'_> {
        PoolRoller::new(&self.pool)
    }
}

fn legacy_rare_count(plan: &Plan, rng: &mut Rng) -> usize {
    let counts = &plan.strategy.chaos_mod_counts;
    if counts.is_empty() {
        4
    } else {
        counts[rng.index(counts.len())] as usize
    }
}

fn used_groups(state: &ItemState) -> ([&str; 6], usize) {
    let mut used = [""; 6];
    let mut count = 0usize;
    for entry in &state.mods {
        if entry.group.is_empty() {
            continue;
        }
        if count == used.len() {
            break;
        }
        used[count] = entry.group.as_str();
        count += 1;
    }
    (used, count)
}

pub fn simulate(
    data: &PobData,
    plan: Plan,
    runs: u64,
    seed: u64,
) -> Result<SimulationReport, SimError> {
    simulate_with_options(data, plan, runs, seed, None, None)
}

/// Simulate with an optional total-attempt budget and progress callback.
/// The budget is checked between runs, so the last started run may overshoot
/// it by up to that run's own attempt count.
pub fn simulate_with_options(
    data: &PobData,
    plan: Plan,
    runs: u64,
    seed: u64,
    max_total_attempts: Option<u64>,
    mut progress: Option<&mut dyn FnMut(u64, u64)>,
) -> Result<SimulationReport, SimError> {
    if runs == 0 {
        return Err(SimError::Invalid("runs must be > 0".to_string()));
    }
    if max_total_attempts == Some(0) {
        return Err(SimError::Invalid(
            "max_total_attempts must be > 0 or omitted".to_string(),
        ));
    }
    let max_attempts = plan.strategy.max_attempts;
    let costs = plan.costs;
    let simulator = Simulator::new(data, plan)?;
    let mut rng = Rng::new(seed);
    let mut successes = 0u64;
    let mut max_attempts_reached = 0u64;
    let mut failed_no_mods = 0u64;
    let mut total_attempts = 0u64;
    let mut completed_runs = 0u64;
    let mut truncated = false;
    let mut total_cost = 0.0;
    let mut total_currency = CurrencyCounts::default();
    let mut success_attempts = Vec::new();
    let mut success_cost = 0.0;
    let mut success_currency = CurrencyCounts::default();
    let progress_step = (runs / 10).max(1);

    for _ in 0..runs {
        if let Some(budget) = max_total_attempts {
            if total_attempts >= budget {
                truncated = true;
                break;
            }
        }
        let outcome = simulator.run_once(&mut rng);
        completed_runs += 1;
        total_attempts += outcome.attempts as u64;
        total_cost += outcome.currency.cost(&costs);
        accumulate(&mut total_currency, &outcome.currency);
        if outcome.success {
            successes += 1;
            success_attempts.push(outcome.attempts);
            success_cost += outcome.currency.cost(&costs);
            accumulate(&mut success_currency, &outcome.currency);
        } else if outcome.no_mods {
            failed_no_mods += 1;
        } else if outcome.attempts >= max_attempts {
            max_attempts_reached += 1;
        }
        if let Some(callback) = progress.as_mut() {
            if completed_runs.is_multiple_of(progress_step) || completed_runs == runs {
                callback(completed_runs, runs);
            }
        }
    }

    if completed_runs == 0 {
        return Err(SimError::Invalid(
            "attempt budget exhausted before the first run completed".to_string(),
        ));
    }
    if truncated {
        if let Some(callback) = progress.as_mut() {
            callback(completed_runs, runs);
        }
    }

    success_attempts.sort_unstable();
    let mean_success_attempts = if successes > 0 {
        Some(
            success_attempts
                .iter()
                .map(|value| *value as f64)
                .sum::<f64>()
                / successes as f64,
        )
    } else {
        None
    };

    let mean_currency = mean_map(&total_currency, completed_runs);
    let mean_currency_success = if successes > 0 {
        Some(mean_map(&success_currency, successes))
    } else {
        None
    };

    Ok(SimulationReport {
        plan_name: simulator.plan.name.clone(),
        base: simulator.pool.base_label.clone(),
        item_level: simulator.pool.item_level,
        seed,
        runs,
        completed_runs,
        truncated,
        attempt_budget: max_total_attempts,
        total_attempts,
        successes,
        success_rate: successes as f64 / completed_runs as f64,
        max_attempts_reached,
        failed_no_mods,
        max_attempts,
        mean_attempts: total_attempts as f64 / completed_runs as f64,
        mean_success_attempts,
        p50_success_attempts: percentile_success(&success_attempts, 0.50),
        p90_success_attempts: percentile_success(&success_attempts, 0.90),
        p99_success_attempts: percentile_success(&success_attempts, 0.99),
        mean_cost: total_cost / completed_runs as f64,
        mean_cost_success: if successes > 0 {
            Some(success_cost / successes as f64)
        } else {
            None
        },
        mean_currency,
        mean_currency_success,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildSimulationReport {
    pub plan_name: String,
    pub base: String,
    pub item_level: u32,
    pub seed: u64,
    pub runs: u64,
    pub completed_runs: u64,
    pub truncated: bool,
    pub attempt_budget: Option<u64>,
    pub total_attempts: u64,
    pub successes: u64,
    pub success_rate: f64,
    pub mean_attempts: f64,
    pub mean_success_attempts: Option<f64>,
    pub mean_cost: f64,
    pub mean_cost_success: Option<f64>,
    pub mean_currency: BTreeMap<String, f64>,
    pub mean_currency_success: Option<BTreeMap<String, f64>>,
    pub steps: Vec<StepEstimate>,
}

pub fn simulate_build(
    data: &PobData,
    plan: BuildPlan,
    runs: u64,
    seed: u64,
    max_total_attempts: Option<u64>,
    mut progress: Option<&mut dyn FnMut(u64, u64)>,
) -> Result<BuildSimulationReport, SimError> {
    if runs == 0 {
        return Err(SimError::Invalid("runs must be > 0".to_string()));
    }
    if max_total_attempts == Some(0) {
        return Err(SimError::Invalid(
            "max_total_attempts must be > 0 or omitted".to_string(),
        ));
    }
    let confidence = plan.options.confidence;
    let costs = plan.costs;
    let simulator = BuildSimulator::new(data, plan)?;
    let plan_name = simulator.plan.name.clone();
    let base = simulator.pool.base_label.clone();
    let item_level = simulator.pool.item_level;
    let step_names = simulator
        .plan
        .steps
        .iter()
        .map(|step| (step.name.clone(), step.action))
        .collect::<Vec<_>>();
    let mut step_actions = vec![0u64; simulator.plan.steps.len()];
    let mut step_accepts = vec![0u64; simulator.plan.steps.len()];
    let mut rng = Rng::new(seed);
    let mut successes = 0u64;
    let mut total_attempts = 0u64;
    let mut completed_runs = 0u64;
    let mut truncated = false;
    let mut total_cost = 0.0;
    let mut total_currency = CurrencyCounts::default();
    let mut success_attempts = Vec::new();
    let mut success_cost = 0.0;
    let mut success_currency = CurrencyCounts::default();
    let progress_step = (runs / 10).max(1);

    for _ in 0..runs {
        if let Some(budget) = max_total_attempts {
            if total_attempts >= budget {
                truncated = true;
                break;
            }
        }
        let outcome = simulator.run_once(&mut rng, 2_000_000);
        completed_runs += 1;
        total_attempts += outcome.attempts as u64;
        total_cost += outcome.currency.cost(&costs);
        accumulate(&mut total_currency, &outcome.currency);
        for (index, value) in outcome.step_actions.iter().enumerate() {
            if let Some(total) = step_actions.get_mut(index) {
                *total += *value;
            }
        }
        for (index, value) in outcome.step_accepts.iter().enumerate() {
            if let Some(total) = step_accepts.get_mut(index) {
                *total += *value;
            }
        }
        if outcome.success {
            successes += 1;
            success_attempts.push(outcome.attempts);
            success_cost += outcome.currency.cost(&costs);
            accumulate(&mut success_currency, &outcome.currency);
        }
        if let Some(callback) = progress.as_mut() {
            if completed_runs.is_multiple_of(progress_step) || completed_runs == runs {
                callback(completed_runs, runs);
            }
        }
    }

    if completed_runs == 0 {
        return Err(SimError::Invalid(
            "attempt budget exhausted before the first run completed".to_string(),
        ));
    }
    if truncated {
        if let Some(callback) = progress.as_mut() {
            callback(completed_runs, runs);
        }
    }

    success_attempts.sort_unstable();
    let mean_success_attempts = if successes > 0 {
        Some(
            success_attempts
                .iter()
                .map(|value| *value as f64)
                .sum::<f64>()
                / successes as f64,
        )
    } else {
        None
    };
    let mean_currency = mean_map(&total_currency, completed_runs);
    let mean_currency_success = if successes > 0 {
        Some(mean_map(&success_currency, successes))
    } else {
        None
    };
    let steps = step_names
        .iter()
        .enumerate()
        .map(|(index, (name, action))| {
            let actions = step_actions.get(index).copied().unwrap_or(0);
            let accepts = step_accepts.get(index).copied().unwrap_or(0);
            let p = (actions > 0).then_some(accepts as f64 / actions as f64);
            StepEstimate {
                step_index: index,
                p_success: p,
                source: EstimateSource::MonteCarlo,
                expected_attempts: p.and_then(expected_attempts),
                confidence_attempts: p.and_then(|p| attempts_for_confidence(p, confidence)),
                actions,
                accepts,
                note: Some(format!("{} ({})", name, action.label())),
            }
        })
        .collect();

    Ok(BuildSimulationReport {
        plan_name,
        base,
        item_level,
        seed,
        runs,
        completed_runs,
        truncated,
        attempt_budget: max_total_attempts,
        total_attempts,
        successes,
        success_rate: successes as f64 / completed_runs as f64,
        mean_attempts: total_attempts as f64 / completed_runs as f64,
        mean_success_attempts,
        mean_cost: total_cost / completed_runs as f64,
        mean_cost_success: (successes > 0).then_some(success_cost / successes as f64),
        mean_currency,
        mean_currency_success,
        steps,
    })
}

fn accumulate(total: &mut CurrencyCounts, value: &CurrencyCounts) {
    total.transmute += value.transmute;
    total.alt += value.alt;
    total.aug += value.aug;
    total.regal += value.regal;
    total.alch += value.alch;
    total.scour += value.scour;
    total.chaos += value.chaos;
}

fn mean_map(total: &CurrencyCounts, count: u64) -> BTreeMap<String, f64> {
    total
        .to_map()
        .into_iter()
        .map(|(name, value)| (name, value / count as f64))
        .collect()
}

fn percentile_success(values: &[u32], percentile: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let index = ((values.len() - 1) as f64 * percentile).ceil() as usize;
    Some(values[index.min(values.len() - 1)] as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{BaseSpec, Goal, Requirement, Strategy};
    use crate::state::StateMod;
    use poe_data::{BaseItem, DataVersion, Mod, ModDomain, ModType, PobData};

    fn data() -> PobData {
        let base = BaseItem {
            name: "Test Ring".to_string(),
            item_class: "Ring".to_string(),
            sub_type: None,
            tags: vec!["default".to_string(), "ring".to_string()],
            influence_tags: BTreeMap::new(),
            implicit_mod_types: vec![],
            implicit_ids: vec![],
            level_req: None,
            str_req: None,
            dex_req: None,
            int_req: None,
        };
        let mods = vec![
            Mod {
                id: "Life".to_string(),
                domain: ModDomain::Explicit,
                mod_type: ModType::Prefix,
                affix: "Hale".to_string(),
                stat_texts: vec!["+(80-90) to maximum Life".to_string()],
                level: 10,
                group: "Life".to_string(),
                weight_keys: vec!["ring".to_string(), "default".to_string()],
                weight_values: vec![1000, 0],
                tags: vec![],
                trade_hashes: vec![],
                item_types: vec![],
            },
            Mod {
                id: "Cold".to_string(),
                domain: ModDomain::Explicit,
                mod_type: ModType::Suffix,
                affix: "of Haast".to_string(),
                stat_texts: vec!["+(40-45)% to Cold Resistance".to_string()],
                level: 10,
                group: "ColdRes".to_string(),
                weight_keys: vec!["ring".to_string(), "default".to_string()],
                weight_values: vec![1000, 0],
                tags: vec![],
                trade_hashes: vec![],
                item_types: vec![],
            },
        ];
        PobData {
            version: DataVersion::default(),
            mods,
            bases: vec![base],
            trade_stats: vec![],
            essences: vec![],
        }
    }

    fn plan(method: Method) -> Plan {
        Plan {
            schema: 1,
            name: "test".to_string(),
            base: BaseSpec {
                item_class: Some("Ring".to_string()),
                base_names: vec!["Test Ring".to_string()],
                tags: vec![],
                item_level: 86,
                unrestricted: false,
                influences: vec![],
            },
            goal: Goal {
                min_prefixes: Some(1),
                required: vec![Requirement {
                    group: Some("Life".to_string()),
                    min_tier: Some(1),
                    ..Requirement::default()
                }],
                ..Goal::default()
            },
            strategy: Strategy {
                method,
                max_attempts: 1000,
                augment: AugmentMode::Always,
                regal_when_score: 1,
                magic_two_mod_chance: 0.5,
                chaos_mod_counts: vec![4, 5, 6],
                start: StartRarity::Normal,
            },
            costs: Costs {
                alt: 0.1,
                aug: 0.2,
                regal: 1.0,
                transmute: 0.01,
                alch: 0.5,
                scour: 0.1,
                chaos: 1.0,
            },
        }
    }

    #[test]
    fn alt_regal_simulation_succeeds() {
        let report = simulate(&data(), plan(Method::AltAugRegal), 2000, 42).unwrap();
        assert!(report.successes > 0);
        assert!(report.success_rate > 0.0);
        assert!(report.mean_cost > 0.0);
        assert!(report.p50_success_attempts.is_some());
    }

    #[test]
    fn chaos_simulation_succeeds() {
        let report = simulate(&data(), plan(Method::Chaos), 500, 7).unwrap();
        assert!(report.successes > 0);
    }

    #[test]
    fn impossible_goal_is_rejected_by_pool() {
        let mut plan = plan(Method::AltAugRegal);
        plan.goal.required[0].group = Some("Missing".to_string());
        assert!(simulate(&data(), plan, 10, 1).is_err());
    }

    #[test]
    fn recommends_next_action() {
        let mut alt_plan = plan(Method::AltAugRegal);
        alt_plan.strategy.augment = AugmentMode::Never;
        alt_plan.strategy.regal_when_score = 1;
        alt_plan.goal.min_prefixes = Some(2);

        let normal = ItemState::new(CraftRarity::Normal, Some(86));
        assert_eq!(next_action(&alt_plan, &normal), NextAction::Transmute);

        let magic = ItemState::new(CraftRarity::Magic, Some(86));
        assert_eq!(next_action(&alt_plan, &magic), NextAction::Alt);

        let mut with_life = magic.clone();
        with_life.mods.push(StateMod {
            slot: crate::plan::Slot::Prefix,
            group: "Life".to_string(),
            affix: "Hale".to_string(),
            tier: Some(1),
            values: vec![85.0],
            percentiles: vec![0.5],
        });
        assert_eq!(next_action(&alt_plan, &with_life), NextAction::Regal);

        let rare = ItemState::new(CraftRarity::Rare, Some(86));
        let chaos_plan = plan(Method::Chaos);
        assert_eq!(next_action(&chaos_plan, &rare), NextAction::Chaos);
    }

    #[test]
    fn alch_scour_counts_initial_scour_for_non_normal_start() {
        let mut plan = plan(Method::AlchScour);
        plan.strategy.start = StartRarity::Rare;
        let simulator = Simulator::new(&data(), plan).unwrap();
        let mut rng = Rng::new(99);
        let outcome = simulator.run_once(&mut rng);
        assert!(outcome.currency.scour >= 1);
        assert!(outcome.currency.alch >= 1);
    }

    #[test]
    fn respects_total_attempt_budget() {
        let report =
            simulate_with_options(&data(), plan(Method::AltAugRegal), 100, 42, Some(1), None)
                .unwrap();
        assert!(report.truncated);
        assert!(report.completed_runs < report.runs);
        assert_eq!(report.attempt_budget, Some(1));
    }

    #[test]
    fn rejects_zero_attempt_budget() {
        let result =
            simulate_with_options(&data(), plan(Method::AltAugRegal), 100, 42, Some(0), None);
        assert!(result.is_err());
    }

    #[test]
    fn run_once_counts_currency() {
        let simulator = Simulator::new(&data(), plan(Method::AltAugRegal)).unwrap();
        let mut rng = Rng::new(123);
        let outcome = simulator.run_once(&mut rng);
        let total = outcome.currency.alt
            + outcome.currency.aug
            + outcome.currency.regal
            + outcome.currency.scour
            + outcome.currency.transmute;
        assert!(total > 0);
        assert!(outcome.attempts > 0);
    }

    fn build_plan() -> BuildPlan {
        let legacy = plan(Method::AltAugRegal);
        BuildPlan {
            schema: 2,
            name: "build test".to_string(),
            start: StartRarity::Normal,
            base: legacy.base,
            options: crate::build::BuildOptions::default(),
            steps: vec![crate::build::PlanStep {
                name: "life".to_string(),
                action: StepAction::Alt,
                goal: Some(legacy.goal),
                on_fail: OnFail::Restart,
                max_attempts: Some(1000),
                augment: AugmentMode::Never,
            }],
            costs: Costs::default(),
        }
    }

    #[test]
    fn continue_past_final_step_fails_instead_of_succeeding() {
        let mut build = build_plan();
        build.steps[0].action = StepAction::Regal;
        build.steps[0].on_fail = OnFail::Continue;
        let mut runner = StepRunner::new();
        let magic = ItemState::new(CraftRarity::Magic, Some(86));
        let transition = runner.after_action(&build, &magic, true);
        assert_eq!(transition, StepTransition::Failed);
        assert!(!runner.finished);
        assert!(runner.failed.is_some());
    }

    #[test]
    fn step_runner_advances_on_accept() {
        let build = build_plan();
        let mut runner = StepRunner::new();
        let normal = ItemState::new(CraftRarity::Normal, Some(86));
        assert_eq!(runner.next_action(&build, &normal), NextAction::Transmute);

        let mut magic = ItemState::new(CraftRarity::Magic, Some(86));
        assert_eq!(runner.next_action(&build, &magic), NextAction::Alt);
        magic.mods.push(StateMod {
            slot: crate::plan::Slot::Prefix,
            group: "Life".to_string(),
            affix: "Hale".to_string(),
            tier: Some(1),
            values: vec![85.0],
            percentiles: vec![0.5],
        });
        runner.after_action(&build, &magic, true);
        assert!(runner.finished);
    }

    #[test]
    fn build_simulator_reports_step_probabilities() {
        let simulator = BuildSimulator::new(&data(), build_plan()).unwrap();
        let mut rng = Rng::new(7);
        let outcome = simulator.run_once(&mut rng, 100_000);
        assert!(outcome.success);
        assert!(outcome.step_actions[0] > 0);
        assert!(outcome.step_accepts[0] > 0);

        let report = simulate_build(&data(), build_plan(), 100, 7, None, None).unwrap();
        assert!(report.successes > 0);
        assert!(report.steps[0].p_success.unwrap_or(0.0) > 0.0);
        assert!(report.steps[0].confidence_attempts.is_some());
    }
}
