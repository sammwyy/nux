//! Wire protocol shared between the nemux daemon and its clients.
//!
//! Every message is length-prefixed (u32 little-endian byte count) followed by a
//! bincode-encoded payload. Requests always flow client -> daemon, responses always
//! flow daemon -> client, so a single duplex stream never mixes the two message
//! shapes on one direction.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Messages larger than this are rejected instead of causing an unbounded allocation.
const MAX_MESSAGE_SIZE: u32 = 64 * 1024 * 1024;

/// Snapshot of a single tab's metadata, as seen from outside the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: u32,
    pub title: String,
    pub command: Vec<String>,
    pub pid: Option<u32>,
    pub created_at: i64,
    pub cols: u16,
    pub rows: u16,
    pub bell: bool,
}

impl TabInfo {
    /// The program name shown to selectors and the tab bar (argv[0], stripped of any path).
    pub fn program(&self) -> &str {
        self.command
            .first()
            .map(|s| s.as_str())
            .unwrap_or("?")
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("?")
    }
}

/// A request sent from a client to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// List all currently open tabs.
    ListTabs,
    /// Spawn a new tab running `command` and attach the connection to it.
    CreateTab {
        command: Vec<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    },
    /// Attach this connection's output stream to an existing tab.
    Attach { tab_id: u32, cols: u16, rows: u16 },
    /// Detach without closing the connection (used before issuing another request).
    Detach,
    /// Raw input bytes to forward to the currently attached tab's PTY.
    Input(Vec<u8>),
    /// Resize the currently attached tab's PTY.
    Resize { cols: u16, rows: u16 },
    /// Kill a tab by id.
    KillTab { tab_id: u32 },
    /// Rename a tab.
    RenameTab { tab_id: u32, title: String },
    /// Kill every tab and terminate the daemon process.
    Shutdown,
    /// Liveness check.
    Ping,
}

/// A response sent from the daemon to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Error(String),
    TabList(Vec<TabInfo>),
    Attached(TabInfo),
    /// ANSI bytes that reproduce (or update) the attached tab's screen when replayed
    /// through a `vt100::Parser` sized to the same dimensions.
    Screen { tab_id: u32, data: Vec<u8> },
    TabUpdated(TabInfo),
    TabClosed(u32),
    Pong,
}

fn to_io_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Writes one length-prefixed, bincode-encoded message.
pub fn write_message<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = bincode::serialize(msg).map_err(to_io_err)?;
    if bytes.len() as u64 > MAX_MESSAGE_SIZE as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Reads one length-prefixed, bincode-encoded message.
pub fn read_message<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_MESSAGE_SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    bincode::deserialize(&buf).map_err(to_io_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_request() {
        let req = Request::CreateTab {
            command: vec!["bash".into(), "-l".into()],
            cwd: Some("/tmp".into()),
            cols: 80,
            rows: 24,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &req).unwrap();
        let mut cursor = io::Cursor::new(buf);
        let decoded: Request = read_message(&mut cursor).unwrap();
        match decoded {
            Request::CreateTab { command, cwd, cols, rows } => {
                assert_eq!(command, vec!["bash".to_string(), "-l".to_string()]);
                assert_eq!(cwd.as_deref(), Some("/tmp"));
                assert_eq!((cols, rows), (80, 24));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn round_trips_response() {
        let resp = Response::TabList(vec![TabInfo {
            id: 3,
            title: "codex".into(),
            command: vec!["/usr/bin/codex".into()],
            pid: Some(1234),
            created_at: 42,
            cols: 120,
            rows: 40,
            bell: false,
        }]);
        let mut buf = Vec::new();
        write_message(&mut buf, &resp).unwrap();
        let mut cursor = io::Cursor::new(buf);
        let decoded: Response = read_message(&mut cursor).unwrap();
        match decoded {
            Response::TabList(tabs) => {
                assert_eq!(tabs.len(), 1);
                assert_eq!(tabs[0].program(), "codex");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_oversized_length_prefix() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_MESSAGE_SIZE + 1).to_le_bytes());
        let mut cursor = io::Cursor::new(buf);
        let err = read_message::<_, Request>(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn program_strips_path() {
        let info = TabInfo {
            id: 0,
            title: "t".into(),
            command: vec!["/usr/local/bin/codex".into(), "--flag".into()],
            pid: None,
            created_at: 0,
            cols: 80,
            rows: 24,
            bell: false,
        };
        assert_eq!(info.program(), "codex");
    }
}
