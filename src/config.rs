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
    /// Leave a tab visible (marked `[exited]`) until dismissed, instead of
    /// removing it the instant its process exits.
    pub keep_exited_tab_open: bool,
    /// Whether plain CLI commands (`ls`, `daemon`, `config`, ...) colorize
    /// their output. `--colors`/`--no-colors` override this per invocation.
    pub color: ColorMode,
    pub keybindings: Keybindings,
    pub layout: LayoutConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: None,
            scrollback_lines: 5000,
            keep_exited_tab_open: false,
            color: ColorMode::Auto,
            keybindings: Keybindings::default(),
            layout: LayoutConfig::default(),
        }
    }
}

/// `Always`/`Never` force color on or off; `Auto` colors only when stdout is
/// a terminal and `NO_COLOR` isn't set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(ColorMode::Auto),
            "always" => Ok(ColorMode::Always),
            "never" => Ok(ColorMode::Never),
            other => Err(format!("expected \"auto\", \"always\" or \"never\", got {other:?}")),
        }
    }
}

/// Which terminal row (`Top`/`Bottom`) and side (`Left`/`Right`) a bar renders in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Row {
    Top,
    Bottom,
}

impl Row {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "top" => Ok(Row::Top),
            "bottom" => Ok(Row::Bottom),
            other => Err(format!("expected \"top\" or \"bottom\", got {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "left" => Ok(Side::Left),
            "right" => Ok(Side::Right),
            other => Err(format!("expected \"left\" or \"right\", got {other:?}")),
        }
    }
}

/// Where the two status bars render.
///
/// `tab_bar` (the "Nux" label plus the scrollable tab strip) and `workspace_bar`
/// (the current tab's directory) each get a row and a side. Two bars sharing a
/// row split it between their sides; a bar alone on its row gets the full
/// width. A row nobody uses isn't reserved at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    pub tab_bar_row: Row,
    pub tab_bar_side: Side,
    pub workspace_bar_row: Row,
    pub workspace_bar_side: Side,
    /// Max columns given to the workspace bar when sharing a row with the tab bar.
    pub workspace_bar_width: u16,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            tab_bar_row: Row::Bottom,
            tab_bar_side: Side::Left,
            workspace_bar_row: Row::Bottom,
            workspace_bar_side: Side::Right,
            workspace_bar_width: 32,
        }
    }
}

impl LayoutConfig {
    /// (top rows used, bottom rows used) — 0 or 1 each, depending on whether
    /// any bar is assigned to that row.
    pub fn reserved_rows(&self) -> (u16, u16) {
        let top = self.tab_bar_row == Row::Top || self.workspace_bar_row == Row::Top;
        let bottom = self.tab_bar_row == Row::Bottom || self.workspace_bar_row == Row::Bottom;
        (top as u16, bottom as u16)
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
            new_tab: "Alt+n".into(),
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

    /// Writes this config to disk at [`Config::config_path`].
    pub fn save(&self) -> std::io::Result<()> {
        self.write_default(&Self::config_path())
    }

    pub fn pretty(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(format!("expected true/false, got {other:?}")),
    }
}

fn keybind_field<'a>(kb: &'a mut Keybindings, name: &str) -> Option<&'a mut String> {
    Some(match name {
        "new_tab" => &mut kb.new_tab,
        "next_tab" => &mut kb.next_tab,
        "prev_tab" => &mut kb.prev_tab,
        "close_tab" => &mut kb.close_tab,
        "rename_tab" => &mut kb.rename_tab,
        "detach" => &mut kb.detach,
        "picker" => &mut kb.picker,
        _ => return None,
    })
}

