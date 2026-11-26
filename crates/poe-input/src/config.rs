use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use winapi::um::winuser::{
    GetAsyncKeyState, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME,
    VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT,
    VK_SPACE, VK_TAB, VK_UP,
};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("reading config {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("parsing config {path}: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },
    #[error("writing config {path}: {source}")]
    Write {
        path: String,
        source: std::io::Error,
    },
    #[error("encoding config: {0}")]
    Encode(#[from] toml::ser::Error),
    #[error("invalid shortcut `{name}`: {message}")]
    InvalidShortcut { name: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    keys: Vec<i32>,
}

impl Shortcut {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut keys = Vec::new();
        for part in text.split('+') {
            let token = part.trim().to_ascii_lowercase();
            if token.is_empty() {
                return Err("empty key in shortcut".to_string());
            }
            let vk = key_code(&token)?;
            if keys.contains(&vk) {
                return Err(format!("duplicate key `{token}`"));
            }
            keys.push(vk);
        }
        if keys.is_empty() {
            return Err("shortcut must contain at least one key".to_string());
        }
        if keys.len() > 4 {
            return Err("shortcut has too many keys".to_string());
        }
        Ok(Self { keys })
    }

    pub fn keys(&self) -> &[i32] {
        &self.keys
    }

    /// True while every key in the shortcut is currently held.
    pub fn is_down(&self) -> bool {
        !self.keys.is_empty() && self.keys.iter().all(|key| key_down(*key))
    }
}

fn key_code(token: &str) -> Result<i32, String> {
    let token = token.trim();
    let code = match token {
        "ctrl" | "control" => VK_CONTROL,
        "alt" => VK_MENU,
        "shift" => VK_SHIFT,
        "win" | "super" | "meta" => VK_LWIN,
        "space" => VK_SPACE,
        "tab" => VK_TAB,
        "enter" | "return" => VK_RETURN,
        "escape" | "esc" => VK_ESCAPE,
        "backspace" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "insert" | "ins" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        _ if token.len() == 1 && token.as_bytes()[0].is_ascii_alphabetic() => {
            token.as_bytes()[0].to_ascii_uppercase() as i32
        }
        _ if token.len() == 1 && token.as_bytes()[0].is_ascii_digit() => token.as_bytes()[0] as i32,
        _ if token.starts_with('f') => {
            let number = token[1..]
                .parse::<u32>()
                .map_err(|_| format!("unknown key `{token}`"))?;
            if !(1..=24).contains(&number) {
                return Err(format!("function key out of range: `{token}`"));
            }
            VK_F1 + (number as i32 - 1)
        }
        _ => return Err(format!("unknown key `{token}`")),
    };
    Ok(code)
}

fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotkeyConfig {
    #[serde(default = "default_start")]
    pub start: String,
    #[serde(default = "default_stop")]
    pub stop: String,
    #[serde(default = "default_calibrate")]
    pub calibrate: String,
    #[serde(default = "default_copy")]
    pub copy: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            start: default_start(),
            stop: default_stop(),
            calibrate: default_calibrate(),
            copy: default_copy(),
        }
    }
}

impl HotkeyConfig {
    pub fn parse(&self) -> Result<ParsedHotkeys, ConfigError> {
        Ok(ParsedHotkeys {
            start: self.parse_one("start", &self.start)?,
            stop: self.parse_one("stop", &self.stop)?,
            calibrate: self.parse_one("calibrate", &self.calibrate)?,
            copy: self.parse_one("copy", &self.copy)?,
        })
    }

    fn parse_one(&self, name: &str, value: &str) -> Result<Shortcut, ConfigError> {
        Shortcut::parse(value).map_err(|message| ConfigError::InvalidShortcut {
            name: name.to_string(),
            message,
        })
    }
}

fn default_start() -> String {
    "F9".to_string()
}

fn default_stop() -> String {
    "F12".to_string()
}

fn default_calibrate() -> String {
    "F8".to_string()
}

fn default_copy() -> String {
    "Ctrl+C".to_string()
}

#[derive(Debug, Clone)]
pub struct ParsedHotkeys {
    pub start: Shortcut,
    pub stop: Shortcut,
    pub calibrate: Shortcut,
    pub copy: Shortcut,
}

impl Default for ParsedHotkeys {
    fn default() -> Self {
        HotkeyConfig::default()
            .parse()
            .expect("default hotkeys must be valid")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
}

impl AppConfig {
    pub fn default_path() -> PathBuf {
        PathBuf::from("config.toml")
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        config.hotkeys.parse()?;
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        self.hotkeys.parse()?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                    path: parent.display().to_string(),
                    source,
                })?;
            }
        }
        let text = toml::to_string_pretty(self)?;
        fs::write(path, text).map_err(|source| ConfigError::Write {
            path: path.display().to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winapi::um::winuser::{VK_F12, VK_F8};

    #[test]
    fn parses_modifier_and_key() {
        let shortcut = Shortcut::parse("Ctrl+C").unwrap();
        assert_eq!(shortcut.keys(), &[VK_CONTROL, 'C' as i32]);
        assert_eq!(Shortcut::parse("F12").unwrap().keys(), &[VK_F12]);
        assert_eq!(
            Shortcut::parse("Shift + F8").unwrap().keys(),
            &[VK_SHIFT, VK_F8]
        );
    }

    #[test]
    fn rejects_invalid_shortcuts() {
        assert!(Shortcut::parse("").is_err());
        assert!(Shortcut::parse("Ctrl+").is_err());
        assert!(Shortcut::parse("F25").is_err());
        assert!(Shortcut::parse("Ctrl+Ctrl").is_err());
        assert!(Shortcut::parse("not-a-key").is_err());
    }

    #[test]
    fn defaults_and_round_trip() {
        let config = AppConfig::default();
        config.hotkeys.parse().unwrap();
        let encoded = toml::to_string(&config).unwrap();
        let decoded: AppConfig = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.hotkeys.start, "F9");
        assert_eq!(decoded.hotkeys.stop, "F12");
        assert_eq!(decoded.hotkeys.calibrate, "F8");
        assert_eq!(decoded.hotkeys.copy, "Ctrl+C");
    }

    #[test]
    fn rejects_invalid_config_value() {
        let config = AppConfig {
            hotkeys: HotkeyConfig {
                stop: "nope".to_string(),
                ..HotkeyConfig::default()
            },
        };
        assert!(config.hotkeys.parse().is_err());
    }
}
