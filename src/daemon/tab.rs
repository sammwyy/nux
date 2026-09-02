//! A single tab: a PTY-backed child process plus the terminal emulator state that
//! mirrors its screen.

use crate::protocol::{Response, TabInfo};
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

struct TabState {
    parser: vt100::Parser,
    title: String,
    bell: bool,
}

pub struct Tab {
    pub id: u32,
    pub command: Vec<String>,
    pub created_at: i64,
    pub pid: Option<u32>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    state: Mutex<TabState>,
    subscribers: Mutex<HashMap<u64, Sender<Response>>>,
    alive: AtomicBool,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Tab {
    /// Spawns `command` in a new PTY of size `cols x rows` and starts the background
    /// thread that pumps PTY output into the terminal emulator and out to subscribers.
    ///
    /// `on_exit` is invoked from the reader thread once the child process's output
    /// stream ends (i.e. the process exited).
    ///
    /// Returns the tab plus the still-unstarted reader thread's inputs; the
    /// caller must call [`spawn_reader`] once the tab is registered wherever
    /// `on_exit` expects to find it (e.g. inserted into the manager's map) —
    /// otherwise a process that exits fast enough could have its `on_exit`
    /// callback run and unregister it *before* it was ever registered.
    #[allow(clippy::type_complexity)]
    pub fn spawn(
        id: u32,
        command: Vec<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        scrollback_lines: usize,
    ) -> anyhow::Result<(
        std::sync::Arc<Tab>,
        Box<dyn std::io::Read + Send>,
        Box<dyn Child + Send + Sync>,
    )> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let program = command.first().cloned().unwrap_or_else(|| "sh".to_string());
        let mut cmd = CommandBuilder::new(&program);
        cmd.args(command.iter().skip(1));
        if let Some(dir) = &cwd {
            cmd.cwd(dir);
        }
        // Child processes should never inherit nemux's own IPC socket env, but they do
        // need a sane TERM to behave like a real terminal.
        cmd.env("TERM", "xterm-256color");

        let child = pair.slave.spawn_command(cmd)?;
        let pid = child.process_id();
        let killer = child.clone_killer();
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let tab = std::sync::Arc::new(Tab {
            id,
            command: command.clone(),
            created_at: now_unix(),
            pid,
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            state: Mutex::new(TabState {
                parser: vt100::Parser::new(rows, cols, scrollback_lines),
                title: program,
                bell: false,
            }),
            subscribers: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
        });

        Ok((tab, reader, child))
    }

    pub fn info(&self) -> TabInfo {
        let state = self.state.lock().unwrap();
        let (rows, cols) = state.parser.screen().size();
        TabInfo {
            id: self.id,
            title: state.title.clone(),
            command: self.command.clone(),
            pid: self.pid,
            created_at: self.created_at,
            cols,
            rows,
            bell: state.bell,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn write_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.lock().unwrap().write_all(bytes)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master.lock().unwrap().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.state.lock().unwrap().parser.set_size(rows, cols);
        Ok(())
    }

    pub fn rename(&self, title: String) {
        let mut state = self.state.lock().unwrap();
        state.title = title;
    }

    pub fn kill(&self) {
        let _ = self.killer.lock().unwrap().kill();
    }

    /// Registers a subscriber and immediately hands back a full redraw of the current
    /// screen so the new subscriber starts in sync.
    pub fn subscribe(&self, conn_id: u64, tx: Sender<Response>) {
        let state = self.state.lock().unwrap();
        let full = state.parser.screen().contents_formatted();
        let _ = tx.send(Response::Screen { tab_id: self.id, data: full });
        drop(state);
        self.subscribers.lock().unwrap().insert(conn_id, tx);
    }

    pub fn unsubscribe(&self, conn_id: u64) {
        self.subscribers.lock().unwrap().remove(&conn_id);
    }

    fn broadcast(&self, event_for: impl Fn() -> Response) {
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|_, tx| tx.send(event_for()).is_ok());
    }
}

/// Starts the background thread that pumps a tab's PTY output into its
/// terminal emulator and out to subscribers. See [`Tab::spawn`].
pub fn spawn_reader(
    tab: std::sync::Arc<Tab>,
    mut reader: Box<dyn std::io::Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    on_exit: impl FnOnce(u32) + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 32 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let (diff, updated) = {
                        let mut state = tab.state.lock().unwrap();
                        let prev = state.parser.screen().clone();
                        state.parser.process(&buf[..n]);
                        let screen = state.parser.screen();
                        let diff = screen.contents_diff(&prev);
                        let title = screen.title().to_string();
                        let bell = !screen.bells_diff(&prev).is_empty();
                        let title_changed = title != state.title;
                        if title_changed && !title.is_empty() {
                            state.title = title;
                        }
                        if bell {
                            state.bell = true;
                        }
                        (diff, title_changed || bell)
                    };
                    if !diff.is_empty() {
                        tab.broadcast(|| Response::Screen { tab_id: tab.id, data: diff.clone() });
                    }
                    if updated {
                        let info = tab.info();
                        tab.broadcast(|| Response::TabUpdated(info.clone()));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        tab.alive.store(false, Ordering::SeqCst);
        let _ = child.wait();
        tab.broadcast(|| Response::TabClosed(tab.id));
        on_exit(tab.id);
    });
}
