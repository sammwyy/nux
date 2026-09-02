//! Rendering: the terminal viewport, status bars and modal overlays (picker, rename).

use super::tui::{Mode, State};
use crate::config::{Row, Side};
use crate::protocol::TabInfo;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget};
use ratatui::Frame;

pub fn render(f: &mut Frame, state: &State) {
    let area = f.area();
    let (top_h, bottom_h) = state.layout.reserved_rows();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(top_h), Constraint::Min(1), Constraint::Length(bottom_h)])
        .split(area);
    let (top_area, term_area, bottom_area) = (chunks[0], chunks[1], chunks[2]);

    if let Some(screen) = state.screen() {
        f.render_widget(TermView { screen }, term_area);
        let (row, col) = screen.cursor_position();
        if !screen.hide_cursor() && row < term_area.height && col < term_area.width {
            f.set_cursor_position((term_area.x + col, term_area.y + row));
        }
    } else {
        let msg = Paragraph::new(state.status.clone().unwrap_or_else(|| "connecting...".into()))
            .alignment(Alignment::Center);
        f.render_widget(msg, term_area);
    }

    render_row(f, top_area, Row::Top, state);
    render_row(f, bottom_area, Row::Bottom, state);

    match &state.mode {
        Mode::Picker { query, matches, selected } => render_picker(f, area, query, matches, *selected),
        Mode::Rename { input } => render_rename(f, area, input),
        Mode::Normal => {}
    }
}

fn render_row(f: &mut Frame, area: Rect, row: Row, state: &State) {
    if area.height == 0 {
        return;
    }
    let layout = &state.layout;
    let tab_here = layout.tab_bar_row == row;
    let ws_here = layout.workspace_bar_row == row;

    match (tab_here, ws_here) {
        (false, false) => {}
        (true, false) => render_tab_bar(f, area, state),
        (false, true) => render_workspace_bar(f, area, state),
        (true, true) if layout.tab_bar_side == layout.workspace_bar_side => render_tab_bar(f, area, state),
        (true, true) => {
            let ws_w = layout.workspace_bar_width.min(area.width);
            let tab_w = area.width - ws_w;
            let left = Rect { x: area.x, y: area.y, width: tab_w, height: area.height };
            let right = Rect { x: area.x + tab_w, y: area.y, width: ws_w, height: area.height };
            let (tab_rect, ws_rect) = if layout.tab_bar_side == Side::Left { (left, right) } else { (right, left) };
            render_tab_bar(f, tab_rect, state);
            render_workspace_bar(f, ws_rect, state);
        }
    }
}

fn vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

struct TermView<'a> {
    screen: &'a vt100::Screen,
}

impl Widget for TermView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (rows, cols) = self.screen.size();
        for row in 0..rows.min(area.height) {
            for col in 0..cols.min(area.width) {
                let Some(cell) = self.screen.cell(row, col) else { continue };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style = Style::default().fg(vt_color(cell.fgcolor())).bg(vt_color(cell.bgcolor()));
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let contents = cell.contents();
                let text = if contents.is_empty() { " " } else { &contents };
                buf.set_string(area.x + col, area.y + row, text, style);
            }
        }
    }
}

fn tab_label(tab: &TabInfo) -> String {
    let title = if tab.title.is_empty() { tab.program().to_string() } else { tab.title.clone() };
    if tab.exit.is_some() {
        format!(" {}:{title} [x] ", tab.id)
    } else {
        format!(" {}:{title} ", tab.id)
    }
}

fn tab_style(tab: &TabInfo, active: bool) -> Style {
    let dead = tab.exit.is_some();
    let mut style = if active {
        let base = Style::default().add_modifier(Modifier::BOLD);
        if dead { base.fg(Color::Black).bg(Color::Yellow) } else { base.fg(Color::Black).bg(Color::Cyan) }
    } else if dead {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Gray)
    };
    if tab.bell && !active {
        style = style.fg(Color::Yellow);
    }
    style
}

