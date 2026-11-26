use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use poe_craft::{
    analytic_estimates, evaluate, next_action, simulate_build, simulate_with_options, BaseSpec,
    BuildPlan, ItemState, Plan, PlanFile, Pool, Requirement, Simulator, StepRunner,
};
use poe_data::{load_cache, save_cache, DataVersion, ModDomain, ModType};
use poe_input::{AppConfig, Calibration, Color, Win32Game, Win32GameIo, TARGET_PANEL};
use poe_item::{parse_item, ResolvedItem, Resolver};
use poe_session::{plan_currencies, CraftSession, Currency, SessionConfig, SessionLogger};
use poe_timing::DelaySettings;

#[derive(Debug, Parser)]
#[command(
    name = "poe",
    version,
    about = "Path of Exile crafting data and item inspector"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build or inspect the local data cache.
    Data {
        #[command(subcommand)]
        command: DataCommand,
    },
    /// Parse and resolve an item text file.
    Inspect {
        /// Path to the MessagePack data cache.
        #[arg(long, default_value = "target/poe-data.msgpack")]
        cache: PathBuf,
        /// Item text file (clipboard output).
        item: PathBuf,
        /// Show modifier groups that did not resolve.
        #[arg(long)]
        all: bool,
        /// Print the resolved item as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Validate, simulate, or check a crafting plan.
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// Calibrate and run real crafting sessions.
    Craft {
        #[command(subcommand)]
        command: CraftCommand,
    },
}

