//! Resolve parsed item mod lines against the Path of Building mod pool.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use poe_data::{Mod, ModDomain, ModType, PobData};

use crate::normalize::{normalize_mod_text, NormalizedText};
use crate::parse::{HeaderKind, LineKind, ModGroup, ModHeader, ParsedItem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TierSource {
    Game,
    Estimated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchedMod {
    pub mod_id: String,
    pub domain: ModDomain,
    pub mod_type: ModType,
    pub affix: String,
    pub level: i64,
    pub group: String,
    pub tier: Option<u32>,
    pub tier_source: TierSource,
    pub roll_percentiles: Vec<f64>,
    pub score: i32,
    pub stat_texts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedGroup {
    pub index: usize,
    pub kind: LineKind,
    pub header: Option<ModHeader>,
    pub lines: Vec<crate::parse::ModLine>,
    pub matched: Option<MatchedMod>,
    pub alternatives: usize,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedItem {
    pub item: ParsedItem,
    pub base_tags: Vec<String>,
    pub groups: Vec<ResolvedGroup>,
}

impl ResolvedItem {
    pub fn matched_groups(&self) -> impl Iterator<Item = &ResolvedGroup> {
        self.groups.iter().filter(|group| group.matched.is_some())
    }
}

pub struct Resolver<'a> {
    data: &'a PobData,
    by_affix: HashMap<(ModType, String), Vec<usize>>,
    by_template: HashMap<String, Vec<usize>>,
}

impl<'a> Resolver<'a> {
    pub fn new(data: &'a PobData) -> Self {
        let mut by_affix: HashMap<(ModType, String), Vec<usize>> = HashMap::new();
        let mut by_template: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, entry) in data.mods.iter().enumerate() {
            for text in &entry.stat_texts {
                let normalized = normalize_mod_text(text);
                by_template
                    .entry(normalized.template)
                    .or_default()
                    .push(index);
            }
            if !entry.affix.is_empty() && entry.mod_type.is_affix() {
                by_affix
                    .entry((entry.mod_type, entry.affix.clone()))
                    .or_default()
                    .push(index);
            }
        }
        Self {
            data,
            by_affix,
            by_template,
        }
    }

    pub fn data(&self) -> &PobData {
        self.data
    }

    pub fn resolve_item(&self, item: &ParsedItem) -> ResolvedItem {
        let base_tags = self.base_tags_for(item);
        let mut groups = Vec::new();
        for (index, group) in item.groups.iter().enumerate() {
            let (matched, alternatives, note) = self.resolve_group(item, group, &base_tags);
            groups.push(ResolvedGroup {
                index,
                kind: group.kind,
                header: group.header.clone(),
                lines: group.lines.clone(),
                matched,
                alternatives,
                note,
            });
        }
        ResolvedItem {
            item: item.clone(),
            base_tags,
            groups,
        }
    }

    fn resolve_group(
        &self,
        item: &ParsedItem,
        group: &ModGroup,
        base_tags: &[String],
    ) -> (Option<MatchedMod>, usize, Option<String>) {
        if group.lines.is_empty() {
            return (None, 0, Some("empty modifier group".to_string()));
        }
        if group.kind == LineKind::Veiled {
            return (None, 0, Some("veiled modifier placeholder".to_string()));
        }

        let header = group.header.as_ref();
        let expected_type = match header.map(|header| header.kind) {
            Some(HeaderKind::Prefix) => Some(ModType::Prefix),
            Some(HeaderKind::Suffix) => Some(ModType::Suffix),
            _ => None,
        };

        let candidate_indices = if let (Some(header), Some(expected_type)) = (header, expected_type)
        {
            if let Some(name) = &header.name {
                self.by_affix
                    .get(&(expected_type, name.clone()))
                    .cloned()
                    .unwrap_or_default()
            } else {
                self.candidates_by_template(group)
            }
        } else {
            self.candidates_by_template(group)
        };

        if candidate_indices.is_empty() {
            return (None, 0, Some("no candidate mod".to_string()));
        }

        let mut scored = Vec::new();
        for index in candidate_indices {
            let entry = &self.data.mods[index];
            if let Some(expected_type) = expected_type {
                if entry.mod_type != expected_type {
                    continue;
                }
            }
            if group.kind == LineKind::Implicit
                && matches!(
                    entry.mod_type,
                    ModType::Prefix | ModType::Suffix | ModType::Enchant
                )
            {
                continue;
            }
            if group.kind == LineKind::Enchant && entry.mod_type != ModType::Enchant {
                continue;
            }
            if group.kind == LineKind::Crucible && entry.mod_type != ModType::Crucible {
                continue;
            }
            let unique_group = group
                .header
                .as_ref()
                .is_some_and(|header| header.kind == HeaderKind::Unique);
            if group.kind == LineKind::Explicit
                && !unique_group
                && !matches!(entry.mod_type, ModType::Prefix | ModType::Suffix)
            {
                continue;
            }
            let score = self.score_candidate(item, group, entry, base_tags);
            scored.push((score, index));
        }

        if scored.is_empty() {
            return (None, 0, Some("no applicable candidate".to_string()));
        }

        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        let alternatives = scored
            .iter()
            .filter(|(score, _)| *score == scored[0].0)
            .count();
        let (score, index) = scored[0];
        let entry = &self.data.mods[index];
        let game_tier = header.and_then(|header| header.tier);
        let suppress_estimated_tier = header.is_some_and(|header| {
            header.crafted
                || header.rank.is_some()
                || matches!(
                    header.kind,
                    HeaderKind::Implicit
                        | HeaderKind::Enchant
                        | HeaderKind::Unique
                        | HeaderKind::Crucible
                )
                || header
                    .name
                    .as_deref()
                    .is_some_and(|name| name.contains("Essence"))
                || entry.affix.contains("Essence")
        });
        let tier = game_tier.or_else(|| {
            if entry.mod_type.is_affix() && !suppress_estimated_tier {
                self.estimate_tier(entry, base_tags, item.item_level)
            } else {
                None
            }
        });
        let tier_source = if game_tier.is_some() {
            TierSource::Game
        } else if tier.is_some() {
            TierSource::Estimated
        } else {
            TierSource::Unknown
        };
        let roll_percentiles = self.roll_percentiles(group, entry);
        let note = if score < 4 {
            Some("low-confidence match".to_string())
        } else {
            None
        };
        (
            Some(MatchedMod {
                mod_id: entry.id.clone(),
                domain: entry.domain,
                mod_type: entry.mod_type,
                affix: entry.affix.clone(),
                level: entry.level,
                group: entry.group.clone(),
                tier,
                tier_source,
                roll_percentiles,
                score,
                stat_texts: entry.stat_texts.clone(),
            }),
            alternatives,
            note,
        )
    }

    fn candidates_by_template(&self, group: &ModGroup) -> Vec<usize> {
        let mut result = BTreeSet::new();
        for line in &group.lines {
            if let Some(candidates) = self.by_template.get(&line.normalized.template) {
                result.extend(candidates.iter().copied());
            }
        }
        result.into_iter().collect()
    }

    fn score_candidate(
        &self,
        item: &ParsedItem,
        group: &ModGroup,
        entry: &Mod,
        base_tags: &[String],
    ) -> i32 {
        let mut score = 0i32;
        let candidate_lines = entry
            .stat_texts
            .iter()
            .map(|text| normalize_mod_text(text))
            .collect::<Vec<NormalizedText>>();

        for (line_index, line) in group.lines.iter().enumerate() {
            let template_match = candidate_lines
                .get(line_index)
                .is_some_and(|candidate| candidate.template == line.normalized.template);
            if template_match {
                score += 10;
            } else if candidate_lines
                .iter()
                .any(|candidate| candidate.template == line.normalized.template)
            {
                score += 6;
            } else {
                score -= 4;
            }

            let candidate = candidate_lines.get(line_index);
            if let Some(candidate) = candidate {
                for roll in &line.normalized.rolls {
                    if candidate
                        .ranges
                        .iter()
                        .any(|(min, max)| approx(*min, roll.min) && approx(*max, roll.max))
                    {
                        score += 5;
                    } else if candidate
                        .ranges
                        .iter()
                        .any(|(min, max)| roll.value >= *min && roll.value <= *max)
                    {
                        score += 2;
                    }
                }
                for (value_index, value) in line.normalized.values.iter().enumerate() {
                    if let Some((min, max)) = candidate.ranges.get(value_index) {
                        if *value >= *min && *value <= *max {
                            score += 3;
                        } else {
                            score -= 3;
                        }
                    } else if let Some(candidate_value) = candidate.values.get(value_index) {
                        if approx(*candidate_value, *value) {
                            score += 8;
                        } else {
                            score -= 4;
                        }
                    }
                }
            }
        }

        if candidate_lines.len() > group.lines.len() {
            score -= 2 * (candidate_lines.len() - group.lines.len()) as i32;
        }

        if let Some(item_level) = item.item_level {
            if entry.level as u32 <= item_level {
                score += 1;
            } else if entry.level as u32 > item_level + 8 {
                score -= 1;
            }
        }

        if entry.spawns_on(base_tags) {
            score += 2;
        }

        if group.kind == LineKind::Implicit {
            let eldritch = group
                .header
                .as_ref()
                .is_some_and(|header| header.tier_label.is_some());
            if !eldritch {
                if let Some(base) = self.base_for_item(item) {
                    if !base.implicit_ids.is_empty() {
                        if base.implicit_ids.iter().any(|id| id == &entry.id) {
                            score += 20;
                        } else {
                            score -= 5;
                        }
                    }
                }
            }
        }

        score += match entry.domain {
            ModDomain::Explicit => 2,
            ModDomain::Master if group.header.as_ref().is_some_and(|header| header.crafted) => 3,
            ModDomain::Eldritch if group.kind == LineKind::Implicit => 3,
            ModDomain::ItemExclusive if group.kind == LineKind::Implicit => 2,
            ModDomain::ItemExclusive if group.kind == LineKind::Explicit => 1,
            _ => 0,
        };

        if let Some(header) = &group.header {
            if header.fractured && entry.domain == ModDomain::Explicit {
                score += 2;
            }
            if header.crafted && entry.domain == ModDomain::Master {
                score += 3;
            }
        }

        score
    }

    fn roll_percentiles(&self, group: &ModGroup, entry: &Mod) -> Vec<f64> {
        let mut result = Vec::new();
        for (line_index, line) in group.lines.iter().enumerate() {
            if !line.normalized.rolls.is_empty() {
                result.extend(line.normalized.rolls.iter().map(|roll| roll.percentile));
                continue;
            }
            let Some(candidate) = entry
                .stat_texts
                .get(line_index)
                .map(|text| normalize_mod_text(text))
            else {
                continue;
            };
            for (value_index, value) in line.normalized.values.iter().enumerate() {
                if let Some((min, max)) = candidate.ranges.get(value_index) {
                    result.push(percentile(*value, *min, *max));
                }
            }
        }
        result
    }

    fn estimate_tier(
        &self,
        entry: &Mod,
        base_tags: &[String],
        item_level: Option<u32>,
    ) -> Option<u32> {
        if entry.group.is_empty() {
            return None;
        }
        let mut peers = self
            .data
            .mods
            .iter()
            .filter(|peer| {
                peer.group == entry.group
                    && peer.mod_type == entry.mod_type
                    && peer.domain == entry.domain
                    && !peer.affix.is_empty()
                    && (base_tags.is_empty() || peer.spawns_on(base_tags))
                    && item_level.is_none_or(|level| peer.level as u32 <= level)
            })
            .collect::<Vec<_>>();
        if peers.is_empty() {
            peers = self
                .data
                .mods
                .iter()
                .filter(|peer| {
                    peer.group == entry.group
                        && peer.mod_type == entry.mod_type
                        && peer.domain == entry.domain
                        && !peer.affix.is_empty()
                })
                .collect::<Vec<_>>();
        }
        peers.sort_by(|left, right| {
            right
                .level
                .cmp(&left.level)
                .then_with(|| left.id.cmp(&right.id))
        });
        let position = peers.iter().position(|peer| peer.id == entry.id)?;
        Some(position as u32 + 1)
    }

    fn base_for_item(&self, item: &ParsedItem) -> Option<&poe_data::BaseItem> {
        let base_type = item.base_type.as_deref()?;
        self.data.base_by_name(clean_base_type(base_type))
    }

    fn base_tags_for(&self, item: &ParsedItem) -> Vec<String> {
        if let Some(base) = self.base_for_item(item) {
            return base.tags.clone();
        }
        let Some(item_class) = &item.item_class else {
            return Vec::new();
        };
        let mut tags = BTreeSet::new();
        for base in &self.data.bases {
            if class_matches(item_class, &base.item_class) {
                tags.extend(base.tags.iter().cloned());
            }
        }
        tags.into_iter().collect()
    }
}

fn clean_base_type(name: &str) -> &str {
    for prefix in ["Synthesised ", "Superior ", "Fractured ", "Foulborn "] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest;
        }
    }
    name
}

