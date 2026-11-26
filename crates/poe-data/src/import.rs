//! Import Path of Building data files into the serialized data model.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::lua::{self, LuaError, LuaTable, LuaValue};
use crate::model::{
    BaseItem, DataVersion, Essence, Mod, ModDomain, ModType, PobData, TradeHash, TradeStat,
};

/// Bump this whenever the serialized data model changes. Old caches are
/// rejected and rebuilt from the pinned local files.
pub const CACHE_SCHEMA: u32 = 2;
pub(crate) const CACHE_MAGIC: &[u8; 8] = b"POECACHE";

const MOD_FILES: &[(&str, ModDomain)] = &[
    ("ModExplicit.lua", ModDomain::Explicit),
    ("ModCorrupted.lua", ModDomain::Corrupted),
    ("ModDelve.lua", ModDomain::Delve),
    ("ModEldritch.lua", ModDomain::Eldritch),
    ("ModMaster.lua", ModDomain::Master),
    ("ModVeiled.lua", ModDomain::Veiled),
    ("ModTincture.lua", ModDomain::Tincture),
    ("ModFlask.lua", ModDomain::Flask),
    ("ModJewel.lua", ModDomain::Jewel),
    ("ModJewelAbyss.lua", ModDomain::JewelAbyss),
    ("ModJewelCharm.lua", ModDomain::JewelCharm),
    ("ModJewelCluster.lua", ModDomain::JewelCluster),
    ("ModItemExclusive.lua", ModDomain::ItemExclusive),
];

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Lua(#[from] LuaError),
    #[error("required data file not found: {0}")]
    Missing(String),
    #[error("{path}: {source}")]
    DataFile {
        path: String,
        #[source]
        source: Box<ImportError>,
    },
    #[error("cache encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("cache decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("data cache is outdated; delete it to rebuild from the pinned data files")]
    CacheOutdated,
}

pub fn parse_mod_file(source: &str, domain: ModDomain) -> Result<Vec<Mod>, ImportError> {
    let table = lua::parse_return_table(source)?;
    let mut mods = Vec::new();
    if !table.fields.is_empty() {
        for (id, value) in &table.fields {
            if let LuaValue::Table(entry) = value {
                mods.push(parse_mod(id.clone(), domain, entry));
            }
        }
    } else {
        for (index, value) in table.array.iter().enumerate() {
            if let LuaValue::Table(entry) = value {
                let id = entry
                    .get_str("id")
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{domain:?}#{index}"));
                mods.push(parse_mod(id, domain, entry));
            }
        }
    }
    Ok(mods)
}

