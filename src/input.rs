use rustix::termios::{self, OptionalActions, Termios};
/// Terminal input parser: raw mode, terminal size, and byte→event decoding.
///
/// Replaces termion. Uses rustix::termios for raw mode and terminal size.
/// Parses VT100/xterm key sequences, mouse events, bracketed paste, and
/// focus events from raw stdin bytes.
use std::io;

// ── Key type ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Null,
    Up,
    Down,
    Left,
    Right,
    ShiftUp,
    ShiftDown,
    ShiftLeft,
    ShiftRight,
    CtrlUp,
    CtrlDown,
    CtrlLeft,
    CtrlRight,
    CtrlShiftUp,
    CtrlShiftDown,
    CtrlShiftLeft,
    CtrlShiftRight,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    Backspace,
    BackTab,
    Esc,
    F(u8),
}

// ── Mouse types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Debug, Clone, Copy)]
pub enum MouseEvent {
    Press(MouseButton, u16, u16),
    Release(u16, u16),
    Hold(u16, u16),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MouseMods {
    pub ctrl: bool,
}

// ── Editor event ──────────────────────────────────────────────────────────

pub enum EditorEvent {
    Key(Key),
    Mouse(MouseEvent, MouseMods),
    Paste(String),
    FocusIn,
}

// ── Raw mode & terminal size ──────────────────────────────────────────────

/// Enable raw mode on stdout. Returns the old termios for later restoration.
pub fn enable_raw_mode() -> io::Result<Termios> {
    let stdout = io::stdout();
    let mut termios = termios::tcgetattr(&stdout)?;
    let old = termios.clone();
    termios.make_raw();
    termios::tcsetattr(&stdout, OptionalActions::Now, &termios)?;
    Ok(old)
}

/// Restore saved terminal settings.
pub fn disable_raw_mode(old: &Termios) -> io::Result<()> {
    let stdout = io::stdout();
    termios::tcsetattr(&stdout, OptionalActions::Now, old)?;
    Ok(())
}

/// Get terminal dimensions.
pub fn terminal_size() -> io::Result<(u16, u16)> {
    let stdout = io::stdout();
    let ws = termios::tcgetwinsize(&stdout)?;
    Ok((ws.ws_col, ws.ws_row))
}

// ── Input parser ──────────────────────────────────────────────────────────

const MAX_ESC_BUF: usize = 32;

/// Paste end marker: `\x1b[201~`
const PASTE_END_MARKER: &[u8] = b"\x1b[201~";

/// State machine for decoding raw terminal bytes into editor events.
///
/// Feed bytes one at a time via `advance()`. The parser buffers escape
/// sequences internally and produces events when a complete sequence
/// is recognized.
///
/// For bare Escape (0x1B not followed by a CSI/SS3 sequence), the caller
/// should use a short read timeout and call `flush()` to emit `Key::Esc`.
pub struct InputParser {
    /// Escape-sequence buffer (filled after seeing 0x1B).
    buf: [u8; MAX_ESC_BUF],
    buf_len: u8,

    /// True when inside a bracketed paste.
    in_paste: bool,
    /// Accumulated paste bytes. Decode only after the paste terminator so
    /// multibyte UTF-8 characters are not treated as separate characters.
    paste_buf: Vec<u8>,
    /// True when the previous paste byte was CR (for CRLF → LF).
    paste_saw_cr: bool,
    /// Number of consecutive bytes matched against PASTE_END_MARKER.
    paste_term_match: u8,
}

impl Default for InputParser {
    fn default() -> Self {
        Self::new()
    }
}

impl InputParser {
    pub fn new() -> Self {
        Self {
            buf: [0; MAX_ESC_BUF],
            buf_len: 0,
            in_paste: false,
            paste_buf: Vec::new(),
            paste_saw_cr: false,
            paste_term_match: 0,
        }
    }

    /// True if the parser has a pending bare ESC. Call `flush()` to emit it.
    pub fn needs_flush(&self) -> bool {
        self.buf_len == 1 && self.buf[0] == 0x1B
    }

