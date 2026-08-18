use std::fmt::Write as FmtWrite;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

type CInt = i32;

#[repr(C)]
struct WinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[cfg_attr(target_os = "linux", link(name = "util"))]
unsafe extern "C" {
    fn openpty(
        master: *mut CInt,
        slave: *mut CInt,
        name: *mut u8,
        termp: *const u8,
        winp: *const WinSize,
    ) -> CInt;
    fn ioctl(fd: CInt, request: usize, ...) -> CInt;
    fn dup(fd: CInt) -> CInt;
    fn fcntl(fd: CInt, command: CInt, argument: CInt) -> CInt;
    fn poll(fds: *mut PollFd, count: usize, timeout_ms: CInt) -> CInt;
    fn setsid() -> CInt;
    fn close(fd: CInt) -> CInt;
    fn kill(pid: CInt, signal: CInt) -> CInt;
}

#[repr(C)]
struct PollFd {
    fd: CInt,
    events: i16,
    revents: i16,
}

#[cfg(target_os = "linux")]
const TIOCSWINSZ: usize = 0x5414;
#[cfg(target_os = "macos")]
const TIOCSWINSZ: usize = 0x8008_7467;
#[cfg(target_os = "linux")]
const TIOCSCTTY: usize = 0x540e;
#[cfg(target_os = "macos")]
const TIOCSCTTY: usize = 0x2000_7461;
const F_SETFD: CInt = 2;
const FD_CLOEXEC: CInt = 1;
const F_GETFD: CInt = 1;
const F_GETFL: CInt = 3;
const F_SETFL: CInt = 4;
const O_NONBLOCK: CInt = 0x4;
const POLLIN: i16 = 0x001;
const POLLOUT: i16 = 0x004;
const POLLERR: i16 = 0x008;
const POLLHUP: i16 = 0x010;
const POLLNVAL: i16 = 0x020;
const SIGTERM: CInt = 15;
const SIGKILL: CInt = 9;

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
        loop {
            let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("e_test_{}_{}", std::process::id(), id));
            match std::fs::create_dir(&path) {
                Ok(()) => return TempDir(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("could not create temporary e2e directory: {error}"),
            }
        }
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
    reader_stop: Arc<AtomicBool>,
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
        Self::spawn(args, rows, cols, None)
    }

    /// Spawn the explicitly selected instrumented release binary and direct
    /// its LLVM profile output to the directory owned by the profile driver.
    /// Keeping this opt-in prevents ordinary end-to-end tests from collecting
    /// profiles or silently exercising a fallback binary.
    pub fn with_profile_size(args: &[&str], rows: u16, cols: u16) -> Self {
        let binary = std::env::var_os("E_PGO_BINARY")
            .map(PathBuf::from)
            .expect("E_PGO_BINARY must select the instrumented application binary");
        assert!(
            binary.is_file(),
            "E_PGO_BINARY is not an executable file: {}",
            binary.display()
        );
        let profile_dir = std::env::var_os("E_PGO_PROFILE_DIR")
            .map(PathBuf::from)
            .expect("E_PGO_PROFILE_DIR must select the profile output directory");
        assert!(
            profile_dir.is_dir(),
            "E_PGO_PROFILE_DIR is not a directory: {}",
            profile_dir.display()
        );
        Self::spawn(args, rows, cols, Some(profile_dir))
    }

    fn spawn(args: &[&str], rows: u16, cols: u16, profile_dir: Option<PathBuf>) -> Self {
        let home = TempDir::new();

        // Open a PTY pair.
        let (master_fd, slave_fd) = unsafe {
            let mut m: CInt = 0;
            let mut s: CInt = 0;
            assert_eq!(
                openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                ),
                0,
                "openpty failed: {}",
                std::io::Error::last_os_error()
            );
            let ws = WinSize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            assert_eq!(ioctl(m, TIOCSWINSZ, &ws), 0, "setting PTY size failed");
            (m, s)
        };

        // Dup master for the reader thread; keep every PTY descriptor out of
        // unrelated child processes and use bounded, nonblocking I/O below.
        let reader_fd = unsafe { dup(master_fd) };
        assert!(reader_fd >= 0, "dup master failed");
        for fd in [master_fd, reader_fd, slave_fd] {
            let flags = unsafe { fcntl(fd, F_GETFD, 0) };
            assert!(flags >= 0, "fcntl(F_GETFD) failed");
            let result = unsafe { fcntl(fd, F_SETFD, flags | FD_CLOEXEC) };
            assert_eq!(result, 0, "fcntl(FD_CLOEXEC) failed");
        }
        let flags = unsafe { fcntl(master_fd, F_GETFL, 0) };
        assert!(flags >= 0, "fcntl(F_GETFL) failed");
        let result = unsafe { fcntl(master_fd, F_SETFL, flags | O_NONBLOCK) };
        assert_eq!(result, 0, "fcntl(O_NONBLOCK) failed");

        // Spawn the editor.
        let binary = if let Some(path) = std::env::var_os("E_PGO_BINARY") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("E_TEST_BINARY") {
            PathBuf::from(path)
        } else {
            let mut path = std::env::current_exe().expect("test executable path unavailable");
            path.pop();
            path.pop();
            path.push("e");
            path
        };
        let mut command = Command::new(&binary);
        command
            .args(args)
            .env("TERM", "xterm-256color")
            .env("HOME", home.path())
            .env("LC_ALL", "en_US.UTF-8")
            .current_dir(home.path())
            // Use HOME as PATH so `which` can't find pbcopy/xclip/etc.
            // This forces internal-only clipboard, avoiding races between
            // parallel tests that share the system clipboard.
            .env("PATH", home.path())
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY");
        if let Some(profile_dir) = profile_dir {
            command.env("LLVM_PROFILE_FILE", profile_dir.join("e-%p.profraw"));
        }
        let stdin_fd = unsafe { dup(slave_fd) };
        let stdout_fd = unsafe { dup(slave_fd) };
        let stderr_fd = unsafe { dup(slave_fd) };
        assert!(stdin_fd >= 0, "dup slave for stdin failed");
        assert!(stdout_fd >= 0, "dup slave for stdout failed");
        assert!(stderr_fd >= 0, "dup slave for stderr failed");
        let child = unsafe {
            command
                .stdin(Stdio::from_raw_fd(stdin_fd))
                .stdout(Stdio::from_raw_fd(stdout_fd))
                .stderr(Stdio::from_raw_fd(stderr_fd))
                .pre_exec(move || {
                    if setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if ioctl(0, TIOCSCTTY, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if close(slave_fd) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                })
                .spawn()
                .expect("Failed to spawn editor")
        };
        // Close slave in parent.
        unsafe {
            close(slave_fd);
        }

        let master = unsafe { std::fs::File::from_raw_fd(master_fd) };

        // --- reader thread: PTY output → channel ---
        let (tx, rx) = mpsc::channel();
        let reader_stop = Arc::new(AtomicBool::new(false));
        let reader_stop_for_thread = Arc::clone(&reader_stop);
        let reader = thread::spawn(move || {
            let mut r = unsafe { std::fs::File::from_raw_fd(reader_fd) };
            let mut buf = [0u8; 4096];
            loop {
                if reader_stop_for_thread.load(Ordering::Acquire) {
                    break;
                }

                let mut poll_fd = PollFd {
                    fd: r.as_raw_fd(),
                    events: POLLIN,
                    revents: 0,
                };
                let ready = unsafe { poll(&mut poll_fd, 1, 50) };
                if ready < 0 {
                    if std::io::Error::last_os_error().kind()
                        == std::io::ErrorKind::Interrupted
                    {
                        continue;
                    }
                    break;
                }
                if ready == 0 {
                    continue;
                }

                if poll_fd.revents & POLLIN != 0 {
                    loop {
                        match r.read(&mut buf) {
                            Ok(0) => return,
                            Ok(n) => {
                                if tx.send(buf[..n].to_vec()).is_err() {
                                    return;
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => return,
                        }
                    }
                }

                if poll_fd.revents & (POLLHUP | POLLERR | POLLNVAL) != 0 {
                    break;
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
            reader_stop,
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
        // A quiet period is useful for coalescing redraw chunks, but it must
        // not turn into an unbounded wait if the child keeps producing data.
        // Give the first frame a little more time under a loaded test runner;
        // otherwise a quiet PTY before the reader thread's first wakeup can
        // make an input action appear to have been ignored.
        let deadline = Instant::now() + Duration::from_secs(1);
        let first_output_deadline = Instant::now() + Duration::from_millis(100);
        let mut saw_output = false;
        loop {
            let end = if saw_output {
                deadline
            } else {
                first_output_deadline.min(deadline)
            };
            let remaining = end.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let wait = quiet.min(remaining);
            match self.rx.recv_timeout(wait) {
                Ok(data) => {
                    saw_output = true;
                    self.process_output(&data);
                }
                Err(mpsc::RecvTimeoutError::Timeout) if !saw_output => continue,
                Err(
                    mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected,
                ) => break,
            }
        }
    }

    /// Wait for non-empty status-bar content, which is the editor's semantic
    /// readiness signal after the initial frame has rendered. A filename may
    /// truncate the language and version markers at narrow terminal widths.
    fn wait_for_startup(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.startup_ready() {
                self.drain_available();
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("Editor did not become ready during startup");
            }
            match self.rx.recv_timeout(remaining) {
                Ok(data) => self.process_output(&data),
                Err(_) => panic!("Editor did not become ready during startup"),
            }
        }
    }

    fn startup_ready(&self) -> bool {
        let row = self.parser.screen().size().0.saturating_sub(2);
        let text = (0..self.parser.screen().size().1)
            .filter_map(|column| self.parser.screen().cell(row, column))
            .map(|cell| cell.contents().chars().next().unwrap_or(' '))
            .collect::<String>();
        !text.trim().is_empty()
    }

    // --- sending input ------------------------------------------------------

    /// Wait for the editor to finish rendering after an action.
    pub fn wait(&mut self) {
        self.drain_timeout(Duration::from_millis(15));
    }

    /// Wait until a semantic editor state is visible, with a bounded timeout.
    ///
    /// PTY output is chunked independently of editor frames. Callers should
    /// use this for state transitions instead of sleeping for a guessed
    /// redraw duration.
    pub fn wait_until<F>(&mut self, timeout: Duration, mut ready: F)
    where
        F: FnMut(&mut Self) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            self.drain_available();
            if ready(self) {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("editor did not reach the expected state within {timeout:?}");
            }
            self.drain_timeout(Duration::from_millis(5).min(remaining));
        }
    }

    fn write_all_timeout(&mut self, bytes: &[u8], timeout: Duration) -> std::io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut written = 0;
        while written < bytes.len() {
            match self.master.write(&bytes[written..]) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "PTY write made no progress",
                    ));
                }
                Ok(n) => written += n,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "PTY write timed out",
                        ));
                    }
                    let timeout_ms = remaining.as_millis().clamp(1, 100) as CInt;
                    let mut poll_fd = PollFd {
                        fd: self.master.as_raw_fd(),
                        events: POLLOUT,
                        revents: 0,
                    };
                    let ready = unsafe { poll(&mut poll_fd, 1, timeout_ms) };
                    if ready < 0 {
                        let poll_error = std::io::Error::last_os_error();
                        if poll_error.kind() == std::io::ErrorKind::Interrupted {
                            continue;
                        }
                        return Err(poll_error);
                    }
                    if poll_fd.revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "PTY master closed",
                        ));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Send raw bytes to the editor's stdin.
    pub fn send_raw(&mut self, bytes: &[u8]) {
        if let Some(rec) = &mut self.recording {
            rec.push('i', bytes);
        }
        self.write_all_timeout(bytes, Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("PTY write failed: {error}"));
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

    pub fn click(&mut self, row: u16, column: u16) {
        let (r, c) = (row + 1, column + 1);
        self.send_raw(format!("\x1b[<0;{c};{r}M").as_bytes());
        self.send_raw(format!("\x1b[<0;{c};{r}m").as_bytes());
        self.wait();
    }

    pub fn double_click(&mut self, row: u16, column: u16) {
        self.click(row, column);
        self.click(row, column);
    }

    pub fn triple_click(&mut self, row: u16, column: u16) {
        self.click(row, column);
        self.click(row, column);
        self.click(row, column);
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
            .map(|column| {
                let cell = screen.cell(row, column);
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

    /// Get cursor position as (row, column), 0-indexed.
    pub fn cursor(&mut self) -> (u16, u16) {
        self.drain_available();
        self.parser.screen().cursor_position()
    }

    /// Whether the software cursor is visible (drawn as bold+reverse at cursor pos).
    pub fn cursor_visible(&mut self) -> bool {
        self.drain_available();
        let (row, column) = self.parser.screen().cursor_position();
        let cell = self.parser.screen().cell(row, column);
        cell.is_some_and(|c| c.bold() && c.inverse())
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
    pub fn cell_fg(&mut self, row: u16, column: u16) -> vt100::Color {
        self.drain_available();
        self.parser
            .screen()
            .cell(row, column)
            .map_or(vt100::Color::Default, |c| c.fgcolor())
    }

    /// Background color of a specific cell.
    pub fn cell_bg(&mut self, row: u16, column: u16) -> vt100::Color {
        self.drain_available();
        self.parser
            .screen()
            .cell(row, column)
            .map_or(vt100::Color::Default, |c| c.bgcolor())
    }

    /// Whether a cell is rendered in reverse video.
    pub fn cell_inverse(&mut self, row: u16, column: u16) -> bool {
        self.drain_available();
        self.parser
            .screen()
            .cell(row, column)
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

        // Try to quit gracefully, but never block teardown on a PTY that has
        // already closed. The editor is a session leader, so the negative PID
        // targets its process group and also cleans up descendants.
        let child_pid = self.child.id() as CInt;
        let mut exited = self.child.try_wait().ok().flatten().is_some();
        if !exited {
            let _ = self.write_all_timeout(b"\x11n", Duration::from_millis(100));
            let deadline = Instant::now() + Duration::from_millis(250);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    exited = true;
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }

        if !exited {
            unsafe {
                kill(-child_pid, SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_millis(250);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    exited = true;
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }

        if !exited {
            unsafe {
                kill(-child_pid, SIGKILL);
            }
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    exited = true;
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            if !exited {
                // The group kill should make this return promptly. Keep a
                // final direct kill for platforms where setsid was rejected.
                let _ = self.child.kill();
                let deadline = Instant::now() + Duration::from_millis(250);
                while Instant::now() < deadline {
                    if self.child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }

        self.reader_stop.store(true, Ordering::Release);
        if let Some(reader) = self._reader.take() {
            let _ = reader.join();
        }
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

#[cfg(test)]
mod tests {
    #[test]
    fn vt100_parser_retains_split_escape_sequences() {
        let mut parser = vt100::Parser::new(3, 12, 0);
        parser.process(b"before\x1b[");
        parser.process(b"2J\x1b[Hafter");

        let screen = parser.screen();
        assert_eq!(screen.cell(0, 0).unwrap().contents(), "a");
        assert_eq!(screen.cell(0, 4).unwrap().contents(), "r");
    }

    #[test]
    fn vt100_parser_tracks_utf8_display_width_and_erase() {
        let mut parser = vt100::Parser::new(3, 12, 0);
        parser.process("日本語".as_bytes());

        assert_eq!(parser.screen().cursor_position(), (0, 6));
        assert_eq!(parser.screen().cell(0, 2).unwrap().contents(), "本");

        parser.process(b"\x1b[1;1H\x1b[31mred\x1b[K");
        let screen = parser.screen();
        assert_eq!(screen.cell(0, 0).unwrap().contents(), "r");
        assert_eq!(screen.cell(0, 0).unwrap().fgcolor(), vt100::Color::Idx(1));
        assert!(screen.cell(0, 5).unwrap().contents().is_empty());
    }
}