/// Picks the contiguous slice of tabs to show and whether either edge has
/// more hidden beyond it, keeping `selected` roughly centered until an edge
/// of the list is reached.
fn tab_window(labels: &[String], selected: usize, width: u16) -> (usize, usize, bool, bool) {
    let n = labels.len();
    if n == 0 {
        return (0, 0, false, false);
    }
    let avail = width.saturating_sub(4) as usize; // room for both arrows
    let mut start = selected;
    let mut end = selected + 1;
    let mut used = labels[selected].chars().count();
    loop {
        let mut grew = false;
        if end < n {
            let w = labels[end].chars().count();
            if used + w <= avail {
                used += w;
                end += 1;
                grew = true;
            }
        }
        if start > 0 {
            let w = labels[start - 1].chars().count();
            if used + w <= avail {
                used += w;
                start -= 1;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    (start, end, start > 0, end < n)
}

fn render_tab_bar(f: &mut Frame, area: Rect, state: &State) {
    let prefix = "Nux \u{203a} ";
    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        prefix,
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )];

    if state.tabs.is_empty() {
        spans.push(Span::styled("no tabs — new-tab to open one", Style::default().fg(Color::DarkGray)));
    } else {
        let labels: Vec<String> = state.tabs.iter().map(tab_label).collect();
        let selected = state.tabs.iter().position(|t| Some(t.id) == state.current_tab).unwrap_or(0);
        let strip_width = area.width.saturating_sub(prefix.chars().count() as u16);
        let (start, end, show_left, show_right) = tab_window(&labels, selected, strip_width);

        if show_left {
            spans.push(Span::styled("\u{2039}", Style::default().fg(Color::DarkGray)));
        }
        for (i, tab) in state.tabs[start..end].iter().enumerate() {
            let active = Some(tab.id) == state.current_tab;
            spans.push(Span::styled(labels[start + i].clone(), tab_style(tab, active)));
        }
        if show_right {
            spans.push(Span::styled("\u{203a}", Style::default().fg(Color::DarkGray)));
        }
    }

    if let Some(cur) = state.tabs.iter().find(|t| Some(t.id) == state.current_tab) {
        if let Some(exit) = cur.exit {
            let msg = if exit.success {
                "  process exited".to_string()
            } else {
                format!("  process exited (code {})", exit.code)
            };
            spans.push(Span::styled(msg, Style::default().fg(Color::Yellow)));
        }
    }
    if let Some(status) = &state.status {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(status.clone(), Style::default().fg(Color::Red)));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_workspace_bar(f: &mut Frame, area: Rect, state: &State) {
    let text = state
        .tabs
        .iter()
        .find(|t| Some(t.id) == state.current_tab)
        .map(|t| trim_path(&t.workspace, area.width))
        .unwrap_or_default();
    let p = Paragraph::new(text).alignment(Alignment::Right).style(Style::default().fg(Color::DarkGray));
    f.render_widget(p, area);
}

fn trim_path(path: &str, width: u16) -> String {
    let width = width as usize;
    let len = path.chars().count();
    if len <= width {
        return format!("{path:>width$}");
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "\u{2026}".to_string();
    }
    let tail: String = path.chars().skip(len - (width - 1)).collect();
    format!("\u{2026}{tail}")
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn render_picker(f: &mut Frame, area: Rect, query: &str, matches: &[TabInfo], selected: usize) {
    let popup = centered(area, 50, 12);
    f.render_widget(Clear, popup);
    let items: Vec<ListItem> = matches
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let title = if t.title.is_empty() { t.program().to_string() } else { t.title.clone() };
            let line = format!("{:>3}  {title}", t.id);
            let style = if i == selected {
                Style::default().bg(Color::Cyan).fg(Color::Black)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" switch to: {query} "));
    let list = List::new(items).block(block);
    f.render_widget(list, popup);
}

fn render_rename(f: &mut Frame, area: Rect, input: &str) {
    let popup = centered(area, 40, 3);
    f.render_widget(Clear, popup);
    let block = Block::default().borders(Borders::ALL).title(" rename tab (Enter/Esc) ");
    let text = Paragraph::new(format!("{input}_")).block(block);
    f.render_widget(text, popup);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(widths: &[usize]) -> Vec<String> {
        widths.iter().map(|w| " ".repeat(*w)).collect()
    }

    #[test]
    fn window_shows_everything_when_it_fits() {
        let labels = labels(&[5, 5, 5]);
        assert_eq!(tab_window(&labels, 1, 30), (0, 3, false, false));
    }

    #[test]
    fn window_centers_selection_in_the_middle() {
        let labels = labels(&[3; 11]);
        let (start, end, left, right) = tab_window(&labels, 5, 3 * 3 + 4);
        assert!(start <= 5 && end > 5, "selection must stay visible");
        assert_eq!(end - start, 3);
        assert!(left && right, "hidden tabs on both sides");
    }

    #[test]
    fn window_clamps_at_the_start() {
        let labels = labels(&[3; 10]);
        let (start, _end, left, right) = tab_window(&labels, 0, 3 * 3 + 4);
        assert_eq!(start, 0);
        assert!(!left);
        assert!(right);
    }

    #[test]
    fn window_clamps_at_the_end() {
        let labels = labels(&[3; 10]);
        let (_start, end, left, right) = tab_window(&labels, 9, 3 * 3 + 4);
        assert_eq!(end, 10);
        assert!(left);
        assert!(!right);
    }

    #[test]
    fn window_handles_empty_tab_list() {
        assert_eq!(tab_window(&[], 0, 80), (0, 0, false, false));
    }

    #[test]
    fn trim_path_pads_short_paths() {
        assert_eq!(trim_path("/tmp", 10), "      /tmp");
    }

    #[test]
    fn trim_path_ellipsizes_long_paths() {
        let trimmed = trim_path("/very/long/workspace/path", 10);
        assert_eq!(trimmed.chars().count(), 10);
        assert!(trimmed.starts_with('\u{2026}'));
        assert!(trimmed.ends_with("path"));
    }
}
