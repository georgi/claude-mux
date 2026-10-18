//! Minimal decoder for raw terminal input bytes. Used only for cmux's own UI
//! (prefix commands, picker, prompts). Input for a pane is forwarded verbatim.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Alt(char),
    Enter,
    /// ESC then Enter (steer in the message prompt).
    AltEnter,
    Tab,
    BackTab,
    Backspace,
    Esc,
    Up,
    Down,
    Left,
    Right,
    CtrlArrow(Dir),
    AltArrow(Dir),
    PageUp,
    PageDown,
    Home,
    End,
    Delete,
    Unknown,
}

/// Decode one key from the start of `b`. Returns the key and the number of
/// bytes consumed (always >= 1 for non-empty input).
pub fn parse_one(b: &[u8]) -> (Key, usize) {
    match b[0] {
        b'\r' | b'\n' => (Key::Enter, 1),
        b'\t' => (Key::Tab, 1),
        0x7f | 0x08 => (Key::Backspace, 1),
        0x1b => parse_escape(b),
        c @ 0x01..=0x1a => (Key::Ctrl((c - 1 + b'a') as char), 1),
        0x00 | 0x1c..=0x1f => (Key::Unknown, 1),
        _ => parse_utf8(b),
    }
}

fn parse_utf8(b: &[u8]) -> (Key, usize) {
    let len = match b[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => return (Key::Unknown, 1),
    };
    if b.len() < len {
        return (Key::Unknown, b.len());
    }
    match std::str::from_utf8(&b[..len])
        .ok()
        .and_then(|s| s.chars().next())
    {
        Some(c) => (Key::Char(c), len),
        None => (Key::Unknown, len),
    }
}

fn parse_escape(b: &[u8]) -> (Key, usize) {
    if b.len() == 1 {
        return (Key::Esc, 1);
    }
    match b[1] {
        b'[' => parse_csi(b),
        b'O' if b.len() >= 3 => {
            let k = match b[2] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                _ => Key::Unknown,
            };
            (k, 3)
        }
        // ESC ESC [ A: Alt+arrow in terminals that prefix ESC for Alt.
        0x1b => match parse_csi_at(&b[1..]) {
            Some((Key::Up, n)) => (Key::AltArrow(Dir::Up), 1 + n),
            Some((Key::Down, n)) => (Key::AltArrow(Dir::Down), 1 + n),
            Some((Key::Left, n)) => (Key::AltArrow(Dir::Left), 1 + n),
            Some((Key::Right, n)) => (Key::AltArrow(Dir::Right), 1 + n),
            _ => (Key::Esc, 1),
        },
        _ => {
            let (k, n) = parse_one(&b[1..]);
            match k {
                Key::Char(c) => (Key::Alt(c), 1 + n),
                Key::Enter => (Key::AltEnter, 1 + n),
                _ => (Key::Esc, 1),
            }
        }
    }
}

fn parse_csi_at(b: &[u8]) -> Option<(Key, usize)> {
    (b.len() >= 3 && b[0] == 0x1b && b[1] == b'[').then(|| parse_csi(b))
}

fn parse_csi(b: &[u8]) -> (Key, usize) {
    // ESC [ params... final(0x40..=0x7e)
    let mut i = 2;
    while i < b.len() && !(0x40..=0x7e).contains(&b[i]) {
        i += 1;
    }
    if i >= b.len() {
        return (Key::Unknown, b.len());
    }
    let params = &b[2..i];
    let dir = match b[i] {
        b'A' => Some(Dir::Up),
        b'B' => Some(Dir::Down),
        b'C' => Some(Dir::Right),
        b'D' => Some(Dir::Left),
        _ => None,
    };
    // xterm modifier parameter: 1;3 is Alt, 1;5 is Ctrl.
    match (dir, params) {
        (Some(d), b"1;5") => return (Key::CtrlArrow(d), i + 1),
        (Some(d), b"1;3") => return (Key::AltArrow(d), i + 1),
        _ => {}
    }
    let k = match (b[i], params) {
        (b'A', _) => Key::Up,
        (b'B', _) => Key::Down,
        (b'C', _) => Key::Right,
        (b'D', _) => Key::Left,
        (b'H', _) => Key::Home,
        (b'F', _) => Key::End,
        (b'Z', _) => Key::BackTab,
        (b'~', b"5") => Key::PageUp,
        (b'~', b"6") => Key::PageDown,
        (b'~', b"3") => Key::Delete,
        (b'~', b"1") | (b'~', b"7") => Key::Home,
        (b'~', b"4") | (b'~', b"8") => Key::End,
        // Bracketed paste markers (200~ / 201~) and anything else: ignore.
        _ => Key::Unknown,
    };
    (k, i + 1)
}

