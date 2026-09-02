//! User configuration: default shell, scrollback size and TUI keybindings.
//!
//! Loaded from `<config dir>/nux/config.toml` (created with sane defaults on first
//! run if it doesn't exist yet).

use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Shell used for new tabs when no explicit command is given. `None` means "detect".
    pub shell: Option<String>,
    /// Lines of scrollback kept per tab by the terminal emulator.
    pub scrollback_lines: usize,
    pub keybindings: Keybindings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: None,
            scrollback_lines: 5000,
            keybindings: Keybindings::default(),
        }
    }
}

/// Keybindings, expressed as strings like `"Alt+n"`, `"Ctrl+q"`, `"F2"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Keybindings {
    pub new_tab: String,
    pub next_tab: String,
    pub prev_tab: String,
    pub close_tab: String,
    pub rename_tab: String,
    pub detach: String,
    pub picker: String,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            new_tab: "Alt+c".into(),
            next_tab: "Alt+Right".into(),
            prev_tab: "Alt+Left".into(),
            close_tab: "Alt+x".into(),
            rename_tab: "Alt+r".into(),
            detach: "Alt+d".into(),
            picker: "Alt+/".into(),
        }
    }
}

impl Config {
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("nux")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// Loads the config file, writing out defaults on first run. Falls back silently to
    /// in-memory defaults if the file can't be read or parsed.
    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_else(|e| {
                log::warn!("failed to parse {}: {e}; using defaults", path.display());
                Self::default()
            }),
            Err(_) => {
                let cfg = Self::default();
                let _ = cfg.write_default(&path);
                cfg
            }
        }
    }

    fn write_default(&self, path: &PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml = toml::to_string_pretty(self).unwrap_or_default();
        std::fs::write(path, toml)
    }
}

/// A parsed keybinding: the key code plus required modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keybind {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

/// Parses a keybind string such as `"Alt+Right"` or `"Ctrl+Shift+n"`.
pub fn parse_keybind(spec: &str) -> Result<Keybind, String> {
    let mut modifiers = KeyModifiers::NONE;
    let mut code = None;
    let parts: Vec<&str> = spec.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err(format!("empty keybind: {spec:?}"));
    }
    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "alt" | "opt" | "option" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            key => {
                if !is_last {
                    return Err(format!("unknown modifier {key:?} in {spec:?}"));
                }
                code = Some(parse_key_code(key)?);
            }
        }
    }
    let code = code.ok_or_else(|| format!("missing key in {spec:?}"))?;
    Ok(Keybind { code, modifiers })
}

fn parse_key_code(key: &str) -> Result<KeyCode, String> {
    if key.chars().count() == 1 {
        return Ok(KeyCode::Char(key.chars().next().unwrap()));
    }
    let code = match key.to_lowercase().as_str() {
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "esc" | "escape" => KeyCode::Esc,
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "space" => KeyCode::Char(' '),
        other if other.starts_with('f') && other[1..].parse::<u8>().is_ok() => {
            KeyCode::F(other[1..].parse().unwrap())
        }
        other => return Err(format!("unknown key {other:?}")),
    };
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_char() {
        let kb = parse_keybind("Alt+c").unwrap();
        assert_eq!(kb.code, KeyCode::Char('c'));
        assert_eq!(kb.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn parses_multiple_modifiers() {
        let kb = parse_keybind("Ctrl+Shift+n").unwrap();
        assert_eq!(kb.code, KeyCode::Char('n'));
        assert!(kb.modifiers.contains(KeyModifiers::CONTROL));
        assert!(kb.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn parses_named_keys() {
        assert_eq!(parse_keybind("Alt+Right").unwrap().code, KeyCode::Right);
        assert_eq!(parse_keybind("F2").unwrap().code, KeyCode::F(2));
        assert_eq!(parse_keybind("Alt+/").unwrap().code, KeyCode::Char('/'));
    }

    #[test]
    fn rejects_empty_and_garbage() {
        assert!(parse_keybind("").is_err());
        assert!(parse_keybind("Ctrl+Alt+").is_err());
        assert!(parse_keybind("Frobnicate+x").is_err());
    }

    #[test]
    fn default_keybindings_all_parse() {
        let kb = Keybindings::default();
        for spec in [
            &kb.new_tab,
            &kb.next_tab,
            &kb.prev_tab,
            &kb.close_tab,
            &kb.rename_tab,
            &kb.detach,
            &kb.picker,
        ] {
            parse_keybind(spec).unwrap_or_else(|e| panic!("default keybind {spec:?} failed: {e}"));
        }
    }
}
