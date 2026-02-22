use std::io::{self, Write, stdout};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use termion::event::{Event, Key, MouseButton, MouseEvent};
use termion::input::TermRead;
use termion::raw::IntoRawMode;
use termion::screen::IntoAlternateScreen;

use crate::clipboard::Clipboard;
use crate::command::{CommandAction, CommandRegistry};
use crate::command_buffer::{CommandBuffer, CommandBufferMode, CommandBufferResult};
use crate::document::Document;
use crate::keybind::{EditorAction, KeybindingTable};
use crate::render::{Renderer, gutter_width};
use crate::selection::{Pos, Selection, is_word_char, prev_word_boundary};
use crate::view::View;

const SCROLL_LINES: usize = 3;

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
    find_matches: Vec<(Pos, Pos)>,
    find_index: usize,
}

enum EditorEvent {
    Term(Event),
    Resize(u16, u16),
    #[allow(dead_code)]
    Tick,
}

impl Editor {
    pub fn new(text: Vec<u8>, filename: Option<String>) -> Self {
        let (w, h) = termion::terminal_size().unwrap_or((80, 24));
        let mut keybindings = KeybindingTable::with_defaults();
        keybindings.load_config();
        Self {
            doc: Document::new(text, filename),
            sel: Selection::caret(Pos::zero()),
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
            find_index: 0,
        }
    }