#[derive(Debug, Subcommand)]
enum CraftCommand {
    /// Show window, calibration, clipboard, and hotkey status.
    Doctor {
        #[arg(long)]
        calibration: Option<PathBuf>,
        /// Hotkey config TOML; defaults are used if the file is absent.
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Capture a calibrated point by focusing the game and pressing the calibrate key.
    Calibrate {
        /// item, panel, transmute, alt, aug, regal, alch, scour, or chaos.
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        calibration: Option<PathBuf>,
        /// Hotkey config TOML; defaults are used if the file is absent.
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// Print current calibration instead of capturing.
        #[arg(long)]
        list: bool,
        /// Capture the live guard and selected tab without clicking.
        #[arg(long)]
        check: bool,
        /// Seconds to wait for the game and calibrate key.
        #[arg(long, default_value_t = 15)]
        timeout_secs: u64,
    },
    /// Run a plan against the live game with postcondition verification.
    Run {
        #[arg(long, default_value = "target/poe-data.msgpack")]
        cache: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        calibration: Option<PathBuf>,
        /// Read the current item and recommend an action without clicking.
        #[arg(long)]
        dry_run: bool,
        /// Stop after one plan step.
        #[arg(long)]
        once: bool,
        #[arg(long, default_value_t = 10_000)]
        max_steps: u32,
        /// Delay profile TOML; defaults are used if the file is absent.
        #[arg(long, default_value = "delays.toml")]
        delays: PathBuf,
        #[arg(long, default_value_t = 3_600)]
        max_seconds: u64,
        #[arg(long)]
        max_cost: Option<f64>,
        #[arg(long, default_value = "logs")]
        log_dir: PathBuf,
        /// Disable currency count verification. Tab source names and empty tab
        /// slots are still checked before pickup.
        #[arg(long)]
        no_currency_verify: bool,
        /// Re-pick the currency for every use instead of holding Shift.
        #[arg(long)]
        no_hold_currency: bool,
        /// Use the hovered item at the cursor as the item anchor. Focus the
        /// game, hover the item, then press the calibrate hotkey.
        #[arg(long)]
        item_at_cursor: bool,
        /// Hotkey config TOML; defaults are used if the file is absent.
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum PlanCommand {
    /// Validate a plan against the current data cache.
    Validate {
        #[arg(long, default_value = "target/poe-data.msgpack")]
        cache: PathBuf,
        /// Plan TOML file.
        #[arg(long)]
        plan: PathBuf,
    },
    /// Monte Carlo simulate a plan.
    Simulate {
        #[arg(long, default_value = "target/poe-data.msgpack")]
        cache: PathBuf,
        /// Plan TOML file.
        #[arg(long)]
        plan: PathBuf,
        /// Number of simulated runs.
        #[arg(long, default_value_t = 10_000)]
        runs: u64,
        /// Deterministic RNG seed.
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Stop after roughly this many attempts; 0 means unlimited.
        #[arg(long, default_value_t = 100_000_000)]
        max_total_attempts: u64,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Evaluate an item file against a plan and recommend the next action.
    Check {
        #[arg(long, default_value = "target/poe-data.msgpack")]
        cache: PathBuf,
        /// Plan TOML file.
        #[arg(long)]
        plan: PathBuf,
        /// Item text file (clipboard output).
        item: PathBuf,
        /// Print the evaluation as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Search explicit mod groups for plan authoring.
    Groups {
        #[arg(long, default_value = "target/poe-data.msgpack")]
        cache: PathBuf,
        /// Case-insensitive text to search in group, affix, or stat text.
        #[arg(long, default_value = "")]
        search: String,
        /// Maximum groups to print.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

#[derive(Debug, Subcommand)]
enum DataCommand {
    /// Import a Path of Building data directory and write the cache.
    Build {
        /// Directory containing src/Data (or the data files directly).
        #[arg(long, default_value = "data/pob")]
        pob_dir: PathBuf,
        /// Output cache path.
        #[arg(long, default_value = "target/poe-data.msgpack")]
        out: PathBuf,
        /// Optional data/pob-lock.json to copy commit metadata from.
        #[arg(long)]
        lock: Option<PathBuf>,
    },
    /// Print information about a data cache.
    Info {
        #[arg(long, default_value = "target/poe-data.msgpack")]
        cache: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Data { command } => match command {
            DataCommand::Build { pob_dir, out, lock } => {
                build_data(pob_dir.as_path(), out.as_path(), lock.as_deref())
            }
            DataCommand::Info { cache } => data_info(cache.as_path()),
        },
        Command::Inspect {
            cache,
            item,
            all,
            json,
        } => inspect(cache.as_path(), item.as_path(), all, json),
        Command::Craft { command } => match command {
            CraftCommand::Doctor {
                calibration,
                config,
            } => craft_doctor(calibration.as_deref(), &config),
            CraftCommand::Calibrate {
                target,
                calibration,
                config,
                list,
                check,
                timeout_secs,
            } => craft_calibrate(
                target.as_deref(),
                calibration.as_deref(),
                &config,
                list,
                check,
                timeout_secs,
            ),
            CraftCommand::Run {
                cache,
                plan,
                calibration,
                dry_run,
                once,
                max_steps,
                delays,
                max_seconds,
                max_cost,
                log_dir,
                no_currency_verify,
                no_hold_currency,
                item_at_cursor,
                config,
                json,
            } => craft_run(CraftRunArgs {
                cache: &cache,
                plan: &plan,
                calibration: calibration.as_deref(),
                dry_run,
                once,
                max_steps,
                delays: &delays,
                max_seconds,
                max_cost,
                log_dir: &log_dir,
                verify_currency: !no_currency_verify,
                hold_currency: !no_hold_currency,
                item_at_cursor,
                config: &config,
                json,
            }),
        },
        Command::Plan { command } => match command {
            PlanCommand::Validate { cache, plan } => plan_validate(cache.as_path(), plan.as_path()),
            PlanCommand::Simulate {
                cache,
                plan,
                runs,
                seed,
                max_total_attempts,
                json,
            } => plan_simulate(
                cache.as_path(),
                plan.as_path(),
                runs,
                seed,
                max_total_attempts,
                json,
            ),
            PlanCommand::Check {
                cache,
                plan,
                item,
                json,
            } => plan_check(cache.as_path(), plan.as_path(), item.as_path(), json),
            PlanCommand::Groups {
                cache,
                search,
                limit,
            } => plan_groups(cache.as_path(), &search, limit),
        },
    }
}

fn build_data(pob_dir: &Path, out: &Path, lock: Option<&Path>) -> Result<()> {
    let mut data = poe_data::from_pob_dir(pob_dir)
        .with_context(|| format!("importing PoB data from {}", pob_dir.display()))?;
    if let Some(lock) = lock {
        apply_lock(&mut data.version, lock)?;
    }
    save_cache(&data, out).with_context(|| format!("writing cache to {}", out.display()))?;
    println!(
        "built {} mods, {} bases, {} trade stats, {} essences -> {}",
        data.mods.len(),
        data.bases.len(),
        data.trade_stats.len(),
        data.essences.len(),
        out.display()
    );
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct LockFile {
    source: LockSource,
}

#[derive(Debug, serde::Deserialize)]
struct LockSource {
    repo: String,
    commit: String,
    commit_date: String,
}

fn apply_lock(version: &mut DataVersion, path: &std::path::Path) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading lock file {}", path.display()))?;
    let lock: LockFile = serde_json::from_str(&text).context("parsing lock file")?;
    version.source = lock.source.repo;
    version.commit = lock.source.commit;
    version.commit_date = lock.source.commit_date;
    Ok(())
}

fn data_info(cache: &Path) -> Result<()> {
    let data = load_data(cache)?;
    println!("source:       {}", data.version.source);
    println!("commit:       {}", data.version.commit);
    println!("commit date:  {}", data.version.commit_date);
    println!("built at:     {}", data.version.built_at);
    println!("files:        {}", data.version.files);
    println!("mods:         {}", data.mods.len());
    println!("bases:        {}", data.bases.len());
    println!("trade stats:  {}", data.trade_stats.len());
    println!("essences:     {}", data.essences.len());

    let mut domains: Vec<(String, usize)> = Vec::new();
    for domain in [
        poe_data::ModDomain::Explicit,
        poe_data::ModDomain::Corrupted,
        poe_data::ModDomain::Delve,
        poe_data::ModDomain::Eldritch,
        poe_data::ModDomain::Master,
        poe_data::ModDomain::Veiled,
        poe_data::ModDomain::Tincture,
        poe_data::ModDomain::Flask,
        poe_data::ModDomain::Jewel,
        poe_data::ModDomain::JewelAbyss,
        poe_data::ModDomain::JewelCharm,
        poe_data::ModDomain::JewelCluster,
        poe_data::ModDomain::ItemExclusive,
        poe_data::ModDomain::Map,
        poe_data::ModDomain::Unknown,
    ] {
        let count = data
            .mods
            .iter()
            .filter(|entry| entry.domain == domain)
            .count();
        if count > 0 {
            domains.push((format!("{domain:?}"), count));
        }
    }
    for (domain, count) in domains {
        println!("  {domain:<14} {count}");
    }
    Ok(())
}

fn inspect(cache: &Path, item_path: &Path, all: bool, json: bool) -> Result<()> {
    let data = load_data(cache)?;
    let text = std::fs::read_to_string(item_path)
        .with_context(|| format!("reading item {}", item_path.display()))?;
    let item = parse_item(&text);
    let resolver = Resolver::new(&data);
    let resolved = resolver.resolve_item(&item);
    if json {
        println!("{}", serde_json::to_string_pretty(&resolved)?);
    } else {
        print_resolved(&resolved, all);
    }
    Ok(())
}

fn load_plan(path: &Path) -> Result<PlanFile> {
    PlanFile::load(path).with_context(|| format!("loading plan {}", path.display()))
}

fn load_data(cache: &Path) -> Result<poe_data::PobData> {
    if poe_data::cache_is_current(cache) {
        match load_cache(cache) {
            Ok(data) => return Ok(data),
            Err(error) => {
                eprintln!(
                    "Data cache {} could not be decoded ({error}); rebuilding...",
                    cache.display()
                );
                let _ = std::fs::remove_file(cache);
            }
        }
    }

    let lock = Path::new("data/pob-lock.json");
    let data_dir = Path::new("data/pob");
    eprintln!(
        "Data cache {} is missing or outdated; downloading pinned Path of Building data...",
        cache.display()
    );
    let mut progress = |progress: poe_data::DataProgress| {
        if progress.downloaded || progress.completed == progress.total {
            eprintln!(
                "Data {}/{}: {}{}",
                progress.completed,
                progress.total,
                progress.path,
                if progress.downloaded { "" } else { " (reused)" }
            );
        }
    };
    let outcome = poe_data::ensure_cache(lock, data_dir, cache, Some(&mut progress))
        .with_context(|| format!("building data cache {}", cache.display()))?;
    eprintln!(
        "Data cache built: {} files, {} downloaded, {} reused",
        outcome.files, outcome.downloaded, outcome.skipped
    );
    load_cache(cache).with_context(|| format!("loading cache {}", cache.display()))
}

fn plan_validate(cache: &Path, plan_path: &Path) -> Result<()> {
    let data = load_data(cache)?;
    match load_plan(plan_path)? {
        PlanFile::Legacy(plan) => {
            let simulator = Simulator::new(&data, plan).context("validating plan")?;
            print_plan_summary(&simulator.plan, &simulator.pool);
            println!();
            println!("plan is valid");
        }
        PlanFile::Steps(plan) => {
            let summary = plan.validate(&data).context("validating plan")?;
            let pool = plan.pool(&data).context("building plan pool")?;
            print_build_summary(&plan, &summary);
            println!();
            for (index, estimate) in analytic_estimates(&plan, &pool).into_iter().enumerate() {
                let step = &plan.steps[index];
                match estimate {
                    Some(estimate) => println!(
                        "Step {}: {} | p={:.4}% expected {:.2} attempts, confidence attempts {}",
                        index + 1,
                        step.name,
                        estimate.p_success.unwrap_or(0.0) * 100.0,
                        estimate.expected_attempts.unwrap_or(f64::INFINITY),
                        estimate
                            .confidence_attempts
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".to_string())
                    ),
                    None => println!(
                        "Step {}: {} ({}) | run simulation for probability",
                        index + 1,
                        step.name,
                        step.action.label()
                    ),
                }
            }
            println!();
            println!("plan is valid");
        }
    }
    Ok(())
}

fn print_build_summary(plan: &BuildPlan, summary: &poe_craft::BuildSummary) {
    println!("Plan: {}", plan.name);
    println!(
        "Base: {} (item level {}, unrestricted {}, {} tags)",
        summary.base,
        summary.item_level,
        plan.base.unrestricted,
        summary.base_tags.len()
    );
    if !plan.base.influences.is_empty() {
        println!(
            "Influences: {}",
            plan.base
                .influences
                .iter()
                .map(|influence| influence.display())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "Pool: {} prefix mods / {} suffix mods; {} steps",
        summary.prefixes, summary.suffixes, summary.steps
    );
    for (index, step) in plan.steps.iter().enumerate() {
        println!(
            "  steps[{}]: {} | {} | on_fail {:?} | accept {}",
            index,
            step.name,
            step.action.label(),
            step.on_fail,
            if step.goal.is_some() {
                "yes"
            } else {
                "no (advance)"
            }
        );
    }
}

fn print_plan_summary(plan: &Plan, pool: &Pool) {
    println!("Plan: {}", plan.name);
    println!(
        "Base: {} (item level {}, {} tags)",
        pool.base_label,
        pool.item_level,
        pool.base_tags.len()
    );
    println!("Tags: {}", pool.base_tags.join(", "));

    let prefix_groups = pool
        .prefixes
        .iter()
        .map(|entry| entry.group.as_str())
        .collect::<BTreeSet<_>>();
    let suffix_groups = pool
        .suffixes
        .iter()
        .map(|entry| entry.group.as_str())
        .collect::<BTreeSet<_>>();
    println!(
        "Pool: {} prefix mods / {} suffix mods; {} prefix groups / {} suffix groups",
        pool.prefixes.len(),
        pool.suffixes.len(),
        prefix_groups.len(),
        suffix_groups.len()
    );

    println!(
        "Goal: {} required, {} weighted, min score {}",
        plan.goal.required.len(),
        plan.goal.weighted.len(),
        plan.goal.min_score
    );
    for (index, requirement) in plan.goal.required.iter().enumerate() {
        print_requirement(&format!("required[{index}]"), requirement);
    }
    for (index, requirement) in plan.goal.weighted.iter().enumerate() {
        print_requirement(&format!("weighted[{index}]"), requirement);
    }
    println!(
        "Counts: prefixes {}-{}, suffixes {}-{}, open prefixes >= {}, open suffixes >= {}",
        plan.goal.min_prefixes.unwrap_or(0),
        plan.goal.max_prefixes.unwrap_or(3),
        plan.goal.min_suffixes.unwrap_or(0),
        plan.goal.max_suffixes.unwrap_or(3),
        plan.goal.min_open_prefixes.unwrap_or(0),
        plan.goal.min_open_suffixes.unwrap_or(0)
    );
    println!(
        "Strategy: {:?}, augment {:?}, regal_when_score {}, max attempts {}, start {:?}",
        plan.strategy.method,
        plan.strategy.augment,
        plan.strategy.regal_when_score,
        plan.strategy.max_attempts,
        plan.strategy.start
    );
    println!(
        "Costs: alt {:.3}, aug {:.3}, regal {:.3}, alch {:.3}, scour {:.3}, chaos {:.3}, transmute {:.3}",
        plan.costs.alt,
        plan.costs.aug,
        plan.costs.regal,
        plan.costs.alch,
        plan.costs.scour,
        plan.costs.chaos,
        plan.costs.transmute
    );
}

fn print_requirement(label: &str, requirement: &Requirement) {
    let label = requirement.label.as_deref().unwrap_or(label);
    let mut parts = vec![format!("{:?}", requirement.slot)];
    if let Some(group) = &requirement.group {
        parts.push(format!("group={group}"));
    }
    if let Some(affix) = &requirement.affix {
        parts.push(format!("affix={affix}"));
    }
    if let Some(tier) = requirement.min_tier {
        parts.push(format!("tier<={tier}"));
    }
    if let Some(value) = requirement.min_value {
        parts.push(format!("value>={value}"));
    }
    if let Some(percentile) = requirement.min_percentile {
        parts.push(format!("percentile>={percentile}"));
    }
    if requirement.weight != 1 {
        parts.push(format!("weight={}", requirement.weight));
    }
    println!("  {label}: {}", parts.join(" "));
}

fn plan_simulate(
    cache: &Path,
    plan_path: &Path,
    runs: u64,
    seed: u64,
    max_total_attempts: u64,
    json: bool,
) -> Result<()> {
    let data = load_data(cache)?;
    let plan = load_plan(plan_path)?;
    let budget = (max_total_attempts > 0).then_some(max_total_attempts);
    let show_progress = !json && runs >= 100;
    let mut last_progress = 0u64;
    let mut progress = |completed: u64, total: u64| {
        if !show_progress {
            return;
        }
        let percent = completed.saturating_mul(100) / total.max(1);
        if percent >= last_progress.saturating_add(10) || completed == total {
            last_progress = percent;
            eprintln!("progress: {completed}/{total} runs");
        }
    };
    match plan {
        PlanFile::Legacy(plan) => {
            let report =
                simulate_with_options(&data, plan, runs, seed, budget, Some(&mut progress))
                    .context("simulating plan")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
        }
        PlanFile::Steps(plan) => {
            let report = simulate_build(&data, plan, runs, seed, budget, Some(&mut progress))
                .context("simulating build plan")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_build_report(&report);
            }
        }
    }
    Ok(())
}

fn print_build_report(report: &poe_craft::BuildSimulationReport) {
    println!("Plan: {}", report.plan_name);
    println!("Base: {} (item level {})", report.base, report.item_level);
    println!(
        "Runs: {} requested, {} completed (seed {})",
        report.runs, report.completed_runs, report.seed
    );
    if report.truncated {
        println!(
            "Budget: stopped after {} attempts (limit {})",
            report.total_attempts,
            report
                .attempt_budget
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }
    println!(
        "Success: {:.2}% ({}/{})",
        report.success_rate * 100.0,
        report.successes,
        report.completed_runs
    );
    println!(
        "Attempts: mean {:.2}{}",
        report.mean_attempts,
        report
            .mean_success_attempts
            .map(|value| format!("; success mean {value:.2}"))
            .unwrap_or_default()
    );
    println!(
        "Cost: mean {:.3}{}",
        report.mean_cost,
        report
            .mean_cost_success
            .map(|value| format!("; success mean {value:.3}"))
            .unwrap_or_default()
    );
    println!("Currency/run: {}", format_map(&report.mean_currency));
    if let Some(success_map) = &report.mean_currency_success {
        println!("Currency/success: {}", format_map(success_map));
    }
    println!("Steps:");
    for step in &report.steps {
        println!(
            "  {}: p={} expected {} confidence {} | actions {} accepts {} | {}",
            step.step_index + 1,
            step.p_success
                .map(|value| format!("{:.4}%", value * 100.0))
                .unwrap_or_else(|| "-".to_string()),
            step.expected_attempts
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "-".to_string()),
            step.confidence_attempts
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            step.actions,
            step.accepts,
            step.note.clone().unwrap_or_default()
        );
    }
}

fn print_report(report: &poe_craft::SimulationReport) {
    println!("Plan: {}", report.plan_name);
    println!("Base: {} (item level {})", report.base, report.item_level);
    println!(
        "Runs: {} requested, {} completed (seed {})",
        report.runs, report.completed_runs, report.seed
    );
    if report.truncated {
        println!(
            "Budget: stopped after {} attempts (limit {})",
            report.total_attempts,
            report
                .attempt_budget
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }
    println!(
        "Success: {:.2}% ({}/{})",
        report.success_rate * 100.0,
        report.successes,
        report.completed_runs
    );
    println!(
        "Attempts: mean {:.2}{}",
        report.mean_attempts,
        report
            .mean_success_attempts
            .map(|value| format!("; success mean {value:.2}"))
            .unwrap_or_default()
    );
    println!(
        "Success attempts: p50 {}, p90 {}, p99 {}",
        format_optional_u64(report.p50_success_attempts),
        format_optional_u64(report.p90_success_attempts),
        format_optional_u64(report.p99_success_attempts)
    );
    println!(
        "Cost: mean {:.3}{}",
        report.mean_cost,
        report
            .mean_cost_success
            .map(|value| format!("; success mean {value:.3}"))
            .unwrap_or_default()
    );
    println!("Currency/run: {}", format_map(&report.mean_currency));
    if let Some(success_map) = &report.mean_currency_success {
        println!("Currency/success: {}", format_map(success_map));
    }
    println!(
        "Failures: max attempts {}, no eligible mods {}",
        report.max_attempts_reached, report.failed_no_mods
    );
}

fn format_optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn format_map(map: &BTreeMap<String, f64>) -> String {
    if map.is_empty() {
        return "-".to_string();
    }
    map.iter()
        .map(|(name, value)| format!("{name} {value:.2}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn plan_base_warning(base: &BaseSpec, item: &poe_item::ParsedItem, pool: &Pool) -> Option<String> {
    if let (Some(expected), Some(actual)) = (&base.item_class, &item.item_class) {
        if !poe_craft::class_matches(actual, expected) {
            return Some(format!(
                "item class {actual} does not match plan class {expected} (target {})",
                pool.base_label
            ));
        }
    }
    if !base.base_names.is_empty() {
        if let Some(actual_base) = &item.base_type {
            let matches = base.base_names.iter().any(|name| {
                actual_base == name || actual_base.ends_with(name) || actual_base.contains(name)
            });
            if !matches {
                return Some(format!(
                    "item base {actual_base} does not match plan base {}",
                    base.base_names.join(" / ")
                ));
            }
        }
    }
    None
}

fn plan_check(cache: &Path, plan_path: &Path, item_path: &Path, json: bool) -> Result<()> {
    let data = load_data(cache)?;
    let plan = load_plan(plan_path)?;
    let text = std::fs::read_to_string(item_path)
        .with_context(|| format!("reading item {}", item_path.display()))?;
    let parsed = parse_item(&text);
    let resolver = Resolver::new(&data);
    let resolved = resolver.resolve_item(&parsed);
    let state = ItemState::from_resolved(&resolved);

    let (plan_name, base, pool, evaluation, action, step_info, goal_min_score) = match &plan {
        PlanFile::Legacy(plan) => {
            let pool = Pool::from_plan(&data, plan).context("validating plan against data")?;
            (
                plan.name.clone(),
                plan.base.clone(),
                pool,
                evaluate(&plan.goal, &state),
                next_action(plan, &state),
                None,
                plan.goal.min_score,
            )
        }
        PlanFile::Steps(plan) => {
            let pool = plan
                .validate_pool(&data)
                .context("validating plan against data")?;
            let mut runner = StepRunner::new();
            runner.observe(plan, &state);
            let goal = if runner.finished {
                plan.final_goal()
            } else {
                runner.target_goal(plan)
            };
            let evaluation = goal
                .map(|goal| evaluate(goal, &state))
                .unwrap_or_else(|| evaluate(&poe_craft::Goal::default(), &state));
            let action = if runner.finished || runner.failed.is_some() {
                poe_craft::NextAction::Stop
            } else {
                runner.next_action(plan, &state)
            };
            let step_info = plan
                .steps
                .get(runner.current_step)
                .map(|step| (runner.current_step, step.name.clone(), plan.steps.len()));
            let summary = goal.map(|goal| goal.min_score).unwrap_or(0);
            (
                plan.name.clone(),
                plan.base.clone(),
                pool,
                evaluation,
                action,
                step_info,
                summary,
            )
        }
    };

    let base_warning = plan_base_warning(&base, &parsed, &pool);
    let recommended = base_warning.is_none().then_some(action);

    if json {
        let value = serde_json::json!({
            "plan": plan_name,
            "item": state,
            "evaluation": evaluation,
            "next_action": recommended,
            "base_warning": base_warning,
            "step_index": step_info.as_ref().map(|(index, _, _)| *index),
            "step_name": step_info.as_ref().map(|(_, name, _)| name.clone()),
            "step_total": step_info.as_ref().map(|(_, _, total)| *total),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("Plan: {plan_name}");
        if let Some(warning) = &base_warning {
            println!("Warning: {warning}");
        }
        println!(
            "Item: {} / {} | {:?} | prefixes {} | suffixes {} | open {} / {}",
            parsed.item_class.as_deref().unwrap_or("?"),
            parsed.name.as_deref().unwrap_or("?"),
            state.rarity,
            state.prefix_count(),
            state.suffix_count(),
            state.open_prefixes(),
            state.open_suffixes()
        );
        if let Some((index, name, total)) = &step_info {
            println!("Step: {}/{} | {name}", index + 1, total);
        }
        println!(
            "Goal: {} | score {}/{} | required {}/{} | weighted {}/{}",
            if evaluation.satisfied {
                "satisfied"
            } else {
                "not satisfied"
            },
            evaluation.score,
            goal_min_score,
            evaluation.required_matched,
            evaluation.required_total,
            evaluation.weighted_matched,
            evaluation.weighted_total
        );
        if !evaluation.counts_ok {
            println!("Counts: not satisfied");
        }
        if !evaluation.missing_required.is_empty() {
            println!(
                "Missing required: {:?}",
                evaluation
                    .missing_required
                    .iter()
                    .map(|index| format!("required[{index}]"))
                    .collect::<Vec<_>>()
            );
        }
        if evaluation.distinct_failure {
            println!("Missing required: requirements match mods but not distinct mods");
        }
        match recommended {
            Some(action) => println!("Next: {action}"),
            None => println!("Next: hold (item does not match plan base)"),
        }
    }
    Ok(())
}

fn plan_groups(cache: &Path, search: &str, limit: usize) -> Result<()> {
    let data = load_data(cache)?;
    let needle = search.to_ascii_lowercase();
    let mut groups: BTreeMap<String, GroupInfo> = BTreeMap::new();

    for entry in &data.mods {
        if entry.domain != ModDomain::Explicit
            || !matches!(entry.mod_type, ModType::Prefix | ModType::Suffix)
        {
            continue;
        }
        if !needle.is_empty() {
            let haystack = format!(
                "{} {} {}",
                entry.group,
                entry.affix,
                entry.stat_texts.join(" ")
            )
            .to_ascii_lowercase();
            if !haystack.contains(&needle) {
                continue;
            }
        }
        let group = groups
            .entry(entry.group.clone())
            .or_insert_with(|| GroupInfo {
                slots: BTreeSet::new(),
                min_level: entry.level,
                max_level: entry.level,
                count: 0,
                example: entry.stat_texts.first().cloned().unwrap_or_default(),
            });
        group
            .slots
            .insert(format!("{:?}", entry.mod_type).to_ascii_lowercase());
        group.min_level = group.min_level.min(entry.level);
        group.max_level = group.max_level.max(entry.level);
        group.count += 1;
    }

    for (group, info) in groups.into_iter().take(limit) {
        println!(
            "{:<42} {:<13} levels {}-{} | {} mods | {}",
            group,
            info.slots.into_iter().collect::<Vec<_>>().join("/"),
            info.min_level,
            info.max_level,
            info.count,
            info.example
        );
    }
    Ok(())
}

struct GroupInfo {
    slots: BTreeSet<String>,
    min_level: i64,
    max_level: i64,
    count: usize,
    example: String,
}

struct CraftRunArgs<'a> {
    cache: &'a Path,
    plan: &'a Path,
    calibration: Option<&'a Path>,
    dry_run: bool,
    once: bool,
    max_steps: u32,
    delays: &'a Path,
    max_seconds: u64,
    max_cost: Option<f64>,
    log_dir: &'a Path,
    verify_currency: bool,
    hold_currency: bool,
    item_at_cursor: bool,
    config: &'a Path,
    json: bool,
}

fn calibration_path(arg: Option<&Path>) -> PathBuf {
    arg.map(Path::to_path_buf)
        .unwrap_or_else(Calibration::default_path)
}

fn craft_doctor(calibration_arg: Option<&Path>, config_path: &Path) -> Result<()> {
    let path = calibration_path(calibration_arg);
    let config = AppConfig::load_or_default(config_path)
        .with_context(|| format!("loading hotkey config {}", config_path.display()))?;
    let hotkeys = config
        .hotkeys
        .parse()
        .with_context(|| format!("validating hotkey config {}", config_path.display()))?;
    let mut game = Win32Game::find("Path of Exile").context("finding game window")?;
    let info = game.window_info()?;
    println!("Window: {}", info.title);
    println!(
        "Client: {}x{} at {},{} (foreground: {})",
        info.client_width, info.client_height, info.origin_x, info.origin_y, info.foreground
    );
    if info.client_width <= 0 || info.client_height <= 0 {
        println!("Warning: client area is empty; is the game minimized?");
    }
    match game.clipboard_text() {
        Ok(text) => println!("Clipboard: readable, {} chars", text.chars().count()),
        Err(error) => println!("Clipboard: {error}"),
    }
    println!(
        "Hotkeys: start {}, stop {}, calibrate {}, copy {}",
        config.hotkeys.start, config.hotkeys.stop, config.hotkeys.calibrate, config.hotkeys.copy
    );
    game.apply_hotkeys(hotkeys);
    if path.exists() {
        let calibration = Calibration::load(&path)?;
        println!("Calibration: {}", path.display());
        let missing = calibration.missing(Calibration::all_targets());
        if missing.is_empty() {
            println!("Targets: all calibrated");
        } else {
            println!("Missing targets: {}", missing.join(", "));
        }
        match &calibration.guard {
            Some(guard) => println!(
                "Image guard: {} ({:.4},{:.4} {:.4}x{:.4}) threshold {:.2}",
                guard.reference.display(),
                guard.region.x,
                guard.region.y,
                guard.region.w,
                guard.region.h,
                guard.threshold
            ),
            None => match calibration.panel {
                Some(panel) => println!(
                    "Panel guard: legacy pixel {:?} at ({:.4}, {:.4}) +/- {}",
                    panel.color, panel.point.x, panel.point.y, panel.tolerance
                ),
                None => println!("Panel guard: not calibrated (optional)"),
            },
        }
        println!(
            "Strips: horizontal {}, vertical {}",
            if calibration.strips.horizontal.is_some() {
                "on"
            } else {
                "off"
            },
            if calibration.strips.vertical.is_some() {
                "on"
            } else {
                "off"
            }
        );
        if let Some(guard) = &calibration.guard {
            let resolved = poe_input::resolve_relative(&path, &guard.reference);
            if !resolved.exists() {
                println!("Warning: guard template is missing: {}", resolved.display());
            }
        }
        for tab in &calibration.tabs {
            println!(
                "Tab {}: {} {}x{} grid at ({:.3},{:.3})",
                tab.index,
                if tab.name.is_empty() {
                    &tab.kind
                } else {
                    &tab.name
                },
                tab.content.columns,
                tab.content.rows,
                tab.content.region.x,
                tab.content.region.y
            );
            for template in [Some(&tab.selected), tab.unselected.as_ref()]
                .into_iter()
                .flatten()
            {
                let resolved = poe_input::resolve_relative(&path, template);
                if !resolved.exists() {
                    println!("  Warning: missing tab template {}", resolved.display());
                }
            }
        }
        if !calibration.items.is_empty() {
            println!(
                "Items: {}",
                calibration
                    .items
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if let (Some(width), Some(height)) =
            (calibration.captured_width, calibration.captured_height)
        {
            if width != info.client_width || height != info.client_height {
                println!(
                    "Warning: templates were calibrated at {width}x{height}; current client is {}x{}.",
                    info.client_width, info.client_height
                );
            }
        }
    } else {
        println!("Calibration: missing (expected {})", path.display());
    }
    println!("Emergency stop: {}", config.hotkeys.stop);
    Ok(())
}

fn craft_calibrate(
    target: Option<&str>,
    calibration_arg: Option<&Path>,
    config_path: &Path,
    list: bool,
    check: bool,
    timeout_secs: u64,
) -> Result<()> {
    let path = calibration_path(calibration_arg);
    let mut calibration = if path.exists() {
        Calibration::load(&path)?
    } else {
        Calibration::empty()
    };

    if check {
        let game = Win32Game::find(&calibration.game_title).context("finding game window")?;
        game.focus()
            .context("bringing Path of Exile to the foreground")?;
        std::thread::sleep(Duration::from_millis(200));
        let mut io = Win32GameIo::new(game, calibration, Some(path.clone()))
            .map_err(|error| anyhow::anyhow!("loading vision templates: {error}"))?;
        print!(
            "{}",
            io.test_vision().map_err(|error| anyhow::anyhow!(error))?
        );
        return Ok(());
    }

    if list || target.is_none() {
        print_calibration(&calibration, &path);
        if target.is_none() && !list {
            println!();
            println!("Run with --target <item|panel|transmute|alt|aug|regal|alch|scour|chaos>.");
            println!("Focus the game, hover the target, and press the calibrate hotkey.");
        }
        return Ok(());
    }

    let target = target.unwrap();
    if !Calibration::capturable_targets().contains(&target) {
        anyhow::bail!(
            "unknown calibration target `{target}`; expected one of {}",
            Calibration::capturable_targets().join(", ")
        );
    }

    let config = AppConfig::load_or_default(config_path)
        .with_context(|| format!("loading hotkey config {}", config_path.display()))?;
    let hotkeys = config
        .hotkeys
        .parse()
        .with_context(|| format!("validating hotkey config {}", config_path.display()))?;
    let mut game = Win32Game::find(&calibration.game_title).context("finding game window")?;
    game.apply_hotkeys(hotkeys);
    let info = game.window_info()?;
    calibration.set_capture_size(info.client_width, info.client_height);
    let hint = if target == TARGET_PANEL {
        "a stable panel pixel (not the item or a currency slot)"
    } else {
        "the target slot"
    };
    println!(
        "Focus {}, hover {}, then press {} within {}s.",
        info.title, hint, config.hotkeys.calibrate, timeout_secs
    );
    let point = game
        .capture_point(Duration::from_secs(timeout_secs))
        .context("capturing calibration point")?;
    if target == TARGET_PANEL {
        let color: Color = game.sample_color(point).context("sampling panel color")?;
        calibration.set_panel(point, color)?;
        calibration.save(&path)?;
        println!(
            "saved panel guard {:?} at ({:.4}, {:.4}) to {}",
            color,
            point.x,
            point.y,
            path.display()
        );
    } else {
        calibration.set_target(target, point)?;
        calibration.save(&path)?;
        println!(
            "saved {} at ({:.4}, {:.4}) to {}",
            target,
            point.x,
            point.y,
            path.display()
        );
    }
    Ok(())
}

fn print_calibration(calibration: &Calibration, path: &Path) {
    println!("Calibration: {}", path.display());
    println!("Game title: {}", calibration.game_title);
    for target in Calibration::all_targets() {
        match calibration.point(target) {
            Some(point) => println!("  {target:<10} ({:.4}, {:.4})", point.x, point.y),
            None => println!("  {target:<10} missing"),
        }
    }
    match &calibration.guard {
        Some(guard) => println!(
            "  {:<10} image {} threshold {:.2}",
            "guard",
            guard.reference.display(),
            guard.threshold
        ),
        None => match calibration.panel {
            Some(panel) => println!(
                "  {:<10} legacy pixel ({:.4}, {:.4}) {:?} +/- {}",
                TARGET_PANEL, panel.point.x, panel.point.y, panel.color, panel.tolerance
            ),
            None => println!("  {:<10} missing (optional)", TARGET_PANEL),
        },
    }
    println!(
        "  strips    horizontal {}, vertical {}",
        if calibration.strips.horizontal.is_some() {
            "on"
        } else {
            "off"
        },
        if calibration.strips.vertical.is_some() {
            "on"
        } else {
            "off"
        }
    );
    for tab in &calibration.tabs {
        println!(
            "  tab {:<3} {} {}x{} grid",
            tab.index,
            if tab.name.is_empty() {
                &tab.kind
            } else {
                &tab.name
            },
            tab.content.columns,
            tab.content.rows
        );
    }
    for (key, item) in &calibration.items {
        match &item.source {
            poe_input::ItemSource::Inventory { x, y } => println!(
                "  item {key:<10} {}, inventory ({x:.4}, {y:.4})",
                item.display_name
            ),
            poe_input::ItemSource::Tab { tab, column, row } => println!(
                "  item {key:<10} {}, tab {tab} cell ({column}, {row})",
                item.display_name
            ),
        }
    }
}

fn craft_run(args: CraftRunArgs<'_>) -> Result<()> {
    let data = load_data(args.cache)?;
    let plan = load_plan(args.plan)?;
    let path = calibration_path(args.calibration);
    let calibration = Calibration::load(&path)
        .with_context(|| format!("loading calibration {}", path.display()))?;
    let used_currencies = plan_currencies(&plan).unwrap_or_else(|| Currency::ALL.to_vec());
    if !args.dry_run {
        let required = used_currencies
            .iter()
            .map(|currency| currency.key())
            .collect::<Vec<_>>();
        let missing = calibration.missing(&required);
        if !missing.is_empty() {
            anyhow::bail!(
                "calibration is incomplete; missing sources: {}. Configure inventory points or tab items in the UI, or run `poe craft calibrate --target ...`.",
                missing.join(", ")
            );
        }
    }

    let delays = DelaySettings::load_or_default(args.delays)
        .with_context(|| format!("loading delay profile {}", args.delays.display()))?;
    print_delays(&delays, args.delays);
    if let Some(guard) = &calibration.guard {
        println!(
            "Image guard: on ({} threshold {:.2})",
            guard.reference.display(),
            guard.threshold
        );
    } else if calibration.panel.is_some() {
        println!("Panel guard: legacy pixel probe");
    } else {
        println!("Panel guard: off (optional)");
    }
    if !calibration.tabs.is_empty() {
        let names = calibration
            .tabs
            .iter()
            .map(|tab| {
                format!(
                    "{}:{}",
                    tab.index,
                    if tab.name.is_empty() {
                        &tab.kind
                    } else {
                        &tab.name
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!("Stash tabs: {names}");
    }

    let app_config = AppConfig::load_or_default(args.config)
        .with_context(|| format!("loading hotkey config {}", args.config.display()))?;
    let hotkeys = app_config
        .hotkeys
        .parse()
        .with_context(|| format!("validating hotkey config {}", args.config.display()))?;
    let mut game = Win32Game::find(&calibration.game_title).context("finding game window")?;
    game.set_action_delay(delays.action);
    game.apply_hotkeys(hotkeys);
    let mut anchor = calibration.item;
    if args.item_at_cursor {
        game.focus()
            .context("bringing Path of Exile to the foreground")?;
        println!(
            "Hover the item, then press {} within 30s to capture the item anchor.",
            app_config.hotkeys.calibrate
        );
        let point = game
            .capture_point(Duration::from_secs(30))
            .context("capturing the hovered item")?;
        anchor = Some(point);
    }
    let Some(anchor) = anchor else {
        anyhow::bail!(
            "no item anchor; pass --item-at-cursor and hover the item, or run `poe craft calibrate --target item`."
        );
    };
    let mut io = Win32GameIo::new(game, calibration, Some(path.clone()))
        .map_err(|error| anyhow::anyhow!("loading vision templates: {error}"))?;
    io.set_diagnostics_dir(args.log_dir.to_path_buf());
    io.set_item_anchor(anchor);
    io.validate_item_anchor(&used_currencies)
        .map_err(|error| anyhow::anyhow!("{error}"))?;

    let max_steps = if args.once || args.dry_run {
        1
    } else {
        args.max_steps
    };
    let config = SessionConfig {
        max_steps,
        max_currency_actions: max_steps.saturating_mul(2).max(2),
        max_seconds: args.max_seconds,
        max_cost: args.max_cost,
        max_same_fingerprint: 3,
        verify_currency: args.verify_currency,
        hold_currency: args.hold_currency,
        require_foreground: true,
    };

    if args.dry_run {
        io.game
            .focus()
            .context("bringing Path of Exile to the foreground")?;
        let mut session =
            CraftSession::new(plan, &data, &mut io, config, None).context("creating session")?;
        let preview = session.preview().context("reading current item")?;
        print_preview(&preview);
        return Ok(());
    }

    println!("Warning: this sends real mouse clicks to Path of Exile.");
    println!(
        "Emergency stop: {}. Keep the stash/inventory open and the item under the captured anchor.",
        app_config.hotkeys.stop
    );
    println!("Bringing Path of Exile to the foreground...");
    io.game
        .focus()
        .context("bringing Path of Exile to the foreground")?;
    std::thread::sleep(Duration::from_millis(250));

    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let log_path = args.log_dir.join(format!("session-{epoch}.jsonl"));
    let logger = SessionLogger::create(&log_path)?;
    let mut session = CraftSession::new(plan, &data, &mut io, config, Some(logger))
        .context("creating session")?;
    let report = session.run();
    match report {
        Ok(report) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_session_report(&report);
            }
            Ok(())
        }
        Err(error) => {
            eprintln!("session stopped: {error}");
            Err(error.into())
        }
    }
}

fn print_delays(delays: &DelaySettings, path: &Path) {
    if path.exists() {
        println!("Delays: {}", path.display());
    } else {
        println!("Delays: built-in defaults (create {})", path.display());
    }
    println!(
        "  action     {:.0}ms +/- {:.0}ms (clamped {}..={}ms)",
        delays.action.mean_ms,
        delays.action.deviation_ms,
        delays.action.min_ms,
        delays.action.max_ms
    );
}

fn print_preview(preview: &poe_session::PreviewReport) {
    println!(
        "Item: {} / {} | {:?}",
        preview.item.item_class.as_deref().unwrap_or("?"),
        preview.item.name.as_deref().unwrap_or("?"),
        preview.item.rarity
    );
    println!(
        "Goal: {} | score {}/{} | required {}/{}",
        if preview.evaluation.satisfied {
            "satisfied"
        } else {
            "not satisfied"
        },
        preview.evaluation.score,
        preview.min_score,
        preview.evaluation.required_matched,
        preview.evaluation.required_total
    );
    println!("Next: {}", preview.next_action);
    if !preview.currencies.is_empty() {
        println!(
            "Currencies: {}",
            preview
                .currencies
                .iter()
                .map(|currency| currency.label())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

fn print_session_report(report: &poe_session::SessionReport) {
    println!(
        "Session: {} ({:?})",
        if report.success { "success" } else { "stopped" },
        report.stop_reason
    );
    println!(
        "Steps: {} | currency actions: {} | elapsed: {:.1}s",
        report.steps, report.currency_actions, report.elapsed_seconds
    );
    if report.currency_used.is_empty() {
        println!("Currency used: none");
    } else {
        println!(
            "Currency used: {}",
            report
                .currency_used
                .iter()
                .map(|(currency, count)| format!("{currency} {count}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "Final item: {} / {} | goal: {}",
        report.final_item.item_class.as_deref().unwrap_or("?"),
        report.final_item.name.as_deref().unwrap_or("?"),
        if report.final_evaluation.satisfied {
            "satisfied"
        } else {
            "not satisfied"
        }
    );
    if let Some(path) = &report.log_path {
        println!("Log: {}", path.display());
    }
    for warning in &report.warnings {
        println!("Warning: {warning}");
    }
}

fn print_resolved(resolved: &ResolvedItem, all: bool) {
    let item = &resolved.item;
    println!(
        "Item: {} | {:?} | {} | ilvl {}",
        item.item_class.as_deref().unwrap_or("?"),
        item.rarity.unwrap_or(poe_item::Rarity::Other),
        item.name.as_deref().unwrap_or("?"),
        item.item_level
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".to_string())
    );
    if let Some(base) = &item.base_type {
        println!("Base: {base}");
    }
    println!("Tags: {}", resolved.base_tags.join(", "));
    let mut flags = Vec::new();
    if item.flags.corrupted {
        flags.push("corrupted");
    }
    if item.flags.fractured {
        flags.push("fractured");
    }
    if item.flags.synthesised {
        flags.push("synthesised");
    }
    if item.flags.mirrored {
        flags.push("mirrored");
    }
    if item.flags.scourged {
        flags.push("scourged");
    }
    if !flags.is_empty() {
        println!("Flags: {}", flags.join(", "));
    }

    let should_show = |group: &poe_item::ResolvedGroup| {
        all || group.matched.is_some()
            || group.header.is_some()
            || group.kind != poe_item::LineKind::Explicit
    };
    let shown = resolved
        .groups
        .iter()
        .filter(|group| should_show(group))
        .count();
    println!("Groups: {} shown / {} parsed", shown, resolved.groups.len());
    println!();

    for group in &resolved.groups {
        if !should_show(group) {
            continue;
        }
        let header = group
            .header
            .as_ref()
            .map(|header| {
                let mut parts = vec![format!("{:?}", header.kind)];
                if let Some(name) = &header.name {
                    parts.push(format!("\"{name}\""));
                }
                if let Some(tier) = header.tier {
                    parts.push(format!("Tier {tier}"));
                }
                if let Some(rank) = header.rank {
                    parts.push(format!("Rank {rank}"));
                }
                if let Some(label) = &header.tier_label {
                    parts.push(format!("({label})"));
                }
                if header.crafted {
                    parts.push("crafted".to_string());
                }
                if header.fractured {
                    parts.push("fractured".to_string());
                }
                if !header.tags.is_empty() {
                    parts.push(format!("[{}]", header.tags.join(", ")));
                }
                if let Some(increased) = header.increased {
                    parts.push(format!("{increased}% increased"));
                }
                parts.join(" ")
            })
            .unwrap_or_else(|| format!("{:?}", group.kind));
        println!("[{:>2}] {header}", group.index);
        for line in &group.lines {
            println!("      {line}", line = line.text);
        }
        match &group.matched {
            Some(matched) => {
                let tier = matched
                    .tier
                    .map(|tier| format!("T{tier} {:?}", matched.tier_source))
                    .unwrap_or_else(|| "tier ?".to_string());
                let percentiles = if matched.roll_percentiles.is_empty() {
                    String::new()
                } else {
                    format!(
                        " rolls [{}]",
                        matched
                            .roll_percentiles
                            .iter()
                            .map(|value| format!("{:.1}%", value * 100.0))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                println!(
                    "      -> {} | {:?} {:?} | {} | group {} | level {}{}{}",
                    matched.mod_id,
                    matched.domain,
                    matched.mod_type,
                    tier,
                    matched.group,
                    matched.level,
                    percentiles,
                    if group.alternatives > 1 {
                        format!(" | {} ties", group.alternatives)
                    } else {
                        String::new()
                    }
                );
                if !matched.affix.is_empty() && Some(&matched.affix) != header_name(group).as_ref()
                {
                    println!("         affix: {}", matched.affix);
                }
            }
            None => {
                println!(
                    "      -> unresolved: {}",
                    group.note.as_deref().unwrap_or("no match")
                );
            }
        }
        if let Some(note) = &group.note {
            if group.matched.is_some() {
                println!("         note: {note}");
            }
        }
        println!();
    }
}

fn header_name(group: &poe_item::ResolvedGroup) -> Option<String> {
    group.header.as_ref().and_then(|header| header.name.clone())
}