/// Sets a dotted config key (e.g. `"scrollback_lines"`, `"keybindings.new_tab"`,
/// `"layout.tab_bar_row"`) from a raw string, type-checking and — for
/// keybindings and layout enums — semantically validating it first. Does not
/// persist the change; call [`Config::save`] afterward.
pub fn set_config_key(cfg: &mut Config, key: &str, value: &str) -> Result<(), String> {
    if let Some(name) = key.strip_prefix("keybindings.") {
        let field =
            keybind_field(&mut cfg.keybindings, name).ok_or_else(|| format!("unknown keybinding {name:?}"))?;
        parse_keybind(value)?;
        *field = value.to_string();
        return Ok(());
    }
    if let Some(name) = key.strip_prefix("layout.") {
        match name {
            "tab_bar_row" => cfg.layout.tab_bar_row = Row::parse(value)?,
            "tab_bar_side" => cfg.layout.tab_bar_side = Side::parse(value)?,
            "workspace_bar_row" => cfg.layout.workspace_bar_row = Row::parse(value)?,
            "workspace_bar_side" => cfg.layout.workspace_bar_side = Side::parse(value)?,
            "workspace_bar_width" => {
                cfg.layout.workspace_bar_width =
                    value.parse().map_err(|_| format!("workspace_bar_width must be an integer, got {value:?}"))?;
            }
            other => return Err(format!("unknown layout key {other:?}")),
        }
        return Ok(());
    }
    match key {
        "scrollback_lines" => {
            cfg.scrollback_lines =
                value.parse().map_err(|_| format!("scrollback_lines must be a non-negative integer, got {value:?}"))?;
        }
        "keep_exited_tab_open" => cfg.keep_exited_tab_open = parse_bool(value)?,
        "color" => cfg.color = ColorMode::parse(value)?,
        "shell" => {
            cfg.shell = if value.is_empty() || value.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(value.to_string())
            };
        }
        other => return Err(format!("unknown config key {other:?}")),
    }
    Ok(())
}

/// Reads a dotted config key's current value, formatted as TOML (so a string
/// comes back quoted, a bool as `true`/`false`, etc).
pub fn get_config_key(cfg: &Config, key: &str) -> Option<String> {
    let value = toml::Value::try_from(cfg).ok()?;
    let mut cur = &value;
    for part in key.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur.to_string())
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

    #[test]
    fn reserved_rows_default_is_bottom_only() {
        assert_eq!(LayoutConfig::default().reserved_rows(), (0, 1));
    }

    #[test]
    fn reserved_rows_both_bars_on_same_row() {
        let layout =
            LayoutConfig { tab_bar_row: Row::Top, workspace_bar_row: Row::Top, ..LayoutConfig::default() };
        assert_eq!(layout.reserved_rows(), (1, 0));
    }

    #[test]
    fn reserved_rows_split_across_rows() {
        let layout =
            LayoutConfig { tab_bar_row: Row::Top, workspace_bar_row: Row::Bottom, ..LayoutConfig::default() };
        assert_eq!(layout.reserved_rows(), (1, 1));
    }

    #[test]
    fn set_config_key_updates_typed_fields() {
        let mut cfg = Config::default();

        set_config_key(&mut cfg, "scrollback_lines", "8000").unwrap();
        assert_eq!(cfg.scrollback_lines, 8000);

        set_config_key(&mut cfg, "keep_exited_tab_open", "true").unwrap();
        assert!(cfg.keep_exited_tab_open);

        set_config_key(&mut cfg, "shell", "/bin/zsh").unwrap();
        assert_eq!(cfg.shell.as_deref(), Some("/bin/zsh"));
        set_config_key(&mut cfg, "shell", "none").unwrap();
        assert_eq!(cfg.shell, None);

        set_config_key(&mut cfg, "keybindings.new_tab", "Ctrl+t").unwrap();
        assert_eq!(cfg.keybindings.new_tab, "Ctrl+t");

        set_config_key(&mut cfg, "layout.tab_bar_row", "Top").unwrap();
        assert_eq!(cfg.layout.tab_bar_row, Row::Top);
        set_config_key(&mut cfg, "layout.workspace_bar_width", "10").unwrap();
        assert_eq!(cfg.layout.workspace_bar_width, 10);
    }

    #[test]
    fn set_config_key_rejects_bad_types_and_unknown_keys() {
        let mut cfg = Config::default();
        assert!(set_config_key(&mut cfg, "scrollback_lines", "not-a-number").is_err());
        assert!(set_config_key(&mut cfg, "keep_exited_tab_open", "maybe").is_err());
        assert!(set_config_key(&mut cfg, "layout.tab_bar_row", "sideways").is_err());
        assert!(set_config_key(&mut cfg, "keybindings.new_tab", "not a keybind at all").is_err());
        assert!(set_config_key(&mut cfg, "nonexistent", "x").is_err());
        // Rejected sets must not mutate the config.
        assert_eq!(cfg.scrollback_lines, Config::default().scrollback_lines);
    }

    #[test]
    fn get_config_key_reads_current_values() {
        let cfg = Config::default();
        assert_eq!(get_config_key(&cfg, "scrollback_lines").as_deref(), Some("5000"));
        assert_eq!(get_config_key(&cfg, "keybindings.new_tab").as_deref(), Some("\"Alt+n\""));
        assert_eq!(get_config_key(&cfg, "nonexistent"), None);
    }
}
