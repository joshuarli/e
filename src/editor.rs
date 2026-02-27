use std::fs::File;
use std::io::{self, Read, Write, stdout};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use termion::event::{Event, Key, MouseButton, MouseEvent};
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::screen::IntoAlternateScreen;

/// Wraps a reader to distinguish Ctrl+J (0x0A) from Enter (0x0D).
///
/// Termion normalises both bytes to `Key::Char('\n')`, making them
/// indistinguishable.  By replacing 0x0A with 0x00 *before* termion
/// parses the stream, the editor receives `Event::Unsupported([0])`
/// for Ctrl+J while Enter still works normally.
struct CtrlJReader<R>(R);

impl<R: Read> Read for CtrlJReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.0.read(buf)?;
        for b in &mut buf[..n] {
            if *b == 0x0A {
                *b = 0x00;
            }
        }
        Ok(n)
    }
}

use crate::buffer;
use crate::clipboard::Clipboard;
use crate::command::{CommandAction, CommandRegistry};
use crate::command_buffer::{CommandBuffer, CommandBufferMode, CommandBufferResult};
use crate::document::Document;
use crate::highlight;
use crate::keybind::{EditorAction, KeybindingTable};
use crate::language;
use crate::render::{Renderer, gutter_width};
use crate::selection::{Pos, Selection, is_word_char, next_word_boundary, prev_word_boundary};
use crate::view::View;

const SCROLL_LINES: usize = 3;

const PASTE_START: &[u8] = &[0x1b, b'[', b'2', b'0', b'0', b'~'];
const PASTE_END: &[u8] = &[0x1b, b'[', b'2', b'0', b'1', b'~'];
const CTRL_SHIFT_UP: &[u8] = &[0x1b, b'[', b'1', b';', b'6', b'A'];
const CTRL_SHIFT_DOWN: &[u8] = &[0x1b, b'[', b'1', b';', b'6', b'B'];
const CTRL_LEFT: &[u8] = &[0x1b, b'[', b'1', b';', b'5', b'D'];
const CTRL_RIGHT: &[u8] = &[0x1b, b'[', b'1', b';', b'5', b'C'];
// rxvt-style (sent by tmux, some terminals)
const CTRL_LEFT_RXVT: &[u8] = &[0x1b, b'O', b'd'];
const CTRL_RIGHT_RXVT: &[u8] = &[0x1b, b'O', b'c'];
const CTRL_SHIFT_LEFT: &[u8] = &[0x1b, b'[', b'1', b';', b'6', b'D'];
const CTRL_SHIFT_RIGHT: &[u8] = &[0x1b, b'[', b'1', b';', b'6', b'C'];
// CSI u encoding for Ctrl+Backspace (kitty, ghostty, etc.)
const CTRL_BACKSPACE_CSI_U: &[u8] = &[0x1b, b'[', b'1', b'2', b'7', b';', b'5', b'u'];
const FOCUS_IN: &[u8] = &[0x1b, b'[', b'I'];

fn auto_close_char(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        _ => None,
    }
}

fn is_close_char(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '"' | '\'')
}

fn is_paste_start(ev: &Event) -> bool {
    matches!(ev, Event::Unsupported(bytes) if bytes == PASTE_START)
}

fn is_paste_end(ev: &Event) -> bool {
    matches!(ev, Event::Unsupported(bytes) if bytes == PASTE_END)
}

fn common_prefix(strings: &[&str]) -> String {
    if strings.is_empty() {
        return String::new();
    }
    let first = strings[0];
    let mut len = first.len();
    for s in &strings[1..] {
        len = len.min(s.len());
        for (i, (a, b)) in first.bytes().zip(s.bytes()).enumerate() {
            if a != b {
                len = len.min(i);
                break;
            }
        }
    }
    first[..len].to_string()
}

pub struct Editor {
    doc: Document,
    sel: Selection,
    desired_col: Option<usize>,
    view: View,
    renderer: Renderer,
    clipboard: Clipboard,
    commands: CommandRegistry,
    keybindings: KeybindingTable,
    cmd_buf: CommandBuffer,
    ruler_on: bool,
    status_msg: String,
    status_time: Option<Instant>,
    running: bool,
    /// Pending quit confirmation (dirty buffer).
    quit_pending: bool,
    // Mouse state
    last_click_time: Option<Instant>,
    last_click_pos: Option<(u16, u16)>,
    click_count: u8,
    dragging: bool,
    // Find state
    find_pattern: String,
    /// Viewport-only matches, repopulated each draw() call.
    find_matches: Vec<(Pos, Pos)>,
    /// Compiled regex cached across keystrokes.
    find_re: Option<regex_lite::Regex>,
    /// The currently-navigated match (start, end).
    find_current: Option<(Pos, Pos)>,
    /// True while browsing find results with up/down arrows.
    find_active: bool,
    /// Temp file path for sudo save flow.
    sudo_save_tmp: Option<String>,
    /// True when stdin was a pipe (e.g. `git show | e`).
    piped_stdin: bool,
    /// Cached file mtime for external modification detection.
    file_mtime: Option<std::time::SystemTime>,
    /// Waiting for y/n response to reload prompt.
    reload_pending: bool,
    /// Cached status-left string; reused each frame to avoid per-draw allocation.
    status_left_cache: String,
}

enum EditorEvent {
    Term(Event),
    Paste(String),
    #[allow(dead_code)]
    Tick,
}

impl Editor {
    pub fn new(text: Vec<u8>, filename: Option<String>, piped_stdin: bool) -> Self {
        let (w, h) = termion::terminal_size().unwrap_or((80, 24));
        let mut keybindings = KeybindingTable::with_defaults();
        keybindings.load_config();
        let mut doc = Document::new(text, filename);
        let file_mtime = doc
            .filename
            .as_ref()
            .and_then(|name| crate::file_io::file_mtime(std::path::Path::new(name)));
        let mut restored_cursor = None;
        if let Some(ref name) = doc.filename {
            let path = std::path::Path::new(name);
            if path.exists() {
                crate::file_io::load_undo_history(path, &mut doc.undo_stack);
            }
            restored_cursor = crate::file_io::load_cursor_position(path);
        }
        // Clamp restored cursor to buffer bounds
        let initial_cursor = if let Some(pos) = restored_cursor {
            let line_count = doc.buf.line_count();
            let line = pos.line.min(line_count.saturating_sub(1));
            let col = pos.col.min(doc.buf.line_char_len(line));
            Pos::new(line, col)
        } else {
            Pos::zero()
        };
        Self {
            doc,
            sel: Selection::caret(initial_cursor),
            desired_col: None,
            view: View::new(w, h),
            renderer: Renderer::new(),
            clipboard: Clipboard::detect(),
            commands: CommandRegistry::new(),
            keybindings,
            cmd_buf: CommandBuffer::new(),
            ruler_on: true,
            status_msg: String::new(),
            status_time: None,
            running: true,
            quit_pending: false,
            last_click_time: None,
            last_click_pos: None,
            click_count: 0,
            dragging: false,
            find_pattern: String::new(),
            find_matches: Vec::new(),
            find_re: None,
            find_current: None,
            find_active: false,
            sudo_save_tmp: None,
            piped_stdin,
            file_mtime,
            reload_pending: false,
            status_left_cache: String::new(),
        }
    }

