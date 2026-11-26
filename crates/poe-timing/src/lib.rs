use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn from_entropy() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self::new(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal sample via Box-Muller.
    pub fn normal(&mut self, mean: f64, deviation: f64) -> f64 {
        if deviation <= 0.0 {
            return mean;
        }
        let u1 = self.next_f64().max(f64::MIN_POSITIVE);
        let u2 = self.next_f64();
        let z = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
        mean + deviation * z
    }

    pub fn chance(&mut self, probability: f64) -> bool {
        self.next_f64() < probability
    }

    pub fn index(&mut self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            (self.next_u64() % len as u64) as usize
        }
    }

    pub fn range_f64(&mut self, min: f64, max: f64) -> f64 {
        min + self.next_f64() * (max - min)
    }

    pub fn range_i64(&mut self, min: i64, max: i64) -> i64 {
        if max <= min {
            return min;
        }
        let span = (max - min + 1) as u64;
        min + (self.next_u64() % span) as i64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayProfile {
    pub mean_ms: f64,
    pub deviation_ms: f64,
    pub min_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Error)]
pub enum TimingError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("toml encode: {0}")]
    TomlEncode(#[from] toml::ser::Error),
    #[error("invalid delay profile: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelaySettings {
    #[serde(default = "default_action")]
    pub action: DelayProfile,
}

impl Default for DelaySettings {
    fn default() -> Self {
        Self {
            action: default_action(),
        }
    }
}

impl DelayProfile {
    pub const fn fixed(millis: u64) -> Self {
        Self {
            mean_ms: millis as f64,
            deviation_ms: 0.0,
            min_ms: millis,
            max_ms: millis,
        }
    }

    pub fn sample(&self, rng: &mut Rng) -> u64 {
        if self.deviation_ms <= 0.0 {
            return self
                .mean_ms
                .round()
                .clamp(self.min_ms as f64, self.max_ms as f64) as u64;
        }
        let value = rng.normal(self.mean_ms, self.deviation_ms);
        if value.is_finite() {
            value.round().clamp(self.min_ms as f64, self.max_ms as f64) as u64
        } else {
            self.mean_ms
                .round()
                .clamp(self.min_ms as f64, self.max_ms as f64) as u64
        }
    }

    pub fn validate(&self, name: &str) -> Result<(), TimingError> {
        if !self.mean_ms.is_finite() || !self.deviation_ms.is_finite() {
            return Err(TimingError::Invalid(format!(
                "{name}: mean and deviation must be finite"
            )));
        }
        if self.deviation_ms < 0.0 {
            return Err(TimingError::Invalid(format!(
                "{name}: deviation must be >= 0"
            )));
        }
        if self.min_ms > self.max_ms {
            return Err(TimingError::Invalid(format!(
                "{name}: min_ms exceeds max_ms"
            )));
        }
        if self.mean_ms < self.min_ms as f64 || self.mean_ms > self.max_ms as f64 {
            return Err(TimingError::Invalid(format!(
                "{name}: mean_ms must be within min_ms..=max_ms"
            )));
        }
        Ok(())
    }
}

impl DelaySettings {
    pub fn load(path: &Path) -> Result<Self, TimingError> {
        let text = std::fs::read_to_string(path)?;
        let settings: Self = toml::from_str(&text)?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn load_or_default(path: &Path) -> Result<Self, TimingError> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), TimingError> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), TimingError> {
        self.action.validate("action")?;
        Ok(())
    }
}

fn default_action() -> DelayProfile {
    DelayProfile {
        mean_ms: 60.0,
        deviation_ms: 10.0,
        min_ms: 50,
        max_ms: 70,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_profile_is_exact() {
        let profile = DelayProfile::fixed(123);
        let mut rng = Rng::new(1);
        for _ in 0..20 {
            assert_eq!(profile.sample(&mut rng), 123);
        }
    }

    #[test]
    fn gaussian_profile_stays_in_bounds() {
        let profile = DelayProfile {
            mean_ms: 60.0,
            deviation_ms: 10.0,
            min_ms: 50,
            max_ms: 70,
        };
        let mut rng = Rng::new(42);
        let mut total = 0u64;
        for _ in 0..10_000 {
            let sample = profile.sample(&mut rng);
            assert!((50..=70).contains(&sample));
            total += sample;
        }
        let mean = total as f64 / 10_000.0;
        assert!((mean - 60.0).abs() < 1.0);
    }

    #[test]
    fn rejects_invalid_profile() {
        let profile = DelayProfile {
            mean_ms: 60.0,
            deviation_ms: 10.0,
            min_ms: 70,
            max_ms: 50,
        };
        assert!(profile.validate("test").is_err());
    }

    #[test]
    fn accepts_integer_toml_numbers() {
        let profile: DelayProfile =
            toml::from_str("mean_ms = 60\ndeviation_ms = 10\nmin_ms = 50\nmax_ms = 70\n").unwrap();
        assert_eq!(profile.mean_ms, 60.0);
        assert_eq!(profile.deviation_ms, 10.0);
    }

    #[test]
    fn loads_partial_settings_with_defaults() {
        let settings: DelaySettings = toml::from_str(
            r#"
[action]
mean_ms = 30.0
deviation_ms = 5.0
min_ms = 20
max_ms = 40
"#,
        )
        .unwrap();
        assert_eq!(settings.action.mean_ms, 30.0);
    }
}
