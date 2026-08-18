use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ptytest::{Color, CommandSpec, Deadline, ExitStatus, ProtocolProfile, PtyTest, Scenario, Size, TerminalBaseline, TestEnv};

// ---------------------------------------------------------------------------
// Key — editor-specific names for exact input sequences
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
// TempDir — editor fixtures remain application-specific
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

    pub fn path(&self) -> &Path { &self.0 }
}

impl Drop for TempDir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

// ---------------------------------------------------------------------------
// Recording — an opt-in e-owned asciicast policy
// ---------------------------------------------------------------------------

struct Recording {
    start: std::time::Instant,
    events: Vec<(f64, char, Vec<u8>)>,
    rows: u16,
    cols: u16,
}

impl Recording {
    fn new(rows: u16, cols: u16) -> Self {
        Self { start: std::time::Instant::now(), events: Vec::new(), rows, cols }
    }

    fn push(&mut self, kind: char, data: &[u8]) {
        self.events.push((self.start.elapsed().as_secs_f64(), kind, data.to_vec()));
    }

    fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).unwrap(); }
        let mut output = std::fs::File::create(path).unwrap();
        use std::io::Write;
        writeln!(output, r#"{{"version": 2, "width": {}, "height": {}}}"#, self.cols, self.rows).unwrap();
        let mut groups: Vec<(f64, Vec<u8>)> = Vec::new();
        for (time, kind, data) in &self.events {
            if *kind != 'o' { continue; }
            if let Some(last) = groups.last_mut() && time - last.0 < 0.05 {
                last.1.extend_from_slice(data);
            } else {
                groups.push((*time, data.clone()));
            }
        }
        let mut adjusted = 0.0;
        for (index, (_, data)) in groups.iter().enumerate() {
            if index > 0 { adjusted += (groups[index].0 - groups[index - 1].0).max(0.4); }
            writeln!(output, r#"[{adjusted:.3}, "o", "{}"]"#, json_escape_bytes(data)).unwrap();
        }
    }
}

fn json_escape_bytes(data: &[u8]) -> String {
    let mut output = String::new();
    for character in String::from_utf8_lossy(data).chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            character if (character as u32) < 0x20 || character == '\x7f' => write!(output, "\\u{:04x}", character as u32).unwrap(),
            character => output.push(character),
        }
    }
    output
}

fn recording_path() -> Option<PathBuf> {
    std::env::var("E2E_RECORD").ok()?;
    let name = std::thread::current().name().unwrap_or("unknown").replace("::", "__");
    Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/recordings").join(format!("{name}.cast")))
}

// ---------------------------------------------------------------------------
// TestEditor — thin e-specific adapter over ptytest
// ---------------------------------------------------------------------------

pub struct TestEditor {
    terminal: PtyTest,
    terminal_baseline: TerminalBaseline,
    restoration_checked: bool,
    pub home: TempDir,
    pub rows: u16,
    pub cols: u16,
    recording: Option<Recording>,
    recorded_output: usize,
}

impl TestEditor {
    pub fn new(args: &[&str]) -> Self { Self::with_size(args, 24, 80) }

    pub fn with_size(args: &[&str], rows: u16, cols: u16) -> Self {
        Self::spawn(args, rows, cols, None)
    }

    pub fn with_profile_size(args: &[&str], rows: u16, cols: u16) -> Self {
        let profile_dir = std::env::var_os("E_PGO_PROFILE_DIR").map(PathBuf::from).expect("E_PGO_PROFILE_DIR must select the profile output directory");
        assert!(profile_dir.is_dir(), "E_PGO_PROFILE_DIR is not a directory: {}", profile_dir.display());
        Self::spawn(args, rows, cols, Some(profile_dir))
    }

    fn spawn(args: &[&str], rows: u16, cols: u16, profile_dir: Option<PathBuf>) -> Self {
        let home = TempDir::new();
        let binary = selected_binary();
        let mut command = CommandSpec::new(binary)
            .args(args.iter().copied())
            .current_dir(home.path())
            // A HOME-only PATH forces the editor's internal clipboard, so
            // parallel E2E tests never race through the system clipboard.
            .env("PATH", home.path())
            .remove_env("WAYLAND_DISPLAY")
            .remove_env("DISPLAY");
        if let Some(profile_dir) = profile_dir {
            command = command.env("LLVM_PROFILE_FILE", profile_dir.join("e-%p.profraw"));
        }
        let environment = TestEnv::hermetic_utf8("C.UTF-8")
            .expect("C.UTF-8 must be available on supported E2E platforms")
            .env("HOME", home.path())
            .env("PATH", home.path());
        let scenario = Scenario::new("e editor E2E")
            .expect("valid scenario label")
            .command(command)
            .size(Size::new(cols, rows).expect("non-zero editor size"))
            .environment(environment)
            .protocol_profile(ProtocolProfile::xterm_minimal_v1());
        let terminal = PtyTest::spawn(scenario).expect("spawn editor through ptytest");
        let terminal_baseline = terminal.terminal_baseline();
        let mut editor = Self {
            terminal,
            terminal_baseline,
            restoration_checked: false,
            home,
            rows,
            cols,
            recording: recording_path().map(|_| Recording::new(rows, cols)),
            recorded_output: 0,
        };
        editor.wait_for_startup();
        editor
    }

