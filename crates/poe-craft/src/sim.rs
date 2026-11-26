//! Crafting strategy state machine and Monte Carlo simulation.

use std::collections::BTreeMap;

use serde::Serialize;
use thiserror::Error;

use poe_data::PobData;

use crate::eval::{evaluate, GoalEvaluation};
use crate::plan::{AugmentMode, Costs, Method, Plan, StartRarity};
use crate::pool::{Pool, PoolError};
use crate::rng::Rng;
use crate::state::{CraftRarity, ItemState, StateMod};

#[derive(Debug, Error)]
pub enum SimError {
    #[error(transparent)]
    Pool(#[from] PoolError),
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
                let Some(magic) = self.roll_magic(rng, 0.0) else {
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
                let Some(magic) = self.roll_magic(rng, self.plan.strategy.magic_two_mod_chance)
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
                    if let Some(rolled) = self.add_mod(rng, &mut state) {
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
                if let Some(rolled) = self.regal(rng, &mut state) {
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
            state = self.roll_rare_count(rng, 4);
            currency.alch += 1;
        } else if state.rarity == CraftRarity::Magic {
            if let Some(rolled) = self.regal(rng, &mut state) {
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
            state = self.roll_rare(rng);
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
            let state = self.roll_rare_count(rng, 4);
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
                .roll_magic(rng, self.plan.strategy.magic_two_mod_chance)
                .unwrap_or_else(|| ItemState::new(CraftRarity::Normal, Some(self.pool.item_level))),
            StartRarity::Rare => self.roll_rare(rng),
        }
    }

    fn roll_magic(&self, rng: &mut Rng, two_mod_chance: f64) -> Option<ItemState> {
        let mut state = ItemState::new(CraftRarity::Magic, Some(self.pool.item_level));
        let mut used: Vec<&str> = Vec::with_capacity(2);
        let count = if rng.chance(two_mod_chance) { 2 } else { 1 };
        let mut produced = 0usize;
        for _ in 0..count {
            let Some(slot) =
                self.pool
                    .choose_slot(rng, &used, state.open_prefixes(), state.open_suffixes())
            else {
                break;
            };
            let Some(pool_mod) = self.pool.pick(slot, rng, &used) else {
                break;
            };
            used.push(pool_mod.group.as_str());
            state.mods.push(pool_mod.roll_state(rng));
            produced += 1;
        }
        if produced == 0 {
            None
        } else {
            Some(state)
        }
    }

    fn roll_rare(&self, rng: &mut Rng) -> ItemState {
        let counts = &self.plan.strategy.chaos_mod_counts;
        let count = counts[rng.index(counts.len())] as usize;
        self.roll_rare_count(rng, count)
    }

    fn roll_rare_count(&self, rng: &mut Rng, count: usize) -> ItemState {
        let mut state = ItemState::new(CraftRarity::Rare, Some(self.pool.item_level));
        let mut used: Vec<&str> = Vec::with_capacity(count);
        for _ in 0..count {
            let Some(slot) =
                self.pool
                    .choose_slot(rng, &used, state.open_prefixes(), state.open_suffixes())
            else {
                break;
            };
            let Some(pool_mod) = self.pool.pick(slot, rng, &used) else {
                break;
            };
            used.push(pool_mod.group.as_str());
            state.mods.push(pool_mod.roll_state(rng));
        }
        state
    }

    fn add_mod(&self, rng: &mut Rng, state: &mut ItemState) -> Option<StateMod> {
        let used = state
            .mods
            .iter()
            .filter(|entry| !entry.group.is_empty())
            .map(|entry| entry.group.as_str())
            .collect::<Vec<_>>();
        let slot =
            self.pool
                .choose_slot(rng, &used, state.open_prefixes(), state.open_suffixes())?;
        let pool_mod = self.pool.pick(slot, rng, &used)?;
        Some(pool_mod.roll_state(rng))
    }

    fn regal(&self, rng: &mut Rng, state: &mut ItemState) -> Option<StateMod> {
        let used = state
            .mods
            .iter()
            .filter(|entry| !entry.group.is_empty())
            .map(|entry| entry.group.as_str())
            .collect::<Vec<_>>();
        let open_prefixes = 3u8.saturating_sub(state.prefix_count());
        let open_suffixes = 3u8.saturating_sub(state.suffix_count());
        let slot = self
            .pool
            .choose_slot(rng, &used, open_prefixes, open_suffixes)?;
        let pool_mod = self.pool.pick(slot, rng, &used)?;
        drop(used);
        state.rarity = CraftRarity::Rare;
        Some(pool_mod.roll_state(rng))
    }
}

pub fn simulate(
    data: &PobData,
    plan: Plan,
    runs: u64,
    seed: u64,
) -> Result<SimulationReport, SimError> {
    if runs == 0 {
        return Err(SimError::Invalid("runs must be > 0".to_string()));
    }
    let max_attempts = plan.strategy.max_attempts;
    let costs = plan.costs;
    let simulator = Simulator::new(data, plan)?;
    let mut rng = Rng::new(seed);
    let mut successes = 0u64;
    let mut max_attempts_reached = 0u64;
    let mut failed_no_mods = 0u64;
    let mut total_attempts = 0u64;
    let mut total_cost = 0.0;
    let mut total_currency = CurrencyCounts::default();
    let mut success_attempts = Vec::new();
    let mut success_cost = 0.0;
    let mut success_currency = CurrencyCounts::default();

    for _ in 0..runs {
        let outcome = simulator.run_once(&mut rng);
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

    let mean_currency = mean_map(&total_currency, runs);
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
        successes,
        success_rate: successes as f64 / runs as f64,
        max_attempts_reached,
        failed_no_mods,
        max_attempts,
        mean_attempts: total_attempts as f64 / runs as f64,
        mean_success_attempts,
        p50_success_attempts: percentile_success(&success_attempts, 0.50),
        p90_success_attempts: percentile_success(&success_attempts, 0.90),
        p99_success_attempts: percentile_success(&success_attempts, 0.99),
        mean_cost: total_cost / runs as f64,
        mean_cost_success: if successes > 0 {
            Some(success_cost / successes as f64)
        } else {
            None
        },
        mean_currency,
        mean_currency_success,
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
            mod_id: "Life".to_string(),
            slot: crate::plan::Slot::Prefix,
            group: "Life".to_string(),
            affix: "Hale".to_string(),
            tier: Some(1),
            values: vec![85.0],
            percentiles: vec![0.5],
            template: String::new(),
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
}
