//! Client-side connection handling: locating/starting the daemon and issuing
//! one-shot requests against it.

mod input;
pub mod tui;
mod ui;

use crate::protocol::{read_message, write_message, Request, Response};
use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::Stream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Connects to an already-running daemon, if any.
pub fn connect() -> std::io::Result<Stream> {
    let name = crate::ipc::socket_name()?;
    Stream::connect(name)
}

pub fn is_running() -> bool {
    connect().is_ok()
}

/// Connects to the daemon, spawning it as a detached background process first if
/// it isn't already running.
pub fn ensure_daemon() -> anyhow::Result<Stream> {
    if let Ok(s) = connect() {
        return Ok(s);
    }
    spawn_daemon_process()?;
    wait_for_daemon()
}

fn wait_for_daemon() -> anyhow::Result<Stream> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(s) = connect() {
            return Ok(s);
        }
        if Instant::now() > deadline {
            anyhow::bail!(
                "nux daemon did not start in time; check {}",
                crate::ipc::log_file().display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn spawn_daemon_process() -> anyhow::Result<()> {
    crate::ipc::ensure_runtime_dir()?;
    let exe = std::env::current_exe()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(crate::ipc::log_file())?;
    Command::new(exe)
        .arg("__daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    Ok(())
}

#[cfg(windows)]
fn spawn_daemon_process() -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: run with no console, independent
    // of the parent's process group so it survives the launching shell exiting.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    crate::ipc::ensure_runtime_dir()?;
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .arg("__daemon")
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

/// Sends a single request and reads back a single response. Only valid for
/// requests that produce exactly one response (i.e. not `Attach`/`CreateTab`
/// followed by a streaming session).
pub fn request_once(stream: &mut Stream, req: &Request) -> std::io::Result<Response> {
    write_message(stream, req)?;
    read_message(stream)
}

/// Kills every tab and terminates the daemon process, if one is running.
pub fn kill_server() -> anyhow::Result<bool> {
    let mut stream = match connect() {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };
    let _ = request_once(&mut stream, &Request::Shutdown);
    Ok(true)
}
