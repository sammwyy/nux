//! The interactive multiplexer view: renders the attached tab's screen plus a
//! bottom tab bar, and turns keystrokes into requests against the daemon.

use super::input::key_to_bytes;
use super::ui;
use crate::config::{parse_keybind, Config, Keybind};
use crate::protocol::{read_message, write_message, Request, Response, TabInfo};
use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use interprocess::local_socket::traits::Stream as _;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::sync::mpsc;
use std::time::Duration;

/// What the TUI should do the moment it connects.
pub enum Start {
    /// Show the tab list and attach to the best-guess "current" tab, creating a
    /// default shell tab if none exist yet.
    Overview,
    /// Attach directly to an existing tab.
    Attach(u32),
    /// Create a new tab running `command` and attach to it.
    Create(Vec<String>),
}

struct ParsedKeybinds {
    new_tab: Keybind,
    next_tab: Keybind,
    prev_tab: Keybind,
    close_tab: Keybind,
    rename_tab: Keybind,
    detach: Keybind,
    picker: Keybind,
}

impl ParsedKeybinds {
    fn from_config(cfg: &Config) -> Self {
        let kb = &cfg.keybindings;
        let parse = |s: &str, fallback: &str| parse_keybind(s).unwrap_or_else(|_| parse_keybind(fallback).unwrap());
        Self {
            new_tab: parse(&kb.new_tab, "Alt+c"),
            next_tab: parse(&kb.next_tab, "Alt+Right"),
            prev_tab: parse(&kb.prev_tab, "Alt+Left"),
            close_tab: parse(&kb.close_tab, "Alt+x"),
            rename_tab: parse(&kb.rename_tab, "Alt+r"),
            detach: parse(&kb.detach, "Alt+d"),
            picker: parse(&kb.picker, "Alt+/"),
        }
    }
}

fn key_matches(key: &KeyEvent, bind: &Keybind) -> bool {
    key.code == bind.code && key.modifiers == bind.modifiers
}

pub enum Mode {
    Normal,
    Picker { query: String, matches: Vec<TabInfo>, selected: usize },
    Rename { input: String },
}

pub struct State {
    pub tabs: Vec<TabInfo>,
    pub current_tab: Option<u32>,
    pub status: Option<String>,
    pub mode: Mode,
    pub cols: u16,
    pub rows: u16,
    parser: Option<vt100::Parser>,
    awaiting_list: bool,
}

impl State {
    pub fn screen(&self) -> Option<&vt100::Screen> {
        self.parser.as_ref().map(|p| p.screen())
    }
}

/// Clamps to a minimum of 1x1: a terminal that hasn't reported a real size
/// yet (or a misbehaving one) can hand back 0 for either dimension, and a
/// zero-sized `vt100` grid panics on the first cell access.
fn viewport(term_size: (u16, u16)) -> (u16, u16) {
    (term_size.0.max(1), term_size.1.saturating_sub(1).max(1))
}