    pub fn run(&mut self) -> io::Result<()> {
        // Center view on restored cursor position
        if self.sel.cursor != Pos::zero() {
            self.center_view_on_line(self.sel.cursor.line);
        }

        let mut stdout = stdout().into_raw_mode()?.into_alternate_screen()?;

        write!(
            stdout,
            "\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1004h"
        )?;
        stdout.flush()?;

        let (tx, rx) = mpsc::channel::<EditorEvent>();

        let tx_input = tx.clone();
        let use_tty = self.piped_stdin;
        std::thread::spawn(move || {
            let tty_file: Option<File> = if use_tty {
                File::open("/dev/tty").ok()
            } else {
                None
            };
            let stdin_handle;
            let events: Box<dyn Iterator<Item = Result<Event, io::Error>>> =
                if let Some(f) = tty_file {
                    Box::new(CtrlJReader(f).events())
                } else {
                    stdin_handle = io::stdin();
                    Box::new(CtrlJReader(stdin_handle.lock()).events())
                };
            let mut in_paste = false;
            let mut paste_buf = String::new();
            for ev in events.flatten() {
                if is_paste_start(&ev) {
                    in_paste = true;
                    paste_buf.clear();
                    continue;
                }
                if is_paste_end(&ev) {
                    in_paste = false;
                    if tx_input
                        .send(EditorEvent::Paste(std::mem::take(&mut paste_buf)))
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
                if in_paste {
                    match &ev {
                        Event::Key(Key::Char(c)) => paste_buf.push(*c),
                        // Key::Null = 0x0A (NL) remapped by CtrlJReader; restore as newline in paste.
                        Event::Key(Key::Null) => paste_buf.push('\n'),
                        Event::Key(Key::Backspace) => paste_buf.push('\x7f'),
                        _ => {}
                    }
                    continue;
                }
                if tx_input.send(EditorEvent::Term(ev)).is_err() {
                    break;
                }
            }
        });

        crate::signal::register_sigwinch();

        while self.running {
            // Expire status messages after 3 seconds
            if let Some(t) = self.status_time
                && t.elapsed().as_secs() >= 3
            {
                self.status_msg.clear();
                self.status_time = None;
            }

            self.draw(&mut stdout)?;

            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(EditorEvent::Term(ev)) => self.handle_event(ev),
                Ok(EditorEvent::Paste(text)) => {
                    if self.cmd_buf.active {
                        let result = self.cmd_buf.insert_str(&text);
                        let mode = self.cmd_buf.mode;
                        self.handle_cmd_result(mode, result);
                    } else {
                        self.paste_text(&text);
                    }
                }
                Ok(EditorEvent::Tick) | Err(mpsc::RecvTimeoutError::Timeout) => {
                    if crate::signal::take_sigwinch()
                        && let Ok((w, h)) = termion::terminal_size()
                    {
                        self.view.width = w;
                        self.view.height = h;
                        self.renderer.force_full_redraw();
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        write!(
            stdout,
            "\x1b[?1004l\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l"
        )?;
        stdout.flush()?;
        Ok(())
    }

    fn set_status(&mut self, msg: String) {
        self.status_msg = msg;
        self.status_time = Some(Instant::now());
    }

    fn cursor(&self) -> Pos {
        self.sel.cursor
    }

    fn set_cursor(&mut self, pos: Pos) {
        self.sel = Selection::caret(pos);
    }

    fn draw(&mut self, out: &mut impl Write) -> io::Result<()> {
        let line_count = self.doc.buf.line_count();
        let gw = if self.ruler_on {
            gutter_width(line_count)
        } else {
            0
        };

        let display_col = self.cursor_display_col();
        let cursor_line = self.sel.cursor.line;
        let mut line_display_width =
            |line: usize| -> usize { self.doc.buf.display_col_at(line, usize::MAX) };
        self.view
            .ensure_cursor_visible(cursor_line, display_col, gw, &mut line_display_width);

        let lang = self.doc.filename.as_deref().and_then(language::detect);
        let lang_name = lang.map(|l| l.name).unwrap_or("Text");
        let sel = if self.sel.is_empty() {
            None
        } else {
            Some(self.sel)
        };
        let ruler_on = self.ruler_on;

        // All &mut self calls must happen before we borrow status_left_cache.
        let bracket_pair = self.find_matching_bracket();

        // Refresh viewport matches on every draw (cheap — only scans visible lines).
        if self.find_re.is_some() {
            self.refresh_viewport_matches();
        }

        let rules = lang.and_then(|l| highlight::rules_for_language(l.name));
        self.renderer.set_syntax(rules);

        // Pure reads — no more &mut self after this point.
        let find_matches = if !self.find_matches.is_empty() {
            Some(self.find_matches.as_slice())
        } else {
            None
        };
        let find_current = if self.find_active {
            self.find_current
        } else {
            None
        };

        let completions = &self.cmd_buf.completions;

        let cmd_cursor = if self.cmd_buf.active {
            Some(self.cmd_buf.prompt.len() + self.cmd_buf.cursor)
        } else {
            None
        };

        // Avoid cloning status_msg: borrow it directly as &str.
        let display_line_owned;
        let cmd_ref: Option<&str> = if self.cmd_buf.active {
            display_line_owned = self.cmd_buf.display_line();
            Some(&display_line_owned)
        } else if !self.status_msg.is_empty() {
            Some(&self.status_msg)
        } else {
            None
        };

        // Rebuild status_left into the reused cache buffer (no allocation after warm-up).
        let name = self.doc.filename.as_deref().unwrap_or("[scratch]");
        Self::build_status_left(name, self.doc.dirty, lang_name, &mut self.status_left_cache);
        let status_left = &self.status_left_cache;
        let status_right = Self::status_right();

        self.renderer.render(
            out,
            &mut self.doc.buf,
            &self.view,
            cursor_line,
            display_col,
            ruler_on,
            status_left,
            status_right,
            cmd_ref,
            sel,
            find_matches,
            find_current,
            completions,
            cmd_cursor,
            self.find_active,
            bracket_pair,
        )
    }

    fn build_status_left(name: &str, dirty: bool, lang_name: &str, out: &mut String) {
        out.clear();
        out.push(' ');
        out.push_str(name);
        if dirty {
            out.push('*');
        }
        out.push_str(" [");
        out.push_str(lang_name);
        out.push(']');
    }

    #[cfg(test)]
    fn status_left(&self, lang_name: &str) -> String {
        let name = self.doc.filename.as_deref().unwrap_or("[scratch]");
        let mut s = String::new();
        Self::build_status_left(name, self.doc.dirty, lang_name, &mut s);
        s
    }

    fn status_right() -> &'static str {
        concat!(" e v", env!("CARGO_PKG_VERSION"), " ")
    }

    fn center_view_on_line(&mut self, line: usize) {
        let gw = if self.ruler_on {
            gutter_width(self.doc.buf.line_count())
        } else {
            0
        };
        let mut ldw = |l: usize| -> usize { self.doc.buf.display_col_at(l, usize::MAX) };
        self.view.center_on_line(line, &mut ldw, gw);
    }

    fn cursor_display_col(&self) -> usize {
        self.doc
            .buf
            .display_col_at(self.cursor().line, self.cursor().col)
    }

    fn find_matching_bracket(&mut self) -> Option<(Pos, Pos)> {
        let cursor = self.cursor();
        let line_count = self.doc.buf.line_count();
        let mut scratch = Vec::new();
        if let Some(match_pos) = highlight::find_bracket_match(
            cursor,
            &mut |line_idx, buf| self.doc.buf.line_text_into(line_idx, buf),
            &mut scratch,
            line_count,
        ) {
            return Some((cursor, match_pos));
        }
        let match_pos = highlight::find_quote_match(
            cursor,
            &mut |line_idx, buf| self.doc.buf.line_text_into(line_idx, buf),
            &mut scratch,
            line_count,
        )?;
        Some((cursor, match_pos))
    }

    fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => {
                if self.cmd_buf.active {
                    self.handle_cmd_key(key);
                } else {
                    self.handle_key(key);
                }
            }
            Event::Mouse(mouse) => {
                if !self.cmd_buf.active {
                    if self.find_active {
                        self.exit_find_mode();
                    }
                    self.handle_mouse(mouse);
                }
            }
            Event::Unsupported(bytes) => {
                if bytes == FOCUS_IN {
                    self.check_external_modification();
                } else if !self.cmd_buf.active {
                    if bytes == CTRL_SHIFT_UP {
                        self.select_above();
                    } else if bytes == CTRL_SHIFT_DOWN {
                        self.select_below();
                    } else if bytes == CTRL_LEFT || bytes == CTRL_LEFT_RXVT {
                        self.word_left();
                    } else if bytes == CTRL_RIGHT || bytes == CTRL_RIGHT_RXVT {
                        self.word_right();
                    } else if bytes == CTRL_SHIFT_LEFT {
                        self.word_left_extend();
                    } else if bytes == CTRL_SHIFT_RIGHT {
                        self.word_right_extend();
                    } else if bytes == CTRL_BACKSPACE_CSI_U {
                        self.ctrl_backspace();
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, key: Key) {
        // Handle quit confirmation
        if self.quit_pending {
            match key {
                Key::Char('y') | Key::Char('Y') => {
                    self.save_file();
                    if !self.cmd_buf.active {
                        // Named file: save completed (or failed); quit now.
                        // If cmd_buf is active, save_file opened a "Save as:" prompt;
                        // quit_pending stays true and the Prompt handler will quit after save.
                        self.running = false;
                    }
                }
                Key::Char('n') | Key::Char('N') => {
                    self.save_undo_if_named();
                    self.running = false;
                }
                _ => {
                    self.quit_pending = false;
                    self.status_msg.clear();
                    self.status_time = None;
                }
            }
            return;
        }

        // Handle reload confirmation
        if self.reload_pending {
            match key {
                Key::Char('y') | Key::Char('Y') => self.reload_file(),
                _ => self.dismiss_reload(),
            }
            return;
        }

        // Find navigation mode: up/down browse matches, anything else exits
        if self.find_active {
            match key {
                Key::Up => {
                    self.find_prev();
                    return;
                }
                Key::Down => {
                    self.find_next();
                    return;
                }
                Key::Esc => {
                    self.exit_find_mode();
                    self.clear_selection();
                    return;
                }
                _ => {
                    self.exit_find_mode();
                    // Fall through to process the key normally
                }
            }
        }

        self.desired_col = match key {
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => self.desired_col,
            _ => None,
        };

        // Check keybinding table first
        if let Some(action) = self.keybindings.lookup(key).cloned() {
            match action {
                EditorAction::Save => self.save_file(),
                EditorAction::Quit => self.try_quit(),
                EditorAction::Undo => self.undo(),
                EditorAction::Redo => self.redo(),
                EditorAction::SelectAll => self.select_all(),
                EditorAction::Copy => self.copy(),
                EditorAction::Cut => self.cut(),
                EditorAction::Paste => self.paste(),
                EditorAction::KillLine => self.kill_line(),
                EditorAction::GotoTop => self.goto_top(),
                EditorAction::GotoEnd => self.goto_end(),
                EditorAction::ToggleRuler => {
                    self.ruler_on = !self.ruler_on;
                    self.renderer.force_full_redraw();
                }
                EditorAction::CommandPalette => {
                    self.cmd_buf.open(CommandBufferMode::Command, "> ", "");
                }
                EditorAction::GotoLine => {
                    self.cmd_buf.open(CommandBufferMode::Goto, "goto: ", "");
                }
                EditorAction::Find => {
                    let prefill = if !self.sel.is_empty() {
                        let (start, end) = self.sel.ordered();
                        let text = self.doc.text_in_range(start, end);
                        let s = String::from_utf8_lossy(&text).to_string();
                        if s.len() <= 100 { s } else { String::new() }
                    } else {
                        String::new()
                    };
                    self.cmd_buf
                        .open(CommandBufferMode::Find, "find: ", &prefill);
                    self.find_matches.clear();
                    self.find_re = None;
                    self.find_current = None;
                }
                EditorAction::CtrlBackspace => self.ctrl_backspace(),
                EditorAction::ToggleComment => self.toggle_comment(),
                EditorAction::DuplicateLine => self.duplicate_line(),
                EditorAction::SelectWord => self.select_word_at(self.cursor()),
            }
            return;
        }

        // Non-configurable keys
        match key {
            // Shift+Arrow
            Key::ShiftUp => self.move_up_extend(),
            Key::ShiftDown => self.move_down_extend(),
            Key::ShiftLeft => self.move_left_extend(),
            Key::ShiftRight => self.move_right_extend(),

            // Movement
            Key::Up => self.move_up(),
            Key::Down => self.move_down(),
            Key::Left => self.move_left(),
            Key::Right => self.move_right(),
            Key::Home => self.move_home(),
            Key::End => self.move_end(),
            Key::CtrlLeft => self.word_left(),
            Key::CtrlRight => self.word_right(),
            Key::PageUp => self.page_up(),
            Key::PageDown => self.page_down(),

            Key::Esc => {
                self.clear_selection();
                self.find_matches.clear();
                self.find_re = None;
                self.find_current = None;
            }

            // Editing
            Key::Delete => self.delete_forward(),
            Key::Backspace => self.backspace(),
            Key::Char('\t') => self.insert_tab(),
            Key::BackTab => self.dedent(),
            Key::Char('\n') => self.insert_newline(),
            // Ctrl+J (0x0A) arrives as Key::Null via CtrlJReader (0x0A → 0x00).
            Key::Null => self.duplicate_line(),
            Key::Char(c) => self.insert_char(c),
            _ => {}
        }
    }

    fn try_quit(&mut self) {
        if self.doc.dirty {
            let name = self.doc.filename.as_deref().unwrap_or("[scratch]");
            self.status_msg = format!("Save changes to {}? (y/n)", name);
            self.status_time = None; // don't expire this message
            self.quit_pending = true;
        } else {
            self.save_undo_if_named();
            self.running = false;
        }
    }

    fn save_undo_if_named(&mut self) {
        if let Some(name) = self.doc.filename.clone() {
            let path = std::path::Path::new(&name);
            crate::file_io::save_cursor_position(path, self.sel.cursor);
            if path.exists() {
                self.doc.seal_undo();
                crate::file_io::save_undo_history(path, &self.doc.undo_stack);
            }
        }
    }

    // -- command buffer key handling ----------------------------------------

    fn handle_cmd_key(&mut self, key: Key) {
        // Key::Null = Ctrl+J (0x0A via CtrlJReader); treat as Enter in command buffer.
        let key = if key == Key::Null {
            Key::Char('\n')
        } else {
            key
        };
        let mode = self.cmd_buf.mode;
        let result = self.cmd_buf.handle_key(key);
        self.handle_cmd_result(mode, result);
    }

    fn handle_cmd_result(&mut self, mode: CommandBufferMode, result: CommandBufferResult) {
        match result {
            CommandBufferResult::Submit(val) => {
                self.cmd_buf.close();
                match mode {
                    CommandBufferMode::Command => self.execute_command(&val),
                    CommandBufferMode::Find => self.find_next_from_submit(&val),
                    CommandBufferMode::Goto => {
                        let cmd = format!("goto {}", val);
                        self.execute_command(&cmd);
                    }
                    CommandBufferMode::Prompt => {
                        // save-as prompt
                        self.doc.filename = Some(val.clone());
                        self.save_file();
                        if self.quit_pending && !self.cmd_buf.active {
                            self.quit_pending = false;
                            self.running = false;
                        }
                    }
                    CommandBufferMode::SudoSave => {
                        self.save_file_sudo(&val);
                    }
                }
            }
            CommandBufferResult::Cancel => {
                self.cmd_buf.close();
                if mode == CommandBufferMode::Find {
                    self.find_matches.clear();
                    self.find_re = None;
                    self.find_current = None;
                    self.status_msg.clear();
                    self.status_time = None;
                }
                if mode == CommandBufferMode::SudoSave {
                    if let Some(tmp) = self.sudo_save_tmp.take() {
                        let _ = std::fs::remove_file(tmp);
                    }
                    self.set_status("sudo save cancelled".to_string());
                }
            }
            CommandBufferResult::Changed(val) => {
                if mode == CommandBufferMode::Find {
                    self.update_find_highlights(&val);
                    if let Some((_, end)) = self.find_current {
                        self.set_cursor(end);
                        self.center_view_on_line(end.line);
                        self.set_find_status();
                    }
                }
            }
            CommandBufferResult::TabComplete => {
                if mode == CommandBufferMode::Command {
                    self.complete_command();
                }
            }
            CommandBufferResult::Continue => {}
        }
    }

    fn complete_command(&mut self) {
        let input = self.cmd_buf.input.trim().to_string();
        let names = self.commands.command_names();

        if input.is_empty() {
            // Show all commands
            self.cmd_buf.completions = names.iter().map(|s| s.to_string()).collect();
        } else {
            let matches: Vec<&str> = names
                .iter()
                .filter(|n| n.starts_with(&input))
                .copied()
                .collect();

            match matches.len() {
                0 => {
                    self.cmd_buf.completions.clear();
                }
                1 => {
                    // Single match — autocomplete
                    self.cmd_buf.input = matches[0].to_string();
                    self.cmd_buf.cursor = self.cmd_buf.input.len();
                    self.cmd_buf.completions.clear();
                }
                _ => {
                    // Multiple matches — show them and complete common prefix
                    self.cmd_buf.completions = matches.iter().map(|s| s.to_string()).collect();
                    let common = common_prefix(&matches);
                    if common.len() > input.len() {
                        self.cmd_buf.input = common;
                        self.cmd_buf.cursor = self.cmd_buf.input.len();
                    }
                }
            }
        }
    }

    // -- commands -----------------------------------------------------------

    fn execute_command(&mut self, input: &str) {
        let action = self.commands.execute(input);
        match action {
            CommandAction::None => {}
            CommandAction::Save => self.save_file(),
            CommandAction::SaveAs(name) => {
                self.doc.filename = Some(name);
                self.save_file();
            }
            CommandAction::Quit => {
                self.save_undo_if_named();
                self.running = false;
            }
            CommandAction::Goto(n) => self.goto_line(n),
            CommandAction::ToggleRuler => {
                self.ruler_on = !self.ruler_on;
                self.renderer.force_full_redraw();
            }
            CommandAction::ReplaceAll {
                pattern,
                replacement,
            } => {
                self.replace_all(&pattern, &replacement);
            }
            CommandAction::Find(pattern) => self.find_next_from_submit(&pattern),
            CommandAction::ToggleComment => self.toggle_comment(),
            CommandAction::CommentOn => self.set_comment(true),
            CommandAction::CommentOff => self.set_comment(false),
            CommandAction::SelectAll => self.select_all(),
            CommandAction::Trim => self.strip_trailing_whitespace(),
            CommandAction::StatusMsg(msg) => self.set_status(msg),
        }
    }

    fn goto_line(&mut self, n: usize) {
        let line_count = self.doc.buf.line_count();
        let target = if n == 0 {
            0
        } else {
            (n - 1).min(line_count.saturating_sub(1))
        };
        self.set_cursor(Pos::new(target, 0));
        self.center_view_on_line(target);
    }

    fn goto_top(&mut self) {
        self.set_cursor(Pos::zero());
    }

    fn goto_end(&mut self) {
        let line_count = self.doc.buf.line_count();
        let last_line = line_count.saturating_sub(1);
        let last_col = self.doc.buf.line_char_len(last_line);
        self.set_cursor(Pos::new(last_line, last_col));
    }

    fn kill_line(&mut self) {
        let c = self.cursor();
        let line_count = self.doc.buf.line_count();
        if line_count == 0 {
            return;
        }
        self.doc.seal_undo();
        let start = Pos::new(c.line, 0);
        let end = if c.line + 1 < line_count {
            Pos::new(c.line + 1, 0)
        } else {
            let len = self.doc.buf.line_char_len(c.line);
            Pos::new(c.line, len)
        };
        self.doc.delete_range(start, end);
        self.doc.seal_undo();
        // Clamp cursor
        let new_line_count = self.doc.buf.line_count();
        let new_line = c.line.min(new_line_count.saturating_sub(1));
        let new_col = self.doc.buf.line_char_len(new_line).min(c.col);
        self.set_cursor(Pos::new(new_line, new_col));
    }

    // -- find ---------------------------------------------------------------

    fn update_find_highlights(&mut self, pattern: &str) {
        self.find_matches.clear();
        self.find_current = None;
        self.find_pattern = pattern.to_string();
        if pattern.is_empty() {
            self.find_re = None;
            return;
        }

        // Smart-case: case-insensitive if all lowercase
        let case_insensitive = pattern.chars().all(|c| !c.is_uppercase());
        let re = if case_insensitive {
            regex_lite::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
        } else {
            regex_lite::Regex::new(pattern)
        };

        let re = match re {
            Ok(r) => r,
            Err(_) => {
                self.find_re = None;
                return; // invalid regex — just don't highlight
            }
        };

        self.find_re = Some(re);
        // Scan only the viewport lines for visible highlights.
        self.refresh_viewport_matches();
        // Pick the first match at or after the cursor; wrap if needed.
        let cursor = self.cursor();
        let in_viewport = self
            .find_matches
            .iter()
            .find(|(s, _)| *s >= cursor)
            .or_else(|| self.find_matches.first())
            .copied();
        if let Some(m) = in_viewport {
            self.find_current = Some(m);
        } else {
            // No match in viewport — search forward through the rest of the file.
            let re = self.find_re.take().unwrap();
            self.find_current = Self::search_forward(&self.doc.buf, &re, cursor);
            self.find_re = Some(re);
        }
    }

    /// Scan only the current viewport lines and populate `find_matches`.
    /// Uses take/restore on `find_re` to satisfy the borrow checker.
    fn refresh_viewport_matches(&mut self) {
        self.find_matches.clear();
        let re = match self.find_re.take() {
            Some(r) => r,
            None => return,
        };
        let line_count = self.doc.buf.line_count();
        // A few extra lines beyond text_rows to account for wrapped lines.
        let viewport_end = (self.view.scroll_line + self.view.text_rows() + 4).min(line_count);
        let mut line_buf = Vec::new();
        for line_idx in self.view.scroll_line..viewport_end {
            self.doc.buf.line_text_into(line_idx, &mut line_buf);
            let Ok(text) = std::str::from_utf8(&line_buf) else {
                continue;
            };
            for m in re.find_iter(text) {
                let start_col = buffer::char_count(&line_buf[..m.start()]);
                let end_col = buffer::char_count(&line_buf[..m.end()]);
                self.find_matches
                    .push((Pos::new(line_idx, start_col), Pos::new(line_idx, end_col)));
            }
        }
        self.find_re = Some(re);
    }

    /// Search forward from `from` (inclusive), wrapping around the file.
    /// Returns the first match found.
    fn search_forward(
        buf: &buffer::GapBuffer,
        re: &regex_lite::Regex,
        from: Pos,
    ) -> Option<(Pos, Pos)> {
        let line_count = buf.line_count();
        let mut line_buf = Vec::new();
        for pass in 0..2 {
            let (start, end) = if pass == 0 {
                (from.line, line_count)
            } else {
                (0, from.line)
            };
            for line_idx in start..end {
                buf.line_text_into(line_idx, &mut line_buf);
                let Ok(text) = std::str::from_utf8(&line_buf) else {
                    continue;
                };
                for m in re.find_iter(text) {
                    let start_col = buffer::char_count(&line_buf[..m.start()]);
                    // On the starting line (pass 0) skip matches before from.col.
                    if pass == 0 && line_idx == from.line && start_col < from.col {
                        continue;
                    }
                    let end_col = buffer::char_count(&line_buf[..m.end()]);
                    return Some((Pos::new(line_idx, start_col), Pos::new(line_idx, end_col)));
                }
            }
        }
        None
    }

    /// Search backward from `from` (exclusive), wrapping around the file.
    /// Returns the last match before the cursor position.
    fn search_backward(
        buf: &buffer::GapBuffer,
        re: &regex_lite::Regex,
        from: Pos,
    ) -> Option<(Pos, Pos)> {
        let line_count = buf.line_count();
        let mut line_buf = Vec::new();
        for pass in 0..2 {
            // pass 0: from.line down to 0, filtering out matches at/after from on from.line
            // pass 1: all lines in reverse (wrap-around), no position filter
            let range: Box<dyn Iterator<Item = usize>> = if pass == 0 {
                Box::new((0..=from.line).rev())
            } else {
                Box::new((0..line_count).rev())
            };
            for line_idx in range {
                buf.line_text_into(line_idx, &mut line_buf);
                let Ok(text) = std::str::from_utf8(&line_buf) else {
                    continue;
                };
                // Find the last eligible match on this line.
                let mut best: Option<(Pos, Pos)> = None;
                for m in re.find_iter(text) {
                    let start_col = buffer::char_count(&line_buf[..m.start()]);
                    let end_col = buffer::char_count(&line_buf[..m.end()]);
                    // On the from-line in pass 0: skip matches whose end >= from.col.
                    if pass == 0 && line_idx == from.line && end_col >= from.col {
                        continue;
                    }
                    best = Some((Pos::new(line_idx, start_col), Pos::new(line_idx, end_col)));
                }
                if best.is_some() {
                    return best;
                }
            }
        }
        None
    }

    fn find_next_from_submit(&mut self, pattern: &str) {
        self.update_find_highlights(pattern);
        if self.find_current.is_none() {
            self.set_status("Find: no matches".to_string());
            return;
        }
        self.find_active = true;
        if let Some((_, end)) = self.find_current {
            self.set_cursor(end);
            self.center_view_on_line(end.line);
            self.set_find_status();
        }
    }

    fn find_next(&mut self) {
        if self.find_re.is_none() {
            return;
        }
        let cursor = self.cursor();
        let re = self.find_re.take().unwrap();
        let result = Self::search_forward(&self.doc.buf, &re, cursor);
        self.find_re = Some(re);
        if let Some(m) = result {
            self.find_current = Some(m);
            let (_, end) = m;
            self.set_cursor(end);
            self.center_view_on_line(end.line);
            self.set_find_status();
        }
    }

    fn find_prev(&mut self) {
        if self.find_re.is_none() {
            return;
        }
        let cursor = self.cursor();
        let re = self.find_re.take().unwrap();
        let result = Self::search_backward(&self.doc.buf, &re, cursor);
        self.find_re = Some(re);
        if let Some(m) = result {
            self.find_current = Some(m);
            let (_, end) = m;
            self.set_cursor(end);
            self.center_view_on_line(end.line);
            self.set_find_status();
        }
    }

    fn set_find_status(&mut self) {
        self.status_msg = format!("Find: {}", self.find_pattern);
        self.status_time = None; // don't auto-expire while browsing
    }

    fn exit_find_mode(&mut self) {
        // Select the current match so copy/backspace/etc. act on it
        if let Some((start, end)) = self.find_current {
            self.sel = Selection {
                anchor: start,
                cursor: end,
            };
        }
        self.find_active = false;
        self.find_matches.clear();
        self.find_re = None;
        self.find_current = None;
        self.status_msg.clear();
        self.status_time = None;
    }

    // -- replace all --------------------------------------------------------

    fn replace_all(&mut self, pattern: &str, replacement: &str) {
        let case_insensitive = pattern.chars().all(|c| !c.is_uppercase());
        let re = if case_insensitive {
            regex_lite::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
        } else {
            regex_lite::Regex::new(pattern)
        };
        let re = match re {
            Ok(r) => r,
            Err(e) => {
                self.set_status(format!("Invalid regex: {}", e));
                return;
            }
        };

        // Determine the range to operate on
        let (range_start, range_end) = if !self.sel.is_empty() {
            self.sel.ordered()
        } else {
            let line_count = self.doc.buf.line_count();
            let last_line = line_count.saturating_sub(1);
            let last_col = self.doc.buf.line_char_len(last_line);
            (Pos::zero(), Pos::new(last_line, last_col))
        };

        let text_bytes = self.doc.text_in_range(range_start, range_end);
        let text = String::from_utf8_lossy(&text_bytes);
        let new_text = re.replace_all(&text, replacement);

        if new_text == text {
            self.set_status("Replaced 0 occurrences".to_string());
            return;
        }

        let count = re.find_iter(&text).count();

        self.doc.seal_undo();
        self.doc.delete_range(range_start, range_end);
        self.doc
            .insert(range_start.line, range_start.col, new_text.as_bytes());
        self.doc.seal_undo();

        self.clear_selection();
        self.set_status(format!("Replaced {} occurrences", count));
    }

    // -- mouse handling -----------------------------------------------------

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse {
            MouseEvent::Press(MouseButton::Left, x, y) => self.mouse_press(x, y),
            MouseEvent::Hold(x, y) => self.mouse_drag(x, y),
            MouseEvent::Release(_, _) => {
                self.dragging = false;
            }
            MouseEvent::Press(MouseButton::WheelUp, _, _) => self.scroll_up(),
            MouseEvent::Press(MouseButton::WheelDown, _, _) => self.scroll_down(),
            _ => {}
        }
    }

    fn screen_to_buffer_pos(&mut self, x: u16, y: u16) -> Pos {
        let line_count = self.doc.buf.line_count();
        let gw = if self.ruler_on {
            gutter_width(line_count)
        } else {
            0
        };
        let text_cols = self.view.text_cols(gw);
        if text_cols == 0 {
            return Pos::zero();
        }

        let target_row = (y as usize).saturating_sub(1);
        let click_col = (x as usize).saturating_sub(1).saturating_sub(gw);

        // Walk from (scroll_line, scroll_wrap) counting screen rows
        let mut screen_row: usize = 0;
        let mut line_idx = self.view.scroll_line;
        let first_wrap = self.view.scroll_wrap;

        while line_idx < line_count {
            let display_width = self.doc.buf.display_col_at(line_idx, usize::MAX);
            let char_len = self.doc.buf.line_char_len(line_idx);
            let total_wraps = crate::view::wrapped_rows(display_width, text_cols);
            let start_wrap = if line_idx == self.view.scroll_line {
                first_wrap
            } else {
                0
            };

            for wrap in start_wrap..total_wraps {
                if screen_row == target_row {
                    // This is the screen row the user clicked on
                    let display_col = wrap * text_cols + click_col;
                    let char_col = self.doc.buf.char_col_from_display(line_idx, display_col);
                    return Pos::new(line_idx, char_col.min(char_len));
                }
                screen_row += 1;
            }

            line_idx += 1;
        }

        // Clicked below all content — return end of last line
        let last_line = line_count.saturating_sub(1);
        let last_col = self.doc.buf.line_char_len(last_line);
        Pos::new(last_line, last_col)
    }

    fn mouse_press(&mut self, x: u16, y: u16) {
        let pos = self.screen_to_buffer_pos(x, y);
        let now = Instant::now();

        let is_multi = self
            .last_click_time
            .is_some_and(|t| now.duration_since(t).as_millis() < 400)
            && self.last_click_pos == Some((x, y));

        if is_multi {
            self.click_count = ((self.click_count % 3) + 1).max(1);
        } else {
            self.click_count = 1;
        }
        self.last_click_time = Some(now);
        self.last_click_pos = Some((x, y));

        match self.click_count {
            1 => {
                self.set_cursor(pos);
                self.dragging = true;
            }
            2 => self.select_word_at(pos),
            3 => self.select_line_at(pos.line),
            _ => {}
        }
    }

    fn mouse_drag(&mut self, x: u16, y: u16) {
        if !self.dragging {
            return;
        }
        let pos = self.screen_to_buffer_pos(x, y);
        self.sel.cursor = pos;
    }

    fn select_word_at(&mut self, pos: Pos) {
        let line_text = self.doc.buf.line_text(pos.line);
        if line_text.is_empty() {
            return;
        }
        let col = pos.col.min(line_text.len().saturating_sub(1));
        if col < line_text.len() && is_word_char(line_text[col]) {
            let mut start = col;
            while start > 0 && is_word_char(line_text[start - 1]) {
                start -= 1;
            }
            let mut end = col;
            while end < line_text.len() && is_word_char(line_text[end]) {
                end += 1;
            }
            self.sel = Selection {
                anchor: Pos::new(pos.line, end),
                cursor: Pos::new(pos.line, start),
            };
        }
    }

    fn select_line_at(&mut self, line: usize) {
        let line_count = self.doc.buf.line_count();
        if line >= line_count {
            return;
        }
        let end = if line + 1 < line_count {
            Pos::new(line + 1, 0)
        } else {
            let len = self.doc.buf.line_char_len(line);
            Pos::new(line, len)
        };
        self.sel = Selection {
            anchor: Pos::new(line, 0),
            cursor: end,
        };
    }

    fn scroll_up(&mut self) {
        if self.view.scroll_line == 0 && self.view.scroll_wrap == 0 {
            return;
        }
        let gw = if self.ruler_on {
            gutter_width(self.doc.buf.line_count())
        } else {
            0
        };
        let text_cols = self.view.text_cols(gw);
        // Scroll back by SCROLL_LINES screen rows
        let mut remaining = SCROLL_LINES;
        while remaining > 0 {
            if self.view.scroll_wrap > 0 {
                let step = remaining.min(self.view.scroll_wrap);
                self.view.scroll_wrap -= step;
                remaining -= step;
            } else if self.view.scroll_line > 0 {
                self.view.scroll_line -= 1;
                let dw = self
                    .doc
                    .buf
                    .display_col_at(self.view.scroll_line, usize::MAX);
                let wraps = crate::view::wrapped_rows(dw, text_cols);
                remaining -= 1;
                if remaining > 0 && wraps > 1 {
                    let step = remaining.min(wraps - 1);
                    self.view.scroll_wrap = (wraps - 1).saturating_sub(step);
                    remaining -= step;
                }
            } else {
                break;
            }
        }
        // Move cursor into viewport if it scrolled past the bottom
        self.clamp_cursor_to_viewport(gw, text_cols);
    }

    fn scroll_down(&mut self) {
        let line_count = self.doc.buf.line_count();
        if self.view.scroll_line >= line_count.saturating_sub(1) {
            return;
        }
        let gw = if self.ruler_on {
            gutter_width(line_count)
        } else {
            0
        };
        let text_cols = self.view.text_cols(gw);
        // Scroll forward by SCROLL_LINES screen rows
        let mut remaining = SCROLL_LINES;
        while remaining > 0 && self.view.scroll_line < line_count.saturating_sub(1) {
            let dw = self
                .doc
                .buf
                .display_col_at(self.view.scroll_line, usize::MAX);
            let wraps = crate::view::wrapped_rows(dw, text_cols);
            let remaining_in_line = wraps.saturating_sub(self.view.scroll_wrap);
            if remaining < remaining_in_line {
                self.view.scroll_wrap += remaining;
                remaining = 0;
            } else {
                remaining -= remaining_in_line;
                self.view.scroll_line += 1;
                self.view.scroll_wrap = 0;
            }
        }
        // Move cursor into viewport if it scrolled past the top
        self.clamp_cursor_to_viewport(gw, text_cols);
    }

    /// After a mouse-wheel scroll, move the cursor so it stays within the visible area.
    fn clamp_cursor_to_viewport(&mut self, _gw: usize, text_cols: usize) {
        let text_rows = self.view.text_rows();
        if text_rows == 0 || text_cols == 0 {
            return;
        }
        let line_count = self.doc.buf.line_count();
        let cursor = self.cursor();
        let cursor_dcol = self.cursor_display_col();
        let cursor_wrap = cursor_dcol / text_cols;

        // Check if cursor is above viewport
        if cursor.line < self.view.scroll_line
            || (cursor.line == self.view.scroll_line && cursor_wrap < self.view.scroll_wrap)
        {
            // Snap cursor to the first visible position
            // The first visible char col is scroll_wrap * text_cols
            let first_dcol = self.view.scroll_wrap * text_cols;
            let char_col = self
                .doc
                .buf
                .char_col_from_display(self.view.scroll_line, first_dcol);
            let line_len = self.doc.buf.line_char_len(self.view.scroll_line);
            self.set_cursor(Pos::new(self.view.scroll_line, char_col.min(line_len)));
            return;
        }

        // Check if cursor is below viewport — walk screen rows to find last visible position
        let mut screen_row = 0usize;
        let mut line_idx = self.view.scroll_line;
        let mut last_visible_line = self.view.scroll_line;
        let mut last_visible_wrap = self.view.scroll_wrap;

        while screen_row < text_rows && line_idx < line_count {
            let dw = self.doc.buf.display_col_at(line_idx, usize::MAX);
            let total = crate::view::wrapped_rows(dw, text_cols);
            let start_w = if line_idx == self.view.scroll_line {
                self.view.scroll_wrap
            } else {
                0
            };
            for w in start_w..total {
                if screen_row >= text_rows {
                    break;
                }
                last_visible_line = line_idx;
                last_visible_wrap = w;
                screen_row += 1;
            }
            line_idx += 1;
        }

        if cursor.line > last_visible_line
            || (cursor.line == last_visible_line && cursor_wrap > last_visible_wrap)
        {
            // Snap cursor to last visible wrap row
            let target_dcol = last_visible_wrap * text_cols;
            let char_col = self
                .doc
                .buf
                .char_col_from_display(last_visible_line, target_dcol);
            let line_len = self.doc.buf.line_char_len(last_visible_line);
            self.set_cursor(Pos::new(last_visible_line, char_col.min(line_len)));
        }
    }

    // -- selection helpers --------------------------------------------------

    fn delete_selection(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        let (start, end) = self.sel.ordered();
        self.doc.seal_undo();
        let pos = self.doc.delete_range(start, end);
        self.doc.seal_undo();
        self.set_cursor(pos);
    }

    fn clear_selection(&mut self) {
        self.sel = Selection::caret(self.cursor());
    }

    fn select_all(&mut self) {
        let line_count = self.doc.buf.line_count();
        let last_line = line_count.saturating_sub(1);
        let last_col = self.doc.buf.line_char_len(last_line);
        self.sel = Selection {
            anchor: Pos::zero(),
            cursor: Pos::new(last_line, last_col),
        };
    }

    fn select_above(&mut self) {
        self.sel = Selection {
            anchor: self.cursor(),
            cursor: Pos::zero(),
        };
        self.desired_col = None;
    }

    fn select_below(&mut self) {
        let last_line = self.doc.buf.line_count().saturating_sub(1);
        let last_col = self.doc.buf.line_char_len(last_line);
        self.sel = Selection {
            anchor: self.cursor(),
            cursor: Pos::new(last_line, last_col),
        };
        self.desired_col = None;
    }

    // -- movement (no selection) --------------------------------------------

    fn move_up(&mut self) {
        if self.cursor().line > 0 {
            let target_col = self.desired_col.unwrap_or(self.cursor().col);
            self.desired_col = Some(target_col);
            let new_line = self.cursor().line - 1;
            let line_len = self.doc.buf.line_char_len(new_line);
            self.set_cursor(Pos::new(new_line, target_col.min(line_len)));
        }
    }

    fn move_down(&mut self) {
        let line_count = self.doc.buf.line_count();
        if self.cursor().line + 1 < line_count {
            let target_col = self.desired_col.unwrap_or(self.cursor().col);
            self.desired_col = Some(target_col);
            let new_line = self.cursor().line + 1;
            let line_len = self.doc.buf.line_char_len(new_line);
            self.set_cursor(Pos::new(new_line, target_col.min(line_len)));
        }
    }

    fn indent_snap_left(&mut self, line: usize, col: usize) -> usize {
        let ls = self.doc.buf.line_start(line);
        let le = self.doc.buf.line_end(line);
        // Count leading whitespace (spaces/tabs are ASCII: byte offset == char offset).
        let mut leading_ws = 0;
        while ls + leading_ws < le {
            match self.doc.buf.byte_at(ls + leading_ws) {
                b' ' | b'\t' => leading_ws += 1,
                _ => break,
            }
        }
        if col <= leading_ws && col >= 1 {
            // Snap only if the bytes before the cursor are all spaces (not tabs).
            if (0..col).all(|i| self.doc.buf.byte_at(ls + i) == b' ') {
                return (col - 1) / 2 * 2;
            }
        }
        col - 1
    }

    fn indent_snap_right(&mut self, line: usize, col: usize) -> usize {
        let ls = self.doc.buf.line_start(line);
        let le = self.doc.buf.line_end(line);
        // Count leading whitespace and check all-spaces in one pass.
        let mut leading_ws = 0;
        let mut all_spaces = true;
        while ls + leading_ws < le {
            match self.doc.buf.byte_at(ls + leading_ws) {
                b' ' => leading_ws += 1,
                b'\t' => {
                    all_spaces = false;
                    leading_ws += 1;
                }
                _ => break,
            }
        }
        if col < leading_ws && all_spaces {
            let target = (col / 2 + 1) * 2;
            return target.min(leading_ws);
        }
        col + 1
    }

    fn move_left(&mut self) {
        if !self.sel.is_empty() {
            let (start, _) = self.sel.ordered();
            self.set_cursor(start);
            return;
        }
        let c = self.cursor();
        if c.col > 0 {
            let new_col = self.indent_snap_left(c.line, c.col);
            self.set_cursor(Pos::new(c.line, new_col));
        } else if c.line > 0 {
            let prev_len = self.doc.buf.line_char_len(c.line - 1);
            self.set_cursor(Pos::new(c.line - 1, prev_len));
        }
    }

    fn move_right(&mut self) {
        if !self.sel.is_empty() {
            let (_, end) = self.sel.ordered();
            self.set_cursor(end);
            return;
        }
        let c = self.cursor();
        let line_len = self.doc.buf.line_char_len(c.line);
        if c.col < line_len {
            let new_col = self.indent_snap_right(c.line, c.col);
            self.set_cursor(Pos::new(c.line, new_col));
        } else if c.line + 1 < self.doc.buf.line_count() {
            self.set_cursor(Pos::new(c.line + 1, 0));
        }
    }

    fn word_left(&mut self) {
        if !self.sel.is_empty() {
            let (start, _) = self.sel.ordered();
            self.set_cursor(start);
            return;
        }
        let c = self.cursor();
        if c.col == 0 {
            if c.line > 0 {
                let prev_len = self.doc.buf.line_char_len(c.line - 1);
                self.set_cursor(Pos::new(c.line - 1, prev_len));
            }
            return;
        }
        let line_text = self.doc.buf.line_text(c.line);
        let boundary = prev_word_boundary(&line_text, c.col);
        self.set_cursor(Pos::new(c.line, boundary));
    }

    fn word_right(&mut self) {
        if !self.sel.is_empty() {
            let (_, end) = self.sel.ordered();
            self.set_cursor(end);
            return;
        }
        let c = self.cursor();
        let line_len = self.doc.buf.line_char_len(c.line);
        if c.col >= line_len {
            if c.line + 1 < self.doc.buf.line_count() {
                self.set_cursor(Pos::new(c.line + 1, 0));
            }
            return;
        }
        let line_text = self.doc.buf.line_text(c.line);
        let boundary = next_word_boundary(&line_text, c.col);
        self.set_cursor(Pos::new(c.line, boundary));
    }

    fn word_left_extend(&mut self) {
        let c = self.cursor();
        if c.col == 0 {
            if c.line > 0 {
                let prev_len = self.doc.buf.line_char_len(c.line - 1);
                self.sel.cursor = Pos::new(c.line - 1, prev_len);
            }
            return;
        }
        let line_text = self.doc.buf.line_text(c.line);
        let boundary = prev_word_boundary(&line_text, c.col);
        self.sel.cursor = Pos::new(c.line, boundary);
    }

    fn word_right_extend(&mut self) {
        let c = self.cursor();
        let line_len = self.doc.buf.line_char_len(c.line);
        if c.col >= line_len {
            if c.line + 1 < self.doc.buf.line_count() {
                self.sel.cursor = Pos::new(c.line + 1, 0);
            }
            return;
        }
        let line_text = self.doc.buf.line_text(c.line);
        let boundary = next_word_boundary(&line_text, c.col);
        self.sel.cursor = Pos::new(c.line, boundary);
    }

    fn move_home(&mut self) {
        self.set_cursor(Pos::new(self.cursor().line, 0));
    }

    fn move_end(&mut self) {
        let c = self.cursor();
        let len = self.doc.buf.line_char_len(c.line);
        self.set_cursor(Pos::new(c.line, len));
    }

    fn page_up(&mut self) {
        let rows = self.view.text_rows();
        let target_col = self.desired_col.unwrap_or(self.cursor().col);
        self.desired_col = Some(target_col);
        let new_line = self.cursor().line.saturating_sub(rows);
        let line_len = self.doc.buf.line_char_len(new_line);
        self.set_cursor(Pos::new(new_line, target_col.min(line_len)));
    }

    fn page_down(&mut self) {
        let rows = self.view.text_rows();
        let line_count = self.doc.buf.line_count();
        let target_col = self.desired_col.unwrap_or(self.cursor().col);
        self.desired_col = Some(target_col);
        let new_line = (self.cursor().line + rows).min(line_count.saturating_sub(1));
        let line_len = self.doc.buf.line_char_len(new_line);
        self.set_cursor(Pos::new(new_line, target_col.min(line_len)));
    }

    // -- movement (extend selection) ----------------------------------------

    fn move_up_extend(&mut self) {
        if self.cursor().line > 0 {
            let target_col = self.desired_col.unwrap_or(self.cursor().col);
            self.desired_col = Some(target_col);
            let new_line = self.cursor().line - 1;
            let line_len = self.doc.buf.line_char_len(new_line);
            self.sel.cursor = Pos::new(new_line, target_col.min(line_len));
        }
    }

    fn move_down_extend(&mut self) {
        let line_count = self.doc.buf.line_count();
        if self.cursor().line + 1 < line_count {
            let target_col = self.desired_col.unwrap_or(self.cursor().col);
            self.desired_col = Some(target_col);
            let new_line = self.cursor().line + 1;
            let line_len = self.doc.buf.line_char_len(new_line);
            self.sel.cursor = Pos::new(new_line, target_col.min(line_len));
        }
    }

    fn move_left_extend(&mut self) {
        let c = self.cursor();
        if c.col > 0 {
            self.sel.cursor = Pos::new(c.line, self.indent_snap_left(c.line, c.col));
        } else if c.line > 0 {
            let prev_len = self.doc.buf.line_char_len(c.line - 1);
            self.sel.cursor = Pos::new(c.line - 1, prev_len);
        }
    }

    fn move_right_extend(&mut self) {
        let c = self.cursor();
        let line_len = self.doc.buf.line_char_len(c.line);
        if c.col < line_len {
            self.sel.cursor = Pos::new(c.line, self.indent_snap_right(c.line, c.col));
        } else if c.line + 1 < self.doc.buf.line_count() {
            self.sel.cursor = Pos::new(c.line + 1, 0);
        }
    }

    // -- editing ------------------------------------------------------------

    fn insert_char(&mut self, c: char) {
        if !self.sel.is_empty() {
            // Wrap selection with matching pairs
            if let Some(close) = auto_close_char(c) {
                let (start, end) = self.sel.ordered();
                let text = self.doc.text_in_range(start, end);
                let mut wrapped = vec![c as u8];
                wrapped.extend_from_slice(&text);
                wrapped.push(close as u8);
                self.doc.begin_undo_group();
                self.doc.delete_range(start, end);
                let after = self.doc.insert(start.line, start.col, &wrapped);
                self.doc.end_undo_group();
                // Select the inner text (between the pair chars)
                self.sel = Selection {
                    anchor: Pos::new(start.line, start.col + 1),
                    cursor: Pos::new(after.line, after.col - 1),
                };
                return;
            }
            self.delete_selection();
        }

        // Skip over closing char if it's already the next character.
        // close chars are ASCII so byte_at(line_start + col) == the char at col.
        if is_close_char(c) {
            let line = self.cursor().line;
            let col = self.cursor().col;
            let ls = self.doc.buf.line_start(line);
            let le = self.doc.buf.line_end(line);
            if ls + col < le && self.doc.buf.byte_at(ls + col) == c as u8 {
                self.set_cursor(Pos::new(line, col + 1));
                return;
            }
        }

        let mut char_buf = [0u8; 4];
        let s = c.encode_utf8(&mut char_buf);

        // Auto-close pairs: insert open+close on a stack buffer, no heap alloc.
        if let Some(close) = auto_close_char(c) {
            let line = self.cursor().line;
            let col = self.cursor().col;
            let ls = self.doc.buf.line_start(line);
            let le = self.doc.buf.line_end(line);
            // Treat end-of-line (\n or past end) as a boundary.
            let next = if ls + col < le {
                self.doc.buf.byte_at(ls + col)
            } else {
                b'\n'
            };
            let next_is_boundary =
                next == b' ' || next == b'\t' || next == b'\n' || is_close_char(next as char);
            if next_is_boundary {
                // Stack-allocate the pair: open char (1–4 bytes) + close char (1 byte).
                let cb = s.as_bytes();
                let mut pair = [0u8; 5];
                pair[..cb.len()].copy_from_slice(cb);
                pair[cb.len()] = close as u8;
                let pos = self.doc.insert(line, col, &pair[..cb.len() + 1]);
                // Place cursor between the pair
                self.set_cursor(Pos::new(pos.line, pos.col - 1));
                return;
            }
        }

        let pos = self
            .doc
            .insert(self.cursor().line, self.cursor().col, s.as_bytes());
        self.set_cursor(pos);
    }

    fn insert_tab(&mut self) {
        if !self.sel.is_empty() {
            self.indent_selection();
            return;
        }
        let use_tab = self.doc.filename.as_ref().is_some_and(|f| {
            f.ends_with(".c") || f.ends_with(".h") || f.ends_with(".go") || f.contains("Makefile")
        });
        let bytes: &[u8] = if use_tab { b"\t" } else { b"  " };
        let pos = self
            .doc
            .insert(self.cursor().line, self.cursor().col, bytes);
        self.set_cursor(pos);
    }

    fn indent_selection(&mut self) {
        let (s, e) = self.sel.ordered();
        let end_line = if e.col == 0 && e.line > s.line {
            e.line - 1
        } else {
            e.line
        };
        let start_line = s.line;

        let use_tab = self.doc.filename.as_ref().is_some_and(|f| {
            f.ends_with(".c") || f.ends_with(".h") || f.ends_with(".go") || f.contains("Makefile")
        });
        let indent_bytes: &[u8] = if use_tab { b"\t" } else { b"  " };
        let indent_char_len = if use_tab { 1 } else { 2 };

        // Pre-read line data to avoid O(n²) cache rebuilds
        let lines: Vec<(Vec<u8>, usize)> = (start_line..=end_line)
            .map(|i| (self.doc.buf.line_text(i), self.doc.buf.line_start(i)))
            .collect();

        let cursor_pos = self.cursor();
        self.doc.begin_undo_group();
        let cursor_line = self.cursor().line;
        let mut cursor_added = 0usize;
        for (idx, (text, line_offset)) in lines.iter().enumerate().rev() {
            let is_blank = text.iter().all(|&b| b == b' ' || b == b'\t');
            if is_blank {
                continue;
            }
            self.doc
                .insert_at_byte(*line_offset, indent_bytes, cursor_pos, cursor_pos);
            let line_idx = start_line + idx;
            if line_idx == cursor_line {
                cursor_added = indent_char_len;
            }
        }
        self.doc.end_undo_group();

        let c = self.cursor();
        self.set_cursor(Pos::new(c.line, c.col + cursor_added));
    }

    fn insert_newline(&mut self) {
        if !self.sel.is_empty() {
            self.delete_selection();
        }
        let line_text = self.doc.buf.line_text(self.cursor().line);
        let indent: Vec<u8> = line_text
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .copied()
            .collect();

        let mut newline = vec![b'\n'];
        newline.extend_from_slice(&indent);

        self.doc.seal_undo();
        let pos = self
            .doc
            .insert(self.cursor().line, self.cursor().col, &newline);
        self.doc.seal_undo();
        self.set_cursor(pos);
    }

    fn backspace(&mut self) {
        if !self.sel.is_empty() {
            self.delete_selection();
            return;
        }
        let c = self.cursor();
        if c.col > 0 {
            let ls = self.doc.buf.line_start(c.line);
            let le = self.doc.buf.line_end(c.line);
            // Count leading whitespace (ASCII: byte offset == char offset here).
            let mut leading_ws = 0;
            while ls + leading_ws < le {
                match self.doc.buf.byte_at(ls + leading_ws) {
                    b' ' | b'\t' => leading_ws += 1,
                    _ => break,
                }
            }

            // Smart 2-space dedent
            if c.col <= leading_ws && c.col >= 2 {
                let all_spaces = (0..c.col).all(|i| self.doc.buf.byte_at(ls + i) == b' ');
                if all_spaces && c.col.is_multiple_of(2) {
                    let end = Pos::new(c.line, c.col);
                    let start = Pos::new(c.line, c.col - 2);
                    self.doc.delete_range(start, end);
                    self.set_cursor(start);
                    return;
                }
            }

            // Delete matching auto-close pair if cursor is between them.
            let prev = self.doc.buf.byte_at(ls + c.col - 1);
            if ls + c.col < le {
                let next = self.doc.buf.byte_at(ls + c.col);
                if auto_close_char(prev as char) == Some(next as char) {
                    let start = Pos::new(c.line, c.col - 1);
                    let end = Pos::new(c.line, c.col + 1);
                    self.doc.delete_range(start, end);
                    self.set_cursor(start);
                    return;
                }
            }

            let start = Pos::new(c.line, c.col - 1);
            let end = Pos::new(c.line, c.col);
            self.doc.delete_range(start, end);
            self.set_cursor(start);
        } else if c.line > 0 {
            let prev_len = self.doc.buf.line_char_len(c.line - 1);
            let start = Pos::new(c.line - 1, prev_len);
            let end = Pos::new(c.line, 0);
            self.doc.delete_range(start, end);
            self.set_cursor(start);
        }
    }

    fn ctrl_backspace(&mut self) {
        if !self.sel.is_empty() {
            self.delete_selection();
            return;
        }
        let c = self.cursor();
        if c.col == 0 && c.line == 0 {
            return;
        }
        if c.col == 0 {
            let prev_len = self.doc.buf.line_char_len(c.line - 1);
            let start = Pos::new(c.line - 1, prev_len);
            let end = Pos::new(c.line, 0);
            self.doc.seal_undo();
            self.doc.delete_range(start, end);
            self.doc.seal_undo();
            self.set_cursor(start);
            return;
        }
        let line_text = self.doc.buf.line_text(c.line);
        let boundary = prev_word_boundary(&line_text, c.col);
        let start = Pos::new(c.line, boundary);
        let end = Pos::new(c.line, c.col);
        self.doc.seal_undo();
        self.doc.delete_range(start, end);
        self.doc.seal_undo();
        self.set_cursor(start);
    }

    fn delete_forward(&mut self) {
        if !self.sel.is_empty() {
            self.delete_selection();
            return;
        }
        let c = self.cursor();
        let line_len = self.doc.buf.line_char_len(c.line);
        if c.col < line_len {
            self.doc
                .delete_range(Pos::new(c.line, c.col), Pos::new(c.line, c.col + 1));
        } else if c.line + 1 < self.doc.buf.line_count() {
            self.doc
                .delete_range(Pos::new(c.line, c.col), Pos::new(c.line + 1, 0));
        }
    }

    fn duplicate_line(&mut self) {
        let c = self.cursor();
        let line_text = self.doc.buf.line_text(c.line);
        let mut new_content = vec![b'\n'];
        new_content.extend_from_slice(&line_text);
        let line_char_len = self.doc.buf.line_char_len(c.line);
        self.doc.seal_undo();
        self.doc.insert(c.line, line_char_len, &new_content);
        self.doc.seal_undo();
        self.set_cursor(Pos::new(c.line + 1, c.col));
    }

    // -- commenting ---------------------------------------------------------

    fn toggle_comment(&mut self) {
        self.comment_impl(None);
    }

    fn set_comment(&mut self, on: bool) {
        self.comment_impl(Some(on));
    }

    /// `force`: None = toggle, Some(true) = comment, Some(false) = uncomment.
    fn comment_impl(&mut self, force: Option<bool>) {
        let comment = match self.doc.filename.as_deref().and_then(language::detect) {
            Some(lang) => lang.comment,
            None => {
                self.set_status("No language detected for commenting".to_string());
                return;
            }
        };

        // Determine line range: selection or current line
        let (start_line, end_line) = if self.sel.is_empty() {
            (self.cursor().line, self.cursor().line)
        } else {
            let (s, e) = self.sel.ordered();
            let end = if e.col == 0 && e.line > s.line {
                e.line - 1
            } else {
                e.line
            };
            (s.line, end)
        };

        let prefix = format!("{} ", comment);

        // Pre-read all line data and byte offsets to avoid O(n²) cache rebuilds.
        // Each insert/delete invalidates the line-start cache; reading it back
        // triggers a full rebuild. By collecting everything up front we rebuild
        // the cache exactly once.
        let lines: Vec<(Vec<u8>, usize)> = (start_line..=end_line)
            .map(|i| {
                let text = self.doc.buf.line_text(i);
                let offset = self.doc.buf.line_start(i);
                (text, offset)
            })
            .collect();

        // Check if all lines are already commented
        let all_commented = lines.iter().all(|(text, _)| {
            let trimmed = text.iter().position(|&b| b != b' ' && b != b'\t');
            match trimmed {
                Some(pos) => text[pos..].starts_with(prefix.as_bytes()),
                None => true, // empty/whitespace-only lines count as commented
            }
        });

        let do_uncomment = match force {
            Some(true) => false,   // comment on → never uncomment
            Some(false) => true,   // comment off → always uncomment
            None => all_commented, // toggle
        };

        let cursor_pos = self.cursor();
        self.doc.begin_undo_group();
        if do_uncomment {
            // Uncomment: remove first occurrence of "comment " from each line
            for (text, line_offset) in lines.iter().rev() {
                let indent_pos = text
                    .iter()
                    .position(|&b| b != b' ' && b != b'\t')
                    .unwrap_or(text.len());
                if text[indent_pos..].starts_with(prefix.as_bytes()) {
                    self.doc.delete_at_byte(
                        line_offset + indent_pos,
                        prefix.len(),
                        cursor_pos,
                        cursor_pos,
                    );
                }
            }
        } else {
            // Comment: find minimum indent, insert comment prefix at that indent
            let min_indent = lines
                .iter()
                .filter_map(|(text, _)| text.iter().position(|&b| b != b' ' && b != b'\t'))
                .min()
                .unwrap_or(0);
            for (text, line_offset) in lines.iter().rev() {
                let is_blank = text.iter().all(|&b| b == b' ' || b == b'\t');
                if is_blank {
                    continue;
                }
                // Skip lines that are already commented
                let indent_pos = text
                    .iter()
                    .position(|&b| b != b' ' && b != b'\t')
                    .unwrap_or(text.len());
                if text[indent_pos..].starts_with(prefix.as_bytes()) {
                    continue;
                }
                self.doc.insert_at_byte(
                    line_offset + min_indent,
                    prefix.as_bytes(),
                    cursor_pos,
                    cursor_pos,
                );
            }
        }
        self.doc.end_undo_group();
    }

    // -- dedent -------------------------------------------------------------

    fn dedent(&mut self) {
        let (start_line, end_line) = if self.sel.is_empty() {
            (self.cursor().line, self.cursor().line)
        } else {
            let (s, e) = self.sel.ordered();
            let end = if e.col == 0 && e.line > s.line {
                e.line - 1
            } else {
                e.line
            };
            (s.line, end)
        };

        // Pre-read line data to avoid O(n²) cache rebuilds
        let lines: Vec<(Vec<u8>, usize)> = (start_line..=end_line)
            .map(|i| (self.doc.buf.line_text(i), self.doc.buf.line_start(i)))
            .collect();

        let cursor_pos = self.cursor();
        self.doc.begin_undo_group();
        let cursor_line = self.cursor().line;
        let mut cursor_removed = 0usize;
        for (idx, (text, line_offset)) in lines.iter().enumerate().rev() {
            let removed = if text.starts_with(b"\t") {
                self.doc
                    .delete_at_byte(*line_offset, 1, cursor_pos, cursor_pos);
                1
            } else if text.starts_with(b"  ") {
                self.doc
                    .delete_at_byte(*line_offset, 2, cursor_pos, cursor_pos);
                2
            } else {
                0
            };
            let line_idx = start_line + idx;
            if line_idx == cursor_line {
                cursor_removed = removed;
            }
        }
        self.doc.end_undo_group();

        let c = self.cursor();
        let new_col = c.col.saturating_sub(cursor_removed);
        self.set_cursor(Pos::new(c.line, new_col));
    }

    // -- clipboard ----------------------------------------------------------

    fn copy(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        let (start, end) = self.sel.ordered();
        let text = self.doc.text_in_range(start, end);
        let s = String::from_utf8_lossy(&text).to_string();
        self.clipboard.copy(&s);
    }

    fn cut(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        self.copy();
        self.delete_selection();
    }

    fn paste(&mut self) {
        let text = self.clipboard.paste();
        self.paste_text(&text);
    }

    fn paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.sel.is_empty() {
            self.delete_selection();
        }
        let text = self.reindent_paste(text);
        self.doc.seal_undo();
        let pos = self
            .doc
            .insert(self.cursor().line, self.cursor().col, text.as_bytes());
        self.doc.seal_undo();
        self.set_cursor(pos);
    }

    /// Re-indent pasted multi-line text to match the current cursor's indent level.
    fn reindent_paste(&mut self, text: &str) -> String {
        let lines: Vec<&str> = text.split('\n').collect();
        if lines.len() < 2 {
            return text.to_string();
        }

        // Find the minimum indentation of non-empty lines in the pasted text
        // (skip the first line since it will be placed at cursor position)
        let min_indent = lines[1..]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);

        // Get the indent of the current line at cursor
        let cur_line_text = self.doc.buf.line_text(self.cursor().line);
        let cur_indent: usize = cur_line_text
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();

        // If the first line of paste has content, use cursor column as the base
        // indent for subsequent lines; otherwise use the current line's indent
        let target_indent = if !lines[0].trim().is_empty() {
            self.cursor().col
        } else {
            cur_indent
        };

        if target_indent == min_indent {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        result.push_str(lines[0]);
        for line in &lines[1..] {
            result.push('\n');
            if line.trim().is_empty() {
                result.push_str(line);
            } else {
                let stripped = &line[min_indent.min(line.len())..];
                for _ in 0..target_indent {
                    result.push(' ');
                }
                result.push_str(stripped);
            }
        }
        result
    }

    // -- undo/redo ----------------------------------------------------------

    fn undo(&mut self) {
        if let Some(pos) = self.doc.undo() {
            self.set_cursor(pos);
        }
    }

    fn redo(&mut self) {
        if let Some(pos) = self.doc.redo() {
            self.set_cursor(pos);
        }
    }

    // -- file I/O -----------------------------------------------------------

    fn strip_trailing_whitespace(&mut self) {
        let line_count = self.doc.buf.line_count();
        self.doc.seal_undo();
        for line_idx in (0..line_count).rev() {
            let text = self.doc.buf.line_text(line_idx);
            let trimmed_len = text
                .iter()
                .rposition(|&b| b != b' ' && b != b'\t')
                .map(|i| i + 1)
                .unwrap_or(0);
            let char_len = crate::buffer::char_count(&text);
            let trim_char_len = crate::buffer::char_count(&text[..trimmed_len]);
            if trim_char_len < char_len {
                self.doc.delete_range(
                    Pos::new(line_idx, trim_char_len),
                    Pos::new(line_idx, char_len),
                );
            }
        }
        self.doc.seal_undo();
        // Adjust cursor if past end of line
        let c = self.cursor();
        let line_len = self.doc.buf.line_char_len(c.line);
        if c.col > line_len {
            self.set_cursor(Pos::new(c.line, line_len));
        }
    }

    fn check_external_modification(&mut self) {
        if self.reload_pending || self.cmd_buf.active || self.quit_pending {
            return;
        }
        let Some(ref name) = self.doc.filename else {
            return;
        };
        let path = std::path::Path::new(name);
        let disk_mtime = crate::file_io::file_mtime(path);
        if disk_mtime != self.file_mtime && disk_mtime.is_some() {
            let short = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| name.clone());
            self.status_msg = format!("{} changed on disk. Reload? (y/n)", short);
            self.status_time = None;
            self.reload_pending = true;
        }
    }

    fn reload_file(&mut self) {
        let Some(name) = self.doc.filename.clone() else {
            return;
        };
        let read_result = crate::file_io::read_file(std::path::Path::new(&name));
        match read_result {
            Ok(data) => {
                let path = std::path::Path::new(&name);
                self.file_mtime = crate::file_io::file_mtime(path);
                self.doc = Document::new(data, Some(name));
                // Clamp cursor to valid position in new buffer
                let lc = self.doc.buf.line_count();
                if self.sel.cursor.line >= lc {
                    let last = lc.saturating_sub(1);
                    self.sel = Selection::caret(Pos::new(last, self.doc.buf.line_char_len(last)));
                } else {
                    let len = self.doc.buf.line_char_len(self.sel.cursor.line);
                    if self.sel.cursor.col > len {
                        self.sel.cursor.col = len;
                    }
                    self.sel.anchor = self.sel.cursor;
                }
                self.desired_col = None;
                self.find_matches.clear();
                self.find_re = None;
                self.find_current = None;
                self.renderer.force_full_redraw();
                self.set_status("Reloaded".to_string());
            }
            Err(e) => self.set_status(format!("Reload failed: {}", e)),
        }
        self.reload_pending = false;
    }

    fn dismiss_reload(&mut self) {
        if let Some(ref name) = self.doc.filename {
            self.file_mtime = crate::file_io::file_mtime(std::path::Path::new(name));
        }
        self.reload_pending = false;
        self.status_msg.clear();
        self.status_time = None;
    }

    fn save_file(&mut self) {
        if self.doc.filename.is_some() {
            let path = self.doc.filename.clone().unwrap();
            let path_ref = std::path::Path::new(&path);

            // mkdir -p for parent directory
            if let Some(parent) = path_ref.parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                match std::fs::create_dir_all(parent) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                        self.start_sudo_save();
                        return;
                    }
                    Err(e) => {
                        self.set_status(format!("Error creating dirs: {}", e));
                        return;
                    }
                }
            }

            match crate::file_io::write_file(path_ref, &self.doc.buf.contents()) {
                Ok(()) => {
                    self.doc.dirty = false;
                    self.doc.seal_undo();
                    self.file_mtime = crate::file_io::file_mtime(path_ref);
                    crate::file_io::save_undo_history(path_ref, &self.doc.undo_stack);
                    self.set_status(format!("Saved {}", path));
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    self.start_sudo_save();
                }
                Err(e) => {
                    self.set_status(format!("Error saving: {}", e));
                }
            }
        } else {
            // Prompt for filename
            self.cmd_buf
                .open(CommandBufferMode::Prompt, "Save as: ", "");
        }
    }

    fn start_sudo_save(&mut self) {
        let pid = std::process::id();
        let tmp = format!("/tmp/e_sudo_{}", pid);
        let contents = self.doc.buf.contents();
        let cleaned = crate::file_io::clean_for_write(&contents);
        match std::fs::write(&tmp, &cleaned) {
            Ok(()) => {
                self.sudo_save_tmp = Some(tmp);
                let path = self.doc.filename.as_deref().unwrap_or("?");
                let prompt = format!("sudo password (to save {}): ", path);
                self.cmd_buf.open(CommandBufferMode::SudoSave, &prompt, "");
            }
            Err(e) => {
                self.set_status(format!("Error writing temp file: {}", e));
            }
        }
    }

    #[cfg(test)]
    fn test_text(&self) -> String {
        String::from_utf8_lossy(&self.doc.buf.contents()).to_string()
    }

    fn save_file_sudo(&mut self, password: &str) {
        let tmp = match self.sudo_save_tmp.take() {
            Some(t) => t,
            None => return,
        };
        let path = match self.doc.filename.clone() {
            Some(p) => p,
            None => return,
        };
        let path_ref = std::path::Path::new(&path);

        // mkdir -p via sudo if needed
        if let Some(parent) = path_ref.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            let status = Command::new("sudo")
                .args(["-S", "mkdir", "-p"])
                .arg(parent)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    if let Some(ref mut stdin) = child.stdin {
                        let _ = stdin.write_all(password.as_bytes());
                        let _ = stdin.write_all(b"\n");
                    }
                    child.wait()
                });
            match status {
                Ok(s) if !s.success() => {
                    let _ = std::fs::remove_file(&tmp);
                    self.set_status("sudo mkdir failed".to_string());
                    return;
                }
                Err(_) => {
                    let _ = std::fs::remove_file(&tmp);
                    self.set_status("sudo mkdir failed".to_string());
                    return;
                }
                _ => {}
            }
        }

        // cp via sudo
        let status = Command::new("sudo")
            .args(["-S", "cp"])
            .arg(&tmp)
            .arg(&path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(ref mut stdin) = child.stdin {
                    let _ = stdin.write_all(password.as_bytes());
                    let _ = stdin.write_all(b"\n");
                }
                child.wait()
            });

        let _ = std::fs::remove_file(&tmp);

        match status {
            Ok(s) if s.success() => {
                self.doc.dirty = false;
                self.doc.seal_undo();
                self.file_mtime = crate::file_io::file_mtime(path_ref);
                crate::file_io::save_undo_history(path_ref, &self.doc.undo_stack);
                self.set_status(format!("Saved {} (sudo)", path));
            }
            _ => {
                self.set_status("sudo save failed".to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termion::event::{Event, Key, MouseButton, MouseEvent};

    fn ed(text: &str) -> Editor {
        ed_impl(text, None)
    }

    fn ed_named(text: &str, name: &str) -> Editor {
        ed_impl(text, Some(name.to_string()))
    }

    fn ed_impl(text: &str, filename: Option<String>) -> Editor {
        let doc = Document::new(text.as_bytes().to_vec(), filename);
        Editor {
            doc,
            sel: Selection::caret(Pos::zero()),
            desired_col: None,
            view: View::new(80, 24),
            renderer: Renderer::new(),
            clipboard: Clipboard::internal_only(),
            commands: CommandRegistry::new(),
            keybindings: KeybindingTable::with_defaults(),
            cmd_buf: CommandBuffer::new(),
            ruler_on: true,
            status_msg: String::new(),
            status_time: None,
            running: true,
            quit_pending: false,
            last_click_time: None,
            last_click_pos: None,
            click_count: 0,
            dragging: false,
            find_pattern: String::new(),
            find_matches: Vec::new(),
            find_re: None,
            find_current: None,
            find_active: false,
            sudo_save_tmp: None,
            piped_stdin: false,
            file_mtime: None,
            reload_pending: false,
            status_left_cache: String::new(),
        }
    }

    // ========================================================================
    // Movement scenarios
    // ========================================================================

    #[test]
    fn test_move_up_down_with_desired_col_stickiness() {
        let mut e = ed("short\nlonger line here\nhi");
        // Move to end of "longer line here" (col 15)
        e.set_cursor(Pos::new(1, 15));
        e.move_up(); // line 0 is 5 chars, should clamp to 5
        assert_eq!(e.cursor(), Pos::new(0, 5));
        // desired_col should be 15 (sticky)
        e.move_down(); // back to line 1, col should restore to 15
        assert_eq!(e.cursor(), Pos::new(1, 15));
        e.move_down(); // line 2 is 2 chars, clamp to 2
        assert_eq!(e.cursor(), Pos::new(2, 2));
    }

    #[test]
    fn test_move_up_at_top() {
        let mut e = ed("hello");
        e.move_up();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn test_move_down_at_bottom() {
        let mut e = ed("hello");
        e.move_down();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn test_move_left_wraps_to_prev_line() {
        let mut e = ed("abc\ndef");
        e.set_cursor(Pos::new(1, 0));
        e.move_left();
        assert_eq!(e.cursor(), Pos::new(0, 3));
    }

    #[test]
    fn test_move_right_wraps_to_next_line() {
        let mut e = ed("abc\ndef");
        e.set_cursor(Pos::new(0, 3));
        e.move_right();
        assert_eq!(e.cursor(), Pos::new(1, 0));
    }

    #[test]
    fn test_move_left_collapses_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 2),
            cursor: Pos::new(0, 7),
        };
        e.move_left();
        assert_eq!(e.cursor(), Pos::new(0, 2));
        assert!(e.sel.is_empty());
    }

    #[test]
    fn test_move_right_collapses_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 2),
            cursor: Pos::new(0, 7),
        };
        e.move_right();
        assert_eq!(e.cursor(), Pos::new(0, 7));
        assert!(e.sel.is_empty());
    }

    #[test]
    fn test_home_end() {
        let mut e = ed("hello world");
        e.set_cursor(Pos::new(0, 5));
        e.move_home();
        assert_eq!(e.cursor(), Pos::new(0, 0));
        e.move_end();
        assert_eq!(e.cursor(), Pos::new(0, 11));
    }

    #[test]
    fn test_page_up_down() {
        // 80x24 terminal = 22 text rows
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.set_cursor(Pos::new(25, 0));
        e.page_up();
        assert_eq!(e.cursor().line, 3); // 25 - 22 = 3
        e.page_down();
        assert_eq!(e.cursor().line, 25);
    }

    #[test]
    fn test_indent_snap_left_right() {
        let mut e = ed("    hello"); // 4 spaces indent
        e.set_cursor(Pos::new(0, 4));
        e.move_left(); // should snap from 4 to 2
        assert_eq!(e.cursor().col, 2);
        e.move_right(); // should snap from 2 to 4
        assert_eq!(e.cursor().col, 4);
    }

    #[test]
    fn test_move_left_at_origin() {
        let mut e = ed("hello");
        e.move_left();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn test_move_right_at_end_of_last_line() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.move_right();
        assert_eq!(e.cursor(), Pos::new(0, 5));
    }

    // ========================================================================
    // Selection scenarios
    // ========================================================================

    #[test]
    fn test_shift_arrow_extends_selection() {
        let mut e = ed("hello");
        e.move_right_extend();
        e.move_right_extend();
        assert_eq!(e.sel.anchor, Pos::new(0, 0));
        assert_eq!(e.sel.cursor, Pos::new(0, 2));
        assert!(!e.sel.is_empty());
    }

    #[test]
    fn test_select_all() {
        let mut e = ed("hello\nworld");
        e.select_all();
        let (start, end) = e.sel.ordered();
        assert_eq!(start, Pos::new(0, 0));
        assert_eq!(end, Pos::new(1, 5));
    }

    #[test]
    fn test_select_word_at() {
        let mut e = ed("hello world");
        e.select_word_at(Pos::new(0, 7));
        let (start, end) = e.sel.ordered();
        assert_eq!(start, Pos::new(0, 6));
        assert_eq!(end, Pos::new(0, 11));
    }

    #[test]
    fn test_select_line_at() {
        let mut e = ed("hello\nworld\nfoo");
        e.select_line_at(1);
        let (start, end) = e.sel.ordered();
        assert_eq!(start, Pos::new(1, 0));
        assert_eq!(end, Pos::new(2, 0));
    }

    #[test]
    fn test_select_line_at_last_line() {
        let mut e = ed("hello\nworld");
        e.select_line_at(1);
        let (start, end) = e.sel.ordered();
        assert_eq!(start, Pos::new(1, 0));
        assert_eq!(end, Pos::new(1, 5));
    }

    #[test]
    fn test_select_above_below() {
        let mut e = ed("hello\nworld\nfoo");
        e.set_cursor(Pos::new(1, 3));
        e.select_above();
        assert_eq!(e.sel.cursor, Pos::new(0, 0));
        assert_eq!(e.sel.anchor, Pos::new(1, 3));

        e.set_cursor(Pos::new(1, 3));
        e.select_below();
        assert_eq!(e.sel.cursor, Pos::new(2, 3));
        assert_eq!(e.sel.anchor, Pos::new(1, 3));
    }

    #[test]
    fn test_delete_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 5),
            cursor: Pos::new(0, 11),
        };
        e.delete_selection();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_clear_selection() {
        let mut e = ed("hello");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.clear_selection();
        assert!(e.sel.is_empty());
        assert_eq!(e.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn test_shift_up_down_extend() {
        let mut e = ed("aaa\nbbb\nccc");
        e.set_cursor(Pos::new(1, 1));
        e.move_up_extend();
        assert_eq!(e.sel.anchor, Pos::new(1, 1));
        assert_eq!(e.sel.cursor, Pos::new(0, 1));
        e.move_down_extend();
        assert_eq!(e.sel.cursor, Pos::new(1, 1));
        e.move_down_extend();
        assert_eq!(e.sel.cursor, Pos::new(2, 1));
    }

    #[test]
    fn test_shift_left_right_extend() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 2));
        e.move_left_extend();
        assert_eq!(e.sel.cursor, Pos::new(0, 1));
        e.move_right_extend();
        assert_eq!(e.sel.cursor, Pos::new(0, 2));
    }

    // ========================================================================
    // Editing scenarios
    // ========================================================================

    #[test]
    fn test_insert_char() {
        let mut e = ed("hllo");
        e.set_cursor(Pos::new(0, 1));
        e.insert_char('e');
        assert_eq!(e.test_text(), "hello");
        assert_eq!(e.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn test_insert_char_replaces_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 5),
            cursor: Pos::new(0, 11),
        };
        e.insert_char('!');
        assert_eq!(e.test_text(), "hello!");
    }

    #[test]
    fn test_insert_tab_spaces() {
        let mut e = ed_named("hello", "test.rs");
        e.insert_tab();
        assert_eq!(e.test_text(), "  hello");
    }

    #[test]
    fn test_insert_tab_actual_tab_for_c_file() {
        let mut e = ed_named("hello", "test.c");
        e.insert_tab();
        assert_eq!(e.test_text(), "\thello");
    }

    #[test]
    fn test_tab_indents_selection() {
        let mut e = ed_named("aaa\nbbb\nccc", "test.rs");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(2, 3),
        };
        e.insert_tab();
        assert_eq!(e.test_text(), "  aaa\n  bbb\n  ccc");
    }

    #[test]
    fn test_insert_newline_with_auto_indent() {
        let mut e = ed("  hello");
        e.set_cursor(Pos::new(0, 7));
        e.insert_newline();
        assert_eq!(e.test_text(), "  hello\n  ");
        assert_eq!(e.cursor(), Pos::new(1, 2));
    }

    #[test]
    fn test_insert_newline_replaces_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 5),
            cursor: Pos::new(0, 11),
        };
        e.insert_newline();
        assert_eq!(e.test_text(), "hello\n");
    }

    #[test]
    fn test_backspace_basic() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.backspace();
        assert_eq!(e.test_text(), "hell");
    }

    #[test]
    fn test_backspace_joins_lines() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(1, 0));
        e.backspace();
        assert_eq!(e.test_text(), "helloworld");
    }

    #[test]
    fn test_backspace_indent_snap() {
        let mut e = ed("    x");
        e.set_cursor(Pos::new(0, 4));
        e.backspace(); // should snap from 4 to 2
        assert_eq!(e.test_text(), "  x");
    }

    #[test]
    fn test_backspace_deletes_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 5),
            cursor: Pos::new(0, 11),
        };
        e.backspace();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_backspace_at_origin_noop() {
        let mut e = ed("hello");
        e.backspace();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_delete_forward() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 0));
        e.delete_forward();
        assert_eq!(e.test_text(), "ello");
    }

    #[test]
    fn test_delete_forward_joins_lines() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(0, 5));
        e.delete_forward();
        assert_eq!(e.test_text(), "helloworld");
    }

    #[test]
    fn test_delete_forward_with_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.delete_forward();
        assert_eq!(e.test_text(), " world");
    }

    #[test]
    fn test_ctrl_backspace_word_delete() {
        let mut e = ed("hello world");
        e.set_cursor(Pos::new(0, 11));
        e.ctrl_backspace();
        assert_eq!(e.test_text(), "hello ");
    }

    #[test]
    fn test_ctrl_backspace_at_line_start() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(1, 0));
        e.ctrl_backspace();
        assert_eq!(e.test_text(), "helloworld");
    }

    #[test]
    fn test_ctrl_backspace_at_origin() {
        let mut e = ed("hello");
        e.ctrl_backspace();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_ctrl_backspace_with_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.ctrl_backspace();
        assert_eq!(e.test_text(), " world");
    }

    #[test]
    fn test_duplicate_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(0, 2));
        e.duplicate_line();
        assert_eq!(e.test_text(), "hello\nhello\nworld");
        assert_eq!(e.cursor(), Pos::new(1, 2));
    }

    // ========================================================================
    // Find/replace scenarios
    // ========================================================================

    #[test]
    fn test_find_highlights_smart_case() {
        let mut e = ed("Hello hello HELLO");
        e.update_find_highlights("hello");
        assert_eq!(e.find_matches.len(), 3); // case-insensitive (all lowercase pattern)
    }

    #[test]
    fn test_find_case_sensitive() {
        let mut e = ed("Hello hello HELLO");
        e.update_find_highlights("Hello");
        assert_eq!(e.find_matches.len(), 1); // uppercase in pattern → case-sensitive
    }

    #[test]
    fn test_find_invalid_regex() {
        let mut e = ed("hello [world");
        e.update_find_highlights("[invalid");
        assert!(e.find_matches.is_empty()); // invalid regex → no matches, no panic
    }

    #[test]
    fn test_find_empty_pattern() {
        let mut e = ed("hello");
        e.update_find_highlights("");
        assert!(e.find_matches.is_empty());
    }

    #[test]
    fn test_find_next_wraps_around() {
        let mut e = ed("aa bb aa");
        e.update_find_highlights("aa");
        assert_eq!(e.find_matches.len(), 2);
        e.find_active = true;
        // Position cursor past all matches
        e.set_cursor(Pos::new(0, 8));
        e.find_next();
        // wrapped around to first match (col 0..2)
        assert_eq!(e.find_current, Some((Pos::new(0, 0), Pos::new(0, 2))));
    }

    #[test]
    fn test_find_prev_wraps_around() {
        let mut e = ed("aa bb aa");
        e.update_find_highlights("aa");
        e.find_active = true;
        e.set_cursor(Pos::new(0, 0));
        e.find_prev();
        // wrapped around to last match (col 6..8)
        assert_eq!(e.find_current, Some((Pos::new(0, 6), Pos::new(0, 8))));
    }

    #[test]
    fn test_find_next_from_submit() {
        let mut e = ed("hello world hello");
        e.find_next_from_submit("hello");
        assert!(e.find_active);
        assert_eq!(e.find_matches.len(), 2);
        assert!(e.find_current.is_some());
    }

    #[test]
    fn test_find_next_from_submit_no_matches() {
        let mut e = ed("hello world");
        e.find_next_from_submit("xyz");
        assert!(!e.find_active);
        assert!(e.status_msg.contains("no matches"));
    }

    #[test]
    fn test_exit_find_mode_selects_match() {
        let mut e = ed("hello world hello");
        e.find_next_from_submit("hello");
        e.exit_find_mode();
        assert!(!e.find_active);
        assert!(e.find_matches.is_empty());
        // Selection should cover the match
        assert!(!e.sel.is_empty());
    }

    #[test]
    fn test_replace_all_whole_file() {
        let mut e = ed("foo bar foo");
        e.replace_all("foo", "baz");
        assert_eq!(e.test_text(), "baz bar baz");
        assert!(e.status_msg.contains("2"));
    }

    #[test]
    fn test_replace_all_in_selection() {
        let mut e = ed("foo bar foo");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 3),
        };
        e.replace_all("foo", "baz");
        assert_eq!(e.test_text(), "baz bar foo");
    }

    #[test]
    fn test_replace_all_no_matches() {
        let mut e = ed("hello world");
        e.replace_all("xyz", "abc");
        assert!(e.status_msg.contains("0"));
    }

    #[test]
    fn test_replace_all_invalid_regex() {
        let mut e = ed("hello");
        e.replace_all("[invalid", "x");
        assert!(e.status_msg.contains("Invalid regex"));
    }

    // ========================================================================
    // Command dispatch scenarios
    // ========================================================================

    #[test]
    fn test_goto_line_in_range() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.goto_line(25);
        assert_eq!(e.cursor().line, 24); // goto is 1-indexed
    }

    #[test]
    fn test_goto_line_zero() {
        let mut e = ed("hello\nworld");
        e.goto_line(0);
        assert_eq!(e.cursor().line, 0);
    }

    #[test]
    fn test_goto_line_beyond_end() {
        let mut e = ed("hello\nworld");
        e.goto_line(999);
        assert_eq!(e.cursor().line, 1); // clamped to last line
    }

    #[test]
    fn test_goto_top_end() {
        let mut e = ed("hello\nworld\nfoo");
        e.set_cursor(Pos::new(1, 2));
        e.goto_top();
        assert_eq!(e.cursor(), Pos::new(0, 0));
        e.goto_end();
        assert_eq!(e.cursor(), Pos::new(2, 3));
    }

    #[test]
    fn test_kill_line_middle() {
        let mut e = ed("aaa\nbbb\nccc");
        e.set_cursor(Pos::new(1, 1));
        e.kill_line();
        assert_eq!(e.test_text(), "aaa\nccc");
    }

    #[test]
    fn test_kill_line_last() {
        let mut e = ed("aaa\nbbb");
        e.set_cursor(Pos::new(1, 0));
        e.kill_line();
        assert_eq!(e.test_text(), "aaa\n");
    }

    #[test]
    fn test_execute_command_goto() {
        let mut e = ed("aaa\nbbb\nccc");
        e.execute_command("goto 2");
        assert_eq!(e.cursor().line, 1);
    }

    #[test]
    fn test_execute_command_ruler_toggle() {
        let mut e = ed("hello");
        assert!(e.ruler_on);
        e.execute_command("ruler");
        assert!(!e.ruler_on);
        e.execute_command("ruler");
        assert!(e.ruler_on);
    }

    #[test]
    fn test_execute_command_quit() {
        let mut e = ed("hello");
        e.execute_command("quit");
        assert!(!e.running);
    }

    #[test]
    fn test_execute_command_unknown() {
        let mut e = ed("hello");
        e.execute_command("foobar");
        assert!(e.status_msg.contains("Unknown"));
    }

    #[test]
    fn test_execute_command_replaceall() {
        let mut e = ed("foo bar foo");
        e.execute_command("replaceall foo baz");
        assert_eq!(e.test_text(), "baz bar baz");
    }

    #[test]
    fn test_execute_command_comment() {
        let mut e = ed_named("hello", "test.rs");
        e.execute_command("comment");
        assert_eq!(e.test_text(), "// hello");
    }

    #[test]
    fn test_execute_command_comment_on() {
        let mut e = ed_named("// hello", "test.rs");
        e.execute_command("comment on");
        // Already commented — idempotent, skips already-commented lines
        assert_eq!(e.test_text(), "// hello");
    }

    #[test]
    fn test_execute_command_comment_on_uncommented() {
        let mut e = ed_named("hello", "test.rs");
        e.execute_command("comment on");
        assert_eq!(e.test_text(), "// hello");
    }

    #[test]
    fn test_execute_command_comment_off() {
        let mut e = ed_named("// hello", "test.rs");
        e.execute_command("comment off");
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_execute_command_comment_off_uncommented() {
        let mut e = ed_named("hello", "test.rs");
        e.execute_command("comment off");
        // Already uncommented, "off" tries to remove but nothing to remove
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_execute_command_selectall() {
        let mut e = ed("hello\nworld");
        e.execute_command("selectall");
        assert!(!e.sel.is_empty());
        let (start, end) = e.sel.ordered();
        assert_eq!(start, Pos::zero());
        assert_eq!(end.line, 1);
    }

    #[test]
    fn test_complete_command_single_match() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "rul");
        e.complete_command();
        assert_eq!(e.cmd_buf.input, "ruler");
        assert!(e.cmd_buf.completions.is_empty());
    }

    #[test]
    fn test_complete_command_multiple_matches() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "q");
        e.complete_command();
        assert_eq!(e.cmd_buf.completions.len(), 2); // "q" and "quit"
    }

    #[test]
    fn test_complete_command_no_matches() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "xyz");
        e.complete_command();
        assert!(e.cmd_buf.completions.is_empty());
    }

    #[test]
    fn test_complete_command_empty_shows_all() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "");
        e.complete_command();
        assert!(!e.cmd_buf.completions.is_empty());
    }

    // ========================================================================
    // handle_cmd_result scenarios
    // ========================================================================

    #[test]
    fn test_handle_cmd_result_submit_find() {
        let mut e = ed("hello world hello");
        e.handle_cmd_result(
            CommandBufferMode::Find,
            CommandBufferResult::Submit("hello".to_string()),
        );
        assert!(e.find_active);
    }

    #[test]
    fn test_handle_cmd_result_submit_goto() {
        let mut e = ed("aaa\nbbb\nccc");
        e.handle_cmd_result(
            CommandBufferMode::Goto,
            CommandBufferResult::Submit("2".to_string()),
        );
        assert_eq!(e.cursor().line, 1);
    }

    #[test]
    fn test_handle_cmd_result_submit_prompt() {
        let dir = std::env::temp_dir().join("e_test_cmd_prompt");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let mut e = ed("hello");
        e.handle_cmd_result(
            CommandBufferMode::Prompt,
            CommandBufferResult::Submit(path.to_str().unwrap().to_string()),
        );
        assert_eq!(e.doc.filename.as_deref(), Some(path.to_str().unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_handle_cmd_result_cancel_find() {
        let mut e = ed("hello");
        e.find_matches = vec![(Pos::new(0, 0), Pos::new(0, 5))];
        e.handle_cmd_result(CommandBufferMode::Find, CommandBufferResult::Cancel);
        assert!(e.find_matches.is_empty());
    }

    #[test]
    fn test_handle_cmd_result_cancel_sudo() {
        let mut e = ed("hello");
        e.sudo_save_tmp = Some("/tmp/nonexistent_test_file".to_string());
        e.handle_cmd_result(CommandBufferMode::SudoSave, CommandBufferResult::Cancel);
        assert!(e.sudo_save_tmp.is_none());
        assert!(e.status_msg.contains("cancelled"));
    }

    #[test]
    fn test_handle_cmd_result_changed_find() {
        let mut e = ed("hello world hello");
        e.handle_cmd_result(
            CommandBufferMode::Find,
            CommandBufferResult::Changed("hello".to_string()),
        );
        assert_eq!(e.find_matches.len(), 2);
    }

    #[test]
    fn test_handle_cmd_result_tab_complete() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "rul");
        e.handle_cmd_result(CommandBufferMode::Command, CommandBufferResult::TabComplete);
        assert_eq!(e.cmd_buf.input, "ruler");
    }

    #[test]
    fn test_handle_cmd_result_continue_noop() {
        let mut e = ed("hello");
        e.handle_cmd_result(CommandBufferMode::Command, CommandBufferResult::Continue);
        // Should not change anything
        assert_eq!(e.test_text(), "hello");
    }

    // ========================================================================
    // Event/key handling scenarios
    // ========================================================================

    #[test]
    fn test_handle_event_dispatches_key() {
        let mut e = ed("hello");
        e.handle_event(Event::Key(Key::Char('x')));
        assert_eq!(e.test_text(), "xhello");
    }

    #[test]
    fn test_handle_event_mouse_ignored_when_cmd_active() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "");
        e.handle_event(Event::Mouse(MouseEvent::Press(MouseButton::Left, 1, 1)));
        // Mouse should be ignored when cmd_buf is active
        assert!(e.cmd_buf.active);
    }

    #[test]
    fn test_handle_event_unsupported_ctrl_shift_up() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(1, 3));
        e.handle_event(Event::Unsupported(CTRL_SHIFT_UP.to_vec()));
        assert_eq!(e.sel.cursor, Pos::new(0, 0));
    }

    #[test]
    fn test_handle_event_unsupported_ctrl_shift_down() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(0, 2));
        e.handle_event(Event::Unsupported(CTRL_SHIFT_DOWN.to_vec()));
        assert_eq!(e.sel.cursor, Pos::new(1, 5));
    }

    #[test]
    fn test_quit_clean_buffer() {
        let mut e = ed("hello");
        e.try_quit();
        assert!(!e.running);
    }

    #[test]
    fn test_quit_dirty_confirms() {
        let mut e = ed("hello");
        e.doc.dirty = true;
        e.try_quit();
        assert!(e.running);
        assert!(e.quit_pending);
        assert!(e.status_msg.contains("Save changes"));
    }

    #[test]
    fn test_quit_dirty_then_y() {
        let dir = std::env::temp_dir().join("e_test_quit_y");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut e = ed_named("hello", path.to_str().unwrap());
        e.doc.dirty = true;
        e.try_quit();
        e.handle_key(Key::Char('y'));
        assert!(!e.running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_quit_scratch_dirty_y_then_save_as() {
        // Regression: quit on a scratch buffer (no filename) with y should open
        // the save-as prompt and only quit after the filename is confirmed —
        // not immediately exit without writing any file.
        let dir = std::env::temp_dir().join("e_test_quit_scratch");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("out.txt");
        let mut e = ed("hello");
        e.doc.dirty = true;
        e.try_quit();
        // Pressing 'y' on a scratch buffer must open the save-as prompt, not quit.
        e.handle_key(Key::Char('y'));
        assert!(e.running, "editor must not quit before filename is given");
        assert!(e.cmd_buf.active, "save-as prompt must be open");
        assert!(
            e.quit_pending,
            "quit_pending must stay true until save completes"
        );
        // Submit the filename — this should save and quit.
        let path_str = path.to_str().unwrap().to_string();
        for ch in path_str.chars() {
            e.handle_cmd_key(Key::Char(ch));
        }
        e.handle_cmd_key(Key::Char('\n'));
        assert!(!e.running, "editor must quit after filename confirmed");
        assert!(path.exists(), "file must have been written to disk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_quit_dirty_then_n() {
        let mut e = ed("hello");
        e.doc.dirty = true;
        e.try_quit();
        e.handle_key(Key::Char('n'));
        assert!(!e.running);
    }

    #[test]
    fn test_quit_dirty_then_cancel() {
        let mut e = ed("hello");
        e.doc.dirty = true;
        e.try_quit();
        e.handle_key(Key::Esc);
        assert!(e.running);
        assert!(!e.quit_pending);
    }

    #[test]
    fn test_find_nav_up_down() {
        let mut e = ed("aa bb aa");
        e.find_next_from_submit("aa");
        assert!(e.find_active);
        let first = e.find_current;
        e.handle_key(Key::Down);
        let second = e.find_current;
        // moved forward to a different match
        assert_ne!(first, second);
        e.handle_key(Key::Up);
        // moved back to (or past) the original
        assert_ne!(e.find_current, second);
    }

    #[test]
    fn test_find_nav_esc_clears() {
        let mut e = ed("aa bb aa");
        e.find_next_from_submit("aa");
        e.handle_key(Key::Esc);
        assert!(!e.find_active);
        assert!(e.sel.is_empty());
    }

    #[test]
    fn test_find_nav_other_key_exits_and_processes() {
        let mut e = ed("aa bb");
        e.find_next_from_submit("aa");
        assert!(e.find_active);
        e.handle_key(Key::Char('x'));
        assert!(!e.find_active);
        // 'x' should have been processed as an insert
        assert!(e.test_text().contains('x'));
    }

    #[test]
    fn test_esc_clears_selection_and_matches() {
        let mut e = ed("hello");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.find_matches = vec![(Pos::new(0, 0), Pos::new(0, 5))];
        e.handle_key(Key::Esc);
        assert!(e.sel.is_empty());
        assert!(e.find_matches.is_empty());
    }

    #[test]
    fn test_keybinding_action_dispatch() {
        let mut e = ed("hello");
        // Ctrl+a should select all
        e.handle_key(Key::Ctrl('a'));
        assert!(!e.sel.is_empty());
    }

    #[test]
    fn test_handle_cmd_key_dispatches() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "");
        e.handle_cmd_key(Key::Char('a'));
        assert_eq!(e.cmd_buf.input, "a");
    }

    // ========================================================================
    // Mouse scenarios
    // ========================================================================

    #[test]
    fn test_mouse_single_click() {
        let mut e = ed("hello\nworld");
        e.mouse_press(6, 2); // col 5, row 1 (1-indexed terminal coords)
        assert_eq!(e.cursor().line, 1);
    }

    #[test]
    fn test_mouse_drag() {
        let mut e = ed("hello world");
        e.mouse_press(3, 1); // start drag
        assert!(e.dragging);
        e.mouse_drag(8, 1);
        assert_ne!(e.sel.anchor, e.sel.cursor);
    }

    #[test]
    fn test_mouse_release() {
        let mut e = ed("hello");
        e.dragging = true;
        e.handle_mouse(MouseEvent::Release(0, 0));
        assert!(!e.dragging);
    }

    #[test]
    fn test_mouse_scroll_up_down() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.view.scroll_line = 10;
        e.set_cursor(Pos::new(15, 0));
        e.scroll_down();
        assert!(e.view.scroll_line > 10);
        let prev = e.view.scroll_line;
        e.scroll_up();
        assert!(e.view.scroll_line < prev);
    }

    #[test]
    fn test_scroll_up_at_top() {
        let mut e = ed("hello\nworld");
        e.scroll_up();
        assert_eq!(e.view.scroll_line, 0);
    }

    #[test]
    fn test_scroll_down_at_bottom() {
        let mut e = ed("hello");
        e.scroll_down();
        assert_eq!(e.view.scroll_line, 0);
    }

    #[test]
    fn test_screen_to_buffer_pos_normal() {
        let mut e = ed("hello\nworld");
        let pos = e.screen_to_buffer_pos(5, 1); // col 4, row 0
        assert_eq!(pos.line, 0);
    }

    #[test]
    fn test_screen_to_buffer_pos_below_content() {
        let mut e = ed("hello");
        let pos = e.screen_to_buffer_pos(1, 20); // way below
        assert_eq!(pos.line, 0);
        assert_eq!(pos.col, 5);
    }

    #[test]
    fn test_clamp_cursor_to_viewport() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.set_cursor(Pos::new(0, 0));
        e.view.scroll_line = 10;
        let gw = gutter_width(e.doc.buf.line_count());
        let tc = e.view.text_cols(gw);
        e.clamp_cursor_to_viewport(gw, tc);
        // Cursor should be moved into viewport
        assert!(e.cursor().line >= e.view.scroll_line);
    }

    #[test]
    fn test_handle_event_mouse_exits_find_active() {
        let mut e = ed("hello world");
        e.find_active = true;
        e.find_matches = vec![(Pos::new(0, 0), Pos::new(0, 5))];
        e.handle_event(Event::Mouse(MouseEvent::Press(MouseButton::Left, 1, 1)));
        assert!(!e.find_active);
    }

    // ========================================================================
    // Clipboard/undo scenarios
    // ========================================================================

    #[test]
    fn test_copy_paste_workflow() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.copy();
        e.set_cursor(Pos::new(0, 11));
        e.paste();
        assert_eq!(e.test_text(), "hello worldhello");
    }

    #[test]
    fn test_cut() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.cut();
        assert_eq!(e.test_text(), " world");
        e.paste();
        assert_eq!(e.test_text(), "hello world");
    }

    #[test]
    fn test_paste_text_replaces_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 6),
            cursor: Pos::new(0, 11),
        };
        e.paste_text("earth");
        assert_eq!(e.test_text(), "hello earth");
    }

    #[test]
    fn test_paste_empty_noop() {
        let mut e = ed("hello");
        e.paste_text("");
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_copy_empty_selection_noop() {
        let mut e = ed("hello");
        e.copy();
        // Internal clipboard should still be empty
        assert_eq!(e.clipboard.paste(), "");
    }

    #[test]
    fn test_undo_redo_chain() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.doc.seal_undo();
        e.insert_char('!');
        e.doc.seal_undo();
        assert_eq!(e.test_text(), "hello!");
        e.undo();
        assert_eq!(e.test_text(), "hello");
        e.redo();
        assert_eq!(e.test_text(), "hello!");
    }

    // ========================================================================
    // Comment/dedent scenarios
    // ========================================================================

    #[test]
    fn test_toggle_comment_on_rs_file() {
        let mut e = ed_named("hello\nworld", "test.rs");
        e.set_cursor(Pos::new(0, 0));
        e.toggle_comment();
        assert_eq!(e.test_text(), "// hello\nworld");
    }

    #[test]
    fn test_toggle_comment_off_rs_file() {
        let mut e = ed_named("// hello\nworld", "test.rs");
        e.set_cursor(Pos::new(0, 0));
        e.toggle_comment();
        assert_eq!(e.test_text(), "hello\nworld");
    }

    #[test]
    fn test_toggle_comment_no_language() {
        let mut e = ed("hello");
        e.toggle_comment();
        assert!(e.status_msg.contains("No language"));
    }

    #[test]
    fn test_toggle_comment_selection() {
        let mut e = ed_named("aaa\nbbb\nccc", "test.rs");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(2, 3),
        };
        e.toggle_comment();
        assert_eq!(e.test_text(), "// aaa\n// bbb\n// ccc");
    }

    #[test]
    fn test_dedent_spaces() {
        let mut e = ed("  hello");
        e.set_cursor(Pos::new(0, 2));
        e.dedent();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_dedent_tab() {
        let mut e = ed("\thello");
        e.set_cursor(Pos::new(0, 1));
        e.dedent();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_dedent_no_indent() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 0));
        e.dedent();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_indent_selection_skips_blank_lines() {
        let mut e = ed_named("aaa\n\nbbb", "test.rs");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(2, 3),
        };
        e.indent_selection();
        assert_eq!(e.test_text(), "  aaa\n\n  bbb");
    }

    // ========================================================================
    // File I/O scenarios
    // ========================================================================

    #[test]
    fn test_strip_trailing_whitespace() {
        let mut e = ed("hello   \nworld  ");
        e.set_cursor(Pos::new(0, 8));
        e.strip_trailing_whitespace();
        assert_eq!(e.test_text(), "hello\nworld");
        // Cursor should be clamped
        assert!(e.cursor().col <= 5);
    }

    #[test]
    fn test_save_no_filename_opens_prompt() {
        let mut e = ed("hello");
        e.save_file();
        assert!(e.cmd_buf.active);
        assert_eq!(e.cmd_buf.mode, CommandBufferMode::Prompt);
    }

    #[test]
    fn test_save_to_temp_file() {
        let dir = std::env::temp_dir().join("e_test_save");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let mut e = ed_named("hello world", path.to_str().unwrap());
        e.doc.dirty = true;
        e.save_file();
        assert!(!e.doc.dirty);
        assert!(e.status_msg.contains("Saved"));
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hello world\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_status_left_scratch() {
        let e = ed("hello");
        let left = e.status_left("Text");
        assert!(left.contains("[scratch]"));
    }

    #[test]
    fn test_status_left_named_clean() {
        let e = ed_named("hello", "test.rs");
        let lang_name = e
            .doc
            .filename
            .as_deref()
            .and_then(language::detect)
            .map(|l| l.name)
            .unwrap_or("Text");
        let left = e.status_left(lang_name);
        assert!(left.contains("test.rs"));
        assert!(left.contains("Rust"));
        assert!(!left.contains('*'));
    }

    #[test]
    fn test_status_left_named_dirty() {
        let mut e = ed_named("hello", "test.rs");
        e.doc.dirty = true;
        let lang_name = e
            .doc
            .filename
            .as_deref()
            .and_then(language::detect)
            .map(|l| l.name)
            .unwrap_or("Text");
        let left = e.status_left(lang_name);
        assert!(left.contains("test.rs*"));
    }

    #[test]
    fn test_status_right() {
        let right = Editor::status_right();
        assert!(right.contains("e v"));
    }

    // ========================================================================
    // Standalone functions
    // ========================================================================

    #[test]
    fn test_common_prefix_basic() {
        assert_eq!(common_prefix(&["abc", "abd", "abe"]), "ab");
    }

    #[test]
    fn test_common_prefix_empty() {
        assert_eq!(common_prefix(&[]), "");
    }

    #[test]
    fn test_common_prefix_single() {
        assert_eq!(common_prefix(&["hello"]), "hello");
    }

    #[test]
    fn test_common_prefix_no_common() {
        assert_eq!(common_prefix(&["abc", "xyz"]), "");
    }

    #[test]
    fn test_is_paste_start_end() {
        assert!(is_paste_start(&Event::Unsupported(PASTE_START.to_vec())));
        assert!(is_paste_end(&Event::Unsupported(PASTE_END.to_vec())));
        assert!(!is_paste_start(&Event::Key(Key::Char('a'))));
        assert!(!is_paste_end(&Event::Key(Key::Char('a'))));
    }

    #[test]
    fn test_cursor_display_col_with_tabs() {
        let mut e = ed("\thello");
        e.set_cursor(Pos::new(0, 1));
        assert_eq!(e.cursor_display_col(), 2); // tab = 2 display cols
    }

    #[test]
    fn test_cursor_display_col_no_tabs() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 3));
        assert_eq!(e.cursor_display_col(), 3);
    }

    #[test]
    fn test_find_matching_bracket() {
        let mut e = ed("(hello)");
        e.set_cursor(Pos::new(0, 0));
        let result = e.find_matching_bracket();
        assert!(result.is_some());
        let (cursor, match_pos) = result.unwrap();
        assert_eq!(cursor, Pos::new(0, 0));
        assert_eq!(match_pos, Pos::new(0, 6));
    }

    #[test]
    fn test_find_matching_bracket_none() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 0));
        assert!(e.find_matching_bracket().is_none());
    }

    #[test]
    fn test_center_view_on_line() {
        let text = (0..100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.center_view_on_line(50);
        // Scroll should be near line 50 - half of text_rows
        assert!(e.view.scroll_line <= 50);
        assert!(e.view.scroll_line + e.view.text_rows() > 50);
    }

    // ========================================================================
    // draw() smoke test
    // ========================================================================

    #[test]
    fn test_draw_does_not_panic() {
        let mut e = ed_named("hello\nworld", "test.rs");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.find_matches = vec![(Pos::new(0, 0), Pos::new(0, 5))];
        e.find_active = true;
        let mut output = Vec::new();
        e.draw(&mut output).unwrap();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_draw_with_cmd_buf_active() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Find, "find: ", "test");
        e.cmd_buf.completions = vec!["comp1".to_string()];
        let mut output = Vec::new();
        e.draw(&mut output).unwrap();
        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("find: test"));
    }

    #[test]
    fn test_draw_ruler_off() {
        let mut e = ed("hello");
        e.ruler_on = false;
        let mut output = Vec::new();
        e.draw(&mut output).unwrap();
        assert!(!output.is_empty());
    }

    // ========================================================================
    // handle_key non-configurable keys
    // ========================================================================

    #[test]
    fn test_handle_key_delete() {
        let mut e = ed("hello");
        e.handle_key(Key::Delete);
        assert_eq!(e.test_text(), "ello");
    }

    #[test]
    fn test_handle_key_backtab() {
        let mut e = ed("  hello");
        e.set_cursor(Pos::new(0, 2));
        e.handle_key(Key::BackTab);
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_handle_key_newline() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.handle_key(Key::Char('\n'));
        assert_eq!(e.test_text(), "hello\n");
    }

    #[test]
    fn test_handle_key_char() {
        let mut e = ed("");
        e.handle_key(Key::Char('a'));
        e.handle_key(Key::Char('b'));
        assert_eq!(e.test_text(), "ab");
    }

    #[test]
    fn test_handle_key_unknown_does_nothing() {
        let mut e = ed("hello");
        e.handle_key(Key::F(12));
        assert_eq!(e.test_text(), "hello");
    }

    // ========================================================================
    // keybinding dispatch
    // ========================================================================

    #[test]
    fn test_keybinding_save() {
        let dir = std::env::temp_dir().join("e_test_kb_save");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut e = ed_named("hello", path.to_str().unwrap());
        e.doc.dirty = true;
        e.handle_key(Key::Ctrl('s'));
        assert!(!e.doc.dirty);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keybinding_undo_redo() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.insert_char('!');
        e.doc.seal_undo();
        e.handle_key(Key::Ctrl('z'));
        assert_eq!(e.test_text(), "hello");
        e.handle_key(Key::Ctrl('y'));
        assert_eq!(e.test_text(), "hello!");
    }

    #[test]
    fn test_keybinding_copy_paste() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('a')); // select all
        e.handle_key(Key::Ctrl('c')); // copy
        e.set_cursor(Pos::new(0, 5));
        e.handle_key(Key::Ctrl('v')); // paste
        assert_eq!(e.test_text(), "hellohello");
    }

    #[test]
    fn test_keybinding_cut() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('a'));
        e.handle_key(Key::Ctrl('x'));
        assert_eq!(e.test_text(), "");
    }

    #[test]
    fn test_keybinding_kill_line() {
        let mut e = ed("hello\nworld");
        e.handle_key(Key::Ctrl('k'));
        assert_eq!(e.test_text(), "world");
    }

    #[test]
    fn test_keybinding_goto_top_end() {
        let mut e = ed("aaa\nbbb\nccc");
        e.set_cursor(Pos::new(1, 1));
        e.handle_key(Key::Ctrl('t'));
        assert_eq!(e.cursor(), Pos::new(0, 0));
        e.handle_key(Key::Ctrl('g'));
        assert_eq!(e.cursor(), Pos::new(2, 3));
    }

    #[test]
    fn test_keybinding_toggle_ruler() {
        let mut e = ed("hello");
        assert!(e.ruler_on);
        e.handle_key(Key::Ctrl('r'));
        assert!(!e.ruler_on);
    }

    #[test]
    fn test_keybinding_command_palette() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('p'));
        assert!(e.cmd_buf.active);
        assert_eq!(e.cmd_buf.mode, CommandBufferMode::Command);
    }

    #[test]
    fn test_keybinding_goto_line() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('l'));
        assert!(e.cmd_buf.active);
        assert_eq!(e.cmd_buf.mode, CommandBufferMode::Goto);
    }

    #[test]
    fn test_keybinding_find() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('f'));
        assert!(e.cmd_buf.active);
        assert_eq!(e.cmd_buf.mode, CommandBufferMode::Find);
    }

    #[test]
    fn test_keybinding_find_prefills_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 6),
            cursor: Pos::new(0, 11),
        };
        e.handle_key(Key::Ctrl('f'));
        assert_eq!(e.cmd_buf.input, "world");
    }

    #[test]
    fn test_keybinding_ctrl_backspace() {
        let mut e = ed("hello world");
        e.set_cursor(Pos::new(0, 11));
        e.handle_key(Key::Ctrl('h'));
        assert_eq!(e.test_text(), "hello ");
    }

    #[test]
    fn test_keybinding_toggle_comment() {
        let mut e = ed_named("hello", "test.rs");
        e.handle_key(Key::Ctrl('d'));
        assert_eq!(e.test_text(), "// hello");
    }

    #[test]
    fn test_keybinding_duplicate_line() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('j'));
        assert_eq!(e.test_text(), "hello\nhello");
    }

    #[test]
    fn test_keybinding_select_word() {
        let mut e = ed("hello world");
        e.set_cursor(Pos::new(0, 7));
        e.handle_key(Key::Ctrl('w'));
        assert!(!e.sel.is_empty());
    }

    // ========================================================================
    // desired_col reset
    // ========================================================================

    #[test]
    fn test_desired_col_reset_on_non_vertical_movement() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(0, 3));
        e.handle_key(Key::Down); // sets desired_col
        assert!(e.desired_col.is_some());
        e.handle_key(Key::Char('x')); // non-vertical key should clear it
        assert!(e.desired_col.is_none());
    }

    // ========================================================================
    // mouse double/triple click
    // ========================================================================

    #[test]
    fn test_select_word_at_empty_line() {
        let mut e = ed("hello\n\nworld");
        e.select_word_at(Pos::new(1, 0));
        // Empty line should not select anything (early return)
        assert!(e.sel.is_empty());
    }

    #[test]
    fn test_select_line_at_out_of_bounds() {
        let mut e = ed("hello");
        e.select_line_at(999);
        assert!(e.sel.is_empty());
    }

    // ========================================================================
    // set_status
    // ========================================================================

    #[test]
    fn test_set_status() {
        let mut e = ed("hello");
        e.set_status("test message".to_string());
        assert_eq!(e.status_msg, "test message");
        assert!(e.status_time.is_some());
    }

    // ========================================================================
    // handle_mouse dispatch
    // ========================================================================

    #[test]
    fn test_handle_mouse_wheel_up() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.view.scroll_line = 10;
        e.set_cursor(Pos::new(15, 0));
        e.handle_mouse(MouseEvent::Press(MouseButton::WheelUp, 1, 1));
        assert!(e.view.scroll_line < 10);
    }

    #[test]
    fn test_handle_mouse_wheel_down() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.set_cursor(Pos::new(5, 0));
        e.handle_mouse(MouseEvent::Press(MouseButton::WheelDown, 1, 1));
        assert!(e.view.scroll_line > 0);
    }

    #[test]
    fn test_handle_mouse_other_button_noop() {
        let mut e = ed("hello");
        e.handle_mouse(MouseEvent::Press(MouseButton::Middle, 1, 1));
        assert_eq!(e.test_text(), "hello");
    }

    // ========================================================================
    // save_undo_if_named
    // ========================================================================

    #[test]
    fn test_save_undo_if_named_no_file() {
        let mut e = ed("hello");
        e.save_undo_if_named(); // should not panic
    }

    // ========================================================================
    // handle_event dispatches cmd_key when cmd_buf active
    // ========================================================================

    #[test]
    fn test_handle_event_dispatches_cmd_key() {
        let mut e = ed("hello");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "");
        e.handle_event(Event::Key(Key::Char('x')));
        assert_eq!(e.cmd_buf.input, "x");
    }

    #[test]
    fn test_unsupported_ignored_when_cmd_active() {
        let mut e = ed("hello\nworld");
        e.cmd_buf.open(CommandBufferMode::Command, "> ", "");
        e.handle_event(Event::Unsupported(CTRL_SHIFT_UP.to_vec()));
        // Should be ignored, cursor unchanged
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    // ========================================================================
    // find_next / find_prev with empty matches
    // ========================================================================

    #[test]
    fn test_find_next_empty_matches() {
        let mut e = ed("hello");
        e.find_next(); // should not panic
    }

    #[test]
    fn test_find_prev_empty_matches() {
        let mut e = ed("hello");
        e.find_prev(); // should not panic
    }

    // ========================================================================
    // kill_line empty buffer
    // ========================================================================

    #[test]
    fn test_kill_line_single_line() {
        let mut e = ed("hello");
        e.kill_line();
        assert_eq!(e.test_text(), "");
    }

    // ========================================================================
    // shift+arrow
    // ========================================================================

    #[test]
    fn test_shift_arrows_dispatch() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(0, 2));
        e.handle_key(Key::ShiftRight);
        assert_eq!(e.sel.cursor, Pos::new(0, 3));
        e.handle_key(Key::ShiftLeft);
        assert_eq!(e.sel.cursor, Pos::new(0, 2));
        e.handle_key(Key::ShiftDown);
        assert_eq!(e.sel.cursor, Pos::new(1, 2));
        e.handle_key(Key::ShiftUp);
        assert_eq!(e.sel.cursor, Pos::new(0, 2));
    }

    // ========================================================================
    // page up/down dispatch
    // ========================================================================

    #[test]
    fn test_page_up_down_dispatch() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.set_cursor(Pos::new(25, 0));
        e.handle_key(Key::PageUp);
        assert!(e.cursor().line < 25);
        e.handle_key(Key::PageDown);
        assert!(e.cursor().line > 3);
    }

    // ========================================================================
    // movement dispatch via handle_key
    // ========================================================================

    #[test]
    fn test_arrow_keys_dispatch() {
        let mut e = ed("hello\nworld");
        e.handle_key(Key::Down);
        assert_eq!(e.cursor().line, 1);
        e.handle_key(Key::Up);
        assert_eq!(e.cursor().line, 0);
        e.handle_key(Key::Right);
        assert_eq!(e.cursor().col, 1);
        e.handle_key(Key::Left);
        assert_eq!(e.cursor().col, 0);
        e.handle_key(Key::End);
        assert_eq!(e.cursor().col, 5);
        e.handle_key(Key::Home);
        assert_eq!(e.cursor().col, 0);
    }

    // ========================================================================
    // Tab key dispatch
    // ========================================================================

    #[test]
    fn test_tab_key_dispatch() {
        let mut e = ed_named("hello", "test.rs");
        e.handle_key(Key::Char('\t'));
        assert_eq!(e.test_text(), "  hello");
    }

    // ========================================================================
    // Coverage gap: scroll_up through wrapped prev line (lines 1071-1073)
    // ========================================================================

    #[test]
    fn test_scroll_up_through_wrapped_prev_line() {
        // Line 0 is very long (wraps many times), line 1 is short
        let long_line = "a".repeat(300);
        let text = format!("{}\nshort", long_line);
        let mut e = ed(&text);
        e.ruler_on = false;
        // Start scrolled at line 1
        e.view.scroll_line = 1;
        e.view.scroll_wrap = 0;
        e.set_cursor(Pos::new(1, 0));
        // Scroll up — should go into line 0's wraps
        e.scroll_up();
        assert_eq!(e.view.scroll_line, 0);
        assert!(e.view.scroll_wrap > 0); // should be partway through wraps
    }

    // ========================================================================
    // Coverage gap: scroll_down partial wrap (lines 1104-1105)
    // ========================================================================

    #[test]
    fn test_scroll_down_partial_wrap_advance() {
        // Single very long line that wraps many times
        // With 80 cols and no ruler, SCROLL_LINES=3 should advance by 3 wraps
        let long_line = "a".repeat(500);
        let text = format!("{}\nend", long_line);
        let mut e = ed(&text);
        e.ruler_on = false;
        e.view.scroll_line = 0;
        e.view.scroll_wrap = 0;
        e.set_cursor(Pos::new(0, 0));
        e.scroll_down();
        // Should have advanced through wraps within line 0
        assert_eq!(e.view.scroll_line, 0);
        assert_eq!(e.view.scroll_wrap, 3); // SCROLL_LINES = 3
    }

    // ========================================================================
    // Coverage gap: handle_key Save keybinding (line 478)
    // ========================================================================

    #[test]
    fn test_save_keybinding_no_filename() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('s'));
        // No filename → opens save-as prompt
        assert!(e.cmd_buf.active);
    }

    // ========================================================================
    // Coverage gap: handle_key Backspace (line 544)
    // ========================================================================

    #[test]
    fn test_backspace_key_dispatch() {
        let mut e = ed("ab");
        e.set_cursor(Pos::new(0, 2));
        e.handle_key(Key::Backspace);
        assert_eq!(e.test_text(), "a");
    }

    // ========================================================================
    // Coverage gap: command submit via handle_cmd_result (line 588)
    // ========================================================================

    #[test]
    fn test_cmd_submit_executes_command() {
        let mut e = ed("hello");
        e.handle_cmd_result(
            crate::command_buffer::CommandBufferMode::Command,
            crate::command_buffer::CommandBufferResult::Submit("ruler".to_string()),
        );
        // ruler command toggles ruler
        assert!(!e.ruler_on);
    }

    // ========================================================================
    // Coverage gap: execute_command None action (line 679)
    // ========================================================================

    #[test]
    fn test_execute_unknown_command() {
        let mut e = ed("hello");
        e.execute_command("nonexistent_command");
        // Should set status message about unknown command
        assert!(e.status_msg.contains("Unknown"));
    }

    // ========================================================================
    // Coverage gap: kill_line on single-line doc (line 731)
    // ========================================================================

    #[test]
    fn test_kill_line_on_last_line() {
        let mut e = ed("hello");
        e.kill_line();
        assert_eq!(e.test_text(), "");
    }

    // ========================================================================
    // Coverage gap: draw with status_msg (line 280)
    // ========================================================================

    #[test]
    fn test_draw_with_status_msg() {
        let mut e = ed("hello\nworld");
        e.set_status("Test status".to_string());
        assert!(!e.status_msg.is_empty());
        let mut buf = Vec::new();
        let _ = e.draw(&mut buf);
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("Test status"));
    }

    // ========================================================================
    // Coverage gap: center_view_on_line with ruler off (line 353)
    // ========================================================================

    #[test]
    fn test_center_view_ruler_off() {
        let mut e =
            ed("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\nq\nr\ns\nt\nu\nv\nw\nx\ny\nz");
        e.ruler_on = false;
        e.center_view_on_line(20);
        // Cursor should be somewhere near line 20
        assert!(e.view.scroll_line > 0);
    }

    // ========================================================================
    // Coverage gap: find_matching_bracket for quotes (line 393)
    // ========================================================================

    #[test]
    fn test_find_matching_quote() {
        let mut e = ed("let s = \"hello\";\n");
        // Place cursor on the opening quote
        e.set_cursor(Pos::new(0, 8));
        let pair = e.find_matching_bracket();
        assert!(pair.is_some());
        let (_, match_pos) = pair.unwrap();
        assert_eq!(match_pos.col, 14); // closing quote
    }

    // ========================================================================
    // Coverage gap: replace_all case-sensitive (line 863)
    // ========================================================================

    #[test]
    fn test_replace_all_case_sensitive() {
        let mut e = ed("Hello hello HELLO");
        // Capital letter in pattern → case-sensitive
        e.replace_all("Hello", "Bye");
        assert_eq!(e.test_text(), "Bye hello HELLO");
    }

    // ========================================================================
    // Coverage gap: mouse drag Hold event (line 909)
    // ========================================================================

    #[test]
    fn test_mouse_hold_drag() {
        let mut e = ed("hello world");
        e.ruler_on = false;
        // Start a press first so dragging=true
        e.handle_mouse(MouseEvent::Press(MouseButton::Left, 1, 1));
        assert!(e.dragging);
        // Now drag
        e.handle_mouse(MouseEvent::Hold(6, 1));
        assert!(!e.sel.is_empty());
    }

    // ========================================================================
    // Coverage gap: mouse release event
    // ========================================================================

    #[test]
    fn test_mouse_release_stops_drag() {
        let mut e = ed("hello");
        e.dragging = true;
        e.handle_mouse(MouseEvent::Release(1, 1));
        assert!(!e.dragging);
    }

    // ========================================================================
    // Coverage gap: screen_to_buffer_pos ruler off (line 924)
    // ========================================================================

    #[test]
    fn test_screen_to_buffer_pos_ruler_off() {
        let mut e = ed("hello\nworld");
        e.ruler_on = false;
        let pos = e.screen_to_buffer_pos(1, 1);
        assert_eq!(pos, Pos::new(0, 0));
        let pos2 = e.screen_to_buffer_pos(1, 2);
        assert_eq!(pos2, Pos::new(1, 0));
    }

    // ========================================================================
    // Coverage gap: screen_to_buffer_pos text_cols=0 (line 928)
    // ========================================================================

    #[test]
    fn test_screen_to_buffer_pos_zero_cols() {
        let mut e = ed("hello");
        e.view = crate::view::View::new(1, 3); // very narrow
        e.ruler_on = true;
        // With gutter eating all columns, text_cols might be 0
        let pos = e.screen_to_buffer_pos(1, 1);
        assert_eq!(pos, Pos::zero());
    }

    // ========================================================================
    // Coverage gap: multi-click double/triple (lines 977-994)
    // ========================================================================

    #[test]
    fn test_double_click_selects_word() {
        let mut e = ed("hello world");
        e.ruler_on = false;
        // First click
        e.mouse_press(1, 1);
        // Simulate double click by setting last_click_time/pos and calling again
        e.mouse_press(1, 1);
        // Should select word "hello"
        assert!(!e.sel.is_empty());
    }

    #[test]
    fn test_triple_click_selects_line() {
        let mut e = ed("hello world\nsecond");
        e.ruler_on = false;
        // Three clicks at the same spot
        e.mouse_press(1, 1);
        e.mouse_press(1, 1);
        e.mouse_press(1, 1);
        // Should select entire first line
        assert!(!e.sel.is_empty());
    }

    // ========================================================================
    // Coverage gap: mouse_drag when not dragging (line 1000)
    // ========================================================================

    #[test]
    fn test_mouse_drag_not_dragging_noop() {
        let mut e = ed("hello");
        e.dragging = false;
        let cursor_before = e.cursor();
        e.mouse_drag(5, 1);
        assert_eq!(e.cursor(), cursor_before);
    }

    // ========================================================================
    // Coverage gap: scroll_up/down with ruler off (lines 1052, 1091)
    // ========================================================================

    #[test]
    fn test_scroll_up_ruler_off() {
        let text = (0..50)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.ruler_on = false;
        e.view.scroll_line = 20;
        e.set_cursor(Pos::new(20, 0));
        e.scroll_up();
        assert!(e.view.scroll_line < 20);
    }

    #[test]
    fn test_scroll_down_ruler_off() {
        let text = (0..50)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.ruler_on = false;
        e.set_cursor(Pos::new(0, 0));
        e.scroll_down();
        assert!(e.view.scroll_line > 0);
    }

    // ========================================================================
    // Coverage gap: scroll_up with scroll_wrap > 0 (lines 1059-1061)
    // ========================================================================

    #[test]
    fn test_scroll_up_with_wrap() {
        let long_line = "a".repeat(200);
        let mut e = ed(&long_line);
        e.ruler_on = false;
        // Set scroll_wrap to simulate being partway through a wrapped line
        e.view.scroll_wrap = 3;
        e.set_cursor(Pos::new(0, 0));
        e.scroll_up();
        assert!(e.view.scroll_wrap < 3);
    }

    // ========================================================================
    // Coverage gap: scroll_down wrapping (lines 1104-1105)
    // ========================================================================

    #[test]
    fn test_scroll_down_with_wrap() {
        let long_line = "a".repeat(200);
        let text = format!("{}\nshort", long_line);
        let mut e = ed(&text);
        e.ruler_on = false;
        e.set_cursor(Pos::new(0, 0));
        // Scroll down — should advance through wraps of the long line
        e.scroll_down();
        assert!(e.view.scroll_wrap > 0 || e.view.scroll_line > 0);
    }

    // ========================================================================
    // Coverage gap: clamp_cursor_to_viewport zero rows/cols (line 1120)
    // ========================================================================

    #[test]
    fn test_clamp_cursor_zero_rows() {
        let mut e = ed("hello");
        e.view = crate::view::View::new(80, 2); // only 2 rows = 0 text rows
        let cursor_before = e.cursor();
        e.clamp_cursor_to_viewport(0, 80);
        // Should return early without changing cursor
        assert_eq!(e.cursor(), cursor_before);
    }

    // ========================================================================
    // Coverage gap: clamp_cursor below viewport (lines 1173-1177)
    // ========================================================================

    #[test]
    fn test_clamp_cursor_below_viewport() {
        let text = (0..50)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.ruler_on = false;
        // Put cursor far below viewport
        e.sel.cursor = Pos::new(45, 0);
        e.sel.anchor = Pos::new(45, 0);
        e.view.scroll_line = 0;
        // Clamp should snap cursor into viewport
        e.clamp_cursor_to_viewport(0, 80);
        assert!(e.cursor().line < 45);
    }

    // ========================================================================
    // Coverage gap: move_left_extend wrapping to prev line (lines 1363-1364)
    // ========================================================================

    #[test]
    fn test_move_left_extend_wraps_to_prev_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(1, 0));
        e.move_left_extend();
        assert_eq!(e.sel.cursor, Pos::new(0, 5));
    }

    // ========================================================================
    // Coverage gap: move_right_extend wrapping to next line (line 1374)
    // ========================================================================

    #[test]
    fn test_move_right_extend_wraps_to_next_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(0, 5));
        e.move_right_extend();
        assert_eq!(e.sel.cursor, Pos::new(1, 0));
    }

    // ========================================================================
    // Coverage gap: indent_selection end line adjustment (line 1410)
    // ========================================================================

    #[test]
    fn test_indent_selection_skips_trailing_empty_line() {
        let mut e = ed_named("aaa\nbbb\nccc\n", "test.rs");
        // Select lines 0-2 with cursor at col 0 of line 3 (empty trailing)
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(3, 0),
        };
        e.indent_selection();
        // Lines 0-2 should be indented, but not the empty line after
        assert!(e.test_text().starts_with("  aaa\n  bbb\n  ccc\n"));
    }

    // ========================================================================
    // Coverage gap: toggle_comment with selection end line adj (line 1576)
    // ========================================================================

    #[test]
    fn test_toggle_comment_selection_end_adj() {
        let mut e = ed_named("aaa\nbbb\nccc\n", "test.rs");
        // Select with cursor at col 0 of a later line
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(2, 0),
        };
        e.toggle_comment();
        // Lines 0-1 should be commented (not line 2 since cursor col=0)
        let text = e.test_text();
        assert!(text.starts_with("// aaa\n// bbb\n"));
    }

    // ========================================================================
    // Coverage gap: toggle_comment with empty/whitespace lines (line 1590, 1630)
    // ========================================================================

    #[test]
    fn test_toggle_comment_with_blank_lines() {
        let mut e = ed_named("aaa\n\nbbb\n", "test.rs");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(2, 3),
        };
        e.toggle_comment();
        // Blank lines should be skipped when commenting
        let text = e.test_text();
        assert!(text.contains("// aaa"));
        assert!(text.contains("// bbb"));
        // The blank line should stay blank (not get "// " prefix)
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "");
    }

    #[test]
    fn test_toggle_comment_skips_already_commented() {
        let mut e = ed_named("aaa\n// bbb\nccc\n", "test.rs");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(2, 3),
        };
        e.toggle_comment();
        let text = e.test_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "// aaa");
        assert_eq!(lines[1], "// bbb"); // not double-commented
        assert_eq!(lines[2], "// ccc");
    }

    #[test]
    fn test_comment_performance_3000_lines() {
        let text: String = (0..3000)
            .map(|i| format!("let x{} = {};", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed_named(&text, "test.rs");
        e.select_all();
        let start = std::time::Instant::now();
        e.toggle_comment();
        let elapsed = start.elapsed();
        assert!(e.test_text().starts_with("// let x0"));
        assert!(
            elapsed.as_millis() < 500,
            "comment on 3000 lines took {:?}",
            elapsed
        );
    }

    // ========================================================================
    // Coverage gap: dedent selection end line adj (lines 1645-1651)
    // ========================================================================

    #[test]
    fn test_dedent_selection_end_adj() {
        let mut e = ed_named("  aaa\n  bbb\n  ccc\n", "test.rs");
        // Select with cursor at col 0 of line 2
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(2, 0),
        };
        e.dedent();
        // Lines 0-1 should be dedented
        let text = e.test_text();
        assert!(text.starts_with("aaa\nbbb\n"));
    }

    // ========================================================================
    // Coverage gap: cut with no selection (line 1695)
    // ========================================================================

    #[test]
    fn test_cut_no_selection_noop() {
        let mut e = ed("hello");
        e.cut();
        assert_eq!(e.test_text(), "hello");
    }

    // ========================================================================
    // Coverage gap: execute_command Save/SaveAs (lines 679-683)
    // ========================================================================

    #[test]
    fn test_execute_command_save_as_file() {
        let dir = std::env::temp_dir().join("e_test_save_as_cmd");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("new.txt");
        let mut e = ed("hello");
        let cmd = format!("save {}", path.to_str().unwrap());
        e.execute_command(&cmd);
        assert_eq!(e.doc.filename.as_deref(), Some(path.to_str().unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_execute_command_quit_via_cmd() {
        let mut e = ed("hello");
        e.execute_command("quit");
        assert!(!e.running);
    }

    // ========================================================================
    // Coverage gap: handle_cmd_result SudoSave mode (lines 599-600)
    // ========================================================================

    #[test]
    fn test_handle_cmd_result_sudo_cancel_cleans_tmp() {
        let mut e = ed("hello");
        e.sudo_save_tmp = Some("/tmp/e_test_sudo_fake".to_string());
        e.handle_cmd_result(
            crate::command_buffer::CommandBufferMode::SudoSave,
            crate::command_buffer::CommandBufferResult::Cancel,
        );
        assert!(e.sudo_save_tmp.is_none());
    }

    // ========================================================================
    // Coverage gap: delete_selection when empty (line 1185)
    // ========================================================================

    #[test]
    fn test_delete_selection_empty_noop() {
        let mut e = ed("hello");
        e.delete_selection();
        assert_eq!(e.test_text(), "hello");
    }

    // ========================================================================
    // Coverage gap: save_file with filename opens save prompt when none
    // ========================================================================

    #[test]
    fn test_save_file_no_name_opens_prompt() {
        let mut e = ed("hello");
        e.save_file();
        assert!(e.cmd_buf.active);
        assert_eq!(
            e.cmd_buf.mode,
            crate::command_buffer::CommandBufferMode::Prompt
        );
    }

    // ========================================================================
    // Coverage gap: save_file to temp file (covers lines 1789-1794)
    // ========================================================================

    #[test]
    fn test_save_file_to_temp() {
        let dir = std::env::temp_dir().join("e_test_save_file");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let mut e = ed("hello world");
        e.doc.filename = Some(path.to_str().unwrap().to_string());
        e.doc.dirty = true;
        e.save_file();
        assert!(!e.doc.dirty);
        assert!(e.status_msg.contains("Saved"));
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hello world\n"); // trailing newline added
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ========================================================================
    // Coverage gap: save_undo_if_named (lines 567-570)
    // ========================================================================

    #[test]
    fn test_save_undo_if_named_with_existing_file() {
        let dir = std::env::temp_dir().join("e_test_save_undo");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut e = ed("hello");
        e.doc.filename = Some(path.to_str().unwrap().to_string());
        e.save_undo_if_named(); // should not panic
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ========================================================================
    // Coverage gap: Find with selection prefill (lines 498-506)
    // ========================================================================

    #[test]
    fn test_find_prefills_from_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.handle_key(Key::Ctrl('f'));
        assert!(e.cmd_buf.active);
        assert_eq!(e.cmd_buf.input, "hello");
    }

    // ========================================================================
    // Coverage gap: multiple completions with common prefix (lines 665-667)
    // ========================================================================

    #[test]
    fn test_command_completion_common_prefix() {
        let mut e = ed("hello");
        e.cmd_buf
            .open(crate::command_buffer::CommandBufferMode::Command, "> ", "");
        e.cmd_buf.input = "go".to_string();
        e.cmd_buf.cursor = 2;
        // Request tab completion — should find "goto" and complete the common prefix
        let result = e.cmd_buf.handle_key(Key::Char('\t'));
        let mode = e.cmd_buf.mode;
        e.handle_cmd_result(mode, result);
        // "goto" and "gotoline" both start with "goto"
        // Depending on commands available, this should complete to at least "goto"
    }

    // ========================================================================
    // External file change detection
    // ========================================================================

    #[test]
    fn test_check_external_modification_detects_change() {
        let dir = std::env::temp_dir().join("e_test_ext_mod");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"original").unwrap();

        let mut e = ed_named("original", path.to_str().unwrap());
        e.file_mtime = crate::file_io::file_mtime(&path);
        assert!(!e.reload_pending);

        // Modify file externally
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"changed").unwrap();

        e.check_external_modification();
        assert!(e.reload_pending);
        assert!(e.status_msg.contains("changed on disk"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_reload_file_replaces_buffer() {
        let dir = std::env::temp_dir().join("e_test_reload_buf");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"original\n").unwrap();

        let mut e = ed_named("original\n", path.to_str().unwrap());
        e.file_mtime = crate::file_io::file_mtime(&path);

        // Modify file externally
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"new content\n").unwrap();

        e.reload_pending = true;
        e.reload_file();
        assert!(!e.reload_pending);
        assert!(e.test_text().contains("new content"));
        assert!(e.status_msg.contains("Reloaded"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dismiss_reload_updates_mtime() {
        let dir = std::env::temp_dir().join("e_test_dismiss");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"original").unwrap();

        let mut e = ed_named("original", path.to_str().unwrap());
        e.file_mtime = crate::file_io::file_mtime(&path);

        // Modify file externally
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"changed").unwrap();
        let new_mtime = crate::file_io::file_mtime(&path);

        e.reload_pending = true;
        e.status_msg = "test.txt changed on disk. Reload? (y/n)".to_string();
        e.dismiss_reload();
        assert!(!e.reload_pending);
        assert!(e.status_msg.is_empty());
        assert_eq!(e.file_mtime, new_mtime);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_reload_clamps_cursor() {
        let dir = std::env::temp_dir().join("e_test_reload_clamp");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"line1\nline2\nline3\n").unwrap();

        let mut e = ed_named("line1\nline2\nline3\n", path.to_str().unwrap());
        e.file_mtime = crate::file_io::file_mtime(&path);
        // Put cursor on line 2
        e.sel = Selection::caret(Pos::new(2, 3));

        // Replace with shorter file
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"short\n").unwrap();

        e.reload_pending = true;
        e.reload_file();
        // Cursor should be clamped to last line
        assert!(e.sel.cursor.line < e.doc.buf.line_count());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_no_reload_for_unnamed() {
        let mut e = ed("hello");
        e.check_external_modification();
        assert!(!e.reload_pending);
        assert!(e.status_msg.is_empty());
    }

    #[test]
    fn test_focus_in_event() {
        let dir = std::env::temp_dir().join("e_test_focus_in");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"original").unwrap();

        let mut e = ed_named("original", path.to_str().unwrap());
        e.file_mtime = crate::file_io::file_mtime(&path);

        // Modify file externally
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"changed").unwrap();

        // Send focus-in event
        e.handle_event(Event::Unsupported(FOCUS_IN.to_vec()));
        assert!(e.reload_pending);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_updates_mtime() {
        let dir = std::env::temp_dir().join("e_test_save_mtime");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"hello\n").unwrap();

        let mut e = ed_named("hello\n", path.to_str().unwrap());
        assert!(e.file_mtime.is_none()); // ed_named doesn't set mtime

        e.save_file();
        assert!(e.file_mtime.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ========================================================================
    // Word navigation (Ctrl+Left / Ctrl+Right)
    // ========================================================================

    #[test]
    fn test_word_left_middle_of_line() {
        let mut e = ed("hello world foo");
        e.set_cursor(Pos::new(0, 15));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 12));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 6));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn test_word_right_middle_of_line() {
        let mut e = ed("hello world foo");
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 6));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 12));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 15));
    }

    #[test]
    fn test_word_left_wraps_to_prev_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(1, 0));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn test_word_right_wraps_to_next_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(Pos::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(1, 0));
    }

    #[test]
    fn test_word_left_collapses_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 2),
            cursor: Pos::new(0, 8),
        };
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 2));
        assert!(e.sel.is_empty());
    }

    #[test]
    fn test_word_right_collapses_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 2),
            cursor: Pos::new(0, 8),
        };
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 8));
        assert!(e.sel.is_empty());
    }

    #[test]
    fn test_word_left_at_origin() {
        let mut e = ed("hello");
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn test_word_right_at_end() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn test_word_left_skips_punctuation() {
        // "foo.bar" at end: skip "bar", skip ".", skip "foo" -> 0
        let mut e = ed("foo.bar");
        e.set_cursor(Pos::new(0, 7));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 4));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn test_word_right_skips_punctuation() {
        // "foo.bar" from 0: skip "foo" to 3, skip "." to 4
        let mut e = ed("foo.bar");
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 4));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 7));
    }

    #[test]
    fn test_word_left_multiple_spaces() {
        let mut e = ed("foo   bar");
        e.set_cursor(Pos::new(0, 9));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 6));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn test_word_right_multiple_spaces() {
        let mut e = ed("foo   bar");
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 6));
    }

    #[test]
    fn test_word_left_empty_line() {
        let mut e = ed("hello\n\nworld");
        e.set_cursor(Pos::new(2, 0));
        e.word_left();
        // wraps to end of empty line 1
        assert_eq!(e.cursor(), Pos::new(1, 0));
        e.word_left();
        // wraps to end of line 0
        assert_eq!(e.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn test_word_right_empty_line() {
        let mut e = ed("hello\n\nworld");
        e.set_cursor(Pos::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(1, 0));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(2, 0));
    }

    #[test]
    fn test_word_left_from_middle_of_word() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 3));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    #[test]
    fn test_word_right_from_middle_of_word() {
        // "hello world" from col 3: skip "lo" to 5, skip " " to 6
        let mut e = ed("hello world");
        e.set_cursor(Pos::new(0, 3));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 6));
    }

    #[test]
    fn test_word_left_underscores() {
        // underscores are word chars
        let mut e = ed("foo_bar baz");
        e.set_cursor(Pos::new(0, 11));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 8));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 0)); // foo_bar is one word
    }

    #[test]
    fn test_word_right_underscores() {
        let mut e = ed("foo_bar baz");
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 8)); // skips whole foo_bar + space
    }

    #[test]
    fn test_word_left_at_last_line_end() {
        let mut e = ed("one\ntwo");
        e.set_cursor(Pos::new(1, 3));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(1, 0));
    }

    #[test]
    fn test_word_right_at_last_line_end() {
        // at end of last line, no next line, stays put
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn test_word_nav_roundtrip() {
        // word_right then word_left should return near starting region
        let mut e = ed("  fn main() {");
        e.word_right();
        // from col 0: skip word chars (none), skip non-word ("  ") -> lands at 2
        assert_eq!(e.cursor(), Pos::new(0, 2));
        e.word_right();
        // from col 2: skip word chars ("fn") to 4, skip non-word (" ") to 5
        assert_eq!(e.cursor(), Pos::new(0, 5));
        e.word_left();
        // from col 5: skip non-word (" ") to 4, skip word ("fn") to 2
        assert_eq!(e.cursor(), Pos::new(0, 2));
        e.word_left();
        assert_eq!(e.cursor(), Pos::new(0, 0));
    }

    // ========================================================================
    // Auto-close pairs
    // ========================================================================

    #[test]
    fn test_autoclose_paren() {
        let mut e = ed("");
        e.insert_char('(');
        assert_eq!(e.test_text(), "()");
        assert_eq!(e.cursor(), Pos::new(0, 1)); // between parens
    }

    #[test]
    fn test_autoclose_bracket() {
        let mut e = ed("");
        e.insert_char('[');
        assert_eq!(e.test_text(), "[]");
        assert_eq!(e.cursor(), Pos::new(0, 1));
    }

    #[test]
    fn test_autoclose_brace() {
        let mut e = ed("");
        e.insert_char('{');
        assert_eq!(e.test_text(), "{}");
        assert_eq!(e.cursor(), Pos::new(0, 1));
    }

    #[test]
    fn test_autoclose_double_quote() {
        let mut e = ed("");
        e.insert_char('"');
        assert_eq!(e.test_text(), "\"\"");
        assert_eq!(e.cursor(), Pos::new(0, 1));
    }

    #[test]
    fn test_autoclose_single_quote() {
        let mut e = ed("");
        e.insert_char('\'');
        assert_eq!(e.test_text(), "''");
        assert_eq!(e.cursor(), Pos::new(0, 1));
    }

    #[test]
    fn test_autoclose_skip_closing_paren() {
        let mut e = ed("");
        e.insert_char('(');
        assert_eq!(e.test_text(), "()");
        assert_eq!(e.cursor(), Pos::new(0, 1));
        e.insert_char(')'); // should skip over the closing paren
        assert_eq!(e.test_text(), "()");
        assert_eq!(e.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn test_autoclose_skip_closing_bracket() {
        let mut e = ed("");
        e.insert_char('[');
        e.insert_char(']');
        assert_eq!(e.test_text(), "[]");
        assert_eq!(e.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn test_autoclose_skip_closing_brace() {
        let mut e = ed("");
        e.insert_char('{');
        e.insert_char('}');
        assert_eq!(e.test_text(), "{}");
        assert_eq!(e.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn test_autoclose_skip_closing_double_quote() {
        let mut e = ed("");
        e.insert_char('"');
        e.insert_char('"');
        assert_eq!(e.test_text(), "\"\"");
        assert_eq!(e.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn test_autoclose_skip_closing_single_quote() {
        let mut e = ed("");
        e.insert_char('\'');
        e.insert_char('\'');
        assert_eq!(e.test_text(), "''");
        assert_eq!(e.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn test_autoclose_no_pair_when_next_is_word() {
        let mut e = ed("hello");
        e.insert_char('(');
        assert_eq!(e.test_text(), "(hello"); // no auto-close
    }

    #[test]
    fn test_autoclose_pair_when_next_is_space() {
        let mut e = ed(" hello");
        e.insert_char('(');
        assert_eq!(e.test_text(), "() hello");
        assert_eq!(e.cursor(), Pos::new(0, 1));
    }

    #[test]
    fn test_autoclose_pair_when_next_is_close_char() {
        let mut e = ed(")");
        e.insert_char('(');
        assert_eq!(e.test_text(), "())");
        assert_eq!(e.cursor(), Pos::new(0, 1));
    }

    #[test]
    fn test_autoclose_backspace_deletes_paren_pair() {
        let mut e = ed("");
        e.insert_char('(');
        assert_eq!(e.test_text(), "()");
        e.backspace();
        assert_eq!(e.test_text(), "");
    }

    #[test]
    fn test_autoclose_backspace_deletes_bracket_pair() {
        let mut e = ed("");
        e.insert_char('[');
        e.backspace();
        assert_eq!(e.test_text(), "");
    }

    #[test]
    fn test_autoclose_backspace_deletes_brace_pair() {
        let mut e = ed("");
        e.insert_char('{');
        e.backspace();
        assert_eq!(e.test_text(), "");
    }

    #[test]
    fn test_autoclose_backspace_deletes_double_quote_pair() {
        let mut e = ed("");
        e.insert_char('"');
        e.backspace();
        assert_eq!(e.test_text(), "");
    }

    #[test]
    fn test_autoclose_backspace_deletes_single_quote_pair() {
        let mut e = ed("");
        e.insert_char('\'');
        e.backspace();
        assert_eq!(e.test_text(), "");
    }

    #[test]
    fn test_autoclose_backspace_only_deletes_pair_when_matched() {
        // "(x" with cursor at 1 — next char is 'x' not ')', so only delete '('
        let mut e = ed("(x");
        e.set_cursor(Pos::new(0, 1));
        e.backspace();
        assert_eq!(e.test_text(), "x");
    }

    #[test]
    fn test_autoclose_wraps_selection_paren() {
        let mut e = ed("hello");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.insert_char('(');
        assert_eq!(e.test_text(), "(hello)");
        // Inner text should be selected
        let (s, end) = e.sel.ordered();
        assert_eq!(s, Pos::new(0, 1));
        assert_eq!(end, Pos::new(0, 6));
    }

    #[test]
    fn test_autoclose_wraps_selection_bracket() {
        let mut e = ed("world");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.insert_char('[');
        assert_eq!(e.test_text(), "[world]");
    }

    #[test]
    fn test_autoclose_wraps_selection_brace() {
        let mut e = ed("abc");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 3),
        };
        e.insert_char('{');
        assert_eq!(e.test_text(), "{abc}");
    }

    #[test]
    fn test_autoclose_wraps_selection_double_quote() {
        let mut e = ed("text");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 4),
        };
        e.insert_char('"');
        assert_eq!(e.test_text(), "\"text\"");
    }

    #[test]
    fn test_autoclose_wraps_selection_single_quote() {
        let mut e = ed("text");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 4),
        };
        e.insert_char('\'');
        assert_eq!(e.test_text(), "'text'");
    }

    #[test]
    fn test_autoclose_wraps_partial_selection() {
        let mut e = ed("hello world");
        e.sel = Selection {
            anchor: Pos::new(0, 6),
            cursor: Pos::new(0, 11),
        };
        e.insert_char('(');
        assert_eq!(e.test_text(), "hello (world)");
    }

    #[test]
    fn test_autoclose_wraps_multiline_selection() {
        let mut e = ed("foo\nbar");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(1, 3),
        };
        e.insert_char('{');
        assert_eq!(e.test_text(), "{foo\nbar}");
    }

    #[test]
    fn test_autoclose_non_pair_char_replaces_selection() {
        // Typing a regular char with selection should replace it (not wrap)
        let mut e = ed("hello");
        e.sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        e.insert_char('x');
        assert_eq!(e.test_text(), "x");
    }

    #[test]
    fn test_autoclose_type_inside_pair() {
        let mut e = ed("");
        e.insert_char('(');
        e.insert_char('x');
        assert_eq!(e.test_text(), "(x)");
        assert_eq!(e.cursor(), Pos::new(0, 2));
    }

    #[test]
    fn test_autoclose_nested_pairs() {
        let mut e = ed("");
        e.insert_char('(');
        e.insert_char('[');
        assert_eq!(e.test_text(), "([])");
        assert_eq!(e.cursor(), Pos::new(0, 2));
        e.insert_char(']');
        assert_eq!(e.test_text(), "([])");
        assert_eq!(e.cursor(), Pos::new(0, 3));
        e.insert_char(')');
        assert_eq!(e.test_text(), "([])");
        assert_eq!(e.cursor(), Pos::new(0, 4));
    }

    #[test]
    fn test_autoclose_at_end_of_line() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.insert_char('(');
        assert_eq!(e.test_text(), "hello()");
        assert_eq!(e.cursor(), Pos::new(0, 6));
    }

    #[test]
    fn test_autoclose_no_pair_before_digit() {
        let mut e = ed("42");
        e.insert_char('(');
        assert_eq!(e.test_text(), "(42"); // digit is word char, no auto-close
    }

    #[test]
    fn test_autoclose_wraps_backward_selection() {
        // Selection where cursor < anchor (backward selection)
        let mut e = ed("hello");
        e.sel = Selection {
            anchor: Pos::new(0, 5),
            cursor: Pos::new(0, 0),
        };
        e.insert_char('(');
        assert_eq!(e.test_text(), "(hello)");
    }

    // ========================================================================
    // Smart paste (auto-indent)
    // ========================================================================

    #[test]
    fn test_smart_paste_reindents_to_cursor() {
        let mut e = ed("    fn main() {\n        ");
        e.set_cursor(Pos::new(1, 8));
        e.paste_text("if true {\n    println!(\"hi\");\n}");
        assert_eq!(
            e.test_text(),
            "    fn main() {\n        if true {\n            println!(\"hi\");\n        }"
        );
    }

    #[test]
    fn test_smart_paste_single_line_no_change() {
        let mut e = ed("    ");
        e.set_cursor(Pos::new(0, 4));
        e.paste_text("hello");
        assert_eq!(e.test_text(), "    hello");
    }

    #[test]
    fn test_smart_paste_already_correct_indent() {
        let mut e = ed("  ");
        e.set_cursor(Pos::new(0, 2));
        e.paste_text("a\n  b\n  c");
        assert_eq!(e.test_text(), "  a\n  b\n  c");
    }

    #[test]
    fn test_smart_paste_dedents_when_needed() {
        // Paste text indented at 8 spaces into a position at col 2
        let mut e = ed("  ");
        e.set_cursor(Pos::new(0, 2));
        e.paste_text("x\n        y\n        z");
        assert_eq!(e.test_text(), "  x\n  y\n  z");
    }

    #[test]
    fn test_smart_paste_preserves_relative_indent() {
        let mut e = ed("    ");
        e.set_cursor(Pos::new(0, 4));
        e.paste_text("if true {\n  nested\n}");
        // min_indent of lines 1+ is 0 (the "}"), target is col 4
        // so all continuation lines get +4
        assert_eq!(e.test_text(), "    if true {\n      nested\n    }");
    }

    #[test]
    fn test_smart_paste_empty_lines_preserved() {
        let mut e = ed("    ");
        e.set_cursor(Pos::new(0, 4));
        e.paste_text("a\n\nb");
        // Empty line should be preserved as-is
        assert_eq!(e.test_text(), "    a\n\n    b");
    }

    #[test]
    fn test_smart_paste_zero_indent_no_change() {
        // Pasting at col 0 with text that has 0 indent — no change
        let mut e = ed("");
        e.paste_text("a\nb\nc");
        assert_eq!(e.test_text(), "a\nb\nc");
    }

    #[test]
    fn test_smart_paste_with_selection_replaces() {
        let mut e = ed("    old stuff\n    more old");
        e.sel = Selection {
            anchor: Pos::new(0, 4),
            cursor: Pos::new(0, 13),
        };
        e.paste_text("new\n    thing");
        // selection deleted first, then pasted at col 4
        assert_eq!(e.test_text(), "    new\n    thing\n    more old");
    }

    #[test]
    fn test_smart_paste_cursor_position() {
        let mut e = ed("");
        e.paste_text("hello\nworld");
        // Cursor should be at end of pasted text
        assert_eq!(e.cursor(), Pos::new(1, 5));
    }

    #[test]
    fn test_smart_paste_empty_string() {
        let mut e = ed("hello");
        e.set_cursor(Pos::new(0, 5));
        e.paste_text("");
        assert_eq!(e.test_text(), "hello");
        assert_eq!(e.cursor(), Pos::new(0, 5));
    }

    #[test]
    fn test_smart_paste_all_empty_continuation_lines() {
        // When all continuation lines are empty, min_indent is 0, should not crash
        let mut e = ed("    ");
        e.set_cursor(Pos::new(0, 4));
        e.paste_text("a\n\n\n");
        assert_eq!(e.test_text(), "    a\n\n\n");
    }

    #[test]
    fn test_smart_paste_tabs_in_pasted_text() {
        // Tabs in pasted text — min_indent is calculated by byte length
        let mut e = ed("  ");
        e.set_cursor(Pos::new(0, 2));
        e.paste_text("a\n\tb");
        // \t is 1 char of indent, target is 2, so re-indented
        assert_eq!(e.test_text(), "  a\n  b");
    }
}