fn class_matches(item_class: &str, base_class: &str) -> bool {
    let left = normalize_class(item_class);
    let right = normalize_class(base_class);
    left == right || left.contains(&right) || right.contains(&left)
}

fn normalize_class(value: &str) -> String {
    let mut value = value.trim().to_ascii_lowercase();
    if value.ends_with('s') {
        value.pop();
    }
    value
}

fn approx(left: f64, right: f64) -> bool {
    (left - right).abs() < 0.0001
}

fn percentile(value: f64, min: f64, max: f64) -> f64 {
    let delta = max - min;
    if delta.abs() < f64::EPSILON {
        0.5
    } else {
        ((value - min) / delta).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_item;
    use poe_data::{BaseItem, DataVersion, Mod};
    use std::collections::BTreeMap;

    fn synthetic_data() -> PobData {
        PobData {
            version: DataVersion::default(),
            mods: vec![
                Mod {
                    id: "Fecund".to_string(),
                    domain: ModDomain::Explicit,
                    mod_type: ModType::Prefix,
                    affix: "Fecund".to_string(),
                    stat_texts: vec!["+(130-144) to maximum Life".to_string()],
                    level: 86,
                    group: "IncreasedLife".to_string(),
                    weight_keys: vec!["helmet".to_string(), "default".to_string()],
                    weight_values: vec![1000, 0],
                    tags: vec![],
                    trade_hashes: vec![],
                    item_types: vec![],
                },
                Mod {
                    id: "ColdRes".to_string(),
                    domain: ModDomain::Explicit,
                    mod_type: ModType::Suffix,
                    affix: "of Haast".to_string(),
                    stat_texts: vec!["+(46-48)% to Cold Resistance".to_string()],
                    level: 84,
                    group: "ColdResistance".to_string(),
                    weight_keys: vec!["boots".to_string(), "default".to_string()],
                    weight_values: vec![1000, 0],
                    tags: vec![],
                    trade_hashes: vec![],
                    item_types: vec![],
                },
                Mod {
                    id: "CraftLightning".to_string(),
                    domain: ModDomain::Master,
                    mod_type: ModType::Suffix,
                    affix: "of Craft".to_string(),
                    stat_texts: vec!["+(29-35)% to Lightning Resistance".to_string()],
                    level: 1,
                    group: "LightningResistance".to_string(),
                    weight_keys: vec![],
                    weight_values: vec![],
                    tags: vec![],
                    trade_hashes: vec![],
                    item_types: vec!["Belt".to_string()],
                },
                Mod {
                    id: "Move1".to_string(),
                    domain: ModDomain::Explicit,
                    mod_type: ModType::Prefix,
                    affix: "Runner's".to_string(),
                    stat_texts: vec!["10% increased Movement Speed".to_string()],
                    level: 1,
                    group: "MovementVelocity".to_string(),
                    weight_keys: vec!["boots".to_string(), "default".to_string()],
                    weight_values: vec![1000, 0],
                    tags: vec![],
                    trade_hashes: vec![],
                    item_types: vec![],
                },
                Mod {
                    id: "Move6".to_string(),
                    domain: ModDomain::Explicit,
                    mod_type: ModType::Prefix,
                    affix: "Hellion's".to_string(),
                    stat_texts: vec!["35% increased Movement Speed".to_string()],
                    level: 86,
                    group: "MovementVelocity".to_string(),
                    weight_keys: vec!["boots".to_string(), "default".to_string()],
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
                influence_tags: BTreeMap::new(),
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

    #[test]
    fn resolves_header_tier_and_roll() {
        let data = synthetic_data();
        let resolver = Resolver::new(&data);
        let item = parse_item(
            "Rarity: Rare\nName\nPhantom Boots\n--------\nItem Level: 84\n--------\n{ Prefix Modifier \"Fecund\" (Tier: 1) \u{2014} Life }\n+143(130-144) to maximum Life\n",
        );
        let resolved = resolver.resolve_item(&item);
        let matched = resolved.groups[0].matched.as_ref().unwrap();
        assert_eq!(matched.mod_id, "Fecund");
        assert_eq!(matched.tier, Some(1));
        assert_eq!(matched.tier_source, TierSource::Game);
        assert!((matched.roll_percentiles[0] - 0.928).abs() < 0.01);
    }

    #[test]
    fn resolves_normal_line_and_estimates_tier() {
        let data = synthetic_data();
        let resolver = Resolver::new(&data);
        let item = parse_item(
            "Rarity: Rare\nName\nPhantom Boots\n--------\nItem Level: 84\n--------\n+47% to Cold Resistance (fractured)\n",
        );
        let resolved = resolver.resolve_item(&item);
        let matched = resolved.groups[0].matched.as_ref().unwrap();
        assert_eq!(matched.mod_id, "ColdRes");
        assert_eq!(matched.tier_source, TierSource::Estimated);
        assert!((matched.roll_percentiles[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn resolves_flat_value_tier() {
        let data = synthetic_data();
        let resolver = Resolver::new(&data);
        let item = parse_item(
            "Rarity: Rare\nName\nPhantom Boots\n--------\nItem Level: 86\n--------\n+35% increased Movement Speed\n",
        );
        let resolved = resolver.resolve_item(&item);
        let matched = resolved.groups[0].matched.as_ref().unwrap();
        assert_eq!(matched.mod_id, "Move6");
        assert_eq!(matched.tier, Some(1));
    }

    #[test]
    fn resolves_all_samples_without_data() {
        let data = PobData::default();
        let resolver = Resolver::new(&data);
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/items");
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("txt") {
                continue;
            }
            let text = std::fs::read_to_string(path).unwrap();
            let item = parse_item(&text);
            let resolved = resolver.resolve_item(&item);
            assert_eq!(resolved.groups.len(), item.groups.len());
        }
    }

    #[test]
    fn resolves_crafted_header() {
        let data = synthetic_data();
        let resolver = Resolver::new(&data);
        let item = parse_item(
            "Rarity: Rare\nName\nCord Belt\n--------\nItem Level: 85\n--------\n{ Master Crafted Suffix Modifier \"of Craft\" (Rank: 3) \u{2014} Elemental, Lightning, Resistance }\n+34(29-35)% to Lightning Resistance\n",
        );
        let resolved = resolver.resolve_item(&item);
        let matched = resolved.groups[0].matched.as_ref().unwrap();
        assert_eq!(matched.mod_id, "CraftLightning");
        assert_eq!(matched.domain, ModDomain::Master);
    }
}