    /// Emit a pending bare Escape key. Returns None if nothing is pending.
    pub fn flush(&mut self) -> Option<EditorEvent> {
        if self.buf_len == 1 && self.buf[0] == 0x1B {
            self.buf_len = 0;
            return Some(EditorEvent::Key(Key::Esc));
        }
        None
    }

    /// Feed a single byte into the parser. Returns `Some(event)` when a
    /// complete event is decoded, or `None` when more bytes are needed.
    pub fn advance(&mut self, byte: u8) -> Option<EditorEvent> {
        // ── Bracketed paste accumulation ──────────────────────────────
        if self.in_paste {
            return self.advance_paste(byte);
        }

        // ── Escape-sequence buffering ─────────────────────────────────
        if self.buf_len > 0 {
            return self.advance_esc(byte);
        }

        // ── ESC → start escape sequence ───────────────────────────────
        if byte == 0x1B {
            self.buf[0] = 0x1B;
            self.buf_len = 1;
            return None;
        }

        // ── Ground-state byte ─────────────────────────────────────────
        Some(Self::ground_key(byte))
    }

    /// Map a ground-state byte to a key event.
    fn ground_key(byte: u8) -> EditorEvent {
        match byte {
            0x00 => EditorEvent::Key(Key::Null),
            0x01..=0x07 | 0x0B | 0x0C | 0x0E..=0x1A => {
                EditorEvent::Key(Key::Ctrl((byte + 0x60) as char))
            }
            0x08 => EditorEvent::Key(Key::Ctrl('h')),
            0x09 => EditorEvent::Key(Key::Char('\t')),
            0x0A => EditorEvent::Key(Key::Null),       // Ctrl+J
            0x0D => EditorEvent::Key(Key::Char('\n')), // Enter
            0x1C..=0x1F => EditorEvent::Key(Key::Ctrl((byte + 0x40) as char)),
            0x20..=0x7E => EditorEvent::Key(Key::Char(byte as char)),
            0x7F => EditorEvent::Key(Key::Backspace),
            _ => EditorEvent::Key(Key::Null), // ignored
        }
    }

    // ── Paste handling ────────────────────────────────────────────────

    /// Process a byte while inside a bracketed paste.
    fn advance_paste(&mut self, byte: u8) -> Option<EditorEvent> {
        let expected = PASTE_END_MARKER[self.paste_term_match as usize];

        if byte == expected {
            self.paste_term_match += 1;
            if self.paste_term_match as usize == PASTE_END_MARKER.len() {
                self.in_paste = false;
                self.paste_term_match = 0;
                let bytes = std::mem::take(&mut self.paste_buf);
                let text = String::from_utf8_lossy(&bytes).into_owned();
                self.paste_saw_cr = false;
                return Some(EditorEvent::Paste(text));
            }
            return None;
        }

        // Mismatch: flush any accumulated terminator bytes as content
        if self.paste_term_match > 0 {
            let end = self.paste_term_match as usize;
            for &byte in &PASTE_END_MARKER[..end] {
                self.emit_paste_byte(byte);
            }
            self.paste_term_match = 0;
        }

        self.emit_paste_byte(byte);
        None
    }

    fn emit_paste_byte(&mut self, byte: u8) {
        match byte {
            0x0D => {
                self.paste_buf.push(b'\n');
                self.paste_saw_cr = true;
            }
            0x0A => {
                if !self.paste_saw_cr {
                    self.paste_buf.push(b'\n');
                }
                self.paste_saw_cr = false;
            }
            0x7F => {
                self.paste_saw_cr = false;
                self.paste_buf.push(0x7f);
            }
            b => {
                self.paste_saw_cr = false;
                self.paste_buf.push(b);
            }
        }
    }

    // ── Escape sequence parsing ───────────────────────────────────────

