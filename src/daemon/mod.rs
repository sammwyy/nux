//! The nux daemon: accepts local socket connections and dispatches requests
//! against a shared [`manager::TabManager`].

pub mod manager;
pub mod tab;

use crate::config::Config;
use crate::protocol::{read_message, write_message, Request, Response};
use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{ListenerOptions, Stream};
use manager::{exit_when_empty, KillOutcome, TabManager};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

/// Runs the daemon's accept loop. Never returns under normal operation; a client
/// sending [`Request::Shutdown`] terminates the process directly.
pub fn run(config: Config) -> anyhow::Result<()> {
    #[cfg(unix)]
    unsafe {
        // Detach from the controlling terminal so the daemon survives the parent
        // shell/session exiting. Failure (e.g. already a session leader) is harmless.
        libc::setsid();
    }

    crate::ipc::ensure_runtime_dir()?;
    write_pid_file()?;

    let name = crate::ipc::socket_name()?;
    let listener = match ListenerOptions::new().name(name.clone()).create_sync() {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // A socket file may be left over from an unclean shutdown on platforms
            // that use filesystem paths. Retry once after clearing it.
            remove_stale_socket_file();
            ListenerOptions::new().name(name).create_sync()?
        }
        Err(e) => return Err(e.into()),
    };

    log::info!("nux daemon listening (pid {})", std::process::id());

    let manager = TabManager::new(config.scrollback_lines, config.keep_exited_tab_open);
    let conn_counter = AtomicU64::new(0);

    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(e) => {
                log::warn!("failed to accept connection: {e}");
                continue;
            }
        };
        let manager = manager.clone();
        let conn_id = conn_counter.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || handle_connection(stream, manager, conn_id));
    }

    Ok(())
}

fn remove_stale_socket_file() {
    let path = crate::ipc::runtime_dir().join(format!(
        "nux-{}.sock",
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "default".into())
    ));
    let _ = std::fs::remove_file(path);
}

fn write_pid_file() -> std::io::Result<()> {
    std::fs::write(crate::ipc::pid_file(), std::process::id().to_string())
}

/// A zero-sized PTY/`vt100` grid panics on the first cell access, so never
/// let a client-supplied size (e.g. from a terminal that hasn't reported its
/// real size yet) through as 0 in either dimension.
fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(1), rows.max(1))
}


fn handle_connection(stream: Stream, manager: TabManager, conn_id: u64) {
    let (mut recv, mut send) = stream.split();
    let (tx, rx) = mpsc::channel::<Response>();

    let writer = std::thread::spawn(move || {
        while let Ok(resp) = rx.recv() {
            if write_message(&mut send, &resp).is_err() {
                break;
            }
        }
    });

    let mut current_tab: Option<u32> = None;

    while let Ok(req) = read_message::<_, Request>(&mut recv) {
        match req {
            Request::ListTabs => {
                let _ = tx.send(Response::TabList(manager.list()));
            }
            Request::CreateTab { command, cwd, cols, rows } => {
                let (cols, rows) = clamp_size(cols, rows);
                match manager.create(command, cwd, cols, rows) {
                    Ok(info) => {
                        // Order matters: the client only starts applying `Screen`
                        // updates once it has seen `Attached` for this tab id, so
                        // that must go out first, before `attach()` triggers the
                        // subscriber's initial full-screen dump.
                        let _ = tx.send(Response::Attached(info.clone()));
                        attach(&manager, &mut current_tab, conn_id, info.id, &tx);
                    }
                    Err(e) => {
                        let _ = tx.send(Response::Error(e.to_string()));
                    }
                }
            }
            Request::Attach { tab_id, cols, rows } => match manager.get(tab_id) {
                Some(tab) => {
                    let (cols, rows) = clamp_size(cols, rows);
                    let _ = tab.resize(cols, rows);
                    let _ = tx.send(Response::Attached(tab.info()));
                    attach(&manager, &mut current_tab, conn_id, tab_id, &tx);
                }
                None => {
                    let _ = tx.send(Response::Error(format!("no such tab: {tab_id}")));
                }
            },
            Request::Detach => {
                detach(&manager, &mut current_tab, conn_id);
                let _ = tx.send(Response::Ok);
            }
            Request::Input(bytes) => {
                if let Some(tab) = current_tab.and_then(|id| manager.get(id)) {
                    let _ = tab.write_input(&bytes);
                }
            }
            Request::Resize { cols, rows } => {
                let (cols, rows) = clamp_size(cols, rows);
                if let Some(tab) = current_tab.and_then(|id| manager.get(id)) {
                    let _ = tab.resize(cols, rows);
                }
            }
            Request::KillTab { tab_id } => match manager.kill(tab_id) {
                KillOutcome::NotFound => {
                    let _ = tx.send(Response::Error(format!("no such tab: {tab_id}")));
                }
                KillOutcome::Killed => {
                    let _ = tx.send(Response::Ok);
                }
                KillOutcome::Removed { daemon_now_empty } => {
                    let _ = tx.send(Response::TabClosed(tab_id));
                    if daemon_now_empty {
                        exit_when_empty();
                    }
                }
            },
            Request::RenameTab { tab_id, title } => match manager.rename(tab_id, title) {
                Some(info) => {
                    let _ = tx.send(Response::TabUpdated(info));
                }
                None => {
                    let _ = tx.send(Response::Error(format!("no such tab: {tab_id}")));
                }
            },
            Request::Shutdown => {
                manager.kill_all();
                let _ = tx.send(Response::Ok);
                std::thread::sleep(std::time::Duration::from_millis(150));
                std::process::exit(0);
            }
            Request::Ping => {
                let _ = tx.send(Response::Pong);
            }
        }
    }

    detach(&manager, &mut current_tab, conn_id);
    drop(tx);
    let _ = writer.join();
}

fn attach(
    manager: &TabManager,
    current_tab: &mut Option<u32>,
    conn_id: u64,
    tab_id: u32,
    tx: &mpsc::Sender<Response>,
) {
    detach(manager, current_tab, conn_id);
    manager.subscribe(tab_id, conn_id, tx.clone());
    *current_tab = Some(tab_id);
}

fn detach(manager: &TabManager, current_tab: &mut Option<u32>, conn_id: u64) {
    if let Some(id) = current_tab.take() {
        manager.unsubscribe(id, conn_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_size_rejects_zero_dimensions() {
        assert_eq!(clamp_size(0, 0), (1, 1));
        assert_eq!(clamp_size(0, 24), (1, 24));
        assert_eq!(clamp_size(80, 0), (80, 1));
        assert_eq!(clamp_size(80, 24), (80, 24));
    }
}
