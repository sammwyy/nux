//! Minimal ANSI colorization for plain CLI command output (`ls`, `daemon`,
//! `config`, ...) — not the TUI, which styles itself through `ratatui`.

use crate::config::ColorMode;
use std::io::IsTerminal;

/// Resolves whether to colorize, given the configured mode and an optional
/// `--colors`/`--no-colors` override (which always wins).
pub fn should_colorize(mode: ColorMode, forced: Option<bool>) -> bool {
    if let Some(f) = forced {
        return f;
    }
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    }
}

#[derive(Clone, Copy)]
pub struct Painter {
    enabled: bool,
}

impl Painter {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.wrap("36", s)
    }

    /// Very small TOML syntax highlight: `[section]` headers in cyan, string
    /// values in green, everything else left alone.
    pub fn toml(&self, source: &str) -> String {
        if !self.enabled {
            return source.to_string();
        }
        source
            .lines()
            .map(|line| {
                let trimmed = line.trim_start();
                if trimmed.starts_with('[') {
                    self.cyan(line)
                } else if let Some(eq) = line.find(" = ") {
                    let (key, rest) = line.split_at(eq);
                    let value = &rest[3..];
                    let value = if value.starts_with('"') { self.green(value) } else { self.yellow(value) };
                    format!("{key} = {value}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_override_wins_over_mode() {
        assert!(should_colorize(ColorMode::Never, Some(true)));
        assert!(!should_colorize(ColorMode::Always, Some(false)));
    }

    #[test]
    fn always_and_never_ignore_terminal_detection() {
        assert!(should_colorize(ColorMode::Always, None));
        assert!(!should_colorize(ColorMode::Never, None));
    }

    #[test]
    fn painter_wraps_only_when_enabled() {
        let on = Painter::new(true);
        let off = Painter::new(false);
        assert_eq!(off.bold("x"), "x");
        assert_ne!(on.bold("x"), "x");
        assert!(on.bold("x").contains('x'));
    }

    #[test]
    fn toml_highlight_is_a_noop_when_disabled() {
        let src = "[layout]\nfoo = \"bar\"";
        assert_eq!(Painter::new(false).toml(src), src);
    }

    #[test]
    fn toml_highlight_wraps_sections_and_strings_when_enabled() {
        let out = Painter::new(true).toml("[layout]\nname = \"x\"\ncount = 3");
        assert!(out.contains("\x1b[36m[layout]\x1b[0m"));
        assert!(out.contains("\x1b[32m\"x\"\x1b[0m"));
        assert!(out.contains("\x1b[33m3\x1b[0m"));
    }
}
