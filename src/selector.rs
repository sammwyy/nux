//! Matches a user-supplied selector string (`nux -t <SELECTOR>`, `nux -k <SELECTOR>`)
//! against the list of currently open tabs.
//!
//! A selector is tried, in order, as:
//! 1. An exact numeric tab id.
//! 2. A case-insensitive substring match against the tab's title or program name.

use crate::protocol::TabInfo;

/// Finds every tab that matches `selector`.
///
/// Returns tabs sorted by id. An empty result means no match; a result with more than
/// one entry means the selector was ambiguous and the caller should let the user pick.
pub fn find_matches<'a>(tabs: &'a [TabInfo], selector: &str) -> Vec<&'a TabInfo> {
    if let Ok(id) = selector.parse::<u32>() {
        if let Some(tab) = tabs.iter().find(|t| t.id == id) {
            return vec![tab];
        }
    }

    let needle = selector.to_lowercase();
    let mut matches: Vec<&TabInfo> = tabs
        .iter()
        .filter(|t| {
            t.title.to_lowercase().contains(&needle) || t.program().to_lowercase().contains(&needle)
        })
        .collect();
    matches.sort_by_key(|t| t.id);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: u32, title: &str, program: &str) -> TabInfo {
        TabInfo {
            id,
            title: title.to_string(),
            command: vec![program.to_string()],
            pid: None,
            created_at: 0,
            cols: 80,
            rows: 24,
            bell: false,
            exit: None,
        }
    }

    #[test]
    fn matches_exact_id() {
        let tabs = vec![tab(0, "shell", "bash"), tab(1, "codex", "codex")];
        let m = find_matches(&tabs, "1");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, 1);
    }

    #[test]
    fn numeric_selector_falls_back_to_text_when_no_id_matches() {
        // A tab titled "1password" should still be reachable even though "1" also looks numeric,
        // as long as no tab actually has id 1.
        let tabs = vec![tab(0, "1password", "op")];
        let m = find_matches(&tabs, "1");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, 0);
    }

    #[test]
    fn matches_title_substring_case_insensitive() {
        let tabs = vec![tab(0, "Codex Session", "codex")];
        let m = find_matches(&tabs, "sess");
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn matches_program_name() {
        let tabs = vec![tab(0, "custom title", "/usr/bin/codex")];
        let m = find_matches(&tabs, "codex");
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn ambiguous_selector_returns_all_matches_sorted() {
        let tabs = vec![tab(2, "b", "vim"), tab(0, "a", "vim"), tab(1, "c", "vim")];
        let m = find_matches(&tabs, "vim");
        assert_eq!(m.iter().map(|t| t.id).collect::<Vec<_>>(), vec![0, 1, 2]);
    }

    #[test]
    fn no_match_returns_empty() {
        let tabs = vec![tab(0, "a", "bash")];
        assert!(find_matches(&tabs, "zzz").is_empty());
    }
}