fn parse_mod(id: String, domain: ModDomain, entry: &LuaTable) -> Mod {
    let mod_type = entry
        .get_str("type")
        .map(ModType::from_pob)
        .unwrap_or_else(|| match domain {
            ModDomain::Corrupted => ModType::Corrupted,
            ModDomain::ItemExclusive => ModType::Unique,
            ModDomain::Eldritch | ModDomain::Delve => ModType::Implicit,
            ModDomain::Veiled => ModType::Veiled,
            _ => ModType::Unknown,
        });

    let stat_texts = entry
        .array_strings()
        .map(str::to_owned)
        .collect::<Vec<String>>();

    let weight_keys = entry.string_list("weightKey");
    let weight_values = entry
        .number_list("weightVal")
        .into_iter()
        .map(|value| value as i32)
        .collect::<Vec<i32>>();

    let tags = entry.string_list("modTags");
    let item_types = entry
        .get_table("types")
        .map(|types| {
            types
                .fields
                .iter()
                .filter(|(_, value)| matches!(value, LuaValue::Bool(true)))
                .map(|(key, _)| key.clone())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    let trade_hashes = entry
        .get_table("tradeHashes")
        .map(|hashes| {
            hashes
                .numeric_fields
                .iter()
                .map(|(hash, value)| TradeHash {
                    hash: *hash as u64,
                    texts: match value {
                        LuaValue::Table(table) => table
                            .array_strings()
                            .map(str::to_owned)
                            .collect::<Vec<String>>(),
                        LuaValue::String(text) => vec![text.clone()],
                        _ => Vec::new(),
                    },
                })
                .collect::<Vec<TradeHash>>()
        })
        .unwrap_or_default();

    Mod {
        id,
        domain,
        mod_type,
        affix: entry.get_str("affix").unwrap_or_default().to_owned(),
        stat_texts,
        level: entry.get_i64("level").unwrap_or(1),
        group: entry.get_str("group").unwrap_or_default().to_owned(),
        weight_keys,
        weight_values,
        tags,
        trade_hashes,
        item_types,
    }
}

pub fn parse_bases_file(source: &str) -> Result<Vec<BaseItem>, ImportError> {
    let assignments = lua::parse_assignments(source, "itemBases")?;
    let mut bases = Vec::with_capacity(assignments.len());
    for (name, table) in assignments {
        let tags = table
            .get_table("tags")
            .map(|tags| {
                tags.fields
                    .iter()
                    .filter(|(_, value)| matches!(value, LuaValue::Bool(true)))
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();

        let implicit_mod_types = table
            .get("implicitModTypes")
            .map(flatten_strings)
            .unwrap_or_default();

        let influence_tags = table
            .get_table("influenceTags")
            .map(|tags| {
                tags.fields
                    .iter()
                    .filter_map(|(key, value)| match value {
                        LuaValue::String(tag) => Some((key.clone(), tag.clone())),
                        _ => None,
                    })
                    .collect::<BTreeMap<String, String>>()
            })
            .unwrap_or_default();

        let req = table.get_table("req");
        bases.push(BaseItem {
            name,
            item_class: table.get_str("type").unwrap_or_default().to_owned(),
            sub_type: table.get_str("subType").map(str::to_owned),
            tags,
            influence_tags,
            implicit_mod_types,
            implicit_ids: table.string_list("implicitIds"),
            level_req: req.and_then(|req| req.get_i64("level")),
            str_req: req.and_then(|req| req.get_i64("str")),
            dex_req: req.and_then(|req| req.get_i64("dex")),
            int_req: req.and_then(|req| req.get_i64("int")),
        });
    }
    Ok(bases)
}

fn flatten_strings(value: &LuaValue) -> Vec<String> {
    match value {
        LuaValue::String(text) => vec![text.clone()],
        LuaValue::Table(table) => {
            let mut result = Vec::new();
            for value in &table.array {
                result.extend(flatten_strings(value));
            }
            result
        }
        _ => Vec::new(),
    }
}

pub fn parse_trade_stats(source: &str) -> Result<Vec<TradeStat>, ImportError> {
    let table = lua::parse_return_table(source)?;
    let mut stats = Vec::new();
    for group in &table.array {
        let LuaValue::Table(group) = group else {
            continue;
        };
        let Some(entries) = group.get_table("entries") else {
            continue;
        };
        for entry in &entries.array {
            let LuaValue::Table(entry) = entry else {
                continue;
            };
            let Some(id) = entry.get_str("id") else {
                continue;
            };
            let Some(text) = entry.get_str("text") else {
                continue;
            };
            stats.push(TradeStat {
                id: id.to_owned(),
                text: text.to_owned(),
                stat_type: entry.get_str("type").unwrap_or_default().to_owned(),
            });
        }
    }
    Ok(stats)
}

pub fn parse_essences(source: &str) -> Result<Vec<Essence>, ImportError> {
    let table = lua::parse_return_table(source)?;
    let mut essences = Vec::new();
    for (metadata, value) in &table.fields {
        let LuaValue::Table(entry) = value else {
            continue;
        };
        let mods = entry
            .get_table("mods")
            .map(|mods| {
                mods.fields
                    .iter()
                    .filter_map(|(class, value)| match value {
                        LuaValue::String(mod_id) => Some((class.clone(), mod_id.clone())),
                        _ => None,
                    })
                    .collect::<BTreeMap<String, String>>()
            })
            .unwrap_or_default();
        essences.push(Essence {
            metadata: metadata.clone(),
            name: entry.get_str("name").unwrap_or_default().to_owned(),
            type_id: entry.get_i64("type").unwrap_or(0),
            tier: entry.get_i64("tier").unwrap_or(0),
            mods,
        });
    }
    Ok(essences)
}

fn resolve_file(root: &Path, name: &str) -> Option<PathBuf> {
    let structured = root.join("src").join("Data").join(name);
    if structured.is_file() {
        return Some(structured);
    }
    let flat = root.join(name);
    if flat.is_file() {
        return Some(flat);
    }
    None
}

fn resolve_bases_dir(root: &Path) -> Option<PathBuf> {
    let structured = root.join("src").join("Data").join("Bases");
    if structured.is_dir() {
        return Some(structured);
    }
    let flat = root.join("Bases");
    if flat.is_dir() {
        return Some(flat);
    }
    None
}

pub fn from_pob_dir(root: &Path) -> Result<PobData, ImportError> {
    let mut mods = Vec::new();
    let mut imported_files = 0usize;
    for (name, domain) in MOD_FILES {
        let Some(path) = resolve_file(root, name) else {
            if *domain == ModDomain::Explicit {
                return Err(ImportError::Missing(format!(
                    "{} (expected under src/Data or the given directory)",
                    name
                )));
            }
            continue;
        };
        let source = fs::read_to_string(&path)?;
        let mut parsed =
            parse_mod_file(&source, *domain).map_err(|error| ImportError::DataFile {
                path: path.display().to_string(),
                source: Box::new(error),
            })?;
        imported_files += 1;
        mods.append(&mut parsed);
    }

    let bases_dir = resolve_bases_dir(root)
        .ok_or_else(|| ImportError::Missing("Bases directory".to_string()))?;
    let mut bases = Vec::new();
    for entry in fs::read_dir(bases_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("lua") {
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) == Some("ModMap.lua") {
            continue;
        }
        let source = fs::read_to_string(&path)?;
        let mut parsed = parse_bases_file(&source).map_err(|error| ImportError::DataFile {
            path: path.display().to_string(),
            source: Box::new(error),
        })?;
        imported_files += 1;
        bases.append(&mut parsed);
    }

    let trade_stats = match resolve_file(root, "TradeSiteStats.lua") {
        Some(path) => {
            imported_files += 1;
            parse_trade_stats(&fs::read_to_string(&path)?).map_err(|error| {
                ImportError::DataFile {
                    path: path.display().to_string(),
                    source: Box::new(error),
                }
            })?
        }
        None => Vec::new(),
    };

    let essences = match resolve_file(root, "Essence.lua") {
        Some(path) => {
            imported_files += 1;
            parse_essences(&fs::read_to_string(&path)?).map_err(|error| ImportError::DataFile {
                path: path.display().to_string(),
                source: Box::new(error),
            })?
        }
        None => Vec::new(),
    };

    mods.sort_by(|left, right| left.id.cmp(&right.id));
    bases.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(PobData {
        version: DataVersion {
            source: "local".to_string(),
            commit: String::new(),
            commit_date: String::new(),
            files: imported_files,
            built_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs().to_string())
                .unwrap_or_default(),
        },
        mods,
        bases,
        trade_stats,
        essences,
    })
}

pub fn save_cache(data: &PobData, path: &Path) -> Result<(), ImportError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = rmp_serde::to_vec(data)?;
    let mut bytes = Vec::with_capacity(12 + payload.len());
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&CACHE_SCHEMA.to_le_bytes());
    bytes.extend_from_slice(&payload);
    let temp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache")
    ));
    fs::write(&temp, bytes)?;
    if let Err(error) = fs::rename(&temp, path) {
        if path.exists() {
            fs::remove_file(path)?;
            fs::rename(&temp, path)?;
        } else {
            return Err(error.into());
        }
    }
    Ok(())
}

