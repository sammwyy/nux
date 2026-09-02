//! End-to-end tests: spawn a real daemon process in an isolated runtime dir /
//! socket namespace and drive it through the actual wire protocol.

use nemux::client;
use nemux::protocol::{Request, Response};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `spawn_isolated_daemon` mutates process-wide env vars (`USER`,
/// `XDG_RUNTIME_DIR`) so this process's own socket resolution matches the
/// child daemon's. `cargo test` runs tests in the same process on multiple
/// threads by default, so every test in this file must hold this lock for
/// its whole body to avoid one test's env vars leaking into another's.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct DaemonProcess {
    child: Child,
    _dir: tempfile::TempDir,
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawns an isolated daemon (unique `USER`/`XDG_RUNTIME_DIR` so it never
/// collides with a real user daemon or with other tests running in parallel)
/// and points this process's own env at the same namespace so `nemux::client`
/// resolves the same socket.
fn spawn_isolated_daemon(tag: &str) -> DaemonProcess {
    let dir = tempfile::tempdir().unwrap();
    let user = format!("nemuxtest-{tag}-{}", std::process::id());

    std::env::set_var("USER", &user);
    std::env::set_var("XDG_RUNTIME_DIR", dir.path());

    let bin = env!("CARGO_BIN_EXE_nemux");
    let mut child = Command::new(bin)
        .arg("__daemon")
        .env("USER", &user)
        .env("XDG_RUNTIME_DIR", dir.path())
        .spawn()
        .expect("failed to spawn daemon");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if client::connect().is_ok() {
            return DaemonProcess { child, _dir: dir };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("daemon did not start listening in time");
}

#[test]
fn create_list_kill_roundtrip() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _daemon = spawn_isolated_daemon("roundtrip");

    let mut create_conn = client::connect().unwrap();
    let resp = client::request_once(
        &mut create_conn,
        &Request::CreateTab {
            command: vec!["sh".into(), "-c".into(), "sleep 30".into()],
            cwd: None,
            cols: 80,
            rows: 24,
        },
    )
    .unwrap();
    let tab_id = match resp {
        Response::Attached(info) => {
            assert_eq!(info.cols, 80);
            assert_eq!(info.rows, 24);
            info.id
        }
        other => panic!("expected Attached, got {other:?}"),
    };

    // A fresh connection should see the tab created by the first one.
    let mut list_conn = client::connect().unwrap();
    let resp = client::request_once(&mut list_conn, &Request::ListTabs).unwrap();
    match resp {
        Response::TabList(tabs) => {
            assert_eq!(tabs.len(), 1);
            assert_eq!(tabs[0].id, tab_id);
        }
        other => panic!("expected TabList, got {other:?}"),
    }

    // First kill: the process is still running, so this just signals it.
    // The tab is *not* removed — it stays listed until marked exited and
    // then explicitly dismissed.
    let resp = client::request_once(&mut list_conn, &Request::KillTab { tab_id }).unwrap();
    assert!(matches!(resp, Response::Ok));

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut conn = client::connect().unwrap();
        let resp = client::request_once(&mut conn, &Request::ListTabs).unwrap();
        if let Response::TabList(tabs) = resp {
            assert_eq!(tabs.len(), 1, "killed tab should stay listed until dismissed");
            if !tabs[0].is_alive() {
                break;
            }
        }
        assert!(Instant::now() < deadline, "tab was not marked exited after being killed");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Second kill, on the now-exited tab: dismisses/removes it for good. It
    // was the only tab, so the daemon also shuts itself down right after —
    // see `daemon_exits_once_last_tab_is_dismissed` for that behavior in
    // isolation; here we just confirm the connection survives long enough to
    // see the dismissal response before the daemon goes away.
    let resp = client::request_once(&mut list_conn, &Request::KillTab { tab_id }).unwrap();
    assert!(matches!(resp, Response::TabClosed(id) if id == tab_id));
}

#[test]
fn rename_and_ping() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _daemon = spawn_isolated_daemon("rename");

    let mut conn = client::connect().unwrap();
    let resp = client::request_once(&mut conn, &Request::Ping).unwrap();
    assert!(matches!(resp, Response::Pong));

    let resp = client::request_once(
        &mut conn,
        &Request::CreateTab { command: vec!["sh".into()], cwd: None, cols: 80, rows: 24 },
    )
    .unwrap();
    let tab_id = match resp {
        Response::Attached(info) => info.id,
        other => panic!("expected Attached, got {other:?}"),
    };

    // `conn` is now attached to the tab and will start receiving async `Screen`
    // pushes, so issue the next request/response pair on a fresh connection
    // instead of racing those pushes (this mirrors how the real `nemux rename`
    // CLI command behaves: it never attaches).
    let mut rename_conn = client::connect().unwrap();
    let resp = client::request_once(&mut rename_conn, &Request::RenameTab { tab_id, title: "hello".into() })
        .unwrap();
    match resp {
        Response::TabUpdated(info) => assert_eq!(info.title, "hello"),
        other => panic!("expected TabUpdated, got {other:?}"),
    }
}

#[test]
fn kill_tab_that_does_not_exist_reports_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _daemon = spawn_isolated_daemon("missing-tab");
    let mut conn = client::connect().unwrap();
    let resp = client::request_once(&mut conn, &Request::KillTab { tab_id: 999 }).unwrap();
    assert!(matches!(resp, Response::Error(_)));
}

#[test]
fn shutdown_stops_the_daemon() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut daemon = spawn_isolated_daemon("shutdown");
    assert!(client::is_running());

    let mut conn = client::connect().unwrap();
    let resp = client::request_once(&mut conn, &Request::Shutdown).unwrap();
    assert!(matches!(resp, Response::Ok));

    let status = daemon.child.wait().unwrap();
    assert!(status.success());
    assert!(!client::is_running());
}

#[test]
fn daemon_exits_once_last_tab_is_dismissed() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut daemon = spawn_isolated_daemon("empty-shutdown");

    // `create_conn` ends up attached (subscribed) to the tab it creates, so
    // it must not be reused for plain request/response calls afterward (it
    // may start receiving async `Screen`/`TabUpdated` pushes) — use a fresh
    // connection for everything else, like the CLI's one-shot commands do.
    let mut create_conn = client::connect().unwrap();
    let resp = client::request_once(
        &mut create_conn,
        &Request::CreateTab { command: vec!["true".into()], cwd: None, cols: 80, rows: 24 },
    )
    .unwrap();
    let tab_id = match resp {
        Response::Attached(info) => info.id,
        other => panic!("expected Attached, got {other:?}"),
    };

    // Wait for `true` to exit and get marked as such.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut poll_conn = client::connect().unwrap();
        let resp = client::request_once(&mut poll_conn, &Request::ListTabs).unwrap();
        if let Response::TabList(tabs) = resp {
            if tabs.iter().any(|t| t.id == tab_id && !t.is_alive()) {
                break;
            }
        }
        assert!(Instant::now() < deadline, "tab never got marked exited");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Dismissing the only (now-dead) tab should make the daemon shut itself
    // down, the same way a tmux server exits when its last session is killed.
    let mut kill_conn = client::connect().unwrap();
    let resp = client::request_once(&mut kill_conn, &Request::KillTab { tab_id }).unwrap();
    assert!(matches!(resp, Response::TabClosed(id) if id == tab_id));

    let status = daemon.child.wait().unwrap();
    assert!(status.success());
    assert!(!client::is_running());
}
