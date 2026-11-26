//! Text normalization shared by item lines and mod templates.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Roll {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub percentile: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LineFlags {
    pub implicit: bool,
    pub crafted: bool,
    pub fractured: bool,
    pub enchant: bool,
    pub scourge: bool,
    pub unscalable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NormalizedText {
    /// Text with every numeric value and range replaced by `#`.
    pub template: String,
    /// Plain numeric values, in text order.
    pub values: Vec<f64>,
    /// Inline `value(min-max)` roll ranges.
    pub rolls: Vec<Roll>,
    /// Every numeric range, including ranges that had no rolled value.
    pub ranges: Vec<(f64, f64)>,
    pub unscalable: bool,
}

impl NormalizedText {
    pub fn matches_template(&self, other: &NormalizedText) -> bool {
        self.template == other.template
    }
}

/// Normalize an item mod line, stripping trailing state markers.
pub fn normalize_item_line(text: &str) -> (NormalizedText, LineFlags) {
    let mut text = text.trim().to_string();
    let mut flags = LineFlags::default();

    for (marker, set) in [
        ("(implicit)", 0usize),
        ("(crafted)", 1),
        ("(fractured)", 2),
        ("(enchant)", 3),
        ("(scourge)", 4),
        ("(unscalable)", 5),
    ] {
        if let Some(index) = text.rfind(marker) {
            if text[index + marker.len()..].trim().is_empty() {
                text.truncate(index);
                text = text.trim_end().to_string();
                match set {
                    0 => flags.implicit = true,
                    1 => flags.crafted = true,
                    2 => flags.fractured = true,
                    3 => flags.enchant = true,
                    4 => flags.scourge = true,
                    5 => flags.unscalable = true,
                    _ => {}
                }
            }
        }
    }

    if let Some(index) = find_unscalable_suffix(&text) {
        text.truncate(index);
        text = text.trim_end().to_string();
        flags.unscalable = true;
    }

    let mut normalized = normalize_text(&text);
    normalized.unscalable = flags.unscalable;
    (normalized, flags)
}

/// Normalize a mod data text, which uses bare ranges such as `+(115-129)`.
pub fn normalize_mod_text(text: &str) -> NormalizedText {
    normalize_text(text)
}

fn find_unscalable_suffix(text: &str) -> Option<usize> {
    for marker in ["\u{2014} Unscalable Value", "- Unscalable Value"] {
        if let Some(index) = text.find(marker) {
            return Some(index);
        }
    }
    None
}

fn normalize_text(text: &str) -> NormalizedText {
    let mut template = String::with_capacity(text.len());
    let mut values = Vec::new();
    let mut rolls = Vec::new();
    let mut ranges = Vec::new();

    let mut index = 0usize;
    while index < text.len() {
        let rest = &text[index..];
        let character = rest.chars().next().unwrap();

        if character == '(' {
            if let Some(range) = parse_range(rest) {
                template.push('#');
                ranges.push((range.0, range.1));
                index += range.2;
                continue;
            }
        }

        if character.is_ascii_digit() || character == '+' || character == '-' || character == '.' {
            if let Some(number) = parse_number(rest) {
                let after = &rest[number.1..];
                if after.starts_with('(') {
                    if let Some(range) = parse_range(after) {
                        let (min, max) = ordered_range(range.0, range.1);
                        rolls.push(Roll {
                            value: number.0,
                            min,
                            max,
                            percentile: percentile(number.0, min, max),
                        });
                        ranges.push((min, max));
                        if number.2 {
                            template.push('-');
                        } else if character == '+' {
                            template.push('+');
                        }
                        template.push('#');
                        index += number.1 + range.2;
                        continue;
                    }
                }
                values.push(number.0);
                if number.2 {
                    template.push('-');
                } else if character == '+' {
                    template.push('+');
                }
                template.push('#');
                index += number.1;
                continue;
            }
        }

        template.push(character);
        index += character.len_utf8();
    }

    NormalizedText {
        // The leading `+` is a display affordance, not part of the modifier
        // identity; some game strings include it and some PoB strings do not.
        template: collapse_whitespace(&template).replace('+', ""),
        values,
        rolls,
        ranges,
        unscalable: false,
    }
}

