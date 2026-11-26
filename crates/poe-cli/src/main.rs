use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use poe_data::{load_cache, save_cache, DataVersion};
use poe_item::{parse_item, ResolvedItem, Resolver};

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
    let data = load_cache(cache).with_context(|| format!("loading cache {}", cache.display()))?;
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
    let data = load_cache(cache).with_context(|| format!("loading cache {}", cache.display()))?;
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