    fn advance_esc(&mut self, byte: u8) -> Option<EditorEvent> {
        if self.buf_len as usize >= MAX_ESC_BUF {
            // Buffer overflow — drop and reset
            self.buf_len = 0;
            return None;
        }

        self.buf[self.buf_len as usize] = byte;
        self.buf_len += 1;

        let seq = &self.buf[..self.buf_len as usize];

        // ── SS3 (ESC O) sequences ───────────────────────────────────
        if seq.len() == 3 && seq[1] == b'O' {
            self.buf_len = 0;
            return Some(Self::ss3_key(seq[2]));
        }

        // ── CSI (ESC [) sequences ───────────────────────────────────
        if seq.len() >= 3 && seq[1] == b'[' {
            // X10 mouse: \x1b[M Cb Cx Cy (6 bytes). M is NOT treated as
            // a CSI final byte here; we need 3 more data bytes.
            if seq.len() == 3 && seq[2] == b'M' {
                // Still need 3 more bytes
                return None;
            }
            // X10 continuation: we're past byte 3 in an ESC[M sequence.
            if seq[2] == b'M' && seq.len() < 6 {
                return None;
            }
            // Normal CSI: final byte in 0x40-0x7E
            let last = seq[seq.len() - 1];
            let is_x10_complete = seq.len() == 6 && seq[2] == b'M';
            if is_x10_complete || (0x40..=0x7E).contains(&last) {
                // Copy the full sequence so we can clear the buffer before
                // dispatching (which needs &mut self for paste state).
                let seq_len = self.buf_len as usize;
                let mut seq_copy = [0u8; MAX_ESC_BUF];
                seq_copy[..seq_len].copy_from_slice(&self.buf[..seq_len]);
                self.buf_len = 0;
                return self.finish_csi(&seq_copy[..seq_len]);
            }
            return None;
        }

        // ── Non-CSI/SS3 second byte: drop the sequence and reset ────
        // (e.g. ESC followed by some non-sequence byte — rare).
        // The caller must use flush() for bare ESC detection.
        if seq.len() == 2 && seq[1] != b'[' && seq[1] != b'O' {
            // Second byte is 0x1B → double ESC. Emit Esc and restart.
            if seq[1] == 0x1B {
                self.buf_len = 0;
                // Recurse to start a new escape sequence
                return Some(EditorEvent::Key(Key::Esc));
            }
            // Unknown two-byte escape — discard
            self.buf_len = 0;
            return None;
        }

        // Still buffering
        None
    }

    /// Static SS3 key dispatch.
    fn ss3_key(byte: u8) -> EditorEvent {
        EditorEvent::Key(match byte {
            b'P' => Key::F(1),
            b'Q' => Key::F(2),
            b'R' => Key::F(3),
            b'S' => Key::F(4),
            b'd' => Key::CtrlLeft,
            b'c' => Key::CtrlRight,
            _ => return EditorEvent::Key(Key::Null),
        })
    }