/// Parse an optional sign plus a number at the start of `text`.
/// Returns `(value, bytes_consumed, negative_sign)`.
fn parse_number(text: &str) -> Option<(f64, usize, bool)> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    let mut negative = false;
    let mut has_sign = false;
    if matches!(bytes.get(index), Some(b'+' | b'-')) {
        negative = bytes[index] == b'-';
        has_sign = true;
        index += 1;
    }
    let start_digits = index;
    while matches!(bytes.get(index), Some(b'0'..=b'9')) {
        index += 1;
    }
    if index == start_digits {
        return None;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while matches!(bytes.get(index), Some(b'0'..=b'9')) {
            index += 1;
        }
    }
    let value = text[..index].parse::<f64>().ok()?;
    Some((value, index, has_sign && negative))
}

/// Parse `(min-max)` at the start of `text`. Returns `(min, max, bytes_consumed)`.
fn parse_range(text: &str) -> Option<(f64, f64, usize)> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let (min, consumed_min, _) = parse_number(&text[1..])?;
    let after_min = 1 + consumed_min;
    if bytes.get(after_min) != Some(&b'-') {
        return None;
    }
    let (max, consumed_max, _) = parse_number(&text[after_min + 1..])?;
    let close = after_min + 1 + consumed_max;
    if bytes.get(close) != Some(&b')') {
        return None;
    }
    Some((min, max, close + 1))
}

fn ordered_range(min: f64, max: f64) -> (f64, f64) {
    if min <= max {
        (min, max)
    } else {
        (max, min)
    }
}

fn percentile(value: f64, min: f64, max: f64) -> f64 {
    let delta = max - min;
    if delta.abs() < f64::EPSILON {
        0.5
    } else {
        ((value - min) / delta).clamp(0.0, 1.0)
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_roll() {
        let (text, flags) = normalize_item_line("+142(130-144) to maximum Life");
        assert_eq!(text.template, "# to maximum Life");
        assert_eq!(text.rolls.len(), 1);
        assert!((text.rolls[0].percentile - 0.857).abs() < 0.001);
        assert!(!flags.unscalable);
    }

    #[test]
    fn inverted_range() {
        let (text, _) = normalize_item_line("27(28-26)% reduced Charges per use");
        assert_eq!(text.template, "#% reduced Charges per use");
        assert_eq!(text.rolls[0].min, 26.0);
        assert_eq!(text.rolls[0].max, 28.0);
        assert!((text.rolls[0].percentile - 0.5).abs() < 0.001);
    }

    #[test]
    fn multi_value_roll() {
        let (text, _) = normalize_item_line("Adds 17(16-20) to 35(30-36) Cold Damage to Spells");
        assert_eq!(text.template, "Adds # to # Cold Damage to Spells");
        assert_eq!(text.rolls.len(), 2);
        assert_eq!(text.ranges, vec![(16.0, 20.0), (30.0, 36.0)]);
    }

    #[test]
    fn mod_template_ranges() {
        let text = normalize_mod_text("+(115-129) to maximum Life");
        assert_eq!(text.template, "# to maximum Life");
        assert_eq!(text.ranges, vec![(115.0, 129.0)]);
        assert!(text.rolls.is_empty());
    }

    #[test]
    fn unscalable_suffix() {
        let (text, flags) = normalize_item_line("Hits can't be Evaded \u{2014} Unscalable Value");
        assert_eq!(text.template, "Hits can't be Evaded");
        assert!(flags.unscalable);
        assert!(text.unscalable);
    }

    #[test]
    fn markers() {
        let (text, flags) = normalize_item_line("+47% to Cold Resistance (fractured)");
        assert_eq!(text.template, "#% to Cold Resistance");
        assert!(flags.fractured);
    }

    #[test]
    fn negative_value() {
        let (text, _) = normalize_item_line("-3% to Critical Strike Chance");
        assert_eq!(text.template, "-#% to Critical Strike Chance");
        assert_eq!(text.values, vec![-3.0]);
    }

    #[test]
    fn bare_range_template() {
        let text = normalize_mod_text("(20-25)% chance to Avoid Elemental Ailments");
        assert_eq!(text.template, "#% chance to Avoid Elemental Ailments");
    }
}
