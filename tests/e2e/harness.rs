use std::fmt::Write as FmtWrite;
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Key — escape sequences for special keys
// ---------------------------------------------------------------------------

pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    ShiftUp,
    ShiftDown,
    ShiftLeft,
    ShiftRight,
    CtrlLeft,
    CtrlRight,
    CtrlShiftUp,
    CtrlShiftDown,
    CtrlShiftLeft,
    CtrlShiftRight,
    ShiftTab,
}

impl Key {
    fn as_bytes(&self) -> &'static [u8] {
        match self {
            Self::Up => b"\x1b[A",
            Self::Down => b"\x1b[B",
            Self::Right => b"\x1b[C",
            Self::Left => b"\x1b[D",
            Self::Home => b"\x1b[H",
            Self::End => b"\x1b[F",
            Self::PageUp => b"\x1b[5~",
            Self::PageDown => b"\x1b[6~",
            Self::Delete => b"\x1b[3~",
            Self::ShiftUp => b"\x1b[1;2A",
            Self::ShiftDown => b"\x1b[1;2B",
            Self::ShiftRight => b"\x1b[1;2C",
            Self::ShiftLeft => b"\x1b[1;2D",
            Self::CtrlLeft => b"\x1b[1;5D",
            Self::CtrlRight => b"\x1b[1;5C",
            Self::CtrlShiftUp => b"\x1b[1;6A",
            Self::CtrlShiftDown => b"\x1b[1;6B",
            Self::CtrlShiftRight => b"\x1b[1;6C",
            Self::CtrlShiftLeft => b"\x1b[1;6D",
            Self::ShiftTab => b"\x1b[Z",
        }
    }
}

// ---------------------------------------------------------------------------
// TempDir — RAII temporary directory
// ---------------------------------------------------------------------------

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("e_test_{}_{}", std::process::id(), id));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Recording — asciicast v2 capture for visual test review
// ---------------------------------------------------------------------------

struct Recording {
    start: Instant,
    events: Vec<(f64, char, Vec<u8>)>, // (elapsed_secs, 'i'|'o', data)
    rows: u16,
    cols: u16,
}

impl Recording {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            start: Instant::now(),
            events: Vec::new(),
            rows,
            cols,
        }
    }

    fn push(&mut self, kind: char, data: &[u8]) {
        let t = self.start.elapsed().as_secs_f64();
        self.events.push((t, kind, data.to_vec()));
    }

    fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(path).unwrap();
        writeln!(
            f,
            r#"{{"version": 2, "width": {}, "height": {}}}"#,
            self.cols, self.rows
        )
        .unwrap();

        // Coalesce output chunks within 50ms into single events.
        let mut groups: Vec<(f64, Vec<u8>)> = Vec::new();
        for (time, kind, data) in &self.events {
            if *kind != 'o' {
                continue;
            }
            if let Some(last) = groups.last_mut()
                && time - last.0 < 0.05
            {
                last.1.extend_from_slice(data);
                continue;
            }
            groups.push((*time, data.clone()));
        }

        // Write with minimum gap of 0.4s so each step is visible.
        let min_gap = 0.4;
        let mut adj = 0.0;
        for (i, (_, data)) in groups.iter().enumerate() {
            if i > 0 {
                let real_gap = groups[i].0 - groups[i - 1].0;
                adj += real_gap.max(min_gap);
            }
            writeln!(f, r#"[{:.3}, "o", "{}"]"#, adj, json_escape_bytes(data)).unwrap();
        }
    }
}