/// Runs the interactive TUI to completion (until the user detaches or the daemon
/// connection is lost). Blocks the calling thread.
pub fn run(config: Config, start: Start) -> anyhow::Result<()> {
    let stream = crate::client::ensure_daemon()?;
    let (mut recv, mut send) = stream.split();

    let (req_tx, req_rx) = mpsc::channel::<Request>();
    let writer = std::thread::spawn(move || {
        while let Ok(req) = req_rx.recv() {
            if write_message(&mut send, &req).is_err() {
                break;
            }
        }
    });

    let (resp_tx, resp_rx) = mpsc::channel::<Response>();
    let reader = std::thread::spawn(move || {
        while let Ok(resp) = read_message::<_, Response>(&mut recv) {
            if resp_tx.send(resp).is_err() {
                break;
            }
        }
    });

    let term_size = crossterm::terminal::size().unwrap_or((80, 24));
    let (cols, rows) = viewport(term_size);

    let mut state = State {
        tabs: Vec::new(),
        current_tab: None,
        status: None,
        mode: Mode::Normal,
        cols,
        rows,
        parser: None,
        awaiting_list: false,
    };

    match start {
        Start::Overview => {
            state.awaiting_list = true;
            let _ = req_tx.send(Request::ListTabs);
        }
        Start::Attach(id) => {
            let _ = req_tx.send(Request::Attach { tab_id: id, cols, rows });
        }
        Start::Create(command) => {
            let _ = req_tx.send(Request::CreateTab { command, cwd: None, cols, rows });
        }
    }

    let guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let keybinds = ParsedKeybinds::from_config(&config);

    let result = event_loop(&mut terminal, &mut state, &req_tx, &resp_rx, &keybinds);
    drop(guard);

    // Let the writer/reader threads wind down; the socket halves are dropped when
    // this function returns, which unblocks their I/O.
    drop(req_tx);
    let _ = writer.join();
    let _ = reader.join();

    result
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut State,
    req_tx: &mpsc::Sender<Request>,
    resp_rx: &mpsc::Receiver<Response>,
    keybinds: &ParsedKeybinds,
) -> anyhow::Result<()> {
    let mut dirty = true;
    loop {
        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if handle_key(key, state, req_tx, keybinds) {
                        break;
                    }
                    dirty = true;
                }
                Event::Resize(cols, rows) => {
                    handle_resize(cols, rows, state, req_tx);
                    dirty = true;
                }
                _ => {}
            }
        }

        loop {
            match resp_rx.try_recv() {
                Ok(resp) => {
                    apply_response(resp, state, req_tx);
                    dirty = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    state.status = Some("daemon connection lost".into());
                    terminal.draw(|f| ui::render(f, state))?;
                    std::thread::sleep(Duration::from_millis(600));
                    return Ok(());
                }
            }
        }

        if dirty {
            terminal.draw(|f| ui::render(f, state))?;
            dirty = false;
        }
    }
    Ok(())
}

fn apply_response(resp: Response, state: &mut State, req_tx: &mpsc::Sender<Request>) {
    match resp {
        Response::TabList(tabs) => {
            state.tabs = tabs;
            if state.awaiting_list {
                state.awaiting_list = false;
                if let Some(target) = state.tabs.iter().max_by_key(|t| t.id) {
                    let id = target.id;
                    let _ = req_tx.send(Request::Attach { tab_id: id, cols: state.cols, rows: state.rows });
                } else {
                    let _ = req_tx.send(Request::CreateTab {
                        command: Vec::new(),
                        cwd: None,
                        cols: state.cols,
                        rows: state.rows,
                    });
                }
            }
        }
        Response::Attached(info) => {
            state.current_tab = Some(info.id);
            state.parser = Some(vt100::Parser::new(state.rows, state.cols, 5000));
            upsert_tab(state, info);
        }
        Response::Screen { tab_id, data } => {
            if Some(tab_id) == state.current_tab {
                if let Some(parser) = state.parser.as_mut() {
                    parser.process(&data);
                }
            }
        }
        Response::TabUpdated(info) => upsert_tab(state, info),
        Response::TabClosed(id) => {
            state.tabs.retain(|t| t.id != id);
            if Some(id) == state.current_tab {
                state.current_tab = None;
                state.parser = None;
                state.status = Some(format!("tab {id} closed"));
                state.awaiting_list = true;
                let _ = req_tx.send(Request::ListTabs);
            }
        }
        Response::Error(e) => state.status = Some(e),
        Response::Ok | Response::Pong => {}
    }
}

fn upsert_tab(state: &mut State, info: TabInfo) {
    match state.tabs.iter_mut().find(|t| t.id == info.id) {
        Some(existing) => *existing = info,
        None => {
            state.tabs.push(info);
            state.tabs.sort_by_key(|t| t.id);
        }
    }
}

fn handle_resize(cols: u16, rows: u16, state: &mut State, req_tx: &mpsc::Sender<Request>) {
    let (cols, rows) = viewport((cols, rows));
    state.cols = cols;
    state.rows = rows;
    if let Some(parser) = state.parser.as_mut() {
        parser.set_size(rows, cols);
    }
    if state.current_tab.is_some() {
        let _ = req_tx.send(Request::Resize { cols, rows });
    }
}

