use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use thiserror::Error;

use poe_craft::{evaluate, next_action, GoalEvaluation, NextAction, Plan};
use poe_data::PobData;
use poe_item::{parse_item, ParsedItem, Rarity, Resolver};

use crate::currency::{currencies_for, Currency};
use crate::io::{GameIo, IoError};

#[derive(Debug, Clone, Serialize)]
pub struct SessionConfig {
    pub max_steps: u32,
    pub max_currency_actions: u32,
    pub max_seconds: u64,
    pub max_cost: Option<f64>,
    pub max_same_fingerprint: u32,
    pub pre_action_delay_ms: u64,
    pub action_delay_ms: u64,
    pub verify_currency: bool,
    pub require_foreground: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            max_currency_actions: 20_000,
            max_seconds: 3_600,
            max_cost: None,
            max_same_fingerprint: 3,
            pre_action_delay_ms: 100,
            action_delay_ms: 250,
            verify_currency: true,
            require_foreground: true,
        }
    }
}

impl SessionConfig {
    pub fn validate(&self) -> Result<(), SessionError> {
        if self.max_steps == 0 {
            return Err(SessionError::InvalidConfig(
                "max_steps must be > 0".to_string(),
            ));
        }
        if self.max_currency_actions == 0 {
            return Err(SessionError::InvalidConfig(
                "max_currency_actions must be > 0".to_string(),
            ));
        }
        if self.max_same_fingerprint == 0 {
            return Err(SessionError::InvalidConfig(
                "max_same_fingerprint must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    GoalReached,
    MaxSteps,
    MaxCurrencyActions,
    TimeBudget,
    CostBudget,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionReport {
    pub success: bool,
    pub stop_reason: StopReason,
    pub steps: u32,
    pub currency_actions: u32,
    pub currency_used: BTreeMap<String, u32>,
    pub elapsed_seconds: f64,
    pub final_item: ParsedItem,
    pub final_evaluation: GoalEvaluation,
    pub log_path: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    Io(#[from] IoError),
    #[error("invalid session config: {0}")]
    InvalidConfig(String),
    #[error("unsupported item: {0}")]
    UnsupportedItem(String),
    #[error("currency `{currency}` is not available")]
    CurrencyMissing { currency: Currency },
    #[error("currency `{currency}` was not consumed (before {before}, after {after})")]
    CurrencyNotConsumed {
        currency: Currency,
        before: u32,
        after: u32,
    },
    #[error("target item changed unexpectedly")]
    ItemChanged,
    #[error("no visible item progress after {count} identical results")]
    NoProgress { count: u32 },
    #[error("emergency stop")]
    EmergencyStop,
    #[error("game lost foreground")]
    FocusLost,
    #[error("session log error: {0}")]
    Log(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    item_class: String,
    item_level: u32,
    sockets: Option<String>,
    implicit_signature: Vec<String>,
}

#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    parsed: ParsedItem,
    state: poe_craft::ItemState,
    evaluation: GoalEvaluation,
    identity: Identity,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewReport {
    pub item: ParsedItem,
    pub evaluation: GoalEvaluation,
    pub min_score: i32,
    pub next_action: NextAction,
    pub currencies: Vec<Currency>,
}

#[derive(Debug, Serialize)]
struct SessionEvent {
    step: u32,
    action: u32,
    currency: String,
    before_text: String,
    after_text: String,
    currency_before: Option<u32>,
    currency_after: Option<u32>,
    evaluation: GoalEvaluation,
    elapsed_seconds: f64,
}

pub struct SessionLogger {
    writer: BufWriter<File>,
    path: PathBuf,
}

impl SessionLogger {
    pub fn create(path: &Path) -> Result<Self, SessionError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| SessionError::Log(format!("creating log dir: {error}")))?;
        }
        let file = File::create(path)
            .map_err(|error| SessionError::Log(format!("creating log: {error}")))?;
        Ok(Self {
            writer: BufWriter::new(file),
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_event(&mut self, event: &SessionEvent) -> Result<(), SessionError> {
        serde_json::to_writer(&mut self.writer, event)
            .map_err(|error| SessionError::Log(format!("serializing event: {error}")))?;
        self.writer
            .write_all(b"\n")
            .and_then(|_| self.writer.flush())
            .map_err(|error| SessionError::Log(format!("writing log: {error}")))
    }
}

pub struct CraftSession<'a> {
    plan: Plan,
    resolver: Resolver<'a>,
    io: &'a mut dyn GameIo,
    config: SessionConfig,
    logger: Option<SessionLogger>,
}

impl<'a> CraftSession<'a> {
    pub fn new(
        plan: Plan,
        data: &'a PobData,
        io: &'a mut dyn GameIo,
        config: SessionConfig,
        logger: Option<SessionLogger>,
    ) -> Result<Self, SessionError> {
        config.validate()?;
        let resolver = Resolver::new(data);
        Ok(Self {
            plan,
            resolver,
            io,
            config,
            logger,
        })
    }

    pub fn preview(&mut self) -> Result<PreviewReport, SessionError> {
        self.check_gate()?;
        let snapshot = self.read_snapshot()?;
        let action = next_action(&self.plan, &snapshot.state);
        Ok(PreviewReport {
            item: snapshot.parsed,
            evaluation: snapshot.evaluation,
            min_score: self.plan.goal.min_score,
            next_action: action,
            currencies: currencies_for(action),
        })
    }

    pub fn run(&mut self) -> Result<SessionReport, SessionError> {
        let start = Instant::now();
        self.check_gate()?;
        let mut current = self.read_snapshot()?;
        let mut current_fingerprint = fingerprint(&current.text);
        let mut same_fingerprint = 0u32;
        let mut steps = 0u32;
        let mut currency_actions = 0u32;
        let mut currency_used: BTreeMap<String, u32> = BTreeMap::new();
        let mut total_cost = 0.0f64;

        loop {
            self.check_gate()?;
            if current.evaluation.satisfied {
                return Ok(self.report(
                    true,
                    StopReason::GoalReached,
                    start,
                    steps,
                    currency_actions,
                    currency_used,
                    &current,
                ));
            }
            if steps >= self.config.max_steps {
                return Ok(self.report(
                    false,
                    StopReason::MaxSteps,
                    start,
                    steps,
                    currency_actions,
                    currency_used,
                    &current,
                ));
            }
            if start.elapsed().as_secs() >= self.config.max_seconds {
                return Ok(self.report(
                    false,
                    StopReason::TimeBudget,
                    start,
                    steps,
                    currency_actions,
                    currency_used,
                    &current,
                ));
            }
            if self
                .config
                .max_cost
                .is_some_and(|limit| total_cost >= limit)
            {
                return Ok(self.report(
                    false,
                    StopReason::CostBudget,
                    start,
                    steps,
                    currency_actions,
                    currency_used,
                    &current,
                ));
            }

            let action = next_action(&self.plan, &current.state);
            let currencies = currencies_for(action);
            if currencies.is_empty() {
                return Ok(self.report(
                    current.evaluation.satisfied,
                    StopReason::GoalReached,
                    start,
                    steps,
                    currency_actions,
                    currency_used,
                    &current,
                ));
            }

            steps += 1;

            for currency in currencies {
                self.check_gate()?;
                if currency_actions >= self.config.max_currency_actions {
                    return Ok(self.report(
                        false,
                        StopReason::MaxCurrencyActions,
                        start,
                        steps,
                        currency_actions,
                        currency_used,
                        &current,
                    ));
                }

                let before_text = current.text.clone();
                let before_fingerprint = current_fingerprint;
                let currency_before = if self.config.verify_currency {
                    Some(self.io.read_currency(currency)?)
                } else {
                    None
                };
                if currency_before == Some(0) {
                    return Err(SessionError::CurrencyMissing { currency });
                }

                self.wait(self.config.pre_action_delay_ms)?;
                self.io.apply_currency(currency)?;
                self.wait(self.config.action_delay_ms)?;

                let currency_after = if self.config.verify_currency {
                    Some(self.io.read_currency(currency)?)
                } else {
                    None
                };
                if let (Some(before), Some(after)) = (currency_before, currency_after) {
                    if after.checked_add(1) != Some(before) {
                        return Err(SessionError::CurrencyNotConsumed {
                            currency,
                            before,
                            after,
                        });
                    }
                }

                let after = self.read_snapshot()?;
                if after.identity != current.identity {
                    return Err(SessionError::ItemChanged);
                }
                let after_fingerprint = fingerprint(&after.text);
                if after_fingerprint == before_fingerprint {
                    same_fingerprint += 1;
                    if same_fingerprint >= self.config.max_same_fingerprint {
                        return Err(SessionError::NoProgress {
                            count: same_fingerprint,
                        });
                    }
                } else {
                    same_fingerprint = 0;
                }

                currency_actions += 1;
                *currency_used.entry(currency.key().to_string()).or_insert(0) += 1;
                total_cost += currency.cost(&self.plan.costs);

                if let Some(logger) = self.logger.as_mut() {
                    logger.write_event(&SessionEvent {
                        step: steps,
                        action: currency_actions,
                        currency: currency.key().to_string(),
                        before_text,
                        after_text: after.text.clone(),
                        currency_before,
                        currency_after,
                        evaluation: after.evaluation.clone(),
                        elapsed_seconds: start.elapsed().as_secs_f64(),
                    })?;
                }

                current = after;
                current_fingerprint = after_fingerprint;

                if current.evaluation.satisfied {
                    return Ok(self.report(
                        true,
                        StopReason::GoalReached,
                        start,
                        steps,
                        currency_actions,
                        currency_used,
                        &current,
                    ));
                }
                if self
                    .config
                    .max_cost
                    .is_some_and(|limit| total_cost >= limit)
                {
                    return Ok(self.report(
                        false,
                        StopReason::CostBudget,
                        start,
                        steps,
                        currency_actions,
                        currency_used,
                        &current,
                    ));
                }
            }
        }
    }

    fn read_snapshot(&mut self) -> Result<Snapshot, SessionError> {
        let text = self.io.read_item()?;
        if text.trim().is_empty() {
            return Err(SessionError::UnsupportedItem(
                "empty item clipboard text".to_string(),
            ));
        }
        let parsed = parse_item(&text);
        match parsed.rarity {
            Some(Rarity::Normal | Rarity::Magic | Rarity::Rare) => {}
            _ => {
                return Err(SessionError::UnsupportedItem(
                    "item must be normal, magic, or rare".to_string(),
                ));
            }
        }
        let item_class = parsed
            .item_class
            .clone()
            .ok_or_else(|| SessionError::UnsupportedItem("missing Item Class".to_string()))?;
        let item_level = parsed
            .item_level
            .ok_or_else(|| SessionError::UnsupportedItem("missing Item Level".to_string()))?;

        let resolved = self.resolver.resolve_item(&parsed);
        let state = poe_craft::ItemState::from_resolved(&resolved);
        let evaluation = evaluate(&self.plan.goal, &state);
        let implicit_signature = resolved
            .item
            .implicit_groups()
            .flat_map(|group| {
                group
                    .lines
                    .iter()
                    .map(|line| line.normalized.template.clone())
            })
            .collect::<Vec<_>>();
        let identity = Identity {
            item_class,
            item_level,
            sockets: parsed.sockets.clone(),
            implicit_signature,
        };
        Ok(Snapshot {
            text,
            parsed,
            state,
            evaluation,
            identity,
        })
    }

    fn check_gate(&mut self) -> Result<(), SessionError> {
        if self.io.emergency_stop() {
            return Err(SessionError::EmergencyStop);
        }
        if self.config.require_foreground && !self.io.is_foreground()? {
            return Err(SessionError::FocusLost);
        }
        Ok(())
    }

    fn wait(&mut self, millis: u64) -> Result<(), SessionError> {
        let mut remaining = millis;
        while remaining > 0 {
            self.check_gate()?;
            let step = remaining.min(50);
            self.io.sleep(step);
            remaining -= step;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn report(
        &self,
        success: bool,
        stop_reason: StopReason,
        start: Instant,
        steps: u32,
        currency_actions: u32,
        currency_used: BTreeMap<String, u32>,
        current: &Snapshot,
    ) -> SessionReport {
        SessionReport {
            success,
            stop_reason,
            steps,
            currency_actions,
            currency_used,
            elapsed_seconds: start.elapsed().as_secs_f64(),
            final_item: current.parsed.clone(),
            final_evaluation: current.evaluation.clone(),
            log_path: self
                .logger
                .as_ref()
                .map(|logger| logger.path().to_path_buf()),
        }
    }
}

fn fingerprint(text: &str) -> u64 {
    let normalized = text.replace("\r\n", "\n");
    let mut hasher = DefaultHasher::new();
    normalized.trim().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::mock::MockGame;
    use poe_data::{BaseItem, DataVersion, Mod, ModDomain, ModType, PobData};

    fn data() -> PobData {
        PobData {
            version: DataVersion::default(),
            mods: vec![
                Mod {
                    id: "Life".to_string(),
                    domain: ModDomain::Explicit,
                    mod_type: ModType::Prefix,
                    affix: "Hale".to_string(),
                    stat_texts: vec!["+(70-89) to maximum Life".to_string()],
                    level: 1,
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
                    affix: "of the Cloud".to_string(),
                    stat_texts: vec!["+(40-45)% to Cold Resistance".to_string()],
                    level: 1,
                    group: "ColdRes".to_string(),
                    weight_keys: vec!["ring".to_string(), "default".to_string()],
                    weight_values: vec![1000, 0],
                    tags: vec![],
                    trade_hashes: vec![],
                    item_types: vec![],
                },
            ],
            bases: vec![BaseItem {
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
            }],
            trade_stats: vec![],
            essences: vec![],
        }
    }

    fn plan() -> Plan {
        Plan::from_toml(
            r#"
schema = 1
name = "session test"

[base]
item_class = "Ring"
base_names = ["Test Ring"]
item_level = 86

[goal]
min_prefixes = 1

[[goal.required]]
slot = "prefix"
group = "Life"

[strategy]
method = "alt_regal"
max_attempts = 100
regal_when_score = 1

[costs]
alt = 0.1
transmute = 0.01
"#,
        )
        .unwrap()
    }

    fn config() -> SessionConfig {
        SessionConfig {
            max_steps: 10,
            max_currency_actions: 20,
            max_seconds: 60,
            max_cost: None,
            max_same_fingerprint: 3,
            pre_action_delay_ms: 0,
            action_delay_ms: 0,
            verify_currency: true,
            require_foreground: true,
        }
    }

    fn item(rarity: &str, name: &str, mods: &[&str]) -> String {
        format!(
            "Item Class: Rings\nRarity: {rarity}\n{name}\n--------\nItem Level: 86\n--------\n{}\n",
            mods.join("\n")
        )
    }

    fn no_life() -> String {
        item("Magic", "Test of the Cloud", &["+40% to Cold Resistance"])
    }

    fn with_life() -> String {
        item("Magic", "Hale Test", &["+80 to maximum Life"])
    }

    fn currencies(alt: u32) -> BTreeMap<Currency, u32> {
        let mut map = BTreeMap::new();
        map.insert(Currency::Alt, alt);
        map
    }

    fn session<'a>(data: &'a PobData, game: &'a mut MockGame) -> CraftSession<'a> {
        CraftSession::new(plan(), data, game, config(), None).unwrap()
    }

    #[test]
    fn applies_currency_and_reaches_goal() {
        let data = data();
        let mut game = MockGame::new(no_life(), vec![with_life()], currencies(10));
        let report = session(&data, &mut game).run().unwrap();
        assert!(report.success);
        assert_eq!(report.stop_reason, StopReason::GoalReached);
        assert_eq!(report.steps, 1);
        assert_eq!(report.currency_used.get("alt"), Some(&1));
        assert_eq!(game.applied, vec![Currency::Alt]);
        assert_eq!(game.currencies.get(&Currency::Alt), Some(&9));
    }

    #[test]
    fn aborts_when_currency_is_missing() {
        let data = data();
        let mut game = MockGame::new(no_life(), vec![with_life()], currencies(0));
        let result = session(&data, &mut game).run();
        assert!(matches!(result, Err(SessionError::CurrencyMissing { .. })));
        assert!(game.applied.is_empty());
    }

    #[test]
    fn aborts_when_currency_was_not_consumed() {
        let data = data();
        let mut game = MockGame::new(no_life(), vec![with_life()], currencies(10));
        game.consume_currency = false;
        let result = session(&data, &mut game).run();
        assert!(matches!(
            result,
            Err(SessionError::CurrencyNotConsumed { .. })
        ));
    }

    #[test]
    fn aborts_on_repeated_unchanged_item() {
        let data = data();
        let mut game = MockGame::new(no_life(), vec![no_life()], currencies(10));
        let mut config = config();
        config.max_same_fingerprint = 2;
        let result = CraftSession::new(plan(), &data, &mut game, config, None)
            .unwrap()
            .run();
        assert!(matches!(result, Err(SessionError::NoProgress { .. })));
    }

    #[test]
    fn aborts_when_game_loses_foreground() {
        let data = data();
        let mut game = MockGame::new(no_life(), vec![with_life()], currencies(10));
        game.foreground = false;
        let result = session(&data, &mut game).run();
        assert!(matches!(result, Err(SessionError::FocusLost)));
    }

    #[test]
    fn aborts_on_emergency_stop() {
        let data = data();
        let mut game = MockGame::new(no_life(), vec![with_life()], currencies(10));
        game.emergency = true;
        let result = session(&data, &mut game).run();
        assert!(matches!(result, Err(SessionError::EmergencyStop)));
    }

    #[test]
    fn stops_at_max_steps() {
        let data = data();
        let mut game = MockGame::new(no_life(), Vec::new(), currencies(10));
        let mut config = config();
        config.max_steps = 2;
        config.max_same_fingerprint = 10;
        let report = CraftSession::new(plan(), &data, &mut game, config, None)
            .unwrap()
            .run()
            .unwrap();
        assert!(!report.success);
        assert_eq!(report.stop_reason, StopReason::MaxSteps);
        assert_eq!(report.steps, 2);
        assert_eq!(game.applied.len(), 2);
    }

    #[test]
    fn aborts_when_target_item_changes() {
        let data = data();
        let changed =
            "Item Class: Boots\nRarity: Magic\nHale Boots\n--------\nItem Level: 86\n--------\n+80 to maximum Life\n"
                .to_string();
        let mut game = MockGame::new(no_life(), vec![changed], currencies(10));
        let result = session(&data, &mut game).run();
        assert!(matches!(result, Err(SessionError::ItemChanged)));
    }

    #[test]
    fn preview_does_not_apply_currency() {
        let data = data();
        let mut game = MockGame::new(no_life(), vec![with_life()], currencies(10));
        let preview = session(&data, &mut game).preview().unwrap();
        assert_eq!(preview.currencies, vec![Currency::Alt]);
        assert!(game.applied.is_empty());
        assert_eq!(game.current, no_life());
    }
}