    pub fn run(&mut self) -> io::Result<()> {
        let mut stdout = stdout().into_raw_mode()?.into_alternate_screen()?;

        write!(stdout, "\x1b[?1000h\x1b[?1002h\x1b[?1006h")?;
        stdout.flush()?;

        let (tx, rx) = mpsc::channel::<EditorEvent>();

        let tx_input = tx.clone();
        std::thread::spawn(move || {
            let stdin = io::stdin();
            for ev in stdin.events().flatten() {
                if tx_input.send(EditorEvent::Term(ev)).is_err() {
                    break;
                }
            }
        });

        let tx_sig = tx.clone();
        std::thread::spawn(move || {
            let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGWINCH])
                .expect("failed to register SIGWINCH");
            for _ in signals.forever() {
                if let Ok((w, h)) = termion::terminal_size()
                    && tx_sig.send(EditorEvent::Resize(w, h)).is_err()
                {
                    break;
                }
            }
        });

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
                Ok(EditorEvent::Resize(w, h)) => {
                    self.view.width = w;
                    self.view.height = h;
                    self.renderer.force_full_redraw();
                }
                Ok(EditorEvent::Tick) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        write!(stdout, "\x1b[?1006l\x1b[?1002l\x1b[?1000l")?;
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
        self.view
            .ensure_cursor_visible(cursor_line, display_col, gw);

        let status_left = self.status_left();
        let status_right = self.status_right();
        let sel = if self.sel.is_empty() {
            None
        } else {
            Some(self.sel)
        };
        let ruler_on = self.ruler_on;

        let cmd_line = if self.cmd_buf.active {
            Some(self.cmd_buf.display_line())
        } else if !self.status_msg.is_empty() {
            Some(self.status_msg.clone())
        } else {
            None
        };
        let cmd_ref = cmd_line.as_deref();

        let find_matches = if !self.find_matches.is_empty() {
            Some(self.find_matches.as_slice())
        } else {
            None
        };

        self.renderer.render(
            out,
            &mut self.doc.buf,
            &self.view,
            cursor_line,
            display_col,
            ruler_on,
            &status_left,
            &status_right,
            cmd_ref,
            sel,
            find_matches,
        )
    }

    fn status_left(&self) -> String {
        let name = self.doc.filename.as_deref().unwrap_or("[scratch]");
        if self.doc.dirty {
            format!(" {}*", name)
        } else {
            format!(" {}", name)
        }
    }

    fn status_right(&mut self) -> String {
        let line_count = self.doc.buf.line_count();
        format!(
            "Ln {}, Col {} \u{2502} {} lines ",
            self.cursor().line + 1,
            self.cursor().col + 1,
            line_count,
        )
    }

    fn cursor_display_col(&mut self) -> usize {
        let line_text = self.doc.buf.line_text(self.cursor().line);
        let mut display_col = 0;
        let mut char_idx = 0;
        let mut byte_idx = 0;
        while char_idx < self.cursor().col && byte_idx < line_text.len() {
            if line_text[byte_idx] == b'\t' {
                display_col += 2;
            } else {
                display_col += 1;
            }
            byte_idx += crate::buffer::utf8_char_len(line_text[byte_idx]);
            char_idx += 1;
        }
        display_col
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
                    self.handle_mouse(mouse);
                }
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, key: Key) {
        // Handle quit confirmation
        if self.quit_pending {
            match key {
                Key::Char('y') | Key::Char('Y') => {
                    self.save_file();
                    self.running = false;
                }
                Key::Char('n') | Key::Char('N') => {
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
                    self.cmd_buf.open(CommandBufferMode::Find, "find: ", "");
                    self.find_matches.clear();
                }
                EditorAction::CtrlBackspace => self.ctrl_backspace(),
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
            Key::PageUp => self.page_up(),
            Key::PageDown => self.page_down(),

            Key::Esc => {
                self.clear_selection();
                self.find_matches.clear();
            }

            // Editing
            Key::Backspace => self.backspace(),
            Key::Char('\t') => self.insert_tab(),
            Key::Char('\n') => self.insert_newline(),
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
            self.running = false;
        }
    }

    // -- command buffer key handling ----------------------------------------

    fn handle_cmd_key(&mut self, key: Key) {
        let mode = self.cmd_buf.mode;
        let result = self.cmd_buf.handle_key(key);

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
                    }
                }
            }
            CommandBufferResult::Cancel => {
                self.cmd_buf.close();
                if mode == CommandBufferMode::Find {
                    self.find_matches.clear();
                }
            }
            CommandBufferResult::Changed(val) => {
                if mode == CommandBufferMode::Find {
                    self.update_find_highlights(&val);
                }
            }
            CommandBufferResult::Continue => {}
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
            CommandAction::Quit => self.running = false,
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
        self.find_pattern = pattern.to_string();
        if pattern.is_empty() {
            return;
        }

        // Smart-case: case-insensitive if all lowercase
        let case_insensitive = pattern.chars().all(|c| !c.is_uppercase());
        let re = if case_insensitive {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
        } else {
            regex::Regex::new(pattern)
        };

        let re = match re {
            Ok(r) => r,
            Err(_) => return, // invalid regex — just don't highlight
        };

        let contents = self.doc.buf.contents();
        let text = String::from_utf8_lossy(&contents);
        for m in re.find_iter(&text) {
            let start = self.doc.buf.offset_to_pos(m.start());
            let end = self.doc.buf.offset_to_pos(m.end());
            self.find_matches
                .push((Pos::new(start.0, start.1), Pos::new(end.0, end.1)));
        }
    }

    fn find_next_from_submit(&mut self, pattern: &str) {
        self.update_find_highlights(pattern);
        self.find_next();
    }

    fn find_next(&mut self) {
        if self.find_matches.is_empty() {
            self.set_status("Find: no matches".to_string());
            return;
        }
        let cursor = self.cursor();
        // Find the next match after cursor
        let idx = self
            .find_matches
            .iter()
            .position(|(start, _)| *start > cursor);
        let idx = idx.unwrap_or(0); // wrap around
        self.find_index = idx;
        let (start, _end) = self.find_matches[idx];
        self.set_cursor(start);
    }

    // -- replace all --------------------------------------------------------

    fn replace_all(&mut self, pattern: &str, replacement: &str) {
        let case_insensitive = pattern.chars().all(|c| !c.is_uppercase());
        let re = if case_insensitive {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
        } else {
            regex::Regex::new(pattern)
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

        let row = (y as usize).saturating_sub(1);
        let col = (x as usize).saturating_sub(1);

        let logical_line = (self.view.scroll_line + row).min(line_count.saturating_sub(1));
        let text_col = col.saturating_sub(gw) + self.view.scroll_col;
        let line_len = self.doc.buf.line_char_len(logical_line);
        Pos::new(logical_line, text_col.min(line_len))
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
                anchor: Pos::new(pos.line, start),
                cursor: Pos::new(pos.line, end),
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
        self.view.scroll_line = self.view.scroll_line.saturating_sub(SCROLL_LINES);
        let max_visible = self.view.scroll_line + self.view.text_rows().saturating_sub(1);
        if self.cursor().line > max_visible {
            let line_len = self.doc.buf.line_char_len(max_visible);
            self.set_cursor(Pos::new(max_visible, self.cursor().col.min(line_len)));
        }
    }

    fn scroll_down(&mut self) {
        let line_count = self.doc.buf.line_count();
        let max_scroll = line_count.saturating_sub(1);
        self.view.scroll_line = (self.view.scroll_line + SCROLL_LINES).min(max_scroll);
        if self.cursor().line < self.view.scroll_line {
            let line_len = self.doc.buf.line_char_len(self.view.scroll_line);
            self.set_cursor(Pos::new(
                self.view.scroll_line,
                self.cursor().col.min(line_len),
            ));
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

    fn move_left(&mut self) {
        if !self.sel.is_empty() {
            let (start, _) = self.sel.ordered();
            self.set_cursor(start);
            return;
        }
        let c = self.cursor();
        if c.col > 0 {
            self.set_cursor(Pos::new(c.line, c.col - 1));
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
            self.set_cursor(Pos::new(c.line, c.col + 1));
        } else if c.line + 1 < self.doc.buf.line_count() {
            self.set_cursor(Pos::new(c.line + 1, 0));
        }
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
            self.sel.cursor = Pos::new(c.line, c.col - 1);
        } else if c.line > 0 {
            let prev_len = self.doc.buf.line_char_len(c.line - 1);
            self.sel.cursor = Pos::new(c.line - 1, prev_len);
        }
    }

    fn move_right_extend(&mut self) {
        let c = self.cursor();
        let line_len = self.doc.buf.line_char_len(c.line);
        if c.col < line_len {
            self.sel.cursor = Pos::new(c.line, c.col + 1);
        } else if c.line + 1 < self.doc.buf.line_count() {
            self.sel.cursor = Pos::new(c.line + 1, 0);
        }
    }

    // -- editing ------------------------------------------------------------

    fn insert_char(&mut self, c: char) {
        if !self.sel.is_empty() {
            self.delete_selection();
        }
        let mut bytes = [0u8; 4];
        let s = c.encode_utf8(&mut bytes);
        let pos = self
            .doc
            .insert(self.cursor().line, self.cursor().col, s.as_bytes());
        self.set_cursor(pos);
    }

    fn insert_tab(&mut self) {
        if !self.sel.is_empty() {
            self.delete_selection();
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
            let line_text = self.doc.buf.line_text(c.line);
            let leading_ws: usize = line_text
                .iter()
                .take_while(|&&b| b == b' ' || b == b'\t')
                .count();

            if c.col <= leading_ws && c.col >= 2 {
                let before = &line_text[..c.col];
                let all_spaces = before.iter().all(|&b| b == b' ');
                if all_spaces && c.col.is_multiple_of(2) {
                    let end = Pos::new(c.line, c.col);
                    let start = Pos::new(c.line, c.col - 2);
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
        if text.is_empty() {
            return;
        }
        if !self.sel.is_empty() {
            self.delete_selection();
        }
        self.doc.seal_undo();
        let pos = self
            .doc
            .insert(self.cursor().line, self.cursor().col, text.as_bytes());
        self.doc.seal_undo();
        self.set_cursor(pos);
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

    fn save_file(&mut self) {
        if let Some(ref path) = self.doc.filename {
            match crate::file_io::write_file(std::path::Path::new(path), &self.doc.buf.contents()) {
                Ok(()) => {
                    self.doc.dirty = false;
                    self.set_status(format!("Saved {}", path));
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
}