pub fn load_cache(path: &Path) -> Result<PobData, ImportError> {
    let bytes = fs::read(path)?;
    if bytes.len() < 12 || &bytes[..8] != CACHE_MAGIC {
        return Err(ImportError::CacheOutdated);
    }
    let schema = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if schema != CACHE_SCHEMA {
        return Err(ImportError::CacheOutdated);
    }
    Ok(rmp_serde::from_slice(&bytes[12..])?)
}

/// Whether the cache exists and carries the current schema header.
pub fn cache_is_current(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut header = [0u8; 12];
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    let has_payload = file.metadata().map(|meta| meta.len() > 12).unwrap_or(false);
    has_payload
        && &header[..8] == CACHE_MAGIC
        && u32::from_le_bytes([header[8], header[9], header[10], header[11]]) == CACHE_SCHEMA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keyed_mods() {
        let source = r#"
            return {
                ["Life1"] = { type = "Prefix", affix = "Hale", "+(3-9) to maximum Life",
                    level = 1, group = "IncreasedLife",
                    weightKey = { "ring", "default" },
                    weightVal = { 1000, 0 },
                    modTags = { "life" },
                    tradeHashes = { [42] = { "+(3-9) to maximum Life" } }, },
            }
        "#;
        let mods = parse_mod_file(source, ModDomain::Explicit).unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].id, "Life1");
        assert_eq!(mods[0].mod_type, ModType::Prefix);
        assert_eq!(mods[0].stat_texts, vec!["+(3-9) to maximum Life"]);
        assert_eq!(mods[0].weight_keys, vec!["ring", "default"]);
        assert_eq!(mods[0].weight_values, vec![1000, 0]);
        assert_eq!(mods[0].trade_hashes[0].hash, 42);
    }

    #[test]
    fn parses_array_mods() {
        let source = r#"
            return {
                { type = "Prefix", affix = "Upgraded", "+(15-25) to maximum Life",
                  level = 15, group = "IncreasedLife", types = { ["Body Armour"] = true }, },
            }
        "#;
        let mods = parse_mod_file(source, ModDomain::Master).unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].affix, "Upgraded");
        assert_eq!(mods[0].item_types, vec!["Body Armour"]);
    }

    #[test]
    fn parses_base_function_file() {
        let source = r#"
            return function(itemBases)
            itemBases["Coral Ring"] = {
                type = "Ring",
                tags = { ring = true, default = true },
                influenceTags = { shaper = "ring_shaper", elder = "ring_elder" },
                implicit = "+(20-30) to Strength",
                implicitModTypes = { { "attribute", "strength" } },
                req = { level = 8 },
            }
            end
        "#;
        let bases = parse_bases_file(source).unwrap();
        assert_eq!(bases.len(), 1);
        assert_eq!(bases[0].name, "Coral Ring");
        assert_eq!(bases[0].item_class, "Ring");
        assert_eq!(bases[0].tags, vec!["default", "ring"]);
        assert_eq!(bases[0].implicit_mod_types, vec!["attribute", "strength"]);
        assert_eq!(
            bases[0].influence_tags.get("shaper").map(String::as_str),
            Some("ring_shaper")
        );
        assert_eq!(bases[0].level_req, Some(8));
    }

    #[test]
    fn header_only_cache_is_not_current() {
        let path = std::env::temp_dir().join(format!(
            "poe-crafter-header-only-{}.msgpack",
            std::process::id()
        ));
        let mut header = Vec::new();
        header.extend_from_slice(CACHE_MAGIC);
        header.extend_from_slice(&CACHE_SCHEMA.to_le_bytes());
        std::fs::write(&path, header).unwrap();
        assert!(!cache_is_current(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn old_cache_header_is_rejected() {
        let path = std::env::temp_dir().join(format!(
            "poe-crafter-old-cache-{}.msgpack",
            std::process::id()
        ));
        std::fs::write(&path, b"old cache").unwrap();
        assert!(matches!(load_cache(&path), Err(ImportError::CacheOutdated)));
        assert!(!cache_is_current(&path));
        let _ = std::fs::remove_file(path);
    }
}