    /// Handle a complete CSI sequence. `seq` is the full raw bytes including ESC [.
    fn finish_csi(&mut self, seq: &[u8]) -> Option<EditorEvent> {
        let body = &seq[2..seq.len() - 1];
        let final_byte = seq[seq.len() - 1];

        // ── Bracketed paste markers ──────────────────────────────────
        if final_byte == b'~' && !body.is_empty() && body[0] != b'<' {
            let params = Self::parse_csi_params(body);
            match params.first().copied().unwrap_or(0) {
                200 => {
                    self.in_paste = true;
                    self.paste_buf.clear();
                    self.paste_saw_cr = false;
                    self.paste_term_match = 0;
                    return None;
                }
                201 => return None,
                _ => {}
            }
        }

        // ── Mouse: SGR (\x1b[<...M/m), X10 (\x1b[M...), rxvt (\x1b[...M) ──
        if !body.is_empty() && body[0] == b'<' {
            return Self::parse_sgr_mouse(seq);
        }
        if seq.len() == 6 && body.len() >= 3 && body[0] == b'M' {
            return Self::parse_x10_mouse(seq);
        }
        if final_byte == b'M' && !body.is_empty() && body[0] != b'<' {
            return Self::parse_rxvt_mouse(seq);
        }

        // ── Focus in: \x1b[I ────────────────────────────────────────
        if final_byte == b'I' && body.is_empty() {
            return Some(EditorEvent::FocusIn);
        }

        // ── No-param CSI ────────────────────────────────────────────
        if body.is_empty() {
            return Some(EditorEvent::Key(match final_byte {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                b'Z' => Key::BackTab,
                _ => return None,
            }));
        }

        // ── Param CSI ───────────────────────────────────────────────
        let params = Self::parse_csi_params(body);
        let p0 = params.first().copied().unwrap_or(0);
        let p1 = params.get(1).copied().unwrap_or(0);

        Some(EditorEvent::Key(match final_byte {
            b'~' => match p0 {
                2 => Key::Insert,
                3 if p1 == 5 => Key::Ctrl('h'),
                3 => Key::Delete,
                5 => Key::PageUp,
                6 => Key::PageDown,
                15 => Key::F(5),
                17 => Key::F(6),
                18 => Key::F(7),
                19 => Key::F(8),
                20 => Key::F(9),
                21 => Key::F(10),
                23 => Key::F(11),
                24 => Key::F(12),
                _ => return None,
            },
            b'A'..=b'D' | b'H' | b'F' => {
                let base = match final_byte {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'C' => Key::Right,
                    b'D' => Key::Left,
                    b'H' => Key::Home,
                    b'F' => Key::End,
                    _ => unreachable!(),
                };
                match p1 {
                    2 => match base {
                        Key::Up => Key::ShiftUp,
                        Key::Down => Key::ShiftDown,
                        Key::Left => Key::ShiftLeft,
                        Key::Right => Key::ShiftRight,
                        _ => base,
                    },
                    5 => match base {
                        Key::Up => Key::CtrlUp,
                        Key::Down => Key::CtrlDown,
                        Key::Left => Key::CtrlLeft,
                        Key::Right => Key::CtrlRight,
                        _ => base,
                    },
                    6 => match base {
                        Key::Up => Key::CtrlShiftUp,
                        Key::Down => Key::CtrlShiftDown,
                        Key::Left => Key::CtrlShiftLeft,
                        Key::Right => Key::CtrlShiftRight,
                        _ => base,
                    },
                    _ => base,
                }
            }
            b'u' if p0 == 127 && p1 == 5 => Key::Ctrl('h'),
            _ => return None,
        }))
    }

    /// Parse semicolon/colon-separated numeric params from CSI body.
    fn parse_csi_params(body: &[u8]) -> [u16; 8] {
        let mut params = [0u16; 8];
        let mut count = 0;
        let mut val: u16 = 0;
        let mut has_digit = false;

        for &b in body {
            match b {
                b'0'..=b'9' => {
                    val = val.saturating_mul(10).saturating_add((b - b'0') as u16);
                    has_digit = true;
                }
                b';' | b':' => {
                    if count < 8 {
                        params[count] = val;
                        count += 1;
                    }
                    val = 0;
                    has_digit = false;
                }
                _ => {}
            }
        }
        if has_digit && count < 8 {
            params[count] = val;
        }
        params
    }

    // ── Mouse parsers ─────────────────────────────────────────────────

    fn parse_sgr_mouse(seq: &[u8]) -> Option<EditorEvent> {
        if seq.len() < 9 {
            return None;
        }
        let final_byte = seq[seq.len() - 1];
        if final_byte != b'M' && final_byte != b'm' {
            return None;
        }
        let body = std::str::from_utf8(&seq[3..seq.len() - 1]).ok()?;
        let mut parts = body.split(';');
        let cb: u16 = parts.next()?.parse().ok()?;
        let cx: u16 = parts.next()?.parse().ok()?;
        let cy: u16 = parts.next()?.parse().ok()?;
        Self::mouse_from_cb(cb, cx, cy, final_byte == b'm')
    }