/// Returns `true` if the TUI should exit (user detached).
fn handle_key(key: KeyEvent, state: &mut State, req_tx: &mpsc::Sender<Request>, keybinds: &ParsedKeybinds) -> bool {
    match &mut state.mode {
        Mode::Rename { input } => {
            match key.code {
                crossterm::event::KeyCode::Enter => {
                    let title = input.clone();
                    state.mode = Mode::Normal;
                    if let Some(id) = state.current_tab {
                        let _ = req_tx.send(Request::RenameTab { tab_id: id, title });
                    }
                }
                crossterm::event::KeyCode::Esc => state.mode = Mode::Normal,
                crossterm::event::KeyCode::Backspace => {
                    input.pop();
                }
                crossterm::event::KeyCode::Char(c) => input.push(c),
                _ => {}
            }
            return false;
        }
        Mode::Picker { query, matches, selected } => {
            match key.code {
                crossterm::event::KeyCode::Esc => state.mode = Mode::Normal,
                crossterm::event::KeyCode::Enter => {
                    if let Some(t) = matches.get(*selected) {
                        let id = t.id;
                        state.mode = Mode::Normal;
                        let _ = req_tx.send(Request::Attach { tab_id: id, cols: state.cols, rows: state.rows });
                    }
                }
                crossterm::event::KeyCode::Up => *selected = selected.saturating_sub(1),
                crossterm::event::KeyCode::Down => {
                    if *selected + 1 < matches.len() {
                        *selected += 1;
                    }
                }
                crossterm::event::KeyCode::Backspace => {
                    query.pop();
                    *matches = crate::selector::find_matches(&state.tabs, query).into_iter().cloned().collect();
                    *selected = 0;
                }
                crossterm::event::KeyCode::Char(c) => {
                    query.push(c);
                    *matches = crate::selector::find_matches(&state.tabs, query).into_iter().cloned().collect();
                    *selected = 0;
                }
                _ => {}
            }
            return false;
        }
        Mode::Normal => {}
    }

    if key_matches(&key, &keybinds.detach) {
        let _ = req_tx.send(Request::Detach);
        return true;
    }
    if key_matches(&key, &keybinds.new_tab) {
        let _ = req_tx.send(Request::CreateTab {
            command: Vec::new(),
            cwd: None,
            cols: state.cols,
            rows: state.rows,
        });
        return false;
    }
    if key_matches(&key, &keybinds.next_tab) {
        switch_relative(state, req_tx, 1);
        return false;
    }
    if key_matches(&key, &keybinds.prev_tab) {
        switch_relative(state, req_tx, -1);
        return false;
    }
    if key_matches(&key, &keybinds.close_tab) {
        if let Some(id) = state.current_tab {
            let _ = req_tx.send(Request::KillTab { tab_id: id });
        }
        return false;
    }
    if key_matches(&key, &keybinds.rename_tab) {
        if state.current_tab.is_some() {
            state.mode = Mode::Rename { input: String::new() };
        }
        return false;
    }
    if key_matches(&key, &keybinds.picker) {
        state.mode = Mode::Picker { query: String::new(), matches: state.tabs.clone(), selected: 0 };
        return false;
    }

    let bytes = key_to_bytes(key);
    if !bytes.is_empty() && state.current_tab.is_some() {
        let _ = req_tx.send(Request::Input(bytes));
    }
    false
}

fn switch_relative(state: &mut State, req_tx: &mpsc::Sender<Request>, delta: i32) {
    if state.tabs.is_empty() {
        return;
    }
    let ids: Vec<u32> = state.tabs.iter().map(|t| t.id).collect();
    let idx = state.current_tab.and_then(|id| ids.iter().position(|&x| x == id)).unwrap_or_default();
    let len = ids.len() as i32;
    let new_idx = ((idx as i32 + delta).rem_euclid(len)) as usize;
    let id = ids[new_idx];
    let _ = req_tx.send(Request::Attach { tab_id: id, cols: state.cols, rows: state.rows });
}
