//! Filesystem/namespace locations used to find the daemon: the local socket name,
//! the runtime directory, the pid file and the log file.

use interprocess::local_socket::{GenericFilePath, GenericNamespaced, Name, NameType, ToFsName, ToNsName};
use std::path::PathBuf;

fn user_tag() -> String {
    for var in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    "default".to_string()
}

/// Directory used for the pid file, log file and (on platforms without namespaced
/// sockets) the socket file itself.
pub fn runtime_dir() -> PathBuf {
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("nux")
}

pub fn ensure_runtime_dir() -> std::io::Result<PathBuf> {
    let dir = runtime_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn pid_file() -> PathBuf {
    runtime_dir().join("nux.pid")
}

pub fn log_file() -> PathBuf {
    runtime_dir().join("nux.log")
}

/// Resolves the local socket name nux's daemon listens on and clients connect to.
///
/// Prefers the namespaced socket type (works uniformly on Linux's abstract namespace
/// and Windows named pipes); falls back to a filesystem path (needed on macOS/BSD).
pub fn socket_name() -> std::io::Result<Name<'static>> {
    let id = format!("nux-{}.sock", user_tag());
    if GenericNamespaced::is_supported() {
        id.to_ns_name::<GenericNamespaced>()
    } else {
        let path = runtime_dir().join(id);
        path.to_fs_name::<GenericFilePath>()
    }
}
