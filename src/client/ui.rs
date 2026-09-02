//! Rendering: the terminal viewport, tab bar and modal overlays (picker, rename).

use super::tui::{Mode, State};
use crate::protocol::TabInfo;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget};
use ratatui::Frame;

pub fn render(f: &mut Frame, state: &State) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let (term_area, bar_area) = (chunks[0], chunks[1]);

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

    f.render_widget(tab_bar(state), bar_area);

    match &state.mode {
        Mode::Picker { query, matches, selected } => render_picker(f, area, query, matches, *selected),
        Mode::Rename { input } => render_rename(f, area, input),
        Mode::Normal => {}
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

fn tab_bar(state: &State) -> Paragraph<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if state.tabs.is_empty() {
        spans.push(Span::styled(
            " no tabs — press your new-tab key ",
            Style::default().fg(Color::DarkGray),
        ));
    }
    for tab in &state.tabs {
        let active = Some(tab.id) == state.current_tab;
        let title = if tab.title.is_empty() { tab.program().to_string() } else { tab.title.clone() };
        let label = format!(" {}:{title} ", tab.id);
        let mut style = if active {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        };
        if tab.bell && !active {
            style = style.fg(Color::Yellow);
        }
        spans.push(Span::styled(label, style));
    }
    if let Some(status) = &state.status {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(status.clone(), Style::default().fg(Color::Red)));
    }
    Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::DarkGray))
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
