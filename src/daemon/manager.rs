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
        // Register the tab before the reader thread can possibly observe the
        // child exiting and try to remove it again — otherwise a process that
        // exits within microseconds could have `remove` run before `insert`,
        // leaving a dead entry that nothing ever cleans up.
        self.inner.tabs.lock().unwrap().insert(id, tab.clone());
        let manager = self.clone();
        super::tab::spawn_reader(tab, reader, child, move |exited_id| manager.remove(exited_id));
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

    pub fn kill(&self, id: u32) -> bool {
        if let Some(tab) = self.inner.tabs.lock().unwrap().get(&id) {
            tab.kill();
            true
        } else {
            false
        }
    }

    pub fn rename(&self, id: u32, title: String) -> Option<TabInfo> {
        let tabs = self.inner.tabs.lock().unwrap();
        let tab = tabs.get(&id)?;
        tab.rename(title);
        Some(tab.info())
    }

    pub fn remove(&self, id: u32) {
        self.inner.tabs.lock().unwrap().remove(&id);
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
    fn create_list_and_kill_a_tab() {
        let manager = TabManager::new(1000);
        // `true` is a real binary on Unix and exits immediately; good enough to
        // exercise the manager without depending on a shell being interactive.
        let info = manager.create(vec!["true".into()], None, 80, 24).unwrap();
        assert_eq!(info.id, 0);

        // Give the reader thread a moment to observe process exit and self-remove.
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(manager.list().iter().all(|t| t.id != info.id), "exited tab should be cleaned up");
    }

    #[test]
    fn ids_increase_monotonically() {
        let manager = TabManager::new(1000);
        let a = manager.create(vec!["true".into()], None, 80, 24).unwrap();
        let b = manager.create(vec!["true".into()], None, 80, 24).unwrap();
        assert!(b.id > a.id);
    }
}