fn json_escape_bytes(data: &[u8]) -> String {
    let s = String::from_utf8_lossy(data);
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                write!(out, "\\u{:04x}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out
}

fn recording_path() -> Option<PathBuf> {
    if std::env::var("E2E_RECORD").is_err() {
        return None;
    }
    let name = std::thread::current()
        .name()
        .unwrap_or("unknown")
        .replace("::", "__");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/recordings");
    Some(dir.join(format!("{name}.cast")))
}

// ---------------------------------------------------------------------------
// TestEditor — spawns `e` in a PTY with a vt100 virtual screen
// ---------------------------------------------------------------------------

pub struct TestEditor {
    master: std::fs::File,
    child: Child,
    parser: vt100::Parser,
    rx: mpsc::Receiver<Vec<u8>>,
    _reader: Option<thread::JoinHandle<()>>,
    pub home: TempDir,
    pub rows: u16,
    pub cols: u16,
    recording: Option<Recording>,
}

impl TestEditor {
    /// Spawn the editor with the given CLI args in an 80×24 PTY.
    pub fn new(args: &[&str]) -> Self {
        Self::with_size(args, 24, 80)
    }

    /// Spawn the editor with a custom terminal size.
    pub fn with_size(args: &[&str], rows: u16, cols: u16) -> Self {
        let home = TempDir::new();

        // --- open PTY pair ---
        let (master_fd, slave_fd) = unsafe {
            let mut m: libc::c_int = 0;
            let mut s: libc::c_int = 0;
            assert_eq!(
                libc::openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ),
                0,
                "openpty failed: {}",
                std::io::Error::last_os_error()
            );
            let ws = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            libc::ioctl(m, libc::TIOCSWINSZ as _, &ws);
            (m, s)
        };

        // Dup master for the reader thread; set CLOEXEC on both.
        let reader_fd = unsafe { libc::dup(master_fd) };
        assert!(reader_fd >= 0, "dup master failed");
        unsafe {
            libc::fcntl(master_fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(reader_fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }

        // --- spawn the editor ---
        let binary = env!("CARGO_BIN_EXE_e");
        let child = unsafe {
            Command::new(binary)
                .args(args)
                .env("TERM", "xterm-256color")
                .env("HOME", home.path())
                .env("LC_ALL", "en_US.UTF-8")
                // Use HOME as PATH so `which` can't find pbcopy/xclip/etc.
                // This forces internal-only clipboard, avoiding races between
                // parallel tests that share the system clipboard.
                .env("PATH", home.path())
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("DISPLAY")
                .stdin(Stdio::from_raw_fd(libc::dup(slave_fd)))
                .stdout(Stdio::from_raw_fd(libc::dup(slave_fd)))
                .stderr(Stdio::from_raw_fd(libc::dup(slave_fd)))
                .pre_exec(move || {
                    libc::setsid();
                    libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                    libc::close(slave_fd);
                    Ok(())
                })
                .spawn()
                .expect("Failed to spawn editor")
        };
        // Close slave in parent.
        unsafe {
            libc::close(slave_fd);
        }

        let master = unsafe { std::fs::File::from_raw_fd(master_fd) };

        // --- reader thread: PTY output → channel ---
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut r = unsafe { std::fs::File::from_raw_fd(reader_fd) };
            let mut buf = [0u8; 4096];
            loop {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let parser = vt100::Parser::new(rows, cols, 0);
        let recording = recording_path().map(|_| Recording::new(rows, cols));
        let mut ed = TestEditor {
            master,
            child,
            parser,
            rx,
            _reader: Some(reader),
            home,
            rows,
            cols,
            recording,
        };
        ed.wait_for_startup();
        ed
    }

    // --- internal helpers ---------------------------------------------------

    /// Feed output bytes to the vt100 parser (and recording if active).
    fn process_output(&mut self, data: &[u8]) {
        self.parser.process(data);
        if let Some(rec) = &mut self.recording {
            rec.push('o', data);
        }
    }

    /// Process any bytes already sitting in the channel (non-blocking).
    fn drain_available(&mut self) {
        while let Ok(data) = self.rx.try_recv() {
            self.process_output(&data);
        }
    }

    /// Block until output quiesces for `quiet`.
    fn drain_timeout(&mut self, quiet: Duration) {
        while let Ok(data) = self.rx.recv_timeout(quiet) {
            self.process_output(&data);
        }
    }

    /// Wait for the very first frame after launch.
    fn wait_for_startup(&mut self) {
        match self.rx.recv_timeout(Duration::from_secs(5)) {
            Ok(data) => self.process_output(&data),
            Err(_) => panic!("Editor produced no output during startup"),
        }
        self.drain_timeout(Duration::from_millis(30));
    }

    // --- sending input ------------------------------------------------------

    /// Wait for the editor to finish rendering after an action.
    pub fn wait(&mut self) {
        self.drain_timeout(Duration::from_millis(15));
    }

    /// Send raw bytes to the editor's stdin.
    pub fn send_raw(&mut self, bytes: &[u8]) {
        if let Some(rec) = &mut self.recording {
            rec.push('i', bytes);
        }
        self.master.write_all(bytes).unwrap();
        self.master.flush().unwrap();
    }

    /// Type printable text.
    pub fn type_text(&mut self, text: &str) {
        self.send_raw(text.as_bytes());
        self.wait();
    }

    /// Send Ctrl+<c> (e.g. ctrl('s') sends Ctrl+S).
    pub fn ctrl(&mut self, c: char) {
        self.send_raw(&[(c as u8) & 0x1f]);
        self.wait();
    }

    /// Send a special key.
    pub fn key(&mut self, k: Key) {
        self.send_raw(k.as_bytes());
        self.wait();
    }

    pub fn enter(&mut self) {
        self.send_raw(b"\r");
        self.wait();
    }

    pub fn backspace(&mut self) {
        self.send_raw(b"\x7f");
        self.wait();
    }

    pub fn tab(&mut self) {
        self.send_raw(b"\t");
        self.wait();
    }

    pub fn escape(&mut self) {
        self.send_raw(b"\x1b");
        self.wait();
    }

    /// Send a bracketed paste.
    pub fn paste(&mut self, text: &str) {
        self.send_raw(b"\x1b[200~");
        self.send_raw(text.as_bytes());
        self.send_raw(b"\x1b[201~");
        self.wait();
    }

    // --- mouse events (SGR mode, 1-indexed) ---------------------------------

    pub fn click(&mut self, row: u16, col: u16) {
        let (r, c) = (row + 1, col + 1);
        self.send_raw(format!("\x1b[<0;{c};{r}M").as_bytes());
        self.send_raw(format!("\x1b[<0;{c};{r}m").as_bytes());
        self.wait();
    }

    pub fn double_click(&mut self, row: u16, col: u16) {
        self.click(row, col);
        self.click(row, col);
    }

    pub fn triple_click(&mut self, row: u16, col: u16) {
        self.click(row, col);
        self.click(row, col);
        self.click(row, col);
    }

    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16)) {
        let (fr, fc) = (from.0 + 1, from.1 + 1);
        let (tr, tc) = (to.0 + 1, to.1 + 1);
        self.send_raw(format!("\x1b[<0;{fc};{fr}M").as_bytes());
        self.send_raw(format!("\x1b[<32;{tc};{tr}M").as_bytes());
        self.send_raw(format!("\x1b[<0;{tc};{tr}m").as_bytes());
        self.wait();
    }

    pub fn scroll_up(&mut self) {
        self.send_raw(b"\x1b[<64;1;1M");
        self.wait();
    }

    pub fn scroll_down(&mut self) {
        self.send_raw(b"\x1b[<65;1;1M");
        self.wait();
    }

    /// Send a terminal focus-in event.
    pub fn focus_in(&mut self) {
        self.send_raw(b"\x1b[I");
        self.wait();
    }

    // --- screen inspection --------------------------------------------------

    /// Get the text of a single screen row (trailing spaces trimmed).
    pub fn row(&mut self, row: u16) -> String {
        self.drain_available();
        let screen = self.parser.screen();
        let cols = screen.size().1;
        (0..cols)
            .map(|col| {
                let cell = screen.cell(row, col);
                match cell {
                    Some(c) => {
                        let s = c.contents();
                        if s.is_empty() {
                            ' '
                        } else {
                            s.chars().next().unwrap_or(' ')
                        }
                    }
                    None => ' ',
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// Get the full screen as text (one line per row, trailing spaces trimmed).
    pub fn screen_text(&mut self) -> String {
        (0..self.rows)
            .map(|r| self.row(r))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Get cursor position as (row, col), 0-indexed.
    pub fn cursor(&mut self) -> (u16, u16) {
        self.drain_available();
        self.parser.screen().cursor_position()
    }

    /// Whether the cursor is currently visible.
    pub fn cursor_visible(&mut self) -> bool {
        self.drain_available();
        !self.parser.screen().hide_cursor()
    }

    /// The status bar row (second-to-last).
    pub fn status_bar(&mut self) -> String {
        self.row(self.rows - 2)
    }

    /// The command line row (last).
    pub fn command_line(&mut self) -> String {
        self.row(self.rows - 1)
    }

    /// Foreground color of a specific cell.
    pub fn cell_fg(&mut self, row: u16, col: u16) -> vt100::Color {
        self.drain_available();
        self.parser
            .screen()
            .cell(row, col)
            .map_or(vt100::Color::Default, |c| c.fgcolor())
    }

    /// Background color of a specific cell.
    pub fn cell_bg(&mut self, row: u16, col: u16) -> vt100::Color {
        self.drain_available();
        self.parser
            .screen()
            .cell(row, col)
            .map_or(vt100::Color::Default, |c| c.bgcolor())
    }

    /// Whether a cell is rendered in reverse video.
    pub fn cell_inverse(&mut self, row: u16, col: u16) -> bool {
        self.drain_available();
        self.parser
            .screen()
            .cell(row, col)
            .is_some_and(|c| c.inverse())
    }

    // --- lifecycle ----------------------------------------------------------

    /// Quit without saving (for dirty buffers).
    pub fn quit_no_save(&mut self) {
        self.ctrl('q');
        // Answer "n" to "Save changes?" if dirty
        self.send_raw(b"n");
        self.wait();
    }

    /// Quit and save (for dirty buffers).
    pub fn quit_saving(&mut self) {
        self.ctrl('q');
        self.send_raw(b"y");
        self.wait();
    }

    /// Wait for the child process to exit and return its status.
    pub fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        self.child.wait().expect("wait failed")
    }

    /// Check if the child has already exited.
    pub fn has_exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }
}

impl Drop for TestEditor {
    fn drop(&mut self) {
        // Save recording before cleanup.
        if let Some(rec) = self.recording.take()
            && let Some(path) = recording_path()
        {
            rec.save(&path);
        }
        // Try to quit gracefully.
        let _ = self.master.write_all(&[0x11]); // Ctrl+Q
        let _ = self.master.flush();
        thread::sleep(Duration::from_millis(30));
        let _ = self.master.write_all(b"n");
        let _ = self.master.flush();
        thread::sleep(Duration::from_millis(30));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a file in the given directory and return its path.
pub fn create_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    path
}