    fn parse_x10_mouse(seq: &[u8]) -> Option<EditorEvent> {
        if seq.len() != 6 {
            return None;
        }
        let cb = seq[3].checked_sub(32)? as u16;
        let cx = seq[4].checked_sub(32)? as u16;
        let cy = seq[5].checked_sub(32)? as u16;
        Self::mouse_from_cb(cb, cx, cy, false)
    }

    fn parse_rxvt_mouse(seq: &[u8]) -> Option<EditorEvent> {
        if seq.len() < 7 {
            return None;
        }
        let body = std::str::from_utf8(&seq[2..seq.len() - 1]).ok()?;
        let mut parts = body.split(';');
        let cb: u16 = parts.next()?.parse().ok()?;
        let cx: u16 = parts.next()?.parse().ok()?;
        let cy: u16 = parts.next()?.parse().ok()?;
        if parts.next().is_some() || cb < 32 {
            return None;
        }
        Self::mouse_from_cb(cb - 32, cx, cy, false)
    }

    fn mouse_from_cb(cb: u16, cx: u16, cy: u16, sgr_release: bool) -> Option<EditorEvent> {
        let mods = MouseMods {
            ctrl: cb & 0x10 != 0,
        };
        let base = cb & 0b11;
        let wheel = cb & 0x40 != 0;
        let hold = cb & 0x20 != 0 && !wheel;

        let event = if hold {
            MouseEvent::Hold(cx, cy)
        } else if sgr_release || (base == 3 && !wheel) {
            MouseEvent::Release(cx, cy)
        } else {
            let button = if wheel {
                match base {
                    0 => MouseButton::WheelUp,
                    1 => MouseButton::WheelDown,
                    2 => MouseButton::WheelLeft,
                    3 => MouseButton::WheelRight,
                    _ => return None,
                }
            } else {
                match base {
                    0 => MouseButton::Left,
                    1 => MouseButton::Middle,
                    2 => MouseButton::Right,
                    _ => return None,
                }
            };
            MouseEvent::Press(button, cx, cy)
        };
        Some(EditorEvent::Mouse(event, mods))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed bytes through the parser and collect all emitted events.
    fn parse_bytes(bytes: &[u8]) -> Vec<EditorEvent> {
        let mut parser = InputParser::new();
        let mut events = Vec::new();
        for &b in bytes {
            if let Some(ev) = parser.advance(b) {
                events.push(ev);
            }
        }
        if let Some(ev) = parser.flush() {
            events.push(ev);
        }
        events
    }

    // ── Single byte keys ──────────────────────────────────────────────

    #[test]
    fn test_printable() {
        let events = parse_bytes(b"abc");
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], EditorEvent::Key(Key::Char('a'))));
        assert!(matches!(events[1], EditorEvent::Key(Key::Char('b'))));
        assert!(matches!(events[2], EditorEvent::Key(Key::Char('c'))));
    }

    #[test]
    fn test_enter() {
        let events = parse_bytes(&[0x0D]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::Char('\n'))));
    }

    #[test]
    fn test_ctrl_j_is_null() {
        let events = parse_bytes(&[0x0A]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::Null)));
    }

    #[test]
    fn test_tab() {
        let events = parse_bytes(&[0x09]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::Char('\t'))));
    }

    #[test]
    fn test_backspace() {
        let events = parse_bytes(&[0x7F]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::Backspace)));
    }

    #[test]
    fn test_ctrl_keys() {
        let events = parse_bytes(&[0x01, 0x13, 0x1A, 0x08]);
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], EditorEvent::Key(Key::Ctrl('a'))));
        assert!(matches!(events[1], EditorEvent::Key(Key::Ctrl('s'))));
        assert!(matches!(events[2], EditorEvent::Key(Key::Ctrl('z'))));
        assert!(matches!(events[3], EditorEvent::Key(Key::Ctrl('h'))));
    }

    #[test]
    fn test_null_bytes() {
        let events = parse_bytes(&[0x00]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::Null)));
    }

    // ── Escape handling ───────────────────────────────────────────────

    #[test]
    fn test_bare_esc_via_flush() {
        let mut parser = InputParser::new();
        assert!(parser.advance(0x1B).is_none());
        assert!(parser.needs_flush());
        let ev = parser.flush();
        assert!(matches!(ev, Some(EditorEvent::Key(Key::Esc))));
        assert!(!parser.needs_flush());
    }

    #[test]
    fn test_double_esc() {
        let events = parse_bytes(&[0x1B, 0x1B]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::Esc)));
        // Second ESC was re-buffered — flush would give another
    }

    // ── Arrow keys ────────────────────────────────────────────────────

    #[test]
    fn test_arrows() {
        let events = parse_bytes(b"\x1b[A\x1b[B\x1b[C\x1b[D");
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], EditorEvent::Key(Key::Up)));
        assert!(matches!(events[1], EditorEvent::Key(Key::Down)));
        assert!(matches!(events[2], EditorEvent::Key(Key::Right)));
        assert!(matches!(events[3], EditorEvent::Key(Key::Left)));
    }

    #[test]
    fn test_shift_arrows() {
        let events = parse_bytes(b"\x1b[1;2A\x1b[1;2B\x1b[1;2C\x1b[1;2D");
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], EditorEvent::Key(Key::ShiftUp)));
        assert!(matches!(events[1], EditorEvent::Key(Key::ShiftDown)));
        assert!(matches!(events[2], EditorEvent::Key(Key::ShiftRight)));
        assert!(matches!(events[3], EditorEvent::Key(Key::ShiftLeft)));
    }

    #[test]
    fn test_ctrl_arrows() {
        let events = parse_bytes(b"\x1b[1;5A\x1b[1;5B\x1b[1;5C\x1b[1;5D");
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], EditorEvent::Key(Key::CtrlUp)));
        assert!(matches!(events[1], EditorEvent::Key(Key::CtrlDown)));
        assert!(matches!(events[2], EditorEvent::Key(Key::CtrlRight)));
        assert!(matches!(events[3], EditorEvent::Key(Key::CtrlLeft)));
    }

    #[test]
    fn test_ctrl_shift_arrows() {
        let events = parse_bytes(b"\x1b[1;6A\x1b[1;6B\x1b[1;6C\x1b[1;6D");
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], EditorEvent::Key(Key::CtrlShiftUp)));
        assert!(matches!(events[1], EditorEvent::Key(Key::CtrlShiftDown)));
        assert!(matches!(events[2], EditorEvent::Key(Key::CtrlShiftRight)));
        assert!(matches!(events[3], EditorEvent::Key(Key::CtrlShiftLeft)));
    }

    #[test]
    fn test_rxvt_ctrl_arrows() {
        let events = parse_bytes(b"\x1bOd\x1bOc");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], EditorEvent::Key(Key::CtrlLeft)));
        assert!(matches!(events[1], EditorEvent::Key(Key::CtrlRight)));
    }

    // ── Navigation keys ───────────────────────────────────────────────

    #[test]
    fn test_home_end() {
        let events = parse_bytes(b"\x1b[H\x1b[F");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], EditorEvent::Key(Key::Home)));
        assert!(matches!(events[1], EditorEvent::Key(Key::End)));
    }

    #[test]
    fn test_page_up_down() {
        let events = parse_bytes(b"\x1b[5~\x1b[6~");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], EditorEvent::Key(Key::PageUp)));
        assert!(matches!(events[1], EditorEvent::Key(Key::PageDown)));
    }

    #[test]
    fn test_delete() {
        let events = parse_bytes(b"\x1b[3~");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::Delete)));
    }

    #[test]
    fn test_backtab() {
        let events = parse_bytes(b"\x1b[Z");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::BackTab)));
    }

    // ── Function keys ─────────────────────────────────────────────────

    #[test]
    fn test_f1_f4() {
        let events = parse_bytes(b"\x1bOP\x1bOQ\x1bOR\x1bOS");
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], EditorEvent::Key(Key::F(1))));
        assert!(matches!(events[1], EditorEvent::Key(Key::F(2))));
        assert!(matches!(events[2], EditorEvent::Key(Key::F(3))));
        assert!(matches!(events[3], EditorEvent::Key(Key::F(4))));
    }

    // ── CSI u ─────────────────────────────────────────────────────────

    #[test]
    fn test_ctrl_backspace_csi_u() {
        let events = parse_bytes(b"\x1b[127;5u");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::Key(Key::Ctrl('h'))));
    }

    // ── Focus in ──────────────────────────────────────────────────────

    #[test]
    fn test_focus_in() {
        let events = parse_bytes(b"\x1b[I");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EditorEvent::FocusIn));
    }

    // ── Mouse ─────────────────────────────────────────────────────────

    #[test]
    fn test_sgr_mouse_press() {
        let events = parse_bytes(b"\x1b[<0;10;20M");
        assert_eq!(events.len(), 1);
        match &events[0] {
            EditorEvent::Mouse(MouseEvent::Press(MouseButton::Left, x, y), _mods) => {
                assert_eq!(*x, 10);
                assert_eq!(*y, 20);
            }
            _ => panic!("expected mouse press"),
        }
    }

    #[test]
    fn test_sgr_mouse_wheel() {
        let events = parse_bytes(b"\x1b[<64;10;20M");
        assert_eq!(events.len(), 1);
        match &events[0] {
            EditorEvent::Mouse(MouseEvent::Press(MouseButton::WheelUp, x, y), _mods) => {
                assert_eq!(*x, 10);
                assert_eq!(*y, 20);
            }
            _ => panic!("expected wheel"),
        }
    }

    #[test]
    fn test_x10_mouse() {
        let events = parse_bytes(&[0x1B, b'[', b'M', 32, 42, 52]);
        assert_eq!(events.len(), 1);
        match &events[0] {
            EditorEvent::Mouse(MouseEvent::Press(MouseButton::Left, x, y), _mods) => {
                assert_eq!(*x, 10);
                assert_eq!(*y, 20);
            }
            _ => panic!("expected mouse press"),
        }
    }

    // ── Bracketed paste ───────────────────────────────────────────────

    #[test]
    fn test_paste() {
        let mut events = Vec::new();
        let mut parser = InputParser::new();
        let input = b"\x1b[200~hello world\x1b[201~";
        for &b in input {
            if let Some(ev) = parser.advance(b) {
                events.push(ev);
            }
        }
        assert_eq!(events.len(), 1);
        match &events[0] {
            EditorEvent::Paste(text) => assert_eq!(text, "hello world"),
            _ => panic!("expected Paste"),
        }
    }

    #[test]
    fn test_paste_with_crlf() {
        let mut events = Vec::new();
        let mut parser = InputParser::new();
        let input = b"\x1b[200~line1\r\nline2\x1b[201~";
        for &b in input {
            if let Some(ev) = parser.advance(b) {
                events.push(ev);
            }
        }
        assert_eq!(events.len(), 1);
        match &events[0] {
            EditorEvent::Paste(text) => assert_eq!(text, "line1\nline2"),
            _ => panic!("expected Paste"),
        }
    }

    #[test]
    fn test_paste_decodes_utf8_as_text() {
        let mut events = Vec::new();
        let mut parser = InputParser::new();
        let text = "— “ café 日本";
        let mut input = b"\x1b[200~".to_vec();
        input.extend_from_slice(text.as_bytes());
        input.extend_from_slice(b"\x1b[201~");

        for byte in input {
            if let Some(event) = parser.advance(byte) {
                events.push(event);
            }
        }

        assert_eq!(events.len(), 1);
        match &events[0] {
            EditorEvent::Paste(pasted) => assert_eq!(pasted, text),
            _ => panic!("expected Paste"),
        }
    }

    // ── Terminal size ─────────────────────────────────────────────────

    #[test]
    fn test_terminal_size_doesnt_panic() {
        let _ = terminal_size();
    }
}
