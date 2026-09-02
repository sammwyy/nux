//! A single tab: a PTY-backed child process plus the terminal emulator state that
//! mirrors its screen.

use crate::protocol::{ExitInfo, Response, TabInfo};
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize};
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
    /// Set once the child process's output stream ends. The tab (and its
    /// final screen, bounded by the parser's scrollback limit) stays around
    /// until a client explicitly dismisses it — see [`Tab::kill`].
    exit: Option<ExitInfo>,
}

pub struct Tab {
    pub id: u32,
    pub command: Vec<String>,
    pub created_at: i64,
    pub pid: Option<u32>,
    pub workspace: String,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
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
    /// Spawns `command` in a new PTY of size `cols x rows`.
    ///
    /// Returns the tab plus the still-unstarted reader thread's inputs; the
    /// caller must call [`spawn_reader`] once the tab is registered wherever
    /// it needs to be discoverable (e.g. inserted into the manager's map) —
    /// otherwise a process that exits fast enough could be marked exited
    /// before anyone could ever see it as running.
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
        // Child processes should never inherit nux's own IPC socket env, but they do
        // need a sane TERM to behave like a real terminal.
        cmd.env("TERM", "xterm-256color");

        let child = pair.slave.spawn_command(cmd)?;
        let pid = child.process_id();
        let killer = child.clone_killer();
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let workspace = cwd.clone().unwrap_or_else(|| {
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()
        });

        let tab = std::sync::Arc::new(Tab {
            id,
            command: command.clone(),
            created_at: now_unix(),
            pid,
            workspace,
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            state: Mutex::new(TabState {
                parser: vt100::Parser::new(rows, cols, scrollback_lines),
                title: program,
                bell: false,
                exit: None,
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
            exit: state.exit,
            workspace: self.workspace.clone(),
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn write_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.lock().unwrap().write_all(bytes)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        if let Some(master) = self.master.lock().unwrap().as_deref() {
            master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        }
        self.state.lock().unwrap().parser.set_size(rows, cols);
        Ok(())
    }

    /// Drops the PTY master. On Windows, ConPTY's output pipe otherwise stays
    /// open after the child exits, so the reader thread's read would hang.
    fn close_master(&self) {
        self.master.lock().unwrap().take();
    }

    pub fn rename(&self, title: String) {
        let mut state = self.state.lock().unwrap();
        state.title = title;
    }

    /// Signals the child process to terminate. Does not remove the tab: the
    /// reader thread will observe the process exiting and mark it as such.
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

    /// Tells every current subscriber that this tab was dismissed/removed
    /// entirely (as opposed to just having exited). Used by the manager when
    /// a client kills an already-dead tab.
    pub fn broadcast_closed(&self) {
        self.broadcast(|| Response::TabClosed(self.id));
    }

    /// Records the process's exit and tells subscribers, without removing
    /// the tab. Used when exited tabs are left around for the user to
    /// dismiss (see `Config::auto_close_exited_tabs`).
    pub fn mark_exited(&self, exit: ExitInfo) {
        self.state.lock().unwrap().exit = Some(exit);
        let info = self.info();
        self.broadcast(|| Response::TabUpdated(info.clone()));
    }

    fn broadcast(&self, event_for: impl Fn() -> Response) {
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|_, tx| tx.send(event_for()).is_ok());
    }
}

fn exit_info(status: &ExitStatus) -> ExitInfo {
    ExitInfo { code: status.exit_code(), success: status.success(), at: now_unix() }
}

/// Starts the reader thread (pumps PTY output) and the waiter thread
/// ([`Child::wait`] is the reliable cross-platform exit signal; it closes the
/// master to unstick the reader, then hands the tab and its `ExitInfo` to
/// `on_exit` — e.g. to mark it exited in place or remove it). See [`Tab::spawn`].
pub fn spawn_reader(
    tab: std::sync::Arc<Tab>,
    mut reader: Box<dyn std::io::Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    on_exit: impl FnOnce(std::sync::Arc<Tab>, ExitInfo) + Send + 'static,
) {
    let reader_tab = tab.clone();
    let reader_thread = std::thread::spawn(move || {
        let tab = reader_tab;
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
    });

    std::thread::spawn(move || {
        let status = child.wait().ok();
        tab.alive.store(false, Ordering::SeqCst);
        tab.close_master();
        let _ = reader_thread.join();
        let exit = status
            .as_ref()
            .map(exit_info)
            .unwrap_or(ExitInfo { code: 0, success: true, at: now_unix() });
        on_exit(tab, exit);
    });
}
