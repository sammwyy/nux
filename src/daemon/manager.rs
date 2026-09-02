//! Owns the set of live tabs and hands out ids.

use super::tab::Tab;
use crate::protocol::{Response, TabInfo};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct TabManager {
    inner: Arc<Inner>,
}

/// Result of [`TabManager::kill`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    NotFound,
    /// The process was signaled; the tab remains, to be marked exited once
    /// it actually dies.
    Killed,
    /// The tab was already exited and has now been dismissed/removed.
    Removed { daemon_now_empty: bool },
}

struct Inner {
    tabs: Mutex<HashMap<u32, Arc<Tab>>>,
    next_id: AtomicU32,
    scrollback_lines: usize,
}

/// Default program used when a caller doesn't specify one: `$SHELL` on Unix,
/// `%COMSPEC%` (falling back to `cmd.exe`) on Windows.
pub fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
}

impl TabManager {
    pub fn new(scrollback_lines: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                tabs: Mutex::new(HashMap::new()),
                next_id: AtomicU32::new(0),
                scrollback_lines,
            }),
        }
    }

    pub fn create(
        &self,
        command: Vec<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<TabInfo> {
        let command = if command.is_empty() {
            vec![default_shell()]
        } else {
            command
        };
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tab, reader, child) =
            Tab::spawn(id, command, cwd, cols, rows, self.inner.scrollback_lines)?;
        let info = tab.info();
        // Register before starting the reader thread: a process that exits
        // within microseconds would otherwise be marked exited on a tab that
        // was never actually visible to anyone.
        self.inner.tabs.lock().unwrap().insert(id, tab.clone());
        super::tab::spawn_reader(tab, reader, child);
        Ok(info)
    }

    pub fn get(&self, id: u32) -> Option<Arc<Tab>> {
        self.inner.tabs.lock().unwrap().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<TabInfo> {
        let tabs = self.inner.tabs.lock().unwrap();
        let mut infos: Vec<TabInfo> = tabs.values().map(|t| t.info()).collect();
        infos.sort_by_key(|t| t.id);
        infos
    }

    /// If `id`'s process is still running, signals it to terminate (the tab
    /// stays around, to be marked exited asynchronously once the process
    /// actually dies). If `id` is already marked exited, dismisses/removes
    /// it instead — telling the caller whether that was the last tab, so the
    /// daemon can shut itself down.
    pub fn kill(&self, id: u32) -> KillOutcome {
        let mut tabs = self.inner.tabs.lock().unwrap();
        match tabs.get(&id) {
            None => KillOutcome::NotFound,
            Some(tab) if tab.is_alive() => {
                tab.kill();
                KillOutcome::Killed
            }
            Some(_) => {
                if let Some(tab) = tabs.remove(&id) {
                    tab.broadcast_closed();
                }
                KillOutcome::Removed { daemon_now_empty: tabs.is_empty() }
            }
        }
    }

    pub fn rename(&self, id: u32, title: String) -> Option<TabInfo> {
        let tabs = self.inner.tabs.lock().unwrap();
        let tab = tabs.get(&id)?;
        tab.rename(title);
        Some(tab.info())
    }

    pub fn kill_all(&self) {
        let tabs = self.inner.tabs.lock().unwrap();
        for tab in tabs.values() {
            tab.kill();
        }
    }

    pub fn subscribe(&self, id: u32, conn_id: u64, tx: Sender<Response>) -> bool {
        match self.get(id) {
            Some(tab) => {
                tab.subscribe(conn_id, tx);
                true
            }
            None => false,
        }
    }

    pub fn unsubscribe(&self, id: u32, conn_id: u64) {
        if let Some(tab) = self.get(id) {
            tab.unsubscribe(conn_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exited_tab_stays_until_dismissed() {
        let manager = TabManager::new(1000);
        // `true` is a real binary on Unix and exits immediately; good enough to
        // exercise the manager without depending on a shell being interactive.
        let info = manager.create(vec!["true".into()], None, 80, 24).unwrap();
        assert_eq!(info.id, 0);

        // The tab is marked exited, but stays listed — not silently removed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let tabs = manager.list();
            assert_eq!(tabs.len(), 1, "tab should still be listed after its process exits");
            if !tabs[0].is_alive() {
                assert!(tabs[0].exit.unwrap().success, "`true` should exit successfully");
                break;
            }
            assert!(std::time::Instant::now() < deadline, "tab never got marked exited");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Killing an already-dead tab dismisses it, and reports the manager is
        // now empty so the daemon knows it can shut down.
        match manager.kill(info.id) {
            KillOutcome::Removed { daemon_now_empty } => assert!(daemon_now_empty),
            other => panic!("expected Removed, got {other:?}"),
        }
        assert!(manager.list().is_empty());
    }

    #[test]
    fn killing_a_running_tab_does_not_remove_it() {
        let manager = TabManager::new(1000);
        let info = manager.create(vec!["sleep".into(), "30".into()], None, 80, 24).unwrap();
        assert_eq!(manager.kill(info.id), KillOutcome::Killed);
        assert_eq!(manager.list().len(), 1, "tab should still be listed right after signaling it");
    }

    #[test]
    fn kill_missing_tab_reports_not_found() {
        let manager = TabManager::new(1000);
        assert_eq!(manager.kill(42), KillOutcome::NotFound);
    }

    #[test]
    fn ids_increase_monotonically() {
        let manager = TabManager::new(1000);
        let a = manager.create(vec!["true".into()], None, 80, 24).unwrap();
        let b = manager.create(vec!["true".into()], None, 80, 24).unwrap();
        assert!(b.id > a.id);
    }
}