/// Decode all keys in `b`.
#[cfg(test)]
pub fn parse_all(mut b: &[u8]) -> Vec<Key> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let (k, n) = parse_one(b);
        out.push(k);
        b = &b[n..];
    }
    out
}

/// Human readable name for a prefix byte, e.g. 0x01 -> "C-a".
pub fn prefix_name(byte: u8) -> String {
    match byte {
        0x01..=0x1a => format!("C-{}", (byte - 1 + b'a') as char),
        _ => format!("0x{byte:02x}"),
    }
}

/// An SGR (1006) mouse report. `col` and `row` are 0-based screen cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mouse {
    pub button: u16,
    pub col: u16,
    pub row: u16,
    pub release: bool,
}

impl Mouse {
    pub fn is_wheel(&self) -> bool {
        self.button & 64 != 0 && self.button & 128 == 0
    }
    pub fn wheel_up(&self) -> bool {
        self.is_wheel() && self.button & 3 == 0
    }
    pub fn wheel_down(&self) -> bool {
        self.is_wheel() && self.button & 3 == 1
    }
    pub fn is_motion(&self) -> bool {
        self.button & 32 != 0
    }
    pub fn is_left_press(&self) -> bool {
        !self.release && self.button & (3 | 32 | 64 | 128) == 0
    }
}

/// Position of the first SGR mouse report (`ESC [ <`) in `b`.
pub fn find_mouse(b: &[u8]) -> Option<usize> {
    b.windows(3).position(|w| w == b"\x1b[<")
}

/// Parse an SGR mouse report at the start of `b`. Returns `None` when `b`
/// does not start with one, and `Some((None, n))` for a malformed report
/// that should be dropped.
pub fn parse_mouse(b: &[u8]) -> Option<(Option<Mouse>, usize)> {
    if !b.starts_with(b"\x1b[<") {
        return None;
    }
    let Some(end) = b[3..]
        .iter()
        .position(|&c| c == b'M' || c == b'm')
        .map(|i| i + 3)
    else {
        return Some((None, b.len()));
    };
    let body = std::str::from_utf8(&b[3..end]).ok();
    let nums: Vec<u16> = body
        .map(|s| s.split(';').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_default();
    let m = match nums[..] {
        [button, col, row] if col >= 1 && row >= 1 => Some(Mouse {
            button,
            col: col - 1,
            row: row - 1,
            release: b[end] == b'm',
        }),
        _ => None,
    };
    Some((m, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_common_keys() {
        assert_eq!(
            parse_all(b"a\x1b[A\x1bOB\r\x7f\x01\x1b[5~\xc3\xa4"),
            vec![
                Key::Char('a'),
                Key::Up,
                Key::Down,
                Key::Enter,
                Key::Backspace,
                Key::Ctrl('a'),
                Key::PageUp,
                Key::Char('ä'),
            ]
        );
        assert_eq!(parse_all(b"\x1b"), vec![Key::Esc]);
        assert_eq!(
            parse_mouse(b"\x1b[<64;10;5Mrest"),
            Some((
                Some(Mouse {
                    button: 64,
                    col: 9,
                    row: 4,
                    release: false
                }),
                11
            ))
        );
        assert_eq!(find_mouse(b"ab\x1b[<0;1;1m"), Some(2));
        assert!(parse_mouse(b"\x1b[<0;1;1m").unwrap().0.unwrap().release);
        assert_eq!(parse_all(b"\x1bx"), vec![Key::Alt('x')]);
        assert_eq!(
            parse_all(b"\x1b[1;5A\x1b[1;3D\x1b\x1b[C"),
            vec![
                Key::CtrlArrow(Dir::Up),
                Key::AltArrow(Dir::Left),
                Key::AltArrow(Dir::Right)
            ]
        );
        assert_eq!(
            parse_all(b"\x1b[200~hi\x1b[201~"),
            vec![Key::Unknown, Key::Char('h'), Key::Char('i'), Key::Unknown]
        );
    }
}