    fn deadline(&self) -> Deadline { self.terminal.deadline(Duration::from_secs(5)) }

    fn capture_output(&mut self) {
        let new_output = self.terminal.raw_output().get(self.recorded_output..).unwrap_or_default().to_vec();
        self.recorded_output += new_output.len();
        if let Some(recording) = &mut self.recording {
            if !new_output.is_empty() { recording.push('o', &new_output); }
        }
    }

    fn wait_for_startup(&mut self) {
        let status_row = self.rows.saturating_sub(2) as usize;
        let deadline = self.terminal.deadline(Duration::from_secs(5));
        self.terminal
            .wait_for_screen(deadline, "editor status row", move |screen| {
                screen.row(status_row).is_some_and(|row| !row.trim().is_empty())
            })
            .expect("editor did not become ready during startup");
        self.capture_output();
    }

    /// Wait for one editor output event. The public tests assert the resulting
    /// semantic row/cell state; a legitimate no-op input is allowed to remain
    /// silent instead of being turned into a timing failure.
    pub fn wait(&mut self) {
        let deadline = self.terminal.deadline(Duration::from_millis(100));
        self.terminal
            .wait_for_output(deadline)
            .expect("wait for editor output");
        self.capture_output();
    }

    pub fn wait_until<F>(&mut self, timeout: Duration, mut ready: F)
    where
        F: FnMut(&mut Self) -> bool,
    {
        let deadline = self.terminal.deadline(timeout);
        loop {
            self.terminal.drain(deadline).expect("drain editor output");
            self.capture_output();
            if ready(self) { return; }
            let saw_output = self.terminal
                .wait_for_output(deadline)
                .expect("wait for editor output");
            self.capture_output();
            if ready(self) { return; }
            if !saw_output {
                // `wait_for_output` returns false only at the caller's
                // deadline. The final semantic check above distinguishes a
                // quiet valid exit from a real state-transition failure.
                panic!("editor did not reach the expected state within {timeout:?}");
            }
        }
    }

    pub fn send_raw(&mut self, bytes: &[u8]) {
        if let Some(recording) = &mut self.recording { recording.push('i', bytes); }
        let deadline = self.deadline();
        self.terminal.send_bytes(deadline, bytes).expect("PTY write failed");
    }

    pub fn type_text(&mut self, text: &str) { self.send_raw(text.as_bytes()); self.wait(); }
    pub fn ctrl(&mut self, character: char) { self.send_raw(&[(character as u8) & 0x1f]); self.wait(); }
    pub fn key(&mut self, key: Key) { self.send_raw(key.as_bytes()); self.wait(); }
    pub fn enter(&mut self) { self.send_raw(b"\r"); self.wait(); }
    pub fn backspace(&mut self) { self.send_raw(b"\x7f"); self.wait(); }
    pub fn tab(&mut self) { self.send_raw(b"\t"); self.wait(); }
    pub fn escape(&mut self) { self.send_raw(b"\x1b"); self.wait(); }

    pub fn paste(&mut self, text: &str) {
        self.send_raw(b"\x1b[200~");
        self.send_raw(text.as_bytes());
        self.send_raw(b"\x1b[201~");
        self.wait();
    }

    // --- mouse events (SGR mode, 1-indexed) ---------------------------------

