//! Translates crossterm key events back into the byte sequences a terminal
//! application expects to read from its PTY.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encodes `key` as raw terminal input bytes. Returns an empty vector for events
/// that don't correspond to any byte sequence (e.g. a bare modifier key).
pub fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let base: Vec<u8> = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                ctrl_byte(c).map(|b| vec![b]).unwrap_or_else(|| encode_char(c))
            } else {
                encode_char(c)
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => f_key_bytes(n),
        _ => return Vec::new(),
    };

    if alt && !base.is_empty() {
        let mut out = vec![0x1b];
        out.extend(base);
        out
    } else {
        base
    }
}

fn encode_char(c: char) -> Vec<u8> {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

/// Maps Ctrl+<letter/symbol> to its C0 control byte, matching typical terminal
/// behavior (`Ctrl+a` -> 0x01 .. `Ctrl+z` -> 0x1a, plus a handful of punctuation).
fn ctrl_byte(c: char) -> Option<u8> {
    let lower = c.to_ascii_lowercase();
    match lower {
        'a'..='z' => Some(lower as u8 - b'a' + 1),
        '@' | ' ' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '?' => Some(0x1f),
        _ => None,
    }
}

fn f_key_bytes(n: u8) -> Vec<u8> {
    match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers).into_normal()
    }

    trait IntoNormal {
        fn into_normal(self) -> Self;
    }
    impl IntoNormal for KeyEvent {
        fn into_normal(mut self) -> Self {
            self.kind = KeyEventKind::Press;
            self
        }
    }

    #[test]
    fn plain_char_passes_through_as_utf8() {
        assert_eq!(key_to_bytes(key(KeyCode::Char('a'), KeyModifiers::NONE)), b"a");
        assert_eq!(key_to_bytes(key(KeyCode::Char('é'), KeyModifiers::NONE)), "é".as_bytes());
    }

    #[test]
    fn ctrl_letters_map_to_control_codes() {
        assert_eq!(key_to_bytes(key(KeyCode::Char('c'), KeyModifiers::CONTROL)), vec![0x03]);
        assert_eq!(key_to_bytes(key(KeyCode::Char('a'), KeyModifiers::CONTROL)), vec![0x01]);
    }

    #[test]
    fn arrows_and_special_keys_map_to_csi_sequences() {
        assert_eq!(key_to_bytes(key(KeyCode::Up, KeyModifiers::NONE)), b"\x1b[A");
        assert_eq!(key_to_bytes(key(KeyCode::Enter, KeyModifiers::NONE)), b"\r");
        assert_eq!(key_to_bytes(key(KeyCode::Backspace, KeyModifiers::NONE)), vec![0x7f]);
    }

    #[test]
    fn alt_prefixes_esc() {
        assert_eq!(key_to_bytes(key(KeyCode::Char('b'), KeyModifiers::ALT)), vec![0x1b, b'b']);
    }

    #[test]
    fn function_keys_encode() {
        assert_eq!(key_to_bytes(key(KeyCode::F(1), KeyModifiers::NONE)), b"\x1bOP");
        assert_eq!(key_to_bytes(key(KeyCode::F(5), KeyModifiers::NONE)), b"\x1b[15~");
    }
}
