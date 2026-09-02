//! End-to-end tests: spawn a real daemon process in an isolated runtime dir /
//! socket namespace and drive it through the actual wire protocol.

use nux::client;
use nux::protocol::{Request, Response};
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

/// Spawns an isolated daemon (unique `USER`/`XDG_RUNTIME_DIR`/`XDG_CONFIG_HOME`
/// so it never collides with a real user daemon, config file, or other tests
/// running in parallel) and points this process's own env at the same
/// namespace so `nux::client` resolves the same socket.
fn spawn_isolated_daemon(tag: &str) -> DaemonProcess {
    spawn_isolated_daemon_with_config(tag, None)
}

/// Like [`spawn_isolated_daemon`], but writes `config_toml` (if given) as the
/// daemon's config file before starting it.
fn spawn_isolated_daemon_with_config(tag: &str, config_toml: Option<&str>) -> DaemonProcess {
    let dir = tempfile::tempdir().unwrap();
    let user = format!("nuxtest-{tag}-{}", std::process::id());
    let config_home = dir.path().join("config");
    let runtime_dir = dir.path().join("runtime");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    if let Some(toml) = config_toml {
        let nux_dir = config_home.join("nux");
        std::fs::create_dir_all(&nux_dir).unwrap();
        std::fs::write(nux_dir.join("config.toml"), toml).unwrap();
    }

    std::env::set_var("USER", &user);
    std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
    std::env::set_var("XDG_CONFIG_HOME", &config_home);

    let bin = env!("CARGO_BIN_EXE_nux");
    let mut child = Command::new(bin)
        .arg("__daemon")
        .env("USER", &user)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("XDG_CONFIG_HOME", &config_home)
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
fn create_list_and_kill_a_running_tab() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Keep a second tab alive so killing the first one never triggers the
    // "last tab gone" daemon shutdown path.
    let _daemon = spawn_isolated_daemon("roundtrip");

    let mut keepalive_conn = client::connect().unwrap();
    client::request_once(
        &mut keepalive_conn,
        &Request::CreateTab { command: vec!["sleep".into(), "30".into()], cwd: None, cols: 80, rows: 24 },
    )
    .unwrap();

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

    // A fresh connection should see both tabs.
    let mut list_conn = client::connect().unwrap();
    let resp = client::request_once(&mut list_conn, &Request::ListTabs).unwrap();
    match resp {
        Response::TabList(tabs) => assert_eq!(tabs.len(), 2),
        other => panic!("expected TabList, got {other:?}"),
    }

    // The process is still running, so this just signals it — by default
    // (`keep_exited_tab_open = false`) it then disappears on its own once it
    // actually dies, with no second kill needed.
    let resp = client::request_once(&mut list_conn, &Request::KillTab { tab_id }).unwrap();
    assert!(matches!(resp, Response::Ok));

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut conn = client::connect().unwrap();
        let resp = client::request_once(&mut conn, &Request::ListTabs).unwrap();
        if let Response::TabList(tabs) = resp {
            if tabs.iter().all(|t| t.id != tab_id) {
                break;
            }
        }
        assert!(Instant::now() < deadline, "killed tab was never auto-removed");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn keep_exited_tab_open_requires_a_second_kill_to_dismiss() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _daemon = spawn_isolated_daemon_with_config("keep-open", Some("keep_exited_tab_open = true\n"));

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

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut conn = client::connect().unwrap();
        let resp = client::request_once(&mut conn, &Request::ListTabs).unwrap();
        if let Response::TabList(tabs) = resp {
            assert_eq!(tabs.len(), 1, "exited tab should stay listed until dismissed");
            if !tabs[0].is_alive() {
                break;
            }
        }
        assert!(Instant::now() < deadline, "tab was not marked exited");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Dismisses/removes the now-exited tab for good.
    let mut kill_conn = client::connect().unwrap();
    let resp = client::request_once(&mut kill_conn, &Request::KillTab { tab_id }).unwrap();
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
    // instead of racing those pushes (this mirrors how the real `nux rename`
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
    let mut daemon = spawn_isolated_daemon_with_config("empty-shutdown", Some("keep_exited_tab_open = true\n"));

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

#[test]
fn daemon_exits_once_last_tab_auto_closes_by_default() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut daemon = spawn_isolated_daemon("empty-shutdown-default");

    let mut create_conn = client::connect().unwrap();
    client::request_once(
        &mut create_conn,
        &Request::CreateTab { command: vec!["true".into()], cwd: None, cols: 80, rows: 24 },
    )
    .unwrap();

    // No kill needed: the only tab auto-closes as soon as `true` exits, and
    // the daemon shuts itself down right behind it.
    let status = daemon.child.wait().unwrap();
    assert!(status.success());
    assert!(!client::is_running());
}