    pub fn click(&mut self, row: u16, column: u16) {
        let (row, column) = (row + 1, column + 1);
        self.send_raw(format!("\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m").as_bytes());
        self.wait();
    }
    pub fn double_click(&mut self, row: u16, column: u16) { self.click(row, column); self.click(row, column); }
    pub fn triple_click(&mut self, row: u16, column: u16) { self.click(row, column); self.click(row, column); self.click(row, column); }
    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16)) {
        let (from_row, from_column) = (from.0 + 1, from.1 + 1);
        let (to_row, to_column) = (to.0 + 1, to.1 + 1);
        self.send_raw(format!("\x1b[<0;{from_column};{from_row}M\x1b[<32;{to_column};{to_row}M\x1b[<0;{to_column};{to_row}m").as_bytes());
        self.wait();
    }
    pub fn scroll_up(&mut self) { self.send_raw(b"\x1b[<64;1;1M"); self.wait(); }
    pub fn scroll_down(&mut self) { self.send_raw(b"\x1b[<65;1;1M"); self.wait(); }
    pub fn focus_in(&mut self) { self.send_raw(b"\x1b[I"); self.wait(); }

    pub fn row(&mut self, row: u16) -> String {
        self.terminal.drain(self.deadline()).expect("drain editor output");
        self.capture_output();
        self.terminal.screen().row(row as usize).unwrap_or_default().trim_end().to_owned()
    }

    pub fn screen_text(&mut self) -> String {
        (0..self.rows).map(|row| self.row(row)).collect::<Vec<_>>().join("\n")
    }

    pub fn cursor(&mut self) -> (u16, u16) {
        self.terminal.drain(self.deadline()).expect("drain editor output");
        self.capture_output();
        let cursor = self.terminal.screen().cursor();
        (cursor.row, cursor.column)
    }

    /// The editor draws its software cursor as bold inverse text.
    pub fn cursor_visible(&mut self) -> bool {
        let (row, column) = self.cursor();
        self.terminal.screen().cell(row as usize, column as usize).is_some_and(|cell| {
            cell.attributes().bold && cell.attributes().inverse
        })
    }

    pub fn status_bar(&mut self) -> String { self.row(self.rows - 2) }
    pub fn command_line(&mut self) -> String { self.row(self.rows - 1) }
    pub fn cell_fg(&mut self, row: u16, column: u16) -> Color {
        self.terminal.drain(self.deadline()).expect("drain editor output");
        self.capture_output();
        self.terminal.screen().cell(row as usize, column as usize).map_or(Color::Default, |cell| cell.attributes().foreground.clone())
    }
    pub fn cell_bg(&mut self, row: u16, column: u16) -> Color {
        self.terminal.drain(self.deadline()).expect("drain editor output");
        self.capture_output();
        self.terminal.screen().cell(row as usize, column as usize).map_or(Color::Default, |cell| cell.attributes().background.clone())
    }
    pub fn cell_inverse(&mut self, row: u16, column: u16) -> bool {
        self.terminal.drain(self.deadline()).expect("drain editor output");
        self.capture_output();
        self.terminal.screen().cell(row as usize, column as usize).is_some_and(|cell| cell.attributes().inverse)
    }

    pub fn quit_no_save(&mut self) { self.ctrl('q'); self.send_raw(b"n"); self.wait(); }
    pub fn quit_saving(&mut self) { self.ctrl('q'); self.send_raw(b"y"); self.wait(); }
    pub fn wait_for_exit(&mut self) -> ExitStatus {
        let deadline = self.deadline();
        let status = self.terminal.wait_for_exit(deadline).expect("wait for editor exit");
        self.assert_normal_exit_restoration(status);
        status
    }
    pub fn has_exited(&mut self) -> bool {
        let status = self.terminal.observe_exit().expect("observe editor exit");
        if let Some(status) = status {
            self.assert_normal_exit_restoration(status);
            true
        } else {
            false
        }
    }

    fn assert_normal_exit_restoration(&mut self, status: ExitStatus) {
        if status == ExitStatus::Code(0) && !self.restoration_checked {
            self.terminal
                .assert_terminal_restored(&self.terminal_baseline)
                .expect("normal editor exit restores applicable terminal modes");
            self.restoration_checked = true;
        }
    }
}

impl Drop for TestEditor {
    fn drop(&mut self) {
        self.terminal.drain(self.deadline()).ok();
        self.capture_output();
        if let Some(recording) = self.recording.take() && let Some(path) = recording_path() {
            recording.save(&path);
        }
        let deadline = self.terminal.deadline(Duration::from_secs(1));
        let _ = self.terminal.finish(deadline);
    }
}

fn selected_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("E_PGO_BINARY").or_else(|| std::env::var_os("E_TEST_BINARY")) {
        let path = PathBuf::from(path);
        assert!(path.is_file(), "selected editor binary is not a file: {}", path.display());
        return path;
    }
    let mut path = std::env::current_exe().expect("test executable path unavailable");
    path.pop();
    path.pop();
    path.push("e");
    path
}

/// Create a file in an application fixture directory.
pub fn create_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).unwrap(); }
    std::fs::write(&path, content).unwrap();
    path
}
