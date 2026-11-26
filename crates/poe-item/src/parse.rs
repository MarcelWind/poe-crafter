//! Parser for Path of Exile clipboard item text, including advanced copy.

use serde::{Deserialize, Serialize};

use crate::normalize::{normalize_item_line, LineFlags, NormalizedText};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rarity {
    Normal,
    Magic,
    Rare,
    Unique,
    Currency,
    Gem,
    Other,
}

impl Rarity {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "normal" => Rarity::Normal,
            "magic" => Rarity::Magic,
            "rare" => Rarity::Rare,
            "unique" | "relic" => Rarity::Unique,
            "currency" => Rarity::Currency,
            "gem" => Rarity::Gem,
            _ => Rarity::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeaderKind {
    Prefix,
    Suffix,
    Implicit,
    Enchant,
    Unique,
    Veiled,
    Crucible,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModHeader {
    pub raw: String,
    pub kind: HeaderKind,
    pub crafted: bool,
    pub fractured: bool,
    pub name: Option<String>,
    pub tier: Option<u32>,
    pub rank: Option<u32>,
    pub tier_label: Option<String>,
    pub tags: Vec<String>,
    pub increased: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    Explicit,
    Implicit,
    Enchant,
    Scourge,
    Unique,
    Crucible,
    Veiled,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModLine {
    pub text: String,
    pub normalized: NormalizedText,
    pub flags: LineFlags,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModGroup {
    pub header: Option<ModHeader>,
    pub kind: LineKind,
    pub lines: Vec<ModLine>,
}

impl ModGroup {
    pub fn template(&self) -> Option<String> {
        self.lines
            .first()
            .map(|line| line.normalized.template.clone())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemFlags {
    pub identified: bool,
    pub corrupted: bool,
    pub fractured: bool,
    pub synthesised: bool,
    pub mirrored: bool,
    pub split: bool,
    pub scourged: bool,
    pub veiled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedItem {
    pub raw: String,
    pub item_class: Option<String>,
    pub rarity: Option<Rarity>,
    pub rarity_text: Option<String>,
    pub name: Option<String>,
    pub base_type: Option<String>,
    pub item_level: Option<u32>,
    pub quality: Option<i32>,
    pub sockets: Option<String>,
    pub flags: ItemFlags,
    pub groups: Vec<ModGroup>,
    pub notes: Vec<String>,
}

impl ParsedItem {
    pub fn is_equipment(&self) -> bool {
        matches!(
            self.rarity,
            Some(Rarity::Normal | Rarity::Magic | Rarity::Rare | Rarity::Unique)
        )
    }

    pub fn explicit_groups(&self) -> impl Iterator<Item = &ModGroup> {
        self.groups
            .iter()
            .filter(|group| group.kind == LineKind::Explicit)
    }

    pub fn implicit_groups(&self) -> impl Iterator<Item = &ModGroup> {
        self.groups
            .iter()
            .filter(|group| group.kind == LineKind::Implicit)
    }
}

pub fn parse_item(raw: &str) -> ParsedItem {
    let lines = raw
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect::<Vec<_>>();

    let mut item = ParsedItem {
        raw: raw.trim_end().to_string(),
        item_class: None,
        rarity: None,
        rarity_text: None,
        name: None,
        base_type: None,
        item_level: None,
        quality: None,
        sockets: None,
        flags: ItemFlags {
            identified: true,
            ..ItemFlags::default()
        },
        groups: Vec::new(),
        notes: Vec::new(),
    };

    let mut index = 0usize;
    let mut header_block: Vec<String> = Vec::new();

    while index < lines.len() {
        let line = lines[index].trim();
        if line == "--------" {
            break;
        }
        if let Some(value) = line.strip_prefix("Item Class:") {
            item.item_class = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Rarity:") {
            let value = value.trim();
            item.rarity = Some(Rarity::parse(value));
            item.rarity_text = Some(value.to_string());
        } else if !line.is_empty() {
            header_block.push(line.to_string());
        }
        index += 1;
    }

    let names = header_block
        .iter()
        .map(String::as_str)
        .filter(|line| !is_header_warning(line))
        .collect::<Vec<_>>();

    match item.rarity {
        Some(Rarity::Rare | Rarity::Unique) => {
            item.name = names.first().map(|value| (*value).to_string());
            item.base_type = names.get(1).map(|value| (*value).to_string());
        }
        Some(_) => {
            item.name = names.first().map(|value| (*value).to_string());
        }
        None => {}
    }

    // Some items put a warning line before the first separator and the name/base
    // in the section after it (e.g. "You cannot use this item").
    if item.name.is_none() {
        let mut probe = index;
        while probe < lines.len() && lines[probe].trim() == "--------" {
            probe += 1;
        }
        let mut alternate_names = Vec::new();
        while probe < lines.len() {
            let line = lines[probe].trim();
            if line == "--------" {
                break;
            }
            if !line.is_empty()
                && !is_help_or_property(line)
                && !is_state_line(line)
                && !is_reminder_text(line)
            {
                alternate_names.push(line.to_string());
            }
            probe += 1;
        }
        if probe < lines.len() {
            probe += 1;
        }
        match item.rarity {
            Some(Rarity::Rare | Rarity::Unique) => {
                item.name = alternate_names.first().cloned();
                item.base_type = alternate_names.get(1).cloned();
            }
            Some(_) => {
                item.name = alternate_names.first().cloned();
            }
            None => {}
        }
        if item.name.is_some() {
            index = probe;
        }
    }

    for line in &lines {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Item Level:") {
            item.item_level = value.trim().parse::<u32>().ok();
        } else if let Some(value) = line.strip_prefix("Sockets:") {
            item.sockets = Some(value.trim().to_string());
        } else if line.starts_with("Quality") && item.quality.is_none() {
            item.quality = parse_quality(line);
        }
    }

    let mut current: Option<ModGroup> = None;
    let mut in_flavor = false;
    let mut seen_item_level = false;

    while index < lines.len() {
        let raw_line = lines[index];
        index += 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if in_flavor {
            if line.ends_with('"') {
                in_flavor = false;
            }
            continue;
        }

        if line == "--------" {
            flush_group(&mut current, &mut item.groups);
            continue;
        }

        if let Some(stripped) = line.strip_prefix('"') {
            flush_group(&mut current, &mut item.groups);
            in_flavor = !stripped.ends_with('"');
            continue;
        }

        if line.starts_with("Item Level:") {
            seen_item_level = true;
            flush_group(&mut current, &mut item.groups);
            continue;
        }

        if is_state_line(line) {
            apply_state_line(line, &mut item.flags);
            flush_group(&mut current, &mut item.groups);
            continue;
        }

        if line.starts_with("Note:") {
            item.notes.push(line.to_string());
            flush_group(&mut current, &mut item.groups);
            continue;
        }

        if let Some(header) = parse_mod_header(line) {
            flush_group(&mut current, &mut item.groups);
            let kind = match header.kind {
                HeaderKind::Prefix | HeaderKind::Suffix | HeaderKind::Unique => LineKind::Explicit,
                HeaderKind::Implicit => LineKind::Implicit,
                HeaderKind::Enchant => LineKind::Enchant,
                HeaderKind::Crucible => LineKind::Crucible,
                HeaderKind::Veiled => LineKind::Veiled,
                HeaderKind::Other => LineKind::Other,
            };
            current = Some(ModGroup {
                header: Some(header),
                kind,
                lines: Vec::new(),
            });
            continue;
        }

        if is_reminder_text(line) {
            continue;
        }

        if let Some(group) = current.as_mut() {
            push_mod_line(group, line);
            continue;
        }

        if !seen_item_level || !is_mod_candidate(line) {
            continue;
        }

        let (_, flags) = normalize_item_line(line);
        let kind = kind_from_flags(&flags);
        let mut group = ModGroup {
            header: None,
            kind,
            lines: Vec::new(),
        };
        push_mod_line(&mut group, line);
        item.groups.push(group);
    }

    flush_group(&mut current, &mut item.groups);
    item
}

fn flush_group(current: &mut Option<ModGroup>, groups: &mut Vec<ModGroup>) {
    if let Some(group) = current.take() {
        if !group.lines.is_empty() {
            groups.push(group);
        }
    }
}

fn push_mod_line(group: &mut ModGroup, line: &str) {
    let (normalized, flags) = normalize_item_line(line);
    group.lines.push(ModLine {
        text: line.to_string(),
        normalized,
        flags,
    });
}

fn kind_from_flags(flags: &LineFlags) -> LineKind {
    if flags.implicit {
        LineKind::Implicit
    } else if flags.enchant {
        LineKind::Enchant
    } else if flags.scourge {
        LineKind::Scourge
    } else {
        LineKind::Explicit
    }
}

fn is_header_warning(line: &str) -> bool {
    line.starts_with("You cannot use this item")
        || line.starts_with("Foil Unique")
        || line.starts_with("Rarity:")
        || line.starts_with("Item Class:")
}

fn parse_quality(line: &str) -> Option<i32> {
    let start = line
        .find('+')
        .or_else(|| line.find(':'))
        .map(|index| index + 1)?;
    let digits = line[start..]
        .trim_start()
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse::<i32>().ok()
}

fn is_reminder_text(line: &str) -> bool {
    line.starts_with('(') && line.ends_with(')')
}

fn is_state_line(line: &str) -> bool {
    matches!(
        line,
        "Corrupted"
            | "Unidentified"
            | "Identified"
            | "Mirrored"
            | "Split"
            | "Fractured Item"
            | "Synthesised Item"
            | "Scourged"
            | "Veiled Prefix"
            | "Veiled Suffix"
            | "Searing Exarch Item"
            | "Eater of Worlds Item"
            | "Shaper Item"
            | "Elder Item"
            | "Crusader Item"
            | "Redeemer Item"
            | "Hunter Item"
            | "Warlord Item"
    )
}

fn apply_state_line(line: &str, flags: &mut ItemFlags) {
    match line {
        "Corrupted" => flags.corrupted = true,
        "Unidentified" => flags.identified = false,
        "Identified" => flags.identified = true,
        "Mirrored" => flags.mirrored = true,
        "Split" => flags.split = true,
        "Fractured Item" => flags.fractured = true,
        "Synthesised Item" => flags.synthesised = true,
        "Scourged" => flags.scourged = true,
        "Veiled Prefix" | "Veiled Suffix" => flags.veiled = true,
        _ => {}
    }
}

fn is_mod_candidate(line: &str) -> bool {
    if line.is_empty() || line.starts_with('(') || line.starts_with("Note:") {
        return false;
    }
    if is_state_line(line) {
        return false;
    }
    let (_, flags) = normalize_item_line(line);
    let has_marker = flags.implicit
        || flags.crafted
        || flags.fractured
        || flags.enchant
        || flags.scourge
        || flags.unscalable;
    if !has_marker && is_help_or_property(line) {
        return false;
    }
    true
}

fn is_help_or_property(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "Quality",
        "Physical Damage:",
        "Elemental Damage:",
        "Critical Strike Chance:",
        "Attacks per Second:",
        "Weapon Range:",
        "Armour:",
        "Evasion Rating:",
        "Energy Shield:",
        "Ward:",
        "Chance to Block:",
        "Sockets:",
        "Requirements:",
        "Level:",
        "Str:",
        "Dex:",
        "Int:",
        "Stack Size:",
        "Lasts ",
        "Consumes ",
        "Currently has ",
        "Recovers ",
        "Map Tier:",
        "Quantity:",
        "Pack Size:",
        "Monster Level:",
        "Area Level:",
        "Radius:",
        "Limited to:",
        "Requires ",
        "Right click",
        "Shift click",
        "Place into",
        "This is a ",
        "Additional Effects From Quality:",
        "Supported Skills have ",
        "Travel to ",
        "Open a portal",
        "Can be exchanged",
        "Can only hold charges",
        "Refills as you kill",
    ];
    PREFIXES.iter().any(|prefix| line.starts_with(prefix))
}

pub fn parse_mod_header(line: &str) -> Option<ModHeader> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let body = trimmed[1..trimmed.len() - 1].trim();
    let crafted = body.contains("Master Crafted");
    let fractured = body.contains("Fractured");
    let kind = if body.contains("Corrupted Enhancement") || body.contains("Corruption Enhancement")
    {
        HeaderKind::Enchant
    } else if body.contains("Crucible") {
        HeaderKind::Crucible
    } else if body.contains("Veiled") {
        HeaderKind::Veiled
    } else if body.contains("Implicit") {
        HeaderKind::Implicit
    } else if body.contains("Unique Modifier") {
        HeaderKind::Unique
    } else if body.contains("Prefix") {
        HeaderKind::Prefix
    } else if body.contains("Suffix") {
        HeaderKind::Suffix
    } else {
        HeaderKind::Other
    };

    let segments = split_header_segments(body);
    let primary = segments.first().map(String::as_str).unwrap_or(body);

    let name = extract_quoted(primary);
    let name = match name.as_deref() {
        Some("Veiled") => None,
        _ => name,
    };

    let mut tier = None;
    let mut rank = None;
    let mut tier_label = None;
    for (open, _) in primary.match_indices('(') {
        let rest = &primary[open + 1..];
        let Some(close) = rest.find(')') else {
            continue;
        };
        let inner = &rest[..close];
        if let Some(value) = inner.strip_prefix("Tier:") {
            tier = value.trim().parse::<u32>().ok();
        } else if let Some(value) = inner.strip_prefix("Rank:") {
            rank = value.trim().parse::<u32>().ok();
        } else if !inner.is_empty() && inner.chars().all(|character| character.is_alphabetic()) {
            tier_label = Some(inner.to_string());
        }
    }

    let tags = segments
        .get(1)
        .map(|segment| {
            segment
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_owned)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    let increased = segments
        .iter()
        .rev()
        .find_map(|segment| parse_increased(segment));

    Some(ModHeader {
        raw: trimmed.to_string(),
        kind,
        crafted,
        fractured,
        name,
        tier,
        rank,
        tier_label,
        tags,
        increased,
    })
}

fn parse_increased(segment: &str) -> Option<u32> {
    let lowered = segment.to_ascii_lowercase();
    if !lowered.contains("increased") {
        return None;
    }
    let digits = segment
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse::<u32>().ok()
}

fn extract_quoted(text: &str) -> Option<String> {
    let start = text.find('"')?;
    let rest = &text[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn split_header_segments(body: &str) -> Vec<String> {
    let chars = body.chars().collect::<Vec<char>>();
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut index = 0usize;
    while index < chars.len() {
        let character = chars[index];
        if character == '"' {
            in_quotes = !in_quotes;
            current.push(character);
            index += 1;
            continue;
        }
        if !in_quotes {
            if character == '\u{2014}' || character == '\u{2013}' {
                segments.push(current.trim().to_string());
                current.clear();
                index += 1;
                continue;
            }
            if character == '-'
                && index > 0
                && index + 1 < chars.len()
                && chars[index - 1] == ' '
                && chars[index + 1] == ' '
            {
                segments.push(current.trim().to_string());
                current.clear();
                index += 1;
                continue;
            }
        }
        current.push(character);
        index += 1;
    }
    if !current.trim().is_empty() {
        segments.push(current.trim().to_string());
    }
    segments.retain(|segment| !segment.is_empty());
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn read_samples() -> Vec<(String, String)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/items");
        let mut result = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("txt") {
                result.push((
                    path.file_name().unwrap().to_string_lossy().to_string(),
                    std::fs::read_to_string(path).unwrap(),
                ));
            }
        }
        result.sort_by(|left, right| left.0.cmp(&right.0));
        result
    }

    fn sample(name_part: &str) -> String {
        let samples = read_samples();
        samples
            .into_iter()
            .find(|(name, _)| name.contains(name_part))
            .map(|(_, text)| text)
            .unwrap_or_else(|| panic!("missing sample containing {name_part}"))
    }

    #[test]
    fn parses_header_forms() {
        let header =
            parse_mod_header("{ Prefix Modifier \"Fecund\" (Tier: 1) \u{2014} Life }").unwrap();
        assert_eq!(header.kind, HeaderKind::Prefix);
        assert_eq!(header.name.as_deref(), Some("Fecund"));
        assert_eq!(header.tier, Some(1));
        assert_eq!(header.tags, vec!["Life"]);

        let header = parse_mod_header(
            "{ Master Crafted Suffix Modifier \"of Craft\" (Rank: 3) \u{2014} Elemental, Cold, Resistance }",
        )
        .unwrap();
        assert!(header.crafted);
        assert_eq!(header.kind, HeaderKind::Suffix);
        assert_eq!(header.rank, Some(3));
        assert_eq!(header.tags, vec!["Elemental", "Cold", "Resistance"]);

        let header = parse_mod_header(
            "{ Prefix Modifier \"Freezing\" (Tier: 5) \u{2014} Damage, Elemental, Cold, Caster  \u{2014} 8% Increased }",
        )
        .unwrap();
        assert_eq!(header.increased, Some(8));

        let header = parse_mod_header(
            "{ Searing Exarch Implicit Modifier (Exquisite) \u{2014} Resistance }",
        )
        .unwrap();
        assert_eq!(header.kind, HeaderKind::Implicit);
        assert_eq!(header.tier_label.as_deref(), Some("Exquisite"));
    }

    #[test]
    fn parses_real_captures() {
        for (name, text) in read_samples() {
            let item = parse_item(&text);
            assert!(
                item.item_class.is_some() || name.contains("pob-test"),
                "{name}: missing item class"
            );
        }
    }

    #[test]
    fn body_armour_capture() {
        let item = parse_item(&sample("advanced-4"));
        assert_eq!(item.item_class.as_deref(), Some("Body Armours"));
        assert_eq!(item.rarity, Some(Rarity::Rare));
        assert_eq!(item.name.as_deref(), Some("Death Shelter"));
        assert_eq!(item.base_type.as_deref(), Some("Necrotic Armour"));
        assert_eq!(item.item_level, Some(86));
        assert_eq!(item.implicit_groups().count(), 2);
        assert_eq!(item.explicit_groups().count(), 6);
        assert_eq!(
            item.groups
                .iter()
                .filter(|group| group.kind == LineKind::Enchant)
                .count(),
            1
        );
    }

    #[test]
    fn fractured_weapon_capture() {
        let item = parse_item(&sample("advanced-7"));
        assert!(item.flags.fractured);
        let explicit = item.explicit_groups().collect::<Vec<_>>();
        assert_eq!(explicit.len(), 6);
        assert!(explicit.iter().any(|group| {
            group
                .header
                .as_ref()
                .is_some_and(|header| header.fractured && header.kind == HeaderKind::Suffix)
        }));
        assert!(explicit.iter().any(|group| {
            group
                .header
                .as_ref()
                .is_some_and(|header| header.crafted && header.name.as_deref() == Some("Upgraded"))
        }));
    }

    #[test]
    fn flask_multiline_affix() {
        let item = parse_item(&sample("advanced-2"));
        let explicit = item.explicit_groups().collect::<Vec<_>>();
        assert_eq!(explicit.len(), 2);
        assert_eq!(explicit[1].lines.len(), 2);
    }

    #[test]
    fn magic_amulet_capture() {
        let item = parse_item(&sample("advanced-1"));
        assert_eq!(item.rarity, Some(Rarity::Magic));
        assert_eq!(
            item.groups
                .iter()
                .filter(|group| group.kind == LineKind::Enchant)
                .count(),
            1
        );
        assert_eq!(
            item.groups
                .iter()
                .filter(|group| group.kind == LineKind::Explicit)
                .count(),
            1
        );
    }

    #[test]
    fn normal_currency_has_no_mods() {
        let item = parse_item(&sample("normal-10"));
        assert_eq!(item.rarity, Some(Rarity::Currency));
        assert!(item.groups.is_empty());
    }

    #[test]
    fn normal_gem_has_no_mods() {
        let item = parse_item(&sample("normal-15"));
        assert_eq!(item.rarity, Some(Rarity::Gem));
        assert!(item.groups.is_empty());
    }

    #[test]
    fn warning_line_before_unique_name() {
        let item = parse_item(&sample("advanced-13"));
        assert_eq!(item.name.as_deref(), Some("Replica Voideye"));
        assert_eq!(item.base_type.as_deref(), Some("Unset Ring"));
    }
}
