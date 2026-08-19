use std::fs::File;
use std::io::{self, Read, Write};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::clipboard::Clipboard;
use crate::command::{CommandAction, CommandRegistry};
use crate::command_buffer::{CommandBuffer, CommandBufferMode, CommandBufferResult};
use crate::document::{Document, TextEdit};
use crate::find::FindState;
use crate::input::{self, EditorEvent, InputParser, Key, MouseButton, MouseEvent, MouseMods};
use crate::keybind::{EditorAction, KeybindingTable};
use crate::language::DetectedLanguage;
use crate::mouse::MouseState;
use crate::render::{Renderer, gutter_width};
use crate::selection::{
    CaretSet, CaretSnapshot, Selection, TextPosition, is_word_char, next_word_boundary,
    prev_word_boundary,
};
use crate::viewport::Viewport;

const SCROLL_LINES: usize = 3;

fn auto_close_char(c: char, lang_name: Option<&str>) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        // Skip single-quote autoclose for plain Text and Markdown: apostrophes
        // and contractions make it far too noisy there.
        '\'' if lang_name.is_some_and(|n| n != "Markdown") => Some('\''),
        '`' => Some('`'),
        _ => None,
    }
}

fn is_close_char(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '"' | '\'' | '`')
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

// The editor keeps character positions in its own selection model. Convert
// only at this storage-agnostic matching boundary; the reusable highlighter
// remains byte-oriented and does not depend on editor types.
fn find_bracket_match(
    pos: TextPosition,
    get_line: &mut impl FnMut(usize, &mut Vec<u8>),
    scratch: &mut Vec<u8>,
    line_count: usize,
) -> Option<TextPosition> {
    hi_lite::find_bracket_match(
        hi_lite::TextPosition::new(pos.line, pos.column),
        get_line,
        scratch,
        line_count,
    )
    .map(|position| TextPosition::new(position.line, position.column))
}

fn find_quote_match(
    pos: TextPosition,
    get_line: &mut impl FnMut(usize, &mut Vec<u8>),
    scratch: &mut Vec<u8>,
    line_count: usize,
) -> Option<TextPosition> {
    hi_lite::find_quote_match(
        hi_lite::TextPosition::new(pos.line, pos.column),
        get_line,
        scratch,
        line_count,
    )
    .map(|position| TextPosition::new(position.line, position.column))
}

pub struct Editor {
    document: Document,
    carets: CaretSet,
    viewport: Viewport,
    renderer: Renderer,
    clipboard: Clipboard,
    commands: CommandRegistry,
    keybindings: KeybindingTable,
    command_buffer: CommandBuffer,
    line_numbers_visible: bool,
    status_message: String,
    status_time: Option<Instant>,
    running: bool,
    /// Pending quit confirmation (dirty buffer).
    quit_pending: bool,
    mouse: MouseState,
    find: FindState,
    /// Temp file path for sudo save flow.
    sudo_save_tmp: Option<String>,
    /// True when stdin was a pipe (e.g. `git show | e`).
    piped_stdin: bool,
    /// Cached file mtime for external modification detection.
    file_modification_time: Option<std::time::SystemTime>,
    /// Waiting for y/n response to reload prompt.
    reload_pending: bool,
    /// Cached status-left string; reused each frame to avoid per-draw allocation.
    status_left_text_cache: String,
    /// Scratch buffer for line_text_into; avoids per-call allocation.
    line_text_scratch: Vec<u8>,
}

struct PlannedCaretEdit {
    start_byte: usize,
    end_byte: usize,
    inserted_bytes: Vec<u8>,
    deleted_bytes: Vec<u8>,
    anchor_after_byte: usize,
    cursor_after_byte: usize,
}

impl Editor {
    pub fn new(text: Vec<u8>, file_path: Option<String>, piped_stdin: bool) -> Self {
        let (w, h) = input::terminal_size().unwrap_or((80, 24));
        let mut keybindings = KeybindingTable::with_defaults();
        keybindings.load_config();
        let mut document = Document::new(text, file_path);
        let file_modification_time = document
            .file_path
            .as_ref()
            .and_then(|name| crate::file_io::file_modification_time(std::path::Path::new(name)));
        let mut restored_cursor = None;
        if let Some(ref name) = document.file_path {
            let path = std::path::Path::new(name);
            if path.exists() {
                crate::file_io::load_undo_history(path, &mut document.undo_stack);
            }
            restored_cursor = crate::file_io::load_cursor_position(path);
        }
        // Clamp restored cursor to buffer bounds
        let initial_cursor = if let Some(pos) = restored_cursor {
            let line_count = document.buffer.line_count();
            let line = pos.line.min(line_count.saturating_sub(1));
            let column = pos.column.min(document.buffer.line_character_count(line));
            TextPosition::new(line, column)
        } else {
            TextPosition::zero()
        };
        Self {
            document,
            carets: CaretSet::new(initial_cursor),
            viewport: Viewport::new(w, h),
            renderer: Renderer::new(),
            clipboard: Clipboard::detect(),
            commands: CommandRegistry::new(),
            keybindings,
            command_buffer: CommandBuffer::new(),
            line_numbers_visible: true,
            status_message: String::new(),
            status_time: None,
            running: true,
            quit_pending: false,
            mouse: MouseState::new(),
            find: FindState::new(),
            sudo_save_tmp: None,
            piped_stdin,
            file_modification_time,
            reload_pending: false,
            status_left_text_cache: String::new(),
            line_text_scratch: Vec::new(),
        }
    }

    pub fn run(&mut self) -> io::Result<()> {
        // Center viewport on restored cursor position
        if self.cursor() != TextPosition::zero() {
            self.center_view_on_line(self.cursor().line);
        }

        let old_termios = input::enable_raw_mode()?;
        let mut stdout = io::stdout();

        write!(stdout, "\x1b[?1049h")?;
        write!(
            stdout,
            "\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1004h\x1b[?25l"
        )?;
        stdout.flush()?;

        let (tx, rx) = mpsc::channel::<EditorEvent>();
        let tx_input = tx.clone();
        let use_tty = self.piped_stdin;
        std::thread::spawn(move || {
            let mut reader: Box<dyn Read> = if use_tty {
                match File::open("/dev/tty") {
                    Ok(f) => Box::new(f),
                    Err(_) => return,
                }
            } else {
                Box::new(io::stdin())
            };
            let mut parser = InputParser::new();
            let mut buf = [0u8; 256];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for &b in &buf[..n] {
                            if let Some(ev) = parser.advance(b)
                                && tx_input.send(ev).is_err()
                            {
                                return;
                            }
                        }
                        // After each read burst, flush pending bare ESC.
                        // Terminal emulators send escape sequences atomically,
                        // so a pending ESC means the user pressed Escape alone.
                        if let Some(ev) = parser.flush()
                            && tx_input.send(ev).is_err()
                        {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        crate::signal::register_sigwinch();

        while self.running {
            // Expire status messages after 3 seconds
            if let Some(t) = self.status_time
                && t.elapsed().as_secs() >= 3
            {
                self.status_message.clear();
                self.status_time = None;
            }

            self.draw(&mut stdout)?;

            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(ev) => {
                    self.dispatch_event(ev);
                    // Drain all pending events before re-rendering so bursts
                    // (e.g. rapid scroll wheel) coalesce into a single frame.
                    while self.running {
                        match rx.try_recv() {
                            Ok(ev) => self.dispatch_event(ev),
                            Err(_) => break,
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if crate::signal::take_sigwinch()
                        && let Ok((w, h)) = input::terminal_size()
                    {
                        self.resize_view(w, h);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        write!(
            stdout,
            "\x1b[?1004l\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?25h"
        )?;
        write!(stdout, "\x1b[?1049l")?;
        stdout.flush()?;
        input::disable_raw_mode(&old_termios)?;
        Ok(())
    }

    /// Resize the terminal while keeping the logical center near the same content.
    fn resize_view(&mut self, width: u16, height: u16) {
        let line_count = self.document.buffer.line_count();
        let gutter = if self.line_numbers_visible {
            gutter_width(line_count)
        } else {
            0
        };
        let anchor = {
            let buf = &self.document.buffer;
            let mut widths = |line: usize| buf.display_column_at(line, usize::MAX);
            self.viewport.center_anchor(line_count, gutter, &mut widths)
        };

        self.viewport.width = width;
        self.viewport.height = height;

        if let Some(anchor) = anchor {
            let buf = &self.document.buffer;
            let mut widths = |line: usize| buf.display_column_at(line, usize::MAX);
            self.viewport
                .center_on_anchor(anchor, line_count, gutter, &mut widths);
        }
        self.renderer.force_full_redraw();
    }

    fn dispatch_event(&mut self, ev: EditorEvent) {
        match ev {
            EditorEvent::Key(key) => {
                if self.command_buffer.active {
                    self.handle_cmd_key(key);
                } else {
                    self.handle_key(key);
                }
            }
            EditorEvent::Mouse(mouse, mods) => {
                if !self.command_buffer.active {
                    if self.find.active {
                        self.exit_find_mode();
                    }
                    self.handle_mouse(mouse, mods);
                }
            }
            EditorEvent::Paste(text) => {
                if self.command_buffer.active {
                    let result = self.command_buffer.insert_str(&text);
                    let mode = self.command_buffer.mode;
                    self.handle_cmd_result(mode, result);
                } else {
                    self.paste_text(&text);
                }
            }
            EditorEvent::FocusIn => {
                self.check_external_modification();
            }
        }
    }

    fn set_status(&mut self, msg: String) {
        self.status_message = msg;
        self.status_time = Some(Instant::now());
    }

    fn cursor(&self) -> TextPosition {
        self.carets.cursor()
    }

    fn set_cursor(&mut self, pos: TextPosition) {
        self.carets.set_single_selection(Selection::caret(pos));
    }

    fn set_cursor_preserving_desired_col(&mut self, pos: TextPosition) {
        let desired_column = self.desired_column();
        self.carets.set_single_selection(Selection::caret(pos));
        self.set_desired_col(desired_column);
    }

    fn set_selection(&mut self, selection: Selection) {
        self.carets.set_single_selection(selection);
    }

    fn restore_carets(&mut self, snapshot: CaretSnapshot) {
        self.carets.restore(snapshot);
    }

    fn has_selection(&self) -> bool {
        !self.carets.selection().is_empty()
    }

    fn selection(&self) -> Selection {
        self.carets.selection()
    }

    fn desired_column(&self) -> Option<usize> {
        self.carets.primary().desired_column
    }

    fn set_desired_col(&mut self, desired_column: Option<usize>) {
        self.carets.primary_mut().desired_column = desired_column;
    }

    fn move_cursor(&mut self, pos: TextPosition, extend: bool) {
        if extend {
            self.carets.primary_mut().selection.cursor = pos;
        } else {
            self.set_cursor_preserving_desired_col(pos);
        }
    }

    fn mutate_carets(
        &mut self,
        normalize: bool,
        mut f: impl FnMut(&Document, &mut Vec<u8>, &mut crate::selection::Caret),
    ) {
        let document = &self.document;
        let mut scratch = std::mem::take(&mut self.line_text_scratch);
        for caret in &mut self.carets.carets {
            f(document, &mut scratch, caret);
        }
        self.line_text_scratch = scratch;
        if normalize {
            self.carets.normalize();
        }
    }

    fn offset_for_pos(&self, pos: TextPosition) -> usize {
        self.document
            .buffer
            .position_to_byte_offset(pos.line, pos.column)
    }

    fn map_offset_after_edits(offset: usize, edits: &[TextEdit]) -> usize {
        let mut delta: isize = 0;
        for edit in edits {
            let mapped_start = (edit.start_byte as isize + delta) as usize;
            if offset < edit.start_byte {
                break;
            }
            if offset <= edit.end_byte {
                return mapped_start + edit.inserted_bytes.len();
            }
            delta +=
                edit.inserted_bytes.len() as isize - (edit.end_byte - edit.start_byte) as isize;
        }
        (offset as isize + delta) as usize
    }

    fn apply_text_edits_with_offsets(
        &mut self,
        text_edits: Vec<TextEdit>,
        final_offsets: Vec<(usize, usize)>,
    ) {
        let before = self.carets.snapshot();
        let after =
            self.document
                .apply_batch(&text_edits, &before, &final_offsets, self.carets.primary);
        self.restore_carets(after);
    }

    fn apply_text_edits_preserving_carets(&mut self, mut text_edits: Vec<TextEdit>) {
        if text_edits.is_empty() {
            return;
        }
        text_edits.sort_by_key(|edit| edit.start_byte);
        let before = self.carets.snapshot();
        let final_offsets: Vec<(usize, usize)> = before
            .selections
            .iter()
            .map(|selection| {
                let anchor = Self::map_offset_after_edits(
                    self.offset_for_pos(selection.anchor),
                    &text_edits,
                );
                let cursor = Self::map_offset_after_edits(
                    self.offset_for_pos(selection.cursor),
                    &text_edits,
                );
                (anchor, cursor)
            })
            .collect();
        self.apply_text_edits_with_offsets(text_edits, final_offsets);
    }

    fn apply_multi_caret_edits(&mut self, planned: Vec<PlannedCaretEdit>) {
        let mut final_offsets = Vec::with_capacity(planned.len());
        let mut delta: isize = 0;
        let mut text_edits = Vec::with_capacity(planned.len());

        for edit in planned {
            final_offsets.push((
                (edit.anchor_after_byte as isize + delta) as usize,
                (edit.cursor_after_byte as isize + delta) as usize,
            ));
            delta +=
                edit.inserted_bytes.len() as isize - (edit.end_byte - edit.start_byte) as isize;
            text_edits.push(TextEdit {
                start_byte: edit.start_byte,
                end_byte: edit.end_byte,
                inserted_bytes: edit.inserted_bytes,
                deleted_bytes: edit.deleted_bytes,
            });
        }

        self.apply_text_edits_with_offsets(text_edits, final_offsets);
    }

    fn line_range_for_selection(selection: Selection) -> (usize, usize) {
        let (start, end) = selection.ordered();
        let end_line = if !selection.is_empty() && end.column == 0 && end.line > start.line {
            end.line - 1
        } else {
            end.line
        };
        (start.line, end_line)
    }

    fn targeted_lines_for_carets(&self) -> Vec<usize> {
        let mut lines = std::collections::BTreeSet::new();
        for caret in self.carets.iter() {
            let (start_line, end_line) = Self::line_range_for_selection(caret.selection);
            for line in start_line..=end_line {
                lines.insert(line);
            }
        }
        lines.into_iter().collect()
    }

    fn indent_snap_left_for(document: &Document, line: usize, column: usize) -> usize {
        let ls = document.buffer.line_start(line);
        let le = document.buffer.line_end(line);
        let mut leading_ws = 0;
        while ls + leading_ws < le {
            match document.buffer.byte_at(ls + leading_ws) {
                b' ' | b'\t' => leading_ws += 1,
                _ => break,
            }
        }
        if column <= leading_ws
            && column >= 1
            && (0..column).all(|i| document.buffer.byte_at(ls + i) == b' ')
        {
            return (column - 1) / 2 * 2;
        }
        column - 1
    }

    fn indent_snap_right_for(document: &Document, line: usize, column: usize) -> usize {
        let ls = document.buffer.line_start(line);
        let le = document.buffer.line_end(line);
        let mut leading_ws = 0;
        let mut all_spaces = true;
        while ls + leading_ws < le {
            match document.buffer.byte_at(ls + leading_ws) {
                b' ' => leading_ws += 1,
                b'\t' => {
                    all_spaces = false;
                    leading_ws += 1;
                }
                _ => break,
            }
        }
        if column < leading_ws && all_spaces {
            return ((column / 2 + 1) * 2).min(leading_ws);
        }
        column + 1
    }

    fn bracket_jump_target_at(
        document: &Document,
        scratch: &mut Vec<u8>,
        pos: TextPosition,
    ) -> Option<TextPosition> {
        find_bracket_match(
            pos,
            &mut |line_idx, buf| document.buffer.line_text_into(line_idx, buf),
            scratch,
            document.buffer.line_count(),
        )
    }

    fn use_tab_indent(&self) -> bool {
        self.document.file_path.as_ref().is_some_and(|f| {
            f.ends_with(".c") || f.ends_with(".h") || f.ends_with(".go") || f.contains("Makefile")
        })
    }

    fn draw(&mut self, out: &mut impl Write) -> io::Result<()> {
        let line_count = self.document.buffer.line_count();
        let gw = if self.line_numbers_visible {
            gutter_width(line_count)
        } else {
            0
        };

        let display_col = self.cursor_display_col();
        let cursor_line = self.cursor().line;
        let mut line_display_width =
            |line: usize| -> usize { self.document.buffer.display_column_at(line, usize::MAX) };
        self.viewport
            .ensure_cursor_visible(cursor_line, display_col, gw, &mut line_display_width);

        let lang = self.document.detect_language();
        let lang_name = lang.map(|l| l.name).unwrap_or("Text");
        let selection = if self.selection().is_empty() {
            None
        } else {
            Some(self.selection())
        };
        let secondary_selections: Vec<Selection> = self
            .carets
            .iter()
            .enumerate()
            .filter_map(|(idx, caret)| {
                if idx == self.carets.primary || caret.selection.is_empty() {
                    None
                } else {
                    Some(caret.selection)
                }
            })
            .collect();
        let secondary_cursors: Vec<TextPosition> = self
            .carets
            .iter()
            .enumerate()
            .filter_map(|(idx, caret)| {
                if idx == self.carets.primary || !caret.selection.is_empty() {
                    None
                } else {
                    Some(caret.selection.cursor)
                }
            })
            .collect();
        let line_numbers_visible = self.line_numbers_visible;

        // All &mut self calls must happen before we borrow status_left_text_cache.
        let bracket_pair = self.find_matching_bracket();

        // Refresh viewport matches on every draw (cheap — only scans visible lines).
        if self.find.re.is_some() {
            self.find
                .refresh_viewport_matches(&self.document.buffer, &self.viewport);
        }

        let language = lang.and_then(DetectedLanguage::syntax);
        self.renderer.set_language(language);
        if self.carets.is_multicursor() {
            self.renderer.force_full_redraw();
        }

        // Pure reads — no more &mut self after this point.
        let find_matches = if !self.find.matches.is_empty() {
            Some(self.find.matches.as_slice())
        } else {
            None
        };
        let find_current = if self.find.active {
            self.find.current
        } else {
            None
        };

        let completions = &self.command_buffer.completions;

        let cmd_cursor = if self.command_buffer.active {
            Some(self.command_buffer.prompt.len() + self.command_buffer.cursor)
        } else {
            None
        };

        // Avoid cloning status_message: borrow it directly as &str.
        let display_line_owned;
        let cmd_ref: Option<&str> = if self.command_buffer.active {
            display_line_owned = self.command_buffer.display_line();
            Some(&display_line_owned)
        } else if !self.status_message.is_empty() {
            Some(&self.status_message)
        } else {
            None
        };

        // Rebuild status_left into the reused cache buffer (no allocation after warm-up).
        let name = self.document.file_path.as_deref().unwrap_or("[scratch]");
        Self::build_status_left(
            name,
            self.document.is_dirty,
            lang_name,
            &mut self.status_left_text_cache,
        );
        let status_left = &self.status_left_text_cache;
        let status_right = Self::status_right();

        self.renderer.render(
            out,
            &mut self.document.buffer,
            &self.viewport,
            cursor_line,
            display_col,
            line_numbers_visible,
            status_left,
            status_right,
            cmd_ref,
            selection,
            &secondary_selections,
            &secondary_cursors,
            find_matches,
            find_current,
            completions,
            cmd_cursor,
            self.find.active,
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
        let name = self.document.file_path.as_deref().unwrap_or("[scratch]");
        let mut s = String::new();
        Self::build_status_left(name, self.document.is_dirty, lang_name, &mut s);
        s
    }

    fn status_right() -> &'static str {
        concat!(" e v", env!("CARGO_PKG_VERSION"), " ")
    }

    fn center_view_on_line(&mut self, line: usize) {
        let gw = if self.line_numbers_visible {
            gutter_width(self.document.buffer.line_count())
        } else {
            0
        };
        let mut ldw = |l: usize| -> usize { self.document.buffer.display_column_at(l, usize::MAX) };
        self.viewport.center_on_line(line, &mut ldw, gw);
    }

    fn cursor_display_col(&self) -> usize {
        self.document
            .buffer
            .display_column_at(self.cursor().line, self.cursor().column)
    }

    fn find_matching_bracket(&mut self) -> Option<(TextPosition, TextPosition)> {
        let cursor = self.cursor();
        let line_count = self.document.buffer.line_count();
        // Reuse the editor's scratch buffer to avoid a per-frame allocation.
        let mut scratch = std::mem::take(&mut self.line_text_scratch);
        if let Some(match_pos) = find_bracket_match(
            cursor,
            &mut |line_idx, buf| self.document.buffer.line_text_into(line_idx, buf),
            &mut scratch,
            line_count,
        ) {
            self.line_text_scratch = scratch;
            return Some((cursor, match_pos));
        }
        let match_pos = find_quote_match(
            cursor,
            &mut |line_idx, buf| self.document.buffer.line_text_into(line_idx, buf),
            &mut scratch,
            line_count,
        );
        self.line_text_scratch = scratch;
        match_pos.map(|p| (cursor, p))
    }

    fn handle_key(&mut self, key: Key) {
        // Handle quit confirmation
        if self.quit_pending {
            match key {
                Key::Char('y') | Key::Char('Y') => {
                    self.save_file();
                    if !self.command_buffer.active {
                        // Named file: save completed (or failed); quit now.
                        // If command_buffer is active, save_file opened a "Save as:" prompt;
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
                    self.status_message.clear();
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
        if self.find.active {
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

        let desired_column = match key {
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => self.desired_column(),
            _ => None,
        };
        self.set_desired_col(desired_column);

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
                    self.line_numbers_visible = !self.line_numbers_visible;
                    self.renderer.force_full_redraw();
                }
                EditorAction::CommandPalette => {
                    self.command_buffer
                        .open(CommandBufferMode::Command, "> ", "");
                }
                EditorAction::GotoLine => {
                    self.command_buffer
                        .open(CommandBufferMode::Goto, "goto: ", "");
                }
                EditorAction::Find => {
                    let prefill = if self.has_selection() {
                        let (start, end) = self.selection().ordered();
                        let text = self.document.text_in_range(start, end);
                        let s = String::from_utf8_lossy(&text).to_string();
                        if s.len() <= 100 { s } else { String::new() }
                    } else {
                        String::new()
                    };
                    self.command_buffer
                        .open(CommandBufferMode::Find, "find: ", &prefill);
                    self.find.clear();
                    self.find.search_start = self.cursor();
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
            Key::CtrlUp => self.move_up(),
            Key::CtrlDown => self.move_down(),
            Key::CtrlShiftUp => self.select_above(),
            Key::CtrlShiftDown => self.select_below(),
            Key::CtrlShiftLeft => self.word_left_extend(),
            Key::CtrlShiftRight => self.word_right_extend(),
            Key::PageUp => self.page_up(),
            Key::PageDown => self.page_down(),

            Key::Esc => {
                self.clear_selection();
                self.find.clear();
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
        if self.document.is_dirty {
            let name = self.document.file_path.as_deref().unwrap_or("[scratch]");
            self.status_message = format!("Save changes to {}? (y/n)", name);
            self.status_time = None; // don't expire this message
            self.quit_pending = true;
        } else {
            self.save_undo_if_named();
            self.running = false;
        }
    }

    fn save_undo_if_named(&mut self) {
        if let Some(name) = self.document.file_path.clone() {
            let path = std::path::Path::new(&name);
            crate::file_io::save_cursor_position(path, self.cursor());
            if path.exists() {
                self.document.seal_undo();
                crate::file_io::save_undo_history(path, &self.document.undo_stack);
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
        let mode = self.command_buffer.mode;
        let result = self.command_buffer.handle_key(key);
        self.handle_cmd_result(mode, result);
    }

    fn handle_cmd_result(&mut self, mode: CommandBufferMode, result: CommandBufferResult) {
        match result {
            CommandBufferResult::Submit(val) => {
                self.command_buffer.close();
                match mode {
                    CommandBufferMode::Command => self.execute_command(&val),
                    CommandBufferMode::Find => self.find_next_from_submit(&val),
                    CommandBufferMode::Goto => {
                        let cmd = format!("goto {}", val);
                        self.execute_command(&cmd);
                    }
                    CommandBufferMode::Prompt => {
                        // save-as prompt
                        self.document.file_path = Some(val.clone());
                        self.save_file();
                        if self.quit_pending && !self.command_buffer.active {
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
                self.command_buffer.close();
                if mode == CommandBufferMode::Find {
                    self.find.clear();
                    self.status_message.clear();
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
                    if let Some((_, end)) = self.find.current {
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
        let input = self.command_buffer.input.trim().to_string();
        let names = self.commands.command_names();

        if input.is_empty() {
            // Show all commands
            self.command_buffer.completions = names.iter().map(|s| s.to_string()).collect();
        } else {
            let matches: Vec<&str> = names
                .iter()
                .filter(|n| n.starts_with(&input))
                .copied()
                .collect();

            match matches.len() {
                0 => {
                    self.command_buffer.completions.clear();
                }
                1 => {
                    // Single match — autocomplete
                    self.command_buffer.input = matches[0].to_string();
                    self.command_buffer.cursor = self.command_buffer.input.len();
                    self.command_buffer.completions.clear();
                }
                _ => {
                    // Multiple matches — show them and complete common prefix
                    self.command_buffer.completions =
                        matches.iter().map(|s| s.to_string()).collect();
                    let common = common_prefix(&matches);
                    if common.len() > input.len() {
                        self.command_buffer.input = common;
                        self.command_buffer.cursor = self.command_buffer.input.len();
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
                self.document.file_path = Some(name);
                self.save_file();
            }
            CommandAction::Quit => {
                self.save_undo_if_named();
                self.running = false;
            }
            CommandAction::Goto(n) => self.goto_line(n),
            CommandAction::ToggleRuler => {
                self.line_numbers_visible = !self.line_numbers_visible;
                self.renderer.force_full_redraw();
            }
            CommandAction::ReplaceAll {
                pattern,
                replacement,
            } => {
                self.replace_all(&pattern, &replacement);
            }
            CommandAction::Find(pattern) => {
                self.find.search_start = self.cursor();
                self.find_next_from_submit(&pattern);
            }
            CommandAction::ToggleComment => self.toggle_comment(),
            CommandAction::CommentOn => self.set_comment(true),
            CommandAction::CommentOff => self.set_comment(false),
            CommandAction::SelectAll => self.select_all(),
            CommandAction::Trim => self.strip_trailing_whitespace(),
            CommandAction::TabsToSpaces => self.tabs_to_spaces(),
            CommandAction::SpacesToTabs => self.spaces_to_tabs(),
            CommandAction::StatusMsg(msg) => self.set_status(msg),
        }
    }

    fn goto_line(&mut self, n: usize) {
        let line_count = self.document.buffer.line_count();
        let target = if n == 0 {
            0
        } else {
            (n - 1).min(line_count.saturating_sub(1))
        };
        self.set_cursor(TextPosition::new(target, 0));
        self.center_view_on_line(target);
        self.renderer.force_full_redraw();
    }

    fn goto_top(&mut self) {
        self.set_cursor(TextPosition::zero());
        self.renderer.force_full_redraw();
    }

    fn goto_end(&mut self) {
        let line_count = self.document.buffer.line_count();
        let last_line = line_count.saturating_sub(1);
        let last_col = self.document.buffer.line_character_count(last_line);
        self.set_cursor(TextPosition::new(last_line, last_col));
        self.renderer.force_full_redraw();
    }

    fn kill_line(&mut self) {
        let c = self.cursor();
        let line_count = self.document.buffer.line_count();
        if line_count == 0 {
            return;
        }
        self.document.seal_undo();
        let start = TextPosition::new(c.line, 0);
        let end = if c.line + 1 < line_count {
            TextPosition::new(c.line + 1, 0)
        } else {
            let len = self.document.buffer.line_character_count(c.line);
            TextPosition::new(c.line, len)
        };
        self.document.delete_range(start, end);
        self.document.seal_undo();
        // Clamp cursor
        let new_line_count = self.document.buffer.line_count();
        let new_line = c.line.min(new_line_count.saturating_sub(1));
        let new_col = self
            .document
            .buffer
            .line_character_count(new_line)
            .min(c.column);
        self.set_cursor(TextPosition::new(new_line, new_col));
    }

    // -- find ---------------------------------------------------------------

    fn update_find_highlights(&mut self, pattern: &str) {
        self.find
            .update_highlights_lazy(pattern, &self.document.buffer, &self.viewport);
    }

    fn find_next_from_submit(&mut self, pattern: &str) {
        self.find
            .update_highlights(pattern, &self.document.buffer, &self.viewport);
        if self.find.current.is_none() {
            self.set_status("Find: no matches".to_string());
            return;
        }
        self.find.active = true;
        if let Some((_, end)) = self.find.current {
            self.set_cursor(end);
            self.center_view_on_line(end.line);
            self.set_find_status();
        }
    }

    fn find_next(&mut self) {
        let cursor = self.cursor();
        if let Some(m) = self.find.find_next(&self.document.buffer, cursor) {
            let (_, end) = m;
            self.set_cursor(end);
            self.center_view_on_line(end.line);
            self.set_find_status();
        }
    }

    fn find_prev(&mut self) {
        let cursor = self.cursor();
        if let Some(m) = self.find.find_prev(&self.document.buffer, cursor) {
            let (_, end) = m;
            self.set_cursor(end);
            self.center_view_on_line(end.line);
            self.set_find_status();
        }
    }

    fn set_find_status(&mut self) {
        self.status_message = self.find.status_text();
        self.status_time = None; // don't auto-expire while browsing
    }

    fn exit_find_mode(&mut self) {
        if let Some((start, end)) = self.find.exit() {
            self.set_selection(Selection {
                anchor: start,
                cursor: end,
            });
        }
        self.status_message.clear();
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
        let (range_start, range_end) = if self.has_selection() {
            self.selection().ordered()
        } else {
            let line_count = self.document.buffer.line_count();
            let last_line = line_count.saturating_sub(1);
            let last_col = self.document.buffer.line_character_count(last_line);
            (TextPosition::zero(), TextPosition::new(last_line, last_col))
        };

        let text_bytes = self.document.text_in_range(range_start, range_end);
        let text = String::from_utf8_lossy(&text_bytes);

        let count = re.find_iter(&text).count();
        if count == 0 {
            self.set_status("Replaced 0 occurrences".to_string());
            return;
        }

        let new_text = re.replace_all(&text, replacement).into_owned();

        self.document.seal_undo();
        self.document.replace_range_with_deleted(
            range_start,
            range_end,
            new_text.as_bytes(),
            text_bytes,
        );
        self.document.seal_undo();

        self.clear_selection();
        self.set_status(format!("Replaced {} occurrences", count));
    }

    // -- mouse handling -----------------------------------------------------

    fn handle_mouse(&mut self, mouse: MouseEvent, mods: MouseMods) {
        match mouse {
            MouseEvent::Press(MouseButton::Left, x, y) => self.mouse_press(x, y, mods),
            MouseEvent::Press(MouseButton::Right, x, y) if mods.ctrl => {
                self.mouse_press(x, y, mods)
            }
            MouseEvent::Press(MouseButton::Right, x, y)
                if cfg!(target_os = "macos") && !mods.ctrl =>
            {
                self.mouse_press(x, y, MouseMods { ctrl: true });
            }
            MouseEvent::Hold(x, y) => self.mouse_drag(x, y),
            MouseEvent::Release(_, _) => {
                self.mouse.release();
            }
            MouseEvent::Press(MouseButton::WheelUp, _, _) => self.scroll_up(),
            MouseEvent::Press(MouseButton::WheelDown, _, _) => self.scroll_down(),
            _ => {}
        }
    }

    fn screen_to_buffer_position(&self, x: u16, y: u16) -> TextPosition {
        crate::mouse::screen_to_buffer_position(
            x,
            y,
            &self.document.buffer,
            &self.viewport,
            self.line_numbers_visible,
        )
    }

    fn mouse_press(&mut self, x: u16, y: u16, mods: MouseMods) {
        let pos = self.screen_to_buffer_position(x, y);
        if mods.ctrl {
            self.carets.add_caret(pos);
            self.mouse.dragging = false;
            return;
        }
        let click_count = self.mouse.press(x, y);

        match click_count {
            1 => {
                self.set_cursor(pos);
                self.mouse.dragging = true;
            }
            2 => self.select_word_at(pos),
            3 => self.select_line_at(pos.line),
            _ => {}
        }
    }

    fn mouse_drag(&mut self, x: u16, y: u16) {
        if !self.mouse.dragging {
            return;
        }
        let pos = self.screen_to_buffer_position(x, y);
        self.carets.primary_mut().selection.cursor = pos;
    }

    fn select_word_at(&mut self, pos: TextPosition) {
        self.document
            .buffer
            .line_text_into(pos.line, &mut self.line_text_scratch);
        let line_text = &self.line_text_scratch;
        if line_text.is_empty() {
            return;
        }
        let column = pos.column.min(line_text.len().saturating_sub(1));
        if column < line_text.len() && is_word_char(line_text[column]) {
            let mut start = column;
            while start > 0 && is_word_char(line_text[start - 1]) {
                start -= 1;
            }
            let mut end = column;
            while end < line_text.len() && is_word_char(line_text[end]) {
                end += 1;
            }
            self.set_selection(Selection {
                anchor: TextPosition::new(pos.line, end),
                cursor: TextPosition::new(pos.line, start),
            });
        }
    }

    fn select_line_at(&mut self, line: usize) {
        let line_count = self.document.buffer.line_count();
        if line >= line_count {
            return;
        }
        let end = if line + 1 < line_count {
            TextPosition::new(line + 1, 0)
        } else {
            let len = self.document.buffer.line_character_count(line);
            TextPosition::new(line, len)
        };
        self.set_selection(Selection {
            anchor: TextPosition::new(line, 0),
            cursor: end,
        });
    }

    fn scroll_up(&mut self) {
        if self.viewport.scroll_line == 0 && self.viewport.scroll_wrap == 0 {
            return;
        }
        let gw = if self.line_numbers_visible {
            gutter_width(self.document.buffer.line_count())
        } else {
            0
        };
        let text_cols = self.viewport.text_cols(gw);
        // Scroll back by SCROLL_LINES screen rows
        let mut remaining = SCROLL_LINES;
        while remaining > 0 {
            if self.viewport.scroll_wrap > 0 {
                let step = remaining.min(self.viewport.scroll_wrap);
                self.viewport.scroll_wrap -= step;
                remaining -= step;
            } else if self.viewport.scroll_line > 0 {
                self.viewport.scroll_line -= 1;
                let dw = self
                    .document
                    .buffer
                    .display_column_at(self.viewport.scroll_line, usize::MAX);
                let wraps = crate::viewport::wrapped_rows(dw, text_cols);
                remaining -= 1;
                if remaining > 0 && wraps > 1 {
                    let step = remaining.min(wraps - 1);
                    self.viewport.scroll_wrap = (wraps - 1).saturating_sub(step);
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
        let line_count = self.document.buffer.line_count();
        if self.viewport.scroll_line >= line_count.saturating_sub(1) {
            return;
        }
        let gw = if self.line_numbers_visible {
            gutter_width(line_count)
        } else {
            0
        };
        let text_cols = self.viewport.text_cols(gw);
        // Scroll forward by SCROLL_LINES screen rows
        let mut remaining = SCROLL_LINES;
        while remaining > 0 && self.viewport.scroll_line < line_count.saturating_sub(1) {
            let dw = self
                .document
                .buffer
                .display_column_at(self.viewport.scroll_line, usize::MAX);
            let wraps = crate::viewport::wrapped_rows(dw, text_cols);
            let remaining_in_line = wraps.saturating_sub(self.viewport.scroll_wrap);
            if remaining < remaining_in_line {
                self.viewport.scroll_wrap += remaining;
                remaining = 0;
            } else {
                remaining -= remaining_in_line;
                self.viewport.scroll_line += 1;
                self.viewport.scroll_wrap = 0;
            }
        }
        // Move cursor into viewport if it scrolled past the top
        self.clamp_cursor_to_viewport(gw, text_cols);
    }

    /// After a mouse-wheel scroll, move the cursor so it stays within the visible area.
    fn clamp_cursor_to_viewport(&mut self, _gw: usize, text_cols: usize) {
        let text_rows = self.viewport.text_rows();
        if text_rows == 0 || text_cols == 0 {
            return;
        }
        let line_count = self.document.buffer.line_count();
        let cursor = self.cursor();
        let cursor_dcol = self.cursor_display_col();
        let cursor_wrap = cursor_dcol / text_cols;

        // Check if cursor is above viewport
        if cursor.line < self.viewport.scroll_line
            || (cursor.line == self.viewport.scroll_line && cursor_wrap < self.viewport.scroll_wrap)
        {
            // Snap cursor to the first visible position
            // The first visible char column is scroll_wrap * text_cols
            let first_dcol = self.viewport.scroll_wrap * text_cols;
            let character_column = self
                .document
                .buffer
                .character_column_from_display(self.viewport.scroll_line, first_dcol);
            let line_len = self
                .document
                .buffer
                .line_character_count(self.viewport.scroll_line);
            self.set_cursor(TextPosition::new(
                self.viewport.scroll_line,
                character_column.min(line_len),
            ));
            return;
        }

        // Check if cursor is below viewport — walk screen rows to find last visible position
        let mut screen_row = 0usize;
        let mut line_idx = self.viewport.scroll_line;
        let mut last_visible_line = self.viewport.scroll_line;
        let mut last_visible_wrap = self.viewport.scroll_wrap;

        while screen_row < text_rows && line_idx < line_count {
            let dw = self.document.buffer.display_column_at(line_idx, usize::MAX);
            let total = crate::viewport::wrapped_rows(dw, text_cols);
            let start_w = if line_idx == self.viewport.scroll_line {
                self.viewport.scroll_wrap
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
            let character_column = self
                .document
                .buffer
                .character_column_from_display(last_visible_line, target_dcol);
            let line_len = self.document.buffer.line_character_count(last_visible_line);
            self.set_cursor(TextPosition::new(
                last_visible_line,
                character_column.min(line_len),
            ));
        }
    }

    // -- selection helpers --------------------------------------------------

    fn delete_selection(&mut self) {
        if !self.has_selection() {
            return;
        }
        let (start, end) = self.selection().ordered();
        self.document.seal_undo();
        let pos = self.document.delete_range(start, end);
        self.document.seal_undo();
        self.set_cursor(pos);
    }

    fn clear_selection(&mut self) {
        self.set_cursor(self.cursor());
    }

    fn select_all(&mut self) {
        let line_count = self.document.buffer.line_count();
        let last_line = line_count.saturating_sub(1);
        let last_col = self.document.buffer.line_character_count(last_line);
        self.set_selection(Selection {
            anchor: TextPosition::zero(),
            cursor: TextPosition::new(last_line, last_col),
        });
    }

    fn select_above(&mut self) {
        self.set_selection(Selection {
            anchor: self.cursor(),
            cursor: TextPosition::zero(),
        });
        self.set_desired_col(None);
    }

    fn select_below(&mut self) {
        let last_line = self.document.buffer.line_count().saturating_sub(1);
        let last_col = self.document.buffer.line_character_count(last_line);
        self.set_selection(Selection {
            anchor: self.cursor(),
            cursor: TextPosition::new(last_line, last_col),
        });
        self.set_desired_col(None);
    }

    // -- movement (no selection) --------------------------------------------

    fn move_up_impl(&mut self, extend: bool) {
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, _, caret| {
                let cursor = caret.selection.cursor;
                if cursor.line == 0 {
                    return;
                }
                let target_col = caret.desired_column.unwrap_or(cursor.column);
                caret.desired_column = Some(target_col);
                let new_line = cursor.line - 1;
                let line_len = document.buffer.line_character_count(new_line);
                let pos = TextPosition::new(new_line, target_col.min(line_len));
                if extend {
                    caret.selection.cursor = pos;
                } else {
                    caret.selection = Selection::caret(pos);
                }
            });
            return;
        }
        if self.cursor().line > 0 {
            let target_col = self.desired_column().unwrap_or(self.cursor().column);
            self.set_desired_col(Some(target_col));
            let new_line = self.cursor().line - 1;
            let line_len = self.document.buffer.line_character_count(new_line);
            self.move_cursor(
                TextPosition::new(new_line, target_col.min(line_len)),
                extend,
            );
        }
    }

    fn move_up(&mut self) {
        self.move_up_impl(false);
    }

    fn move_down_impl(&mut self, extend: bool) {
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, _, caret| {
                let cursor = caret.selection.cursor;
                if cursor.line + 1 >= document.buffer.line_count() {
                    return;
                }
                let target_col = caret.desired_column.unwrap_or(cursor.column);
                caret.desired_column = Some(target_col);
                let new_line = cursor.line + 1;
                let line_len = document.buffer.line_character_count(new_line);
                let pos = TextPosition::new(new_line, target_col.min(line_len));
                if extend {
                    caret.selection.cursor = pos;
                } else {
                    caret.selection = Selection::caret(pos);
                }
            });
            return;
        }
        let line_count = self.document.buffer.line_count();
        if self.cursor().line + 1 < line_count {
            let target_col = self.desired_column().unwrap_or(self.cursor().column);
            self.set_desired_col(Some(target_col));
            let new_line = self.cursor().line + 1;
            let line_len = self.document.buffer.line_character_count(new_line);
            self.move_cursor(
                TextPosition::new(new_line, target_col.min(line_len)),
                extend,
            );
        }
    }

    fn move_down(&mut self) {
        self.move_down_impl(false);
    }

    fn indent_snap_left(&mut self, line: usize, column: usize) -> usize {
        Self::indent_snap_left_for(&self.document, line, column)
    }

    fn indent_snap_right(&mut self, line: usize, column: usize) -> usize {
        Self::indent_snap_right_for(&self.document, line, column)
    }

    fn move_left_impl(&mut self, extend: bool) {
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, _, caret| {
                if !extend && !caret.selection.is_empty() {
                    let (start, _) = caret.selection.ordered();
                    caret.selection = Selection::caret(start);
                    return;
                }
                let cursor = caret.selection.cursor;
                if cursor.column > 0 {
                    let new_col = Self::indent_snap_left_for(document, cursor.line, cursor.column);
                    let pos = TextPosition::new(cursor.line, new_col);
                    if extend {
                        caret.selection.cursor = pos;
                    } else {
                        caret.selection = Selection::caret(pos);
                    }
                } else if cursor.line > 0 {
                    let prev_len = document.buffer.line_character_count(cursor.line - 1);
                    let pos = TextPosition::new(cursor.line - 1, prev_len);
                    if extend {
                        caret.selection.cursor = pos;
                    } else {
                        caret.selection = Selection::caret(pos);
                    }
                }
            });
            return;
        }
        if !extend && self.has_selection() {
            let (start, _) = self.selection().ordered();
            self.set_cursor(start);
            return;
        }
        let c = self.cursor();
        if c.column > 0 {
            let new_col = self.indent_snap_left(c.line, c.column);
            self.move_cursor(TextPosition::new(c.line, new_col), extend);
        } else if c.line > 0 {
            let prev_len = self.document.buffer.line_character_count(c.line - 1);
            self.move_cursor(TextPosition::new(c.line - 1, prev_len), extend);
        }
    }

    fn move_left(&mut self) {
        self.set_desired_col(None);
        self.move_left_impl(false);
    }

    fn move_right_impl(&mut self, extend: bool) {
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, _, caret| {
                if !extend && !caret.selection.is_empty() {
                    let (_, end) = caret.selection.ordered();
                    caret.selection = Selection::caret(end);
                    return;
                }
                let cursor = caret.selection.cursor;
                let line_len = document.buffer.line_character_count(cursor.line);
                if cursor.column < line_len {
                    let new_col = Self::indent_snap_right_for(document, cursor.line, cursor.column);
                    let pos = TextPosition::new(cursor.line, new_col);
                    if extend {
                        caret.selection.cursor = pos;
                    } else {
                        caret.selection = Selection::caret(pos);
                    }
                } else if cursor.line + 1 < document.buffer.line_count() {
                    let pos = TextPosition::new(cursor.line + 1, 0);
                    if extend {
                        caret.selection.cursor = pos;
                    } else {
                        caret.selection = Selection::caret(pos);
                    }
                }
            });
            return;
        }
        if !extend && self.has_selection() {
            let (_, end) = self.selection().ordered();
            self.set_cursor(end);
            return;
        }
        let c = self.cursor();
        let line_len = self.document.buffer.line_character_count(c.line);
        if c.column < line_len {
            let new_col = self.indent_snap_right(c.line, c.column);
            self.move_cursor(TextPosition::new(c.line, new_col), extend);
        } else if c.line + 1 < self.document.buffer.line_count() {
            self.move_cursor(TextPosition::new(c.line + 1, 0), extend);
        }
    }

    fn move_right(&mut self) {
        self.set_desired_col(None);
        self.move_right_impl(false);
    }

    /// If the cursor is on a bracket character, return the matching bracket position.
    fn bracket_jump_target(&mut self) -> Option<TextPosition> {
        let mut scratch = std::mem::take(&mut self.line_text_scratch);
        let result = Self::bracket_jump_target_at(&self.document, &mut scratch, self.cursor());
        self.line_text_scratch = scratch;
        result
    }

    fn word_left(&mut self) {
        self.set_desired_col(None);
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, scratch, caret| {
                caret.desired_column = None;
                if !caret.selection.is_empty() {
                    let (start, _) = caret.selection.ordered();
                    caret.selection = Selection::caret(start);
                    return;
                }
                let cursor = caret.selection.cursor;
                if let Some(target) = Self::bracket_jump_target_at(document, scratch, cursor) {
                    caret.selection = Selection::caret(target);
                    return;
                }
                if cursor.column == 0 {
                    if cursor.line > 0 {
                        let prev_len = document.buffer.line_character_count(cursor.line - 1);
                        caret.selection =
                            Selection::caret(TextPosition::new(cursor.line - 1, prev_len));
                    }
                    return;
                }
                document.buffer.line_text_into(cursor.line, scratch);
                let boundary = prev_word_boundary(scratch, cursor.column);
                caret.selection = Selection::caret(TextPosition::new(cursor.line, boundary));
            });
            return;
        }
        if self.has_selection() {
            let (start, _) = self.selection().ordered();
            self.set_cursor(start);
            return;
        }
        if let Some(target) = self.bracket_jump_target() {
            self.set_cursor(target);
            return;
        }
        let c = self.cursor();
        if c.column == 0 {
            if c.line > 0 {
                let prev_len = self.document.buffer.line_character_count(c.line - 1);
                self.set_cursor(TextPosition::new(c.line - 1, prev_len));
            }
            return;
        }
        self.document
            .buffer
            .line_text_into(c.line, &mut self.line_text_scratch);
        let boundary = prev_word_boundary(&self.line_text_scratch, c.column);
        self.set_cursor(TextPosition::new(c.line, boundary));
    }

    fn word_right(&mut self) {
        self.set_desired_col(None);
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, scratch, caret| {
                caret.desired_column = None;
                if !caret.selection.is_empty() {
                    let (_, end) = caret.selection.ordered();
                    caret.selection = Selection::caret(end);
                    return;
                }
                let cursor = caret.selection.cursor;
                if let Some(target) = Self::bracket_jump_target_at(document, scratch, cursor) {
                    caret.selection = Selection::caret(target);
                    return;
                }
                let line_len = document.buffer.line_character_count(cursor.line);
                if cursor.column >= line_len {
                    if cursor.line + 1 < document.buffer.line_count() {
                        caret.selection = Selection::caret(TextPosition::new(cursor.line + 1, 0));
                    }
                    return;
                }
                document.buffer.line_text_into(cursor.line, scratch);
                let boundary = next_word_boundary(scratch, cursor.column);
                caret.selection = Selection::caret(TextPosition::new(cursor.line, boundary));
            });
            return;
        }
        if self.has_selection() {
            let (_, end) = self.selection().ordered();
            self.set_cursor(end);
            return;
        }
        if let Some(target) = self.bracket_jump_target() {
            self.set_cursor(target);
            return;
        }
        let c = self.cursor();
        let line_len = self.document.buffer.line_character_count(c.line);
        if c.column >= line_len {
            if c.line + 1 < self.document.buffer.line_count() {
                self.set_cursor(TextPosition::new(c.line + 1, 0));
            }
            return;
        }
        self.document
            .buffer
            .line_text_into(c.line, &mut self.line_text_scratch);
        let boundary = next_word_boundary(&self.line_text_scratch, c.column);
        self.set_cursor(TextPosition::new(c.line, boundary));
    }

    fn word_left_extend(&mut self) {
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, scratch, caret| {
                caret.desired_column = None;
                let cursor = caret.selection.cursor;
                if let Some(target) = Self::bracket_jump_target_at(document, scratch, cursor) {
                    caret.selection.cursor = target;
                    return;
                }
                if cursor.column == 0 {
                    if cursor.line > 0 {
                        let prev_len = document.buffer.line_character_count(cursor.line - 1);
                        caret.selection.cursor = TextPosition::new(cursor.line - 1, prev_len);
                    }
                    return;
                }
                document.buffer.line_text_into(cursor.line, scratch);
                let boundary = prev_word_boundary(scratch, cursor.column);
                caret.selection.cursor = TextPosition::new(cursor.line, boundary);
            });
            return;
        }
        if let Some(target) = self.bracket_jump_target() {
            self.carets.primary_mut().selection.cursor = target;
            return;
        }
        let c = self.cursor();
        if c.column == 0 {
            if c.line > 0 {
                let prev_len = self.document.buffer.line_character_count(c.line - 1);
                self.carets.primary_mut().selection.cursor =
                    TextPosition::new(c.line - 1, prev_len);
            }
            return;
        }
        self.document
            .buffer
            .line_text_into(c.line, &mut self.line_text_scratch);
        let boundary = prev_word_boundary(&self.line_text_scratch, c.column);
        self.carets.primary_mut().selection.cursor = TextPosition::new(c.line, boundary);
    }

    fn word_right_extend(&mut self) {
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, scratch, caret| {
                caret.desired_column = None;
                let cursor = caret.selection.cursor;
                if let Some(target) = Self::bracket_jump_target_at(document, scratch, cursor) {
                    caret.selection.cursor = target;
                    return;
                }
                let line_len = document.buffer.line_character_count(cursor.line);
                if cursor.column >= line_len {
                    if cursor.line + 1 < document.buffer.line_count() {
                        caret.selection.cursor = TextPosition::new(cursor.line + 1, 0);
                    }
                    return;
                }
                document.buffer.line_text_into(cursor.line, scratch);
                let boundary = next_word_boundary(scratch, cursor.column);
                caret.selection.cursor = TextPosition::new(cursor.line, boundary);
            });
            return;
        }
        if let Some(target) = self.bracket_jump_target() {
            self.carets.primary_mut().selection.cursor = target;
            return;
        }
        let c = self.cursor();
        let line_len = self.document.buffer.line_character_count(c.line);
        if c.column >= line_len {
            if c.line + 1 < self.document.buffer.line_count() {
                self.carets.primary_mut().selection.cursor = TextPosition::new(c.line + 1, 0);
            }
            return;
        }
        self.document
            .buffer
            .line_text_into(c.line, &mut self.line_text_scratch);
        let boundary = next_word_boundary(&self.line_text_scratch, c.column);
        self.carets.primary_mut().selection.cursor = TextPosition::new(c.line, boundary);
    }

    fn move_home(&mut self) {
        self.set_desired_col(None);
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |_, _, caret| {
                caret.desired_column = None;
                let pos = TextPosition::new(caret.selection.cursor.line, 0);
                caret.selection = Selection::caret(pos);
            });
            return;
        }
        self.set_cursor(TextPosition::new(self.cursor().line, 0));
    }

    fn move_end(&mut self) {
        self.set_desired_col(None);
        if self.carets.is_multicursor() {
            self.mutate_carets(true, |document, _, caret| {
                caret.desired_column = None;
                let cursor = caret.selection.cursor;
                let len = document.buffer.line_character_count(cursor.line);
                caret.selection = Selection::caret(TextPosition::new(cursor.line, len));
            });
            return;
        }
        let c = self.cursor();
        let len = self.document.buffer.line_character_count(c.line);
        self.set_cursor(TextPosition::new(c.line, len));
    }

    fn page_up(&mut self) {
        if self.carets.is_multicursor() {
            let rows = self.viewport.text_rows();
            self.mutate_carets(true, |document, _, caret| {
                let cursor = caret.selection.cursor;
                let target_col = caret.desired_column.unwrap_or(cursor.column);
                caret.desired_column = Some(target_col);
                let new_line = cursor.line.saturating_sub(rows);
                let line_len = document.buffer.line_character_count(new_line);
                caret.selection =
                    Selection::caret(TextPosition::new(new_line, target_col.min(line_len)));
            });
            return;
        }
        let rows = self.viewport.text_rows();
        let target_col = self.desired_column().unwrap_or(self.cursor().column);
        self.set_desired_col(Some(target_col));
        let new_line = self.cursor().line.saturating_sub(rows);
        let line_len = self.document.buffer.line_character_count(new_line);
        self.set_cursor_preserving_desired_col(TextPosition::new(
            new_line,
            target_col.min(line_len),
        ));
    }

    fn page_down(&mut self) {
        if self.carets.is_multicursor() {
            let rows = self.viewport.text_rows();
            self.mutate_carets(true, |document, _, caret| {
                let cursor = caret.selection.cursor;
                let target_col = caret.desired_column.unwrap_or(cursor.column);
                caret.desired_column = Some(target_col);
                let new_line =
                    (cursor.line + rows).min(document.buffer.line_count().saturating_sub(1));
                let line_len = document.buffer.line_character_count(new_line);
                caret.selection =
                    Selection::caret(TextPosition::new(new_line, target_col.min(line_len)));
            });
            return;
        }
        let rows = self.viewport.text_rows();
        let line_count = self.document.buffer.line_count();
        let target_col = self.desired_column().unwrap_or(self.cursor().column);
        self.set_desired_col(Some(target_col));
        let new_line = (self.cursor().line + rows).min(line_count.saturating_sub(1));
        let line_len = self.document.buffer.line_character_count(new_line);
        self.set_cursor_preserving_desired_col(TextPosition::new(
            new_line,
            target_col.min(line_len),
        ));
    }

    // -- movement (extend selection) ----------------------------------------

    fn move_up_extend(&mut self) {
        self.move_up_impl(true);
    }

    fn move_down_extend(&mut self) {
        self.move_down_impl(true);
    }

    fn move_left_extend(&mut self) {
        self.move_left_impl(true);
    }

    fn move_right_extend(&mut self) {
        self.move_right_impl(true);
    }

    // -- editing ------------------------------------------------------------

    fn insert_char(&mut self, c: char) {
        let lang_name = self.document.detect_language().map(|l| l.name);
        if self.carets.is_multicursor() {
            let mut planned = Vec::with_capacity(self.carets.len());
            let mut char_buf = [0u8; 4];
            let encoded = c.encode_utf8(&mut char_buf);
            let encoded_bytes = encoded.as_bytes();

            for caret in self.carets.iter().copied() {
                let selection = caret.selection;
                if !selection.is_empty() {
                    let (start, end) = selection.ordered();
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    let end_offset = start_offset + deleted.len();
                    if let Some(close) = auto_close_char(c, lang_name) {
                        let mut insert =
                            Vec::with_capacity(deleted.len() + encoded_bytes.len() + 1);
                        insert.extend_from_slice(encoded_bytes);
                        insert.extend_from_slice(&deleted);
                        insert.push(close as u8);
                        planned.push(PlannedCaretEdit {
                            start_byte: start_offset,
                            end_byte: end_offset,
                            inserted_bytes: insert,
                            deleted_bytes: deleted,
                            anchor_after_byte: start_offset + encoded_bytes.len(),
                            cursor_after_byte: start_offset
                                + encoded_bytes.len()
                                + (end_offset - start_offset),
                        });
                    } else {
                        planned.push(PlannedCaretEdit {
                            start_byte: start_offset,
                            end_byte: end_offset,
                            inserted_bytes: encoded_bytes.to_vec(),
                            deleted_bytes: deleted,
                            anchor_after_byte: start_offset + encoded_bytes.len(),
                            cursor_after_byte: start_offset + encoded_bytes.len(),
                        });
                    }
                    continue;
                }

                let pos = selection.cursor;
                let offset = self.offset_for_pos(pos);

                if is_close_char(c) {
                    let ls = self.document.buffer.line_start(pos.line);
                    let le = self.document.buffer.line_end(pos.line);
                    if ls + pos.column < le
                        && self.document.buffer.byte_at(ls + pos.column) == c as u8
                    {
                        planned.push(PlannedCaretEdit {
                            start_byte: offset,
                            end_byte: offset,
                            inserted_bytes: Vec::new(),
                            deleted_bytes: Vec::new(),
                            anchor_after_byte: offset + 1,
                            cursor_after_byte: offset + 1,
                        });
                        continue;
                    }
                }

                if let Some(close) = auto_close_char(c, lang_name) {
                    let ls = self.document.buffer.line_start(pos.line);
                    let le = self.document.buffer.line_end(pos.line);
                    let next = if ls + pos.column < le {
                        self.document.buffer.byte_at(ls + pos.column)
                    } else {
                        b'\n'
                    };
                    let next_is_boundary = next == b' '
                        || next == b'\t'
                        || next == b'\n'
                        || is_close_char(next as char);
                    if next_is_boundary {
                        let mut insert = Vec::with_capacity(encoded_bytes.len() + 1);
                        insert.extend_from_slice(encoded_bytes);
                        insert.push(close as u8);
                        planned.push(PlannedCaretEdit {
                            start_byte: offset,
                            end_byte: offset,
                            inserted_bytes: insert,
                            deleted_bytes: Vec::new(),
                            anchor_after_byte: offset + encoded_bytes.len(),
                            cursor_after_byte: offset + encoded_bytes.len(),
                        });
                        continue;
                    }
                }

                planned.push(PlannedCaretEdit {
                    start_byte: offset,
                    end_byte: offset,
                    inserted_bytes: encoded_bytes.to_vec(),
                    deleted_bytes: Vec::new(),
                    anchor_after_byte: offset + encoded_bytes.len(),
                    cursor_after_byte: offset + encoded_bytes.len(),
                });
            }

            self.apply_multi_caret_edits(planned);
            return;
        }

        if self.has_selection() {
            // Wrap selection with matching pairs
            if let Some(close) = auto_close_char(c, lang_name) {
                let (start, end) = self.selection().ordered();
                let text = self.document.text_in_range(start, end);
                let mut wrapped = vec![c as u8];
                wrapped.extend_from_slice(&text);
                wrapped.push(close as u8);
                self.document.begin_undo_group();
                self.document.delete_range(start, end);
                let after = self.document.insert(start.line, start.column, &wrapped);
                self.document.end_undo_group();
                // Select the inner text (between the pair chars)
                self.set_selection(Selection {
                    anchor: TextPosition::new(start.line, start.column + 1),
                    cursor: TextPosition::new(after.line, after.column - 1),
                });
                return;
            }
            self.delete_selection();
        }

        // Skip over closing char if it's already the next character.
        // close chars are ASCII so byte_at(line_start + column) == the char at column.
        if is_close_char(c) {
            let line = self.cursor().line;
            let column = self.cursor().column;
            let ls = self.document.buffer.line_start(line);
            let le = self.document.buffer.line_end(line);
            if ls + column < le && self.document.buffer.byte_at(ls + column) == c as u8 {
                self.set_cursor(TextPosition::new(line, column + 1));
                return;
            }
        }

        let mut char_buf = [0u8; 4];
        let s = c.encode_utf8(&mut char_buf);

        // Auto-close pairs: insert open+close on a stack buffer, no heap alloc.
        if let Some(close) = auto_close_char(c, lang_name) {
            let line = self.cursor().line;
            let column = self.cursor().column;
            let ls = self.document.buffer.line_start(line);
            let le = self.document.buffer.line_end(line);
            // Treat end-of-line (\n or past end) as a boundary.
            let next = if ls + column < le {
                self.document.buffer.byte_at(ls + column)
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
                let pos = self.document.insert(line, column, &pair[..cb.len() + 1]);
                // Place cursor between the pair
                self.set_cursor(TextPosition::new(pos.line, pos.column - 1));
                return;
            }
        }

        let pos = self
            .document
            .insert(self.cursor().line, self.cursor().column, s.as_bytes());
        self.set_cursor(pos);
    }

    fn insert_tab(&mut self) {
        if self.carets.is_multicursor() {
            if self.carets.iter().any(|caret| !caret.selection.is_empty()) {
                self.indent_caret_lines();
                return;
            }
            let bytes: &[u8] = if self.use_tab_indent() { b"\t" } else { b"  " };
            let mut planned = Vec::with_capacity(self.carets.len());
            for caret in self.carets.iter().copied() {
                let selection = caret.selection;
                let (start, end) = selection.ordered();
                let start_offset = self.offset_for_pos(start);
                let deleted = if selection.is_empty() {
                    Vec::new()
                } else {
                    self.document.text_in_range(start, end)
                };
                let end_offset = start_offset + deleted.len();
                planned.push(PlannedCaretEdit {
                    start_byte: start_offset,
                    end_byte: end_offset,
                    inserted_bytes: bytes.to_vec(),
                    deleted_bytes: deleted,
                    anchor_after_byte: start_offset + bytes.len(),
                    cursor_after_byte: start_offset + bytes.len(),
                });
            }
            self.apply_multi_caret_edits(planned);
            return;
        }

        if self.has_selection() {
            self.indent_selection();
            return;
        }
        let bytes: &[u8] = if self.use_tab_indent() { b"\t" } else { b"  " };
        let pos = self
            .document
            .insert(self.cursor().line, self.cursor().column, bytes);
        self.set_cursor(pos);
    }

    fn indent_caret_lines(&mut self) {
        let use_tab = self.use_tab_indent();
        let indent_bytes: &[u8] = if use_tab { b"\t" } else { b"  " };
        let mut text_edits = Vec::new();

        for line in self.targeted_lines_for_carets() {
            let text = self.document.buffer.line_text(line);
            let is_blank = text.iter().all(|&b| b == b' ' || b == b'\t');
            if is_blank {
                continue;
            }
            text_edits.push(TextEdit {
                start_byte: self.document.buffer.line_start(line),
                end_byte: self.document.buffer.line_start(line),
                inserted_bytes: indent_bytes.to_vec(),
                deleted_bytes: Vec::new(),
            });
        }

        self.apply_text_edits_preserving_carets(text_edits);
    }

    fn indent_selection(&mut self) {
        let (s, e) = self.selection().ordered();
        let end_line = if e.column == 0 && e.line > s.line {
            e.line - 1
        } else {
            e.line
        };
        let start_line = s.line;

        let use_tab = self.use_tab_indent();
        let indent_bytes: &[u8] = if use_tab { b"\t" } else { b"  " };
        let indent_char_len = if use_tab { 1 } else { 2 };

        // Pre-read line data to avoid O(n²) cache rebuilds
        let lines: Vec<(Vec<u8>, usize)> = (start_line..=end_line)
            .map(|i| {
                (
                    self.document.buffer.line_text(i),
                    self.document.buffer.line_start(i),
                )
            })
            .collect();

        let cursor_pos = self.cursor();
        self.document.begin_undo_group();
        let anchor_line = self.selection().anchor.line;
        let cursor_line = self.selection().cursor.line;
        let mut anchor_added = 0usize;
        let mut cursor_added = 0usize;
        for (idx, (text, line_offset)) in lines.iter().enumerate().rev() {
            let is_blank = text.iter().all(|&b| b == b' ' || b == b'\t');
            if is_blank {
                continue;
            }
            self.document
                .insert_at_byte(*line_offset, indent_bytes, cursor_pos, cursor_pos);
            let line_idx = start_line + idx;
            if line_idx == cursor_line {
                cursor_added = indent_char_len;
            }
            if line_idx == anchor_line {
                anchor_added = indent_char_len;
            }
        }
        self.document.end_undo_group();

        // Preserve the selection so the user can indent multiple times.
        self.carets.primary_mut().selection.cursor.column += cursor_added;
        self.carets.primary_mut().selection.anchor.column += anchor_added;
    }

    fn insert_newline(&mut self) {
        if self.carets.is_multicursor() {
            let mut planned = Vec::with_capacity(self.carets.len());
            for caret in self.carets.iter().copied() {
                let selection = caret.selection;
                let (start, end) = selection.ordered();
                let base = if selection.is_empty() {
                    selection.cursor
                } else {
                    start
                };
                let start_offset = self.offset_for_pos(start);
                let deleted = if selection.is_empty() {
                    Vec::new()
                } else {
                    self.document.text_in_range(start, end)
                };
                let end_offset = start_offset + deleted.len();

                self.document
                    .buffer
                    .line_text_into(base.line, &mut self.line_text_scratch);
                let indent: Vec<u8> = self
                    .line_text_scratch
                    .iter()
                    .take_while(|&&b| b == b' ' || b == b'\t')
                    .copied()
                    .collect();

                if selection.is_empty() && base.column > 0 {
                    let ls = self.document.buffer.line_start(base.line);
                    let le = self.document.buffer.line_end(base.line);
                    let prev = self.document.buffer.byte_at(ls + base.column - 1);
                    let close_opt = match prev {
                        b'(' => Some(b')'),
                        b'[' => Some(b']'),
                        b'{' => Some(b'}'),
                        _ => None,
                    };
                    if let Some(close) = close_opt
                        && ls + base.column < le
                        && self.document.buffer.byte_at(ls + base.column) == close
                    {
                        let extra: &[u8] = if self.use_tab_indent() { b"\t" } else { b"  " };
                        let mut insert = vec![b'\n'];
                        insert.extend_from_slice(&indent);
                        insert.extend_from_slice(extra);
                        let cursor_offset = start_offset + insert.len();
                        insert.push(b'\n');
                        insert.extend_from_slice(&indent);
                        planned.push(PlannedCaretEdit {
                            start_byte: start_offset,
                            end_byte: end_offset,
                            inserted_bytes: insert,
                            deleted_bytes: deleted,
                            anchor_after_byte: cursor_offset,
                            cursor_after_byte: cursor_offset,
                        });
                        continue;
                    }
                }

                let mut insert = vec![b'\n'];
                insert.extend_from_slice(&indent);
                let cursor_offset = start_offset + insert.len();
                planned.push(PlannedCaretEdit {
                    start_byte: start_offset,
                    end_byte: end_offset,
                    inserted_bytes: insert,
                    deleted_bytes: deleted,
                    anchor_after_byte: cursor_offset,
                    cursor_after_byte: cursor_offset,
                });
            }
            self.apply_multi_caret_edits(planned);
            return;
        }

        if self.has_selection() {
            self.delete_selection();
        }
        let c = self.cursor();
        self.document
            .buffer
            .line_text_into(c.line, &mut self.line_text_scratch);
        let indent: Vec<u8> = self
            .line_text_scratch
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .copied()
            .collect();

        // Between bracket pairs ({|}, (|), [|]): split with extra indent level.
        if c.column > 0 {
            let ls = self.document.buffer.line_start(c.line);
            let le = self.document.buffer.line_end(c.line);
            let prev = self.document.buffer.byte_at(ls + c.column - 1);
            let close_opt = match prev {
                b'(' => Some(b')'),
                b'[' => Some(b']'),
                b'{' => Some(b'}'),
                _ => None,
            };
            if let Some(close) = close_opt
                && ls + c.column < le
                && self.document.buffer.byte_at(ls + c.column) == close
            {
                let extra: &[u8] = if self.use_tab_indent() { b"\t" } else { b"  " };
                let mut split = vec![b'\n'];
                split.extend_from_slice(&indent);
                split.extend_from_slice(extra);
                let cursor_col = indent.len() + extra.len();
                split.push(b'\n');
                split.extend_from_slice(&indent);
                self.document.seal_undo();
                self.document.insert(c.line, c.column, &split);
                self.document.seal_undo();
                self.set_cursor(TextPosition::new(c.line + 1, cursor_col));
                return;
            }
        }

        let mut newline = vec![b'\n'];
        newline.extend_from_slice(&indent);

        self.document.seal_undo();
        let pos = self
            .document
            .insert(self.cursor().line, self.cursor().column, &newline);
        self.document.seal_undo();
        self.set_cursor(pos);
    }

    fn backspace(&mut self) {
        if self.carets.is_multicursor() {
            let mut planned = Vec::with_capacity(self.carets.len());
            for caret in self.carets.iter().copied() {
                let selection = caret.selection;
                if !selection.is_empty() {
                    let (start, end) = selection.ordered();
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    planned.push(PlannedCaretEdit {
                        start_byte: start_offset,
                        end_byte: start_offset + deleted.len(),
                        inserted_bytes: Vec::new(),
                        deleted_bytes: deleted,
                        anchor_after_byte: start_offset,
                        cursor_after_byte: start_offset,
                    });
                    continue;
                }

                let c = selection.cursor;
                if c.column > 0 {
                    let ls = self.document.buffer.line_start(c.line);
                    let le = self.document.buffer.line_end(c.line);
                    let mut leading_ws = 0;
                    while ls + leading_ws < le {
                        match self.document.buffer.byte_at(ls + leading_ws) {
                            b' ' | b'\t' => leading_ws += 1,
                            _ => break,
                        }
                    }

                    if c.column <= leading_ws && c.column >= 2 {
                        let all_spaces =
                            (0..c.column).all(|i| self.document.buffer.byte_at(ls + i) == b' ');
                        if all_spaces && c.column.is_multiple_of(2) {
                            let start = TextPosition::new(c.line, c.column - 2);
                            let end = TextPosition::new(c.line, c.column);
                            let start_offset = self.offset_for_pos(start);
                            let deleted = self.document.text_in_range(start, end);
                            planned.push(PlannedCaretEdit {
                                start_byte: start_offset,
                                end_byte: start_offset + deleted.len(),
                                inserted_bytes: Vec::new(),
                                deleted_bytes: deleted,
                                anchor_after_byte: start_offset,
                                cursor_after_byte: start_offset,
                            });
                            continue;
                        }
                    }

                    let prev = self.document.buffer.byte_at(ls + c.column - 1);
                    if ls + c.column < le {
                        let next = self.document.buffer.byte_at(ls + c.column);
                        let lang_name = self.document.detect_language().map(|l| l.name);
                        if auto_close_char(prev as char, lang_name) == Some(next as char) {
                            let start = TextPosition::new(c.line, c.column - 1);
                            let end = TextPosition::new(c.line, c.column + 1);
                            let start_offset = self.offset_for_pos(start);
                            let deleted = self.document.text_in_range(start, end);
                            planned.push(PlannedCaretEdit {
                                start_byte: start_offset,
                                end_byte: start_offset + deleted.len(),
                                inserted_bytes: Vec::new(),
                                deleted_bytes: deleted,
                                anchor_after_byte: start_offset,
                                cursor_after_byte: start_offset,
                            });
                            continue;
                        }
                    }

                    let start = TextPosition::new(c.line, c.column - 1);
                    let end = TextPosition::new(c.line, c.column);
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    planned.push(PlannedCaretEdit {
                        start_byte: start_offset,
                        end_byte: start_offset + deleted.len(),
                        inserted_bytes: Vec::new(),
                        deleted_bytes: deleted,
                        anchor_after_byte: start_offset,
                        cursor_after_byte: start_offset,
                    });
                } else if c.line > 0 {
                    let prev_len = self.document.buffer.line_character_count(c.line - 1);
                    let start = TextPosition::new(c.line - 1, prev_len);
                    let end = TextPosition::new(c.line, 0);
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    planned.push(PlannedCaretEdit {
                        start_byte: start_offset,
                        end_byte: start_offset + deleted.len(),
                        inserted_bytes: Vec::new(),
                        deleted_bytes: deleted,
                        anchor_after_byte: start_offset,
                        cursor_after_byte: start_offset,
                    });
                } else {
                    let offset = self.offset_for_pos(c);
                    planned.push(PlannedCaretEdit {
                        start_byte: offset,
                        end_byte: offset,
                        inserted_bytes: Vec::new(),
                        deleted_bytes: Vec::new(),
                        anchor_after_byte: offset,
                        cursor_after_byte: offset,
                    });
                }
            }
            self.apply_multi_caret_edits(planned);
            return;
        }

        if self.has_selection() {
            self.delete_selection();
            return;
        }
        let c = self.cursor();
        if c.column > 0 {
            let ls = self.document.buffer.line_start(c.line);
            let le = self.document.buffer.line_end(c.line);
            // Count leading whitespace (ASCII: byte offset == char offset here).
            let mut leading_ws = 0;
            while ls + leading_ws < le {
                match self.document.buffer.byte_at(ls + leading_ws) {
                    b' ' | b'\t' => leading_ws += 1,
                    _ => break,
                }
            }

            // Smart 2-space dedent
            if c.column <= leading_ws && c.column >= 2 {
                let all_spaces =
                    (0..c.column).all(|i| self.document.buffer.byte_at(ls + i) == b' ');
                if all_spaces && c.column.is_multiple_of(2) {
                    let end = TextPosition::new(c.line, c.column);
                    let start = TextPosition::new(c.line, c.column - 2);
                    self.document.delete_range(start, end);
                    self.set_cursor(start);
                    return;
                }
            }

            // Delete matching auto-close pair if cursor is between them.
            let prev = self.document.buffer.byte_at(ls + c.column - 1);
            if ls + c.column < le {
                let next = self.document.buffer.byte_at(ls + c.column);
                let lang_name = self.document.detect_language().map(|l| l.name);
                if auto_close_char(prev as char, lang_name) == Some(next as char) {
                    let start = TextPosition::new(c.line, c.column - 1);
                    let end = TextPosition::new(c.line, c.column + 1);
                    self.document.delete_range(start, end);
                    self.set_cursor(start);
                    return;
                }
            }

            let start = TextPosition::new(c.line, c.column - 1);
            let end = TextPosition::new(c.line, c.column);
            self.document.delete_range(start, end);
            self.set_cursor(start);
        } else if c.line > 0 {
            let prev_len = self.document.buffer.line_character_count(c.line - 1);
            let start = TextPosition::new(c.line - 1, prev_len);
            let end = TextPosition::new(c.line, 0);
            self.document.delete_range(start, end);
            self.set_cursor(start);
        }
    }

    fn ctrl_backspace(&mut self) {
        if self.carets.is_multicursor() {
            let mut planned = Vec::with_capacity(self.carets.len());
            for caret in self.carets.iter().copied() {
                let selection = caret.selection;
                if !selection.is_empty() {
                    let (start, end) = selection.ordered();
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    planned.push(PlannedCaretEdit {
                        start_byte: start_offset,
                        end_byte: start_offset + deleted.len(),
                        inserted_bytes: Vec::new(),
                        deleted_bytes: deleted,
                        anchor_after_byte: start_offset,
                        cursor_after_byte: start_offset,
                    });
                    continue;
                }

                let c = selection.cursor;
                if c.column == 0 && c.line == 0 {
                    let offset = self.offset_for_pos(c);
                    planned.push(PlannedCaretEdit {
                        start_byte: offset,
                        end_byte: offset,
                        inserted_bytes: Vec::new(),
                        deleted_bytes: Vec::new(),
                        anchor_after_byte: offset,
                        cursor_after_byte: offset,
                    });
                    continue;
                }
                if c.column == 0 {
                    let prev_len = self.document.buffer.line_character_count(c.line - 1);
                    let start = TextPosition::new(c.line - 1, prev_len);
                    let end = TextPosition::new(c.line, 0);
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    planned.push(PlannedCaretEdit {
                        start_byte: start_offset,
                        end_byte: start_offset + deleted.len(),
                        inserted_bytes: Vec::new(),
                        deleted_bytes: deleted,
                        anchor_after_byte: start_offset,
                        cursor_after_byte: start_offset,
                    });
                    continue;
                }

                self.document
                    .buffer
                    .line_text_into(c.line, &mut self.line_text_scratch);
                let boundary = prev_word_boundary(&self.line_text_scratch, c.column);
                let start = TextPosition::new(c.line, boundary);
                let end = TextPosition::new(c.line, c.column);
                let start_offset = self.offset_for_pos(start);
                let deleted = self.document.text_in_range(start, end);
                planned.push(PlannedCaretEdit {
                    start_byte: start_offset,
                    end_byte: start_offset + deleted.len(),
                    inserted_bytes: Vec::new(),
                    deleted_bytes: deleted,
                    anchor_after_byte: start_offset,
                    cursor_after_byte: start_offset,
                });
            }
            self.apply_multi_caret_edits(planned);
            return;
        }

        if self.has_selection() {
            self.delete_selection();
            return;
        }
        let c = self.cursor();
        if c.column == 0 && c.line == 0 {
            return;
        }
        if c.column == 0 {
            let prev_len = self.document.buffer.line_character_count(c.line - 1);
            let start = TextPosition::new(c.line - 1, prev_len);
            let end = TextPosition::new(c.line, 0);
            self.document.seal_undo();
            self.document.delete_range(start, end);
            self.document.seal_undo();
            self.set_cursor(start);
            return;
        }
        self.document
            .buffer
            .line_text_into(c.line, &mut self.line_text_scratch);
        let boundary = prev_word_boundary(&self.line_text_scratch, c.column);
        let start = TextPosition::new(c.line, boundary);
        let end = TextPosition::new(c.line, c.column);
        self.document.seal_undo();
        self.document.delete_range(start, end);
        self.document.seal_undo();
        self.set_cursor(start);
    }

    fn delete_forward(&mut self) {
        if self.carets.is_multicursor() {
            let mut planned = Vec::with_capacity(self.carets.len());
            for caret in self.carets.iter().copied() {
                let selection = caret.selection;
                if !selection.is_empty() {
                    let (start, end) = selection.ordered();
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    planned.push(PlannedCaretEdit {
                        start_byte: start_offset,
                        end_byte: start_offset + deleted.len(),
                        inserted_bytes: Vec::new(),
                        deleted_bytes: deleted,
                        anchor_after_byte: start_offset,
                        cursor_after_byte: start_offset,
                    });
                    continue;
                }

                let c = selection.cursor;
                let line_len = self.document.buffer.line_character_count(c.line);
                if c.column < line_len {
                    let start = TextPosition::new(c.line, c.column);
                    let end = TextPosition::new(c.line, c.column + 1);
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    planned.push(PlannedCaretEdit {
                        start_byte: start_offset,
                        end_byte: start_offset + deleted.len(),
                        inserted_bytes: Vec::new(),
                        deleted_bytes: deleted,
                        anchor_after_byte: start_offset,
                        cursor_after_byte: start_offset,
                    });
                } else if c.line + 1 < self.document.buffer.line_count() {
                    let start = TextPosition::new(c.line, c.column);
                    let end = TextPosition::new(c.line + 1, 0);
                    let start_offset = self.offset_for_pos(start);
                    let deleted = self.document.text_in_range(start, end);
                    planned.push(PlannedCaretEdit {
                        start_byte: start_offset,
                        end_byte: start_offset + deleted.len(),
                        inserted_bytes: Vec::new(),
                        deleted_bytes: deleted,
                        anchor_after_byte: start_offset,
                        cursor_after_byte: start_offset,
                    });
                } else {
                    let offset = self.offset_for_pos(c);
                    planned.push(PlannedCaretEdit {
                        start_byte: offset,
                        end_byte: offset,
                        inserted_bytes: Vec::new(),
                        deleted_bytes: Vec::new(),
                        anchor_after_byte: offset,
                        cursor_after_byte: offset,
                    });
                }
            }
            self.apply_multi_caret_edits(planned);
            return;
        }

        if self.has_selection() {
            self.delete_selection();
            return;
        }
        let c = self.cursor();
        let line_len = self.document.buffer.line_character_count(c.line);
        if c.column < line_len {
            self.document.delete_range(
                TextPosition::new(c.line, c.column),
                TextPosition::new(c.line, c.column + 1),
            );
        } else if c.line + 1 < self.document.buffer.line_count() {
            self.document.delete_range(
                TextPosition::new(c.line, c.column),
                TextPosition::new(c.line + 1, 0),
            );
        }
    }

    fn duplicate_line(&mut self) {
        let c = self.cursor();
        self.document
            .buffer
            .line_text_into(c.line, &mut self.line_text_scratch);
        let mut new_content = vec![b'\n'];
        new_content.extend_from_slice(&self.line_text_scratch);
        let line_character_count = self.document.buffer.line_character_count(c.line);
        self.document.seal_undo();
        self.document
            .insert(c.line, line_character_count, &new_content);
        self.document.seal_undo();
        self.set_cursor(TextPosition::new(c.line + 1, c.column));
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
        let comment = match self.document.detect_language() {
            Some(lang) => lang.comment,
            None => {
                self.set_status("No language detected for commenting".to_string());
                return;
            }
        };

        if self.carets.is_multicursor() {
            let prefix = format!("{} ", comment);
            let targeted_lines = self.targeted_lines_for_carets();
            let lines: Vec<(Vec<u8>, usize)> = targeted_lines
                .iter()
                .map(|&line| {
                    (
                        self.document.buffer.line_text(line),
                        self.document.buffer.line_start(line),
                    )
                })
                .collect();

            let all_commented = lines.iter().all(|(text, _)| {
                let trimmed = text.iter().position(|&b| b != b' ' && b != b'\t');
                match trimmed {
                    Some(pos) => text[pos..].starts_with(prefix.as_bytes()),
                    None => true,
                }
            });

            let do_uncomment = match force {
                Some(true) => false,
                Some(false) => true,
                None => all_commented,
            };

            let mut text_edits = Vec::new();
            if do_uncomment {
                for (text, line_offset) in &lines {
                    let indent_pos = text
                        .iter()
                        .position(|&b| b != b' ' && b != b'\t')
                        .unwrap_or(text.len());
                    if text[indent_pos..].starts_with(prefix.as_bytes()) {
                        text_edits.push(TextEdit {
                            start_byte: line_offset + indent_pos,
                            end_byte: line_offset + indent_pos + prefix.len(),
                            inserted_bytes: Vec::new(),
                            deleted_bytes: prefix.as_bytes().to_vec(),
                        });
                    }
                }
            } else {
                let min_indent = lines
                    .iter()
                    .filter_map(|(text, _)| text.iter().position(|&b| b != b' ' && b != b'\t'))
                    .min()
                    .unwrap_or(0);
                for (text, line_offset) in &lines {
                    let is_blank = text.iter().all(|&b| b == b' ' || b == b'\t');
                    if is_blank {
                        continue;
                    }
                    let indent_pos = text
                        .iter()
                        .position(|&b| b != b' ' && b != b'\t')
                        .unwrap_or(text.len());
                    if text[indent_pos..].starts_with(prefix.as_bytes()) {
                        continue;
                    }
                    text_edits.push(TextEdit {
                        start_byte: line_offset + min_indent,
                        end_byte: line_offset + min_indent,
                        inserted_bytes: prefix.as_bytes().to_vec(),
                        deleted_bytes: Vec::new(),
                    });
                }
            }

            self.apply_text_edits_preserving_carets(text_edits);
            return;
        }

        // Determine line range: selection or current line
        let (start_line, end_line) = if !self.has_selection() {
            (self.cursor().line, self.cursor().line)
        } else {
            let (s, e) = self.selection().ordered();
            let end = if e.column == 0 && e.line > s.line {
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
                let text = self.document.buffer.line_text(i);
                let offset = self.document.buffer.line_start(i);
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
        self.document.begin_undo_group();
        if do_uncomment {
            // Uncomment: remove first occurrence of "comment " from each line
            for (text, line_offset) in lines.iter().rev() {
                let indent_pos = text
                    .iter()
                    .position(|&b| b != b' ' && b != b'\t')
                    .unwrap_or(text.len());
                if text[indent_pos..].starts_with(prefix.as_bytes()) {
                    self.document.delete_at_byte(
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
                self.document.insert_at_byte(
                    line_offset + min_indent,
                    prefix.as_bytes(),
                    cursor_pos,
                    cursor_pos,
                );
            }
        }
        self.document.end_undo_group();
    }

    // -- dedent -------------------------------------------------------------

    fn dedent(&mut self) {
        if self.carets.is_multicursor() {
            let mut text_edits = Vec::new();
            for line in self.targeted_lines_for_carets() {
                let text = self.document.buffer.line_text(line);
                let line_offset = self.document.buffer.line_start(line);
                if text.starts_with(b"\t") {
                    text_edits.push(TextEdit {
                        start_byte: line_offset,
                        end_byte: line_offset + 1,
                        inserted_bytes: Vec::new(),
                        deleted_bytes: vec![b'\t'],
                    });
                } else if text.starts_with(b"  ") {
                    text_edits.push(TextEdit {
                        start_byte: line_offset,
                        end_byte: line_offset + 2,
                        inserted_bytes: Vec::new(),
                        deleted_bytes: b"  ".to_vec(),
                    });
                }
            }
            self.apply_text_edits_preserving_carets(text_edits);
            return;
        }
        let (start_line, end_line) = if !self.has_selection() {
            (self.cursor().line, self.cursor().line)
        } else {
            let (s, e) = self.selection().ordered();
            let end = if e.column == 0 && e.line > s.line {
                e.line - 1
            } else {
                e.line
            };
            (s.line, end)
        };

        // Pre-read line data to avoid O(n²) cache rebuilds
        let lines: Vec<(Vec<u8>, usize)> = (start_line..=end_line)
            .map(|i| {
                (
                    self.document.buffer.line_text(i),
                    self.document.buffer.line_start(i),
                )
            })
            .collect();

        let cursor_pos = self.cursor();
        self.document.begin_undo_group();
        let anchor_line = self.selection().anchor.line;
        let cursor_line = self.selection().cursor.line;
        let mut anchor_removed = 0usize;
        let mut cursor_removed = 0usize;
        for (idx, (text, line_offset)) in lines.iter().enumerate().rev() {
            let removed = if text.starts_with(b"\t") {
                self.document
                    .delete_at_byte(*line_offset, 1, cursor_pos, cursor_pos);
                1
            } else if text.starts_with(b"  ") {
                self.document
                    .delete_at_byte(*line_offset, 2, cursor_pos, cursor_pos);
                2
            } else {
                0
            };
            let line_idx = start_line + idx;
            if line_idx == cursor_line {
                cursor_removed = removed;
            }
            if line_idx == anchor_line {
                anchor_removed = removed;
            }
        }
        self.document.end_undo_group();

        // Preserve the selection so the user can dedent multiple times.
        self.carets.primary_mut().selection.cursor.column =
            self.cursor().column.saturating_sub(cursor_removed);
        self.carets.primary_mut().selection.anchor.column = self
            .selection()
            .anchor
            .column
            .saturating_sub(anchor_removed);
    }

    // -- clipboard ----------------------------------------------------------

    fn copy(&mut self) {
        if self.carets.is_multicursor() {
            let fragments: Vec<String> = self
                .carets
                .iter()
                .filter_map(|caret| {
                    if caret.selection.is_empty() {
                        return None;
                    }
                    let (start, end) = caret.selection.ordered();
                    let text = self.document.text_in_range(start, end);
                    Some(String::from_utf8_lossy(&text).to_string())
                })
                .collect();
            if fragments.is_empty() {
                return;
            }
            if fragments.len() == self.carets.len() {
                self.clipboard.copy_multi(&fragments);
            } else {
                self.clipboard.copy(&fragments.join("\n"));
            }
            return;
        }

        if !self.has_selection() {
            return;
        }
        let (start, end) = self.selection().ordered();
        let text = self.document.text_in_range(start, end);
        let s = String::from_utf8_lossy(&text).to_string();
        self.clipboard.copy(&s);
    }

    fn cut(&mut self) {
        if self.carets.is_multicursor() {
            let fragments: Vec<String> = self
                .carets
                .iter()
                .filter_map(|caret| {
                    if caret.selection.is_empty() {
                        return None;
                    }
                    let (start, end) = caret.selection.ordered();
                    let text = self.document.text_in_range(start, end);
                    Some(String::from_utf8_lossy(&text).to_string())
                })
                .collect();
            if fragments.is_empty() {
                return;
            }
            if fragments.len() == self.carets.len() {
                self.clipboard.copy_multi(&fragments);
            } else {
                self.clipboard.copy(&fragments.join("\n"));
            }
            let mut planned = Vec::with_capacity(self.carets.len());
            for caret in self.carets.iter().copied() {
                let selection = caret.selection;
                let (start, end) = selection.ordered();
                let start_offset = self.offset_for_pos(start);
                let deleted = if selection.is_empty() {
                    Vec::new()
                } else {
                    self.document.text_in_range(start, end)
                };
                planned.push(PlannedCaretEdit {
                    start_byte: start_offset,
                    end_byte: start_offset + deleted.len(),
                    inserted_bytes: Vec::new(),
                    deleted_bytes: deleted,
                    anchor_after_byte: start_offset,
                    cursor_after_byte: start_offset,
                });
            }
            self.apply_multi_caret_edits(planned);
            return;
        }

        if !self.has_selection() {
            return;
        }
        self.copy();
        self.delete_selection();
    }

    fn paste(&mut self) {
        let contents = self.clipboard.paste_contents();
        if self.carets.is_multicursor() {
            let fragment_texts = contents
                .fragments
                .filter(|fragments| fragments.len() == self.carets.len());
            let mut planned = Vec::with_capacity(self.carets.len());
            let carets = self.carets.carets.clone();
            for (idx, caret) in carets.into_iter().enumerate() {
                let text = fragment_texts
                    .as_ref()
                    .map(|fragments| fragments[idx].as_str())
                    .unwrap_or(contents.text.as_str());
                let selection = caret.selection;
                let (start, end) = selection.ordered();
                let mut insert_pos = if selection.is_empty() {
                    selection.cursor
                } else {
                    start
                };
                let mut start_offset = self.offset_for_pos(start);
                let mut deleted = if selection.is_empty() {
                    Vec::new()
                } else {
                    self.document.text_in_range(start, end)
                };

                if selection.is_empty() && text.contains('\n') {
                    self.document
                        .buffer
                        .line_text_into(insert_pos.line, &mut self.line_text_scratch);
                    let is_blank = self
                        .line_text_scratch
                        .iter()
                        .all(|&b| b == b' ' || b == b'\t');
                    if is_blank {
                        let char_len = crate::buffer::character_count(&self.line_text_scratch);
                        if char_len > 0 {
                            deleted = self.document.text_in_range(
                                TextPosition::new(insert_pos.line, 0),
                                TextPosition::new(insert_pos.line, char_len),
                            );
                            insert_pos = TextPosition::new(insert_pos.line, 0);
                            start_offset = self.offset_for_pos(insert_pos);
                        }
                    }
                }

                let insert_text = self.reindent_paste_for_cursor(text, insert_pos);
                let insert_len = insert_text.len();
                planned.push(PlannedCaretEdit {
                    start_byte: start_offset,
                    end_byte: start_offset + deleted.len(),
                    inserted_bytes: insert_text.into_bytes(),
                    deleted_bytes: deleted,
                    anchor_after_byte: start_offset + insert_len,
                    cursor_after_byte: start_offset + insert_len,
                });
            }
            self.apply_multi_caret_edits(planned);
            return;
        }

        self.paste_text(&contents.text);
    }

    fn paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.has_selection() {
            self.delete_selection();
        }
        // Multi-line paste: clear any auto-indent on a blank line and anchor at
        // column 0 so the pasted text's own indentation is used as-is.
        // Indentation is a copy-time property, not a paste-time one.
        if text.contains('\n') {
            let c = self.cursor();
            self.document
                .buffer
                .line_text_into(c.line, &mut self.line_text_scratch);
            let is_blank = self
                .line_text_scratch
                .iter()
                .all(|&b| b == b' ' || b == b'\t');
            if is_blank {
                let char_len = crate::buffer::character_count(&self.line_text_scratch);
                if char_len > 0 {
                    self.document.delete_range(
                        TextPosition::new(c.line, 0),
                        TextPosition::new(c.line, char_len),
                    );
                }
                self.set_cursor(TextPosition::new(c.line, 0));
            }
        }
        let text = self.reindent_paste(text);
        self.document.seal_undo();
        let pos = self
            .document
            .insert(self.cursor().line, self.cursor().column, text.as_bytes());
        self.document.seal_undo();
        self.set_cursor(pos);
    }

    /// Re-indent pasted multi-line text to match the current cursor's indent level.
    ///
    /// Continuation lines (lines[1..]) keep their indentation relative to lines[0]:
    /// each line's new indent = cursor_col + original_indent_of_that_line.
    ///
    /// For multi-line paste on a blank line, `paste_text` already resets the cursor
    /// to column 0, so this function is effectively a no-op in that case.
    fn reindent_paste(&mut self, text: &str) -> String {
        self.reindent_paste_for_cursor(text, self.cursor())
    }

    fn reindent_paste_for_cursor(&mut self, text: &str, cursor: TextPosition) -> String {
        let lines: Vec<&str> = text.split('\n').collect();
        if lines.len() < 2 {
            return text.to_string();
        }

        // Get the indent of the current line at cursor
        self.document
            .buffer
            .line_text_into(cursor.line, &mut self.line_text_scratch);
        let cur_indent: usize = self
            .line_text_scratch
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();

        // If lines[0] has content it lands at cursor.column; otherwise the pasted
        // block starts on a new line relative to the current line's indent.
        let target_base = if !lines[0].trim().is_empty() {
            cursor.column
        } else {
            cur_indent
        };

        // Pasting at column 0: the text already carries correct indentation.
        if target_base == 0 {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        result.push_str(lines[0]);
        for line in &lines[1..] {
            result.push('\n');
            if line.trim().is_empty() {
                result.push_str(line);
            } else {
                // Preserve relative indentation: new position = cursor column + original indent.
                let ik = line.len() - line.trim_start().len();
                let new_indent = target_base + ik;
                for _ in 0..new_indent {
                    result.push(' ');
                }
                result.push_str(line.trim_start());
            }
        }
        result
    }

    // -- undo/redo ----------------------------------------------------------

    fn undo(&mut self) {
        if let Some(carets) = self.document.undo() {
            self.restore_carets(carets);
        }
    }

    fn redo(&mut self) {
        if let Some(carets) = self.document.redo() {
            self.restore_carets(carets);
        }
    }

    // -- file I/O -----------------------------------------------------------

    fn strip_trailing_whitespace(&mut self) {
        let line_count = self.document.buffer.line_count();
        self.document.seal_undo();
        for line_idx in (0..line_count).rev() {
            self.document
                .buffer
                .line_text_into(line_idx, &mut self.line_text_scratch);
            let trimmed_len = self
                .line_text_scratch
                .iter()
                .rposition(|&b| b != b' ' && b != b'\t')
                .map(|i| i + 1)
                .unwrap_or(0);
            let char_len = crate::buffer::character_count(&self.line_text_scratch);
            let trim_char_len =
                crate::buffer::character_count(&self.line_text_scratch[..trimmed_len]);
            if trim_char_len < char_len {
                self.document.delete_range(
                    TextPosition::new(line_idx, trim_char_len),
                    TextPosition::new(line_idx, char_len),
                );
            }
        }
        self.document.seal_undo();
        // Adjust cursor if past end of line
        let c = self.cursor();
        let line_len = self.document.buffer.line_character_count(c.line);
        if c.column > line_len {
            self.set_cursor(TextPosition::new(c.line, line_len));
        }
    }

    fn tabs_to_spaces(&mut self) {
        let line_count = self.document.buffer.line_count();
        self.document.seal_undo();
        for line_idx in 0..line_count {
            self.document
                .buffer
                .line_text_into(line_idx, &mut self.line_text_scratch);
            if !self.line_text_scratch.contains(&b'\t') {
                continue;
            }
            let mut new_text = Vec::with_capacity(self.line_text_scratch.len() * 2);
            for &b in &self.line_text_scratch {
                if b == b'\t' {
                    new_text.extend_from_slice(b"  ");
                } else {
                    new_text.push(b);
                }
            }
            let char_len = crate::buffer::character_count(&self.line_text_scratch);
            self.document.delete_range(
                TextPosition::new(line_idx, 0),
                TextPosition::new(line_idx, char_len),
            );
            self.document.insert(line_idx, 0, &new_text);
        }
        self.document.seal_undo();
        let c = self.cursor();
        let line_len = self.document.buffer.line_character_count(c.line);
        if c.column > line_len {
            self.set_cursor(TextPosition::new(c.line, line_len));
        }
        self.set_status("Converted tabs to spaces".to_string());
    }

    fn spaces_to_tabs(&mut self) {
        let line_count = self.document.buffer.line_count();
        self.document.seal_undo();
        for line_idx in 0..line_count {
            self.document
                .buffer
                .line_text_into(line_idx, &mut self.line_text_scratch);
            // Only convert leading whitespace (indentation)
            let mut new_text = Vec::with_capacity(self.line_text_scratch.len());
            let mut i = 0;
            while i < self.line_text_scratch.len() {
                if self.line_text_scratch[i] == b'\t' {
                    new_text.push(b'\t');
                    i += 1;
                } else if i + 1 < self.line_text_scratch.len()
                    && self.line_text_scratch[i] == b' '
                    && self.line_text_scratch[i + 1] == b' '
                {
                    new_text.push(b'\t');
                    i += 2;
                } else {
                    // End of leading whitespace (or lone space): copy rest verbatim
                    new_text.extend_from_slice(&self.line_text_scratch[i..]);
                    break;
                }
            }
            if new_text[..] == self.line_text_scratch[..] {
                continue;
            }
            let char_len = crate::buffer::character_count(&self.line_text_scratch);
            self.document.delete_range(
                TextPosition::new(line_idx, 0),
                TextPosition::new(line_idx, char_len),
            );
            self.document.insert(line_idx, 0, &new_text);
        }
        self.document.seal_undo();
        let c = self.cursor();
        let line_len = self.document.buffer.line_character_count(c.line);
        if c.column > line_len {
            self.set_cursor(TextPosition::new(c.line, line_len));
        }
        self.set_status("Converted spaces to tabs".to_string());
    }

    fn check_external_modification(&mut self) {
        if self.reload_pending || self.command_buffer.active || self.quit_pending {
            return;
        }
        let Some(ref name) = self.document.file_path else {
            return;
        };
        let path = std::path::Path::new(name);
        let disk_mtime = crate::file_io::file_modification_time(path);
        if disk_mtime != self.file_modification_time && disk_mtime.is_some() {
            let short = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| name.clone());
            self.status_message = format!("{} changed on disk. Reload? (y/n)", short);
            self.status_time = None;
            self.reload_pending = true;
        }
    }

    fn reload_file(&mut self) {
        let Some(name) = self.document.file_path.clone() else {
            return;
        };
        let read_result = crate::file_io::read_file(std::path::Path::new(&name));
        match read_result {
            Ok(data) => {
                let path = std::path::Path::new(&name);
                self.file_modification_time = crate::file_io::file_modification_time(path);
                self.document = Document::new(data, Some(name));
                // Clamp cursor to valid position in new buffer
                let lc = self.document.buffer.line_count();
                if self.cursor().line >= lc {
                    let last = lc.saturating_sub(1);
                    self.set_cursor(TextPosition::new(
                        last,
                        self.document.buffer.line_character_count(last),
                    ));
                } else {
                    let cursor = self.cursor();
                    let len = self.document.buffer.line_character_count(cursor.line);
                    if cursor.column > len {
                        self.set_cursor(TextPosition::new(cursor.line, len));
                    }
                }
                self.set_desired_col(None);
                self.find.clear();
                self.renderer.force_full_redraw();
                self.set_status("Reloaded".to_string());
            }
            Err(e) => self.set_status(format!("Reload failed: {}", e)),
        }
        self.reload_pending = false;
    }

    fn dismiss_reload(&mut self) {
        if let Some(ref name) = self.document.file_path {
            self.file_modification_time =
                crate::file_io::file_modification_time(std::path::Path::new(name));
        }
        self.reload_pending = false;
        self.status_message.clear();
        self.status_time = None;
    }

    fn save_file(&mut self) {
        if let Some(path) = self.document.file_path.clone() {
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

            match crate::file_io::write_file(path_ref, &self.document.buffer.contents()) {
                Ok(()) => {
                    self.document.is_dirty = false;
                    self.document.seal_undo();
                    self.file_modification_time = crate::file_io::file_modification_time(path_ref);
                    crate::file_io::save_undo_history(path_ref, &self.document.undo_stack);
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
            self.command_buffer
                .open(CommandBufferMode::Prompt, "Save as: ", "");
        }
    }

    fn start_sudo_save(&mut self) {
        let pid = std::process::id();
        let tmp = format!("/tmp/e_sudo_{}", pid);
        let contents = self.document.buffer.contents();
        let cleaned = crate::file_io::clean_for_write(&contents);
        match std::fs::write(&tmp, &cleaned) {
            Ok(()) => {
                self.sudo_save_tmp = Some(tmp);
                let path = self.document.file_path.as_deref().unwrap_or("?");
                let prompt = format!("sudo password (to save {}): ", path);
                self.command_buffer
                    .open(CommandBufferMode::SudoSave, &prompt, "");
            }
            Err(e) => {
                self.set_status(format!("Error writing temp file: {}", e));
            }
        }
    }

    #[cfg(test)]
    fn test_text(&self) -> String {
        String::from_utf8_lossy(&self.document.buffer.contents()).to_string()
    }

    fn save_file_sudo(&mut self, password: &str) {
        let tmp = match self.sudo_save_tmp.take() {
            Some(t) => t,
            None => return,
        };
        let path = match self.document.file_path.clone() {
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
                self.document.is_dirty = false;
                self.document.seal_undo();
                self.file_modification_time = crate::file_io::file_modification_time(path_ref);
                crate::file_io::save_undo_history(path_ref, &self.document.undo_stack);
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
    use crate::input::{Key, MouseButton, MouseEvent};
    use crate::selection::Caret;

    fn ed(text: &str) -> Editor {
        ed_impl(text, None)
    }

    fn ed_named(text: &str, name: &str) -> Editor {
        ed_impl(text, Some(name.to_string()))
    }

    fn ed_impl(text: &str, file_path: Option<String>) -> Editor {
        let document = Document::new(text.as_bytes().to_vec(), file_path);
        Editor {
            document,
            carets: CaretSet::new(TextPosition::zero()),
            viewport: Viewport::new(80, 24),
            renderer: Renderer::new(),
            clipboard: Clipboard::internal_only(),
            commands: CommandRegistry::new(),
            keybindings: KeybindingTable::with_defaults(),
            command_buffer: CommandBuffer::new(),
            line_numbers_visible: true,
            status_message: String::new(),
            status_time: None,
            running: true,
            quit_pending: false,
            mouse: MouseState::new(),
            find: FindState::new(),
            sudo_save_tmp: None,
            piped_stdin: false,
            file_modification_time: None,
            reload_pending: false,
            status_left_text_cache: String::new(),
            line_text_scratch: Vec::new(),
        }
    }

    fn selection(e: &Editor) -> Selection {
        e.selection()
    }

    fn set_sel(e: &mut Editor, selection: Selection) {
        e.set_selection(selection);
    }

    // ========================================================================
    // Movement scenarios
    // ========================================================================

    #[test]
    fn test_move_up_down_with_desired_col_stickiness() {
        let mut e = ed("short\nlonger line here\nhi");
        // Move to end of "longer line here" (column 15)
        e.set_cursor(TextPosition::new(1, 15));
        e.move_up(); // line 0 is 5 chars, should clamp to 5
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
        // desired_column should be 15 (sticky)
        e.move_down(); // back to line 1, column should restore to 15
        assert_eq!(e.cursor(), TextPosition::new(1, 15));
        e.move_down(); // line 2 is 2 chars, clamp to 2
        assert_eq!(e.cursor(), TextPosition::new(2, 2));
    }

    #[test]
    fn test_move_up_at_top() {
        let mut e = ed("hello");
        e.move_up();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    #[test]
    fn test_move_down_at_bottom() {
        let mut e = ed("hello");
        e.move_down();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    #[test]
    fn test_move_left_wraps_to_prev_line() {
        let mut e = ed("abc\ndef");
        e.set_cursor(TextPosition::new(1, 0));
        e.move_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 3));
    }

    #[test]
    fn test_move_right_wraps_to_next_line() {
        let mut e = ed("abc\ndef");
        e.set_cursor(TextPosition::new(0, 3));
        e.move_right();
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
    }

    #[test]
    fn test_move_left_collapses_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 2),
                cursor: TextPosition::new(0, 7),
            },
        );
        e.move_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
        assert!(selection(&e).is_empty());
    }

    #[test]
    fn test_move_right_collapses_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 2),
                cursor: TextPosition::new(0, 7),
            },
        );
        e.move_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 7));
        assert!(selection(&e).is_empty());
    }

    #[test]
    fn test_home_end() {
        let mut e = ed("hello world");
        e.set_cursor(TextPosition::new(0, 5));
        e.move_home();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
        e.move_end();
        assert_eq!(e.cursor(), TextPosition::new(0, 11));
    }

    #[test]
    fn test_page_up_down() {
        // 80x24 terminal = 22 text rows
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.set_cursor(TextPosition::new(25, 0));
        e.page_up();
        assert_eq!(e.cursor().line, 3); // 25 - 22 = 3
        e.page_down();
        assert_eq!(e.cursor().line, 25);
    }

    #[test]
    fn test_indent_snap_left_right() {
        let mut e = ed("    hello"); // 4 spaces indent
        e.set_cursor(TextPosition::new(0, 4));
        e.move_left(); // should snap from 4 to 2
        assert_eq!(e.cursor().column, 2);
        e.move_right(); // should snap from 2 to 4
        assert_eq!(e.cursor().column, 4);
    }

    #[test]
    fn test_move_left_at_origin() {
        let mut e = ed("hello");
        e.move_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    #[test]
    fn test_move_right_at_end_of_last_line() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 5));
        e.move_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
    }

    // ========================================================================
    // Selection scenarios
    // ========================================================================

    #[test]
    fn test_shift_arrow_extends_selection() {
        let mut e = ed("hello");
        e.move_right_extend();
        e.move_right_extend();
        assert_eq!(selection(&e).anchor, TextPosition::new(0, 0));
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 2));
        assert!(!selection(&e).is_empty());
    }

    #[test]
    fn test_select_all() {
        let mut e = ed("hello\nworld");
        e.select_all();
        let (start, end) = selection(&e).ordered();
        assert_eq!(start, TextPosition::new(0, 0));
        assert_eq!(end, TextPosition::new(1, 5));
    }

    #[test]
    fn test_select_word_at() {
        let mut e = ed("hello world");
        e.select_word_at(TextPosition::new(0, 7));
        let (start, end) = selection(&e).ordered();
        assert_eq!(start, TextPosition::new(0, 6));
        assert_eq!(end, TextPosition::new(0, 11));
    }

    #[test]
    fn test_select_line_at() {
        let mut e = ed("hello\nworld\nfoo");
        e.select_line_at(1);
        let (start, end) = selection(&e).ordered();
        assert_eq!(start, TextPosition::new(1, 0));
        assert_eq!(end, TextPosition::new(2, 0));
    }

    #[test]
    fn test_select_line_at_last_line() {
        let mut e = ed("hello\nworld");
        e.select_line_at(1);
        let (start, end) = selection(&e).ordered();
        assert_eq!(start, TextPosition::new(1, 0));
        assert_eq!(end, TextPosition::new(1, 5));
    }

    #[test]
    fn test_select_above_below() {
        let mut e = ed("hello\nworld\nfoo");
        e.set_cursor(TextPosition::new(1, 3));
        e.select_above();
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 0));
        assert_eq!(selection(&e).anchor, TextPosition::new(1, 3));

        e.set_cursor(TextPosition::new(1, 3));
        e.select_below();
        assert_eq!(selection(&e).cursor, TextPosition::new(2, 3));
        assert_eq!(selection(&e).anchor, TextPosition::new(1, 3));
    }

    #[test]
    fn test_delete_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 5),
                cursor: TextPosition::new(0, 11),
            },
        );
        e.delete_selection();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_clear_selection() {
        let mut e = ed("hello");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.clear_selection();
        assert!(selection(&e).is_empty());
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
    }

    #[test]
    fn test_shift_up_down_extend() {
        let mut e = ed("aaa\nbbb\nccc");
        e.set_cursor(TextPosition::new(1, 1));
        e.move_up_extend();
        assert_eq!(selection(&e).anchor, TextPosition::new(1, 1));
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 1));
        e.move_down_extend();
        assert_eq!(selection(&e).cursor, TextPosition::new(1, 1));
        e.move_down_extend();
        assert_eq!(selection(&e).cursor, TextPosition::new(2, 1));
    }

    #[test]
    fn test_shift_left_right_extend() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 2));
        e.move_left_extend();
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 1));
        e.move_right_extend();
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 2));
    }

    // ========================================================================
    // Editing scenarios
    // ========================================================================

    #[test]
    fn test_insert_char() {
        let mut e = ed("hllo");
        e.set_cursor(TextPosition::new(0, 1));
        e.insert_char('e');
        assert_eq!(e.test_text(), "hello");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
    }

    #[test]
    fn test_insert_char_replaces_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 5),
                cursor: TextPosition::new(0, 11),
            },
        );
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
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(2, 3),
            },
        );
        e.insert_tab();
        assert_eq!(e.test_text(), "  aaa\n  bbb\n  ccc");
    }

    #[test]
    fn test_tab_indents_selection_preserves_selection() {
        // Selection should remain after Tab so the user can indent multiple times.
        let mut e = ed_named("aaa\nbbb\nccc", "test.rs");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(2, 3),
            },
        );
        e.insert_tab();
        assert!(!selection(&e).is_empty());
        assert_eq!(selection(&e).anchor, TextPosition::new(0, 2));
        assert_eq!(selection(&e).cursor, TextPosition::new(2, 5));
    }

    #[test]
    fn test_insert_newline_with_auto_indent() {
        let mut e = ed("  hello");
        e.set_cursor(TextPosition::new(0, 7));
        e.insert_newline();
        assert_eq!(e.test_text(), "  hello\n  ");
        assert_eq!(e.cursor(), TextPosition::new(1, 2));
    }

    #[test]
    fn test_insert_newline_replaces_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 5),
                cursor: TextPosition::new(0, 11),
            },
        );
        e.insert_newline();
        assert_eq!(e.test_text(), "hello\n");
    }

    #[test]
    fn test_insert_newline_between_braces() {
        let mut e = ed("{}");
        e.set_cursor(TextPosition::new(0, 1));
        e.insert_newline();
        assert_eq!(e.test_text(), "{\n  \n}");
        assert_eq!(e.cursor(), TextPosition::new(1, 2));
    }

    #[test]
    fn test_insert_newline_between_parens() {
        let mut e = ed("()");
        e.set_cursor(TextPosition::new(0, 1));
        e.insert_newline();
        assert_eq!(e.test_text(), "(\n  \n)");
        assert_eq!(e.cursor(), TextPosition::new(1, 2));
    }

    #[test]
    fn test_insert_newline_between_brackets() {
        let mut e = ed("[]");
        e.set_cursor(TextPosition::new(0, 1));
        e.insert_newline();
        assert_eq!(e.test_text(), "[\n  \n]");
        assert_eq!(e.cursor(), TextPosition::new(1, 2));
    }

    #[test]
    fn test_insert_newline_between_braces_preserves_indent() {
        let mut e = ed("  {}");
        e.set_cursor(TextPosition::new(0, 3));
        e.insert_newline();
        assert_eq!(e.test_text(), "  {\n    \n  }");
        assert_eq!(e.cursor(), TextPosition::new(1, 4));
    }

    #[test]
    fn test_insert_newline_not_between_pair_no_split() {
        // Cursor after `{` but no closing `}` immediately after — normal newline.
        let mut e = ed("{hello}");
        e.set_cursor(TextPosition::new(0, 1));
        e.insert_newline();
        assert_eq!(e.test_text(), "{\nhello}");
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
    }

    #[test]
    fn test_backspace_basic() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 5));
        e.backspace();
        assert_eq!(e.test_text(), "hell");
    }

    #[test]
    fn test_backspace_joins_lines() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(1, 0));
        e.backspace();
        assert_eq!(e.test_text(), "helloworld");
    }

    #[test]
    fn test_backspace_indent_snap() {
        let mut e = ed("    x");
        e.set_cursor(TextPosition::new(0, 4));
        e.backspace(); // should snap from 4 to 2
        assert_eq!(e.test_text(), "  x");
    }

    #[test]
    fn test_backspace_deletes_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 5),
                cursor: TextPosition::new(0, 11),
            },
        );
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
        e.set_cursor(TextPosition::new(0, 0));
        e.delete_forward();
        assert_eq!(e.test_text(), "ello");
    }

    #[test]
    fn test_delete_forward_joins_lines() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(0, 5));
        e.delete_forward();
        assert_eq!(e.test_text(), "helloworld");
    }

    #[test]
    fn test_delete_forward_with_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.delete_forward();
        assert_eq!(e.test_text(), " world");
    }

    #[test]
    fn test_ctrl_backspace_word_delete() {
        let mut e = ed("hello world");
        e.set_cursor(TextPosition::new(0, 11));
        e.ctrl_backspace();
        assert_eq!(e.test_text(), "hello ");
    }

    #[test]
    fn test_ctrl_backspace_at_line_start() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(1, 0));
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
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.ctrl_backspace();
        assert_eq!(e.test_text(), " world");
    }

    #[test]
    fn test_duplicate_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(0, 2));
        e.duplicate_line();
        assert_eq!(e.test_text(), "hello\nhello\nworld");
        assert_eq!(e.cursor(), TextPosition::new(1, 2));
    }

    // ========================================================================
    // Find/replace scenarios
    // ========================================================================

    #[test]
    fn test_find_highlights_smart_case() {
        let mut e = ed("Hello hello HELLO");
        e.update_find_highlights("hello");
        assert_eq!(e.find.matches.len(), 3); // case-insensitive (all lowercase pattern)
    }

    #[test]
    fn test_find_case_sensitive() {
        let mut e = ed("Hello hello HELLO");
        e.update_find_highlights("Hello");
        assert_eq!(e.find.matches.len(), 1); // uppercase in pattern → case-sensitive
    }

    #[test]
    fn test_find_invalid_regex() {
        let mut e = ed("hello [world");
        e.update_find_highlights("[invalid");
        assert!(e.find.matches.is_empty()); // invalid regex → no matches, no panic
    }

    #[test]
    fn test_find_empty_pattern() {
        let mut e = ed("hello");
        e.update_find_highlights("");
        assert!(e.find.matches.is_empty());
    }

    #[test]
    fn test_find_next_wraps_around() {
        let mut e = ed("aa bb aa");
        e.update_find_highlights("aa");
        assert_eq!(e.find.matches.len(), 2);
        e.find.active = true;
        // Position cursor past all matches
        e.set_cursor(TextPosition::new(0, 8));
        e.find_next();
        // wrapped around to first match (column 0..2)
        assert_eq!(
            e.find.current,
            Some((TextPosition::new(0, 0), TextPosition::new(0, 2)))
        );
    }

    #[test]
    fn test_find_prev_wraps_around() {
        let mut e = ed("aa bb aa");
        e.update_find_highlights("aa");
        e.find.active = true;
        e.set_cursor(TextPosition::new(0, 0));
        e.find_prev();
        // wrapped around to last match (column 6..8)
        assert_eq!(
            e.find.current,
            Some((TextPosition::new(0, 6), TextPosition::new(0, 8)))
        );
    }

    #[test]
    fn test_find_next_from_submit() {
        let mut e = ed("hello world hello");
        e.find_next_from_submit("hello");
        assert!(e.find.active);
        assert_eq!(e.find.matches.len(), 2);
        assert!(e.find.current.is_some());
    }

    #[test]
    fn test_find_next_from_submit_no_matches() {
        let mut e = ed("hello world");
        e.find_next_from_submit("xyz");
        assert!(!e.find.active);
        assert!(e.status_message.contains("no matches"));
    }

    #[test]
    fn test_exit_find_mode_selects_match() {
        let mut e = ed("hello world hello");
        e.find_next_from_submit("hello");
        e.exit_find_mode();
        assert!(!e.find.active);
        assert!(e.find.matches.is_empty());
        // Selection should cover the match
        assert!(!selection(&e).is_empty());
    }

    #[test]
    fn test_replace_all_whole_file() {
        let mut e = ed("foo bar foo");
        e.replace_all("foo", "baz");
        assert_eq!(e.test_text(), "baz bar baz");
        assert!(e.status_message.contains("2"));
    }

    #[test]
    fn test_replace_all_in_selection() {
        let mut e = ed("foo bar foo");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 3),
            },
        );
        e.replace_all("foo", "baz");
        assert_eq!(e.test_text(), "baz bar foo");
    }

    #[test]
    fn test_replace_all_no_matches() {
        let mut e = ed("hello world");
        e.replace_all("xyz", "abc");
        assert!(e.status_message.contains("0"));
    }

    #[test]
    fn test_replace_all_invalid_regex() {
        let mut e = ed("hello");
        e.replace_all("[invalid", "x");
        assert!(e.status_message.contains("Invalid regex"));
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
        e.set_cursor(TextPosition::new(1, 2));
        e.goto_top();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
        e.goto_end();
        assert_eq!(e.cursor(), TextPosition::new(2, 3));
    }

    #[test]
    fn test_kill_line_middle() {
        let mut e = ed("aaa\nbbb\nccc");
        e.set_cursor(TextPosition::new(1, 1));
        e.kill_line();
        assert_eq!(e.test_text(), "aaa\nccc");
    }

    #[test]
    fn test_kill_line_last() {
        let mut e = ed("aaa\nbbb");
        e.set_cursor(TextPosition::new(1, 0));
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
        assert!(e.line_numbers_visible);
        e.execute_command("ruler");
        assert!(!e.line_numbers_visible);
        e.execute_command("ruler");
        assert!(e.line_numbers_visible);
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
        assert!(e.status_message.contains("Unknown"));
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
        assert!(!selection(&e).is_empty());
        let (start, end) = selection(&e).ordered();
        assert_eq!(start, TextPosition::zero());
        assert_eq!(end.line, 1);
    }

    #[test]
    fn test_complete_command_single_match() {
        let mut e = ed("hello");
        e.command_buffer
            .open(CommandBufferMode::Command, "> ", "rul");
        e.complete_command();
        assert_eq!(e.command_buffer.input, "ruler");
        assert!(e.command_buffer.completions.is_empty());
    }

    #[test]
    fn test_complete_command_multiple_matches() {
        let mut e = ed("hello");
        e.command_buffer.open(CommandBufferMode::Command, "> ", "q");
        e.complete_command();
        assert_eq!(e.command_buffer.completions.len(), 2); // "q" and "quit"
    }

    #[test]
    fn test_complete_command_no_matches() {
        let mut e = ed("hello");
        e.command_buffer
            .open(CommandBufferMode::Command, "> ", "xyz");
        e.complete_command();
        assert!(e.command_buffer.completions.is_empty());
    }

    #[test]
    fn test_complete_command_empty_shows_all() {
        let mut e = ed("hello");
        e.command_buffer.open(CommandBufferMode::Command, "> ", "");
        e.complete_command();
        assert!(!e.command_buffer.completions.is_empty());
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
        assert!(e.find.active);
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
        assert_eq!(
            e.document.file_path.as_deref(),
            Some(path.to_str().unwrap())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_handle_cmd_result_cancel_find() {
        let mut e = ed("hello");
        e.find.matches = vec![(TextPosition::new(0, 0), TextPosition::new(0, 5))];
        e.handle_cmd_result(CommandBufferMode::Find, CommandBufferResult::Cancel);
        assert!(e.find.matches.is_empty());
    }

    #[test]
    fn test_handle_cmd_result_cancel_sudo() {
        let mut e = ed("hello");
        e.sudo_save_tmp = Some("/tmp/nonexistent_test_file".to_string());
        e.handle_cmd_result(CommandBufferMode::SudoSave, CommandBufferResult::Cancel);
        assert!(e.sudo_save_tmp.is_none());
        assert!(e.status_message.contains("cancelled"));
    }

    #[test]
    fn test_handle_cmd_result_changed_find() {
        let mut e = ed("hello world hello");
        e.handle_cmd_result(
            CommandBufferMode::Find,
            CommandBufferResult::Changed("hello".to_string()),
        );
        assert_eq!(e.find.matches.len(), 2);
    }

    #[test]
    fn test_handle_cmd_result_tab_complete() {
        let mut e = ed("hello");
        e.command_buffer
            .open(CommandBufferMode::Command, "> ", "rul");
        e.handle_cmd_result(CommandBufferMode::Command, CommandBufferResult::TabComplete);
        assert_eq!(e.command_buffer.input, "ruler");
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
        e.dispatch_event(EditorEvent::Key(Key::Char('x')));
        assert_eq!(e.test_text(), "xhello");
    }

    #[test]
    fn test_handle_event_mouse_ignored_when_cmd_active() {
        let mut e = ed("hello");
        e.command_buffer.open(CommandBufferMode::Command, "> ", "");
        e.dispatch_event(EditorEvent::Mouse(
            MouseEvent::Press(MouseButton::Left, 1, 1),
            MouseMods::default(),
        ));
        // Mouse should be ignored when command_buffer is active
        assert!(e.command_buffer.active);
    }

    #[test]
    fn test_handle_event_unsupported_ctrl_shift_up() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(1, 3));
        e.dispatch_event(EditorEvent::Key(Key::CtrlShiftUp));
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 0));
    }

    #[test]
    fn test_handle_event_unsupported_ctrl_shift_down() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(0, 2));
        e.dispatch_event(EditorEvent::Key(Key::CtrlShiftDown));
        assert_eq!(selection(&e).cursor, TextPosition::new(1, 5));
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
        e.document.is_dirty = true;
        e.try_quit();
        assert!(e.running);
        assert!(e.quit_pending);
        assert!(e.status_message.contains("Save changes"));
    }

    #[test]
    fn test_quit_dirty_then_y() {
        let dir = std::env::temp_dir().join("e_test_quit_y");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut e = ed_named("hello", path.to_str().unwrap());
        e.document.is_dirty = true;
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
        e.document.is_dirty = true;
        e.try_quit();
        // Pressing 'y' on a scratch buffer must open the save-as prompt, not quit.
        e.handle_key(Key::Char('y'));
        assert!(e.running, "editor must not quit before filename is given");
        assert!(e.command_buffer.active, "save-as prompt must be open");
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
        e.document.is_dirty = true;
        e.try_quit();
        e.handle_key(Key::Char('n'));
        assert!(!e.running);
    }

    #[test]
    fn test_quit_dirty_then_cancel() {
        let mut e = ed("hello");
        e.document.is_dirty = true;
        e.try_quit();
        e.handle_key(Key::Esc);
        assert!(e.running);
        assert!(!e.quit_pending);
    }

    #[test]
    fn test_find_nav_up_down() {
        let mut e = ed("aa bb aa");
        e.find_next_from_submit("aa");
        assert!(e.find.active);
        let first = e.find.current;
        e.handle_key(Key::Down);
        let second = e.find.current;
        // moved forward to a different match
        assert_ne!(first, second);
        e.handle_key(Key::Up);
        // moved back to (or past) the original
        assert_ne!(e.find.current, second);
    }

    #[test]
    fn test_find_nav_esc_clears() {
        let mut e = ed("aa bb aa");
        e.find_next_from_submit("aa");
        e.handle_key(Key::Esc);
        assert!(!e.find.active);
        assert!(selection(&e).is_empty());
    }

    #[test]
    fn test_find_nav_other_key_exits_and_processes() {
        let mut e = ed("aa bb");
        e.find_next_from_submit("aa");
        assert!(e.find.active);
        e.handle_key(Key::Char('x'));
        assert!(!e.find.active);
        // 'x' should have been processed as an insert
        assert!(e.test_text().contains('x'));
    }

    #[test]
    fn test_esc_clears_selection_and_matches() {
        let mut e = ed("hello");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.find.matches = vec![(TextPosition::new(0, 0), TextPosition::new(0, 5))];
        e.handle_key(Key::Esc);
        assert!(selection(&e).is_empty());
        assert!(e.find.matches.is_empty());
    }

    #[test]
    fn test_keybinding_action_dispatch() {
        let mut e = ed("hello");
        // Ctrl+a should select all
        e.handle_key(Key::Ctrl('a'));
        assert!(!selection(&e).is_empty());
    }

    #[test]
    fn test_handle_cmd_key_dispatches() {
        let mut e = ed("hello");
        e.command_buffer.open(CommandBufferMode::Command, "> ", "");
        e.handle_cmd_key(Key::Char('a'));
        assert_eq!(e.command_buffer.input, "a");
    }

    // ========================================================================
    // Mouse scenarios
    // ========================================================================

    #[test]
    fn test_mouse_single_click() {
        let mut e = ed("hello\nworld");
        e.mouse_press(6, 2, MouseMods::default()); // column 5, row 1 (1-indexed terminal coords)
        assert_eq!(e.cursor().line, 1);
    }

    #[test]
    fn test_mouse_drag() {
        let mut e = ed("hello world");
        e.mouse_press(3, 1, MouseMods::default()); // start drag
        assert!(e.mouse.dragging);
        e.mouse_drag(8, 1);
        assert_ne!(selection(&e).anchor, selection(&e).cursor);
    }

    #[test]
    fn test_mouse_release() {
        let mut e = ed("hello");
        e.mouse.dragging = true;
        e.handle_mouse(MouseEvent::Release(0, 0), MouseMods::default());
        assert!(!e.mouse.dragging);
    }

    #[test]
    fn test_ctrl_click_adds_caret_and_inserts_at_all_carets() {
        let mut e = ed("abc\ndef");
        e.line_numbers_visible = false;

        e.mouse_press(2, 2, MouseMods { ctrl: true });

        assert_eq!(e.carets.len(), 2);
        assert_eq!(e.cursor(), TextPosition::new(1, 1));

        e.insert_char('X');
        assert_eq!(e.test_text(), "Xabc\ndXef");
        assert_eq!(e.carets.len(), 2);
    }

    #[test]
    fn test_multicursor_vertical_movement_preserves_all_carets() {
        let mut e = ed("abc\n123456\nz");
        e.carets.carets = vec![
            Caret::new(TextPosition::new(1, 5)),
            Caret::new(TextPosition::new(1, 2)),
        ];
        e.carets.primary = 0;
        e.carets.normalize();

        e.move_up();
        assert_eq!(e.carets.len(), 2);
        assert_eq!(e.cursor(), TextPosition::new(0, 3));
        assert_eq!(
            e.carets
                .iter()
                .map(|caret| caret.selection.cursor)
                .collect::<Vec<_>>(),
            vec![TextPosition::new(0, 2), TextPosition::new(0, 3)]
        );

        e.move_down();
        assert_eq!(e.carets.len(), 2);
        assert_eq!(e.cursor(), TextPosition::new(1, 5));
        assert_eq!(
            e.carets
                .iter()
                .map(|caret| caret.selection.cursor)
                .collect::<Vec<_>>(),
            vec![TextPosition::new(1, 2), TextPosition::new(1, 5)]
        );
    }

    #[test]
    fn test_multicursor_word_right_moves_all_carets() {
        let mut e = ed("hello world\nfoo bar");
        e.carets.carets = vec![
            Caret::new(TextPosition::new(0, 0)),
            Caret::new(TextPosition::new(1, 0)),
        ];
        e.carets.primary = 1;
        e.carets.normalize();

        e.word_right();

        assert_eq!(e.carets.len(), 2);
        assert_eq!(e.cursor(), TextPosition::new(1, 4));
        assert_eq!(
            e.carets
                .iter()
                .map(|caret| caret.selection.cursor)
                .collect::<Vec<_>>(),
            vec![TextPosition::new(0, 6), TextPosition::new(1, 4)]
        );
    }

    #[test]
    fn test_mouse_scroll_up_down() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.viewport.scroll_line = 10;
        e.set_cursor(TextPosition::new(15, 0));
        e.scroll_down();
        assert!(e.viewport.scroll_line > 10);
        let prev = e.viewport.scroll_line;
        e.scroll_up();
        assert!(e.viewport.scroll_line < prev);
    }

    #[test]
    fn test_scroll_up_at_top() {
        let mut e = ed("hello\nworld");
        e.scroll_up();
        assert_eq!(e.viewport.scroll_line, 0);
    }

    #[test]
    fn test_scroll_down_at_bottom() {
        let mut e = ed("hello");
        e.scroll_down();
        assert_eq!(e.viewport.scroll_line, 0);
    }

    #[test]
    fn test_screen_to_buffer_position_normal() {
        let e = ed("hello\nworld");
        let pos = e.screen_to_buffer_position(5, 1); // column 4, row 0
        assert_eq!(pos.line, 0);
    }

    #[test]
    fn test_screen_to_buffer_position_below_content() {
        let e = ed("hello");
        let pos = e.screen_to_buffer_position(1, 20); // way below
        assert_eq!(pos.line, 0);
        assert_eq!(pos.column, 5);
    }

    #[test]
    fn test_clamp_cursor_to_viewport() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.set_cursor(TextPosition::new(0, 0));
        e.viewport.scroll_line = 10;
        let gw = gutter_width(e.document.buffer.line_count());
        let tc = e.viewport.text_cols(gw);
        e.clamp_cursor_to_viewport(gw, tc);
        // Cursor should be moved into viewport
        assert!(e.cursor().line >= e.viewport.scroll_line);
    }

    #[test]
    fn test_handle_event_mouse_exits_find_active() {
        let mut e = ed("hello world");
        e.find.active = true;
        e.find.matches = vec![(TextPosition::new(0, 0), TextPosition::new(0, 5))];
        e.dispatch_event(EditorEvent::Mouse(
            MouseEvent::Press(MouseButton::Left, 1, 1),
            MouseMods::default(),
        ));
        assert!(!e.find.active);
    }

    // ========================================================================
    // Clipboard/undo scenarios
    // ========================================================================

    #[test]
    fn test_copy_paste_workflow() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.copy();
        e.set_cursor(TextPosition::new(0, 11));
        e.paste();
        assert_eq!(e.test_text(), "hello worldhello");
    }

    #[test]
    fn test_cut() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.cut();
        assert_eq!(e.test_text(), " world");
        e.paste();
        assert_eq!(e.test_text(), "hello world");
    }

    #[test]
    fn test_multi_cursor_internal_clipboard_pastes_fragments_per_caret() {
        let mut e = ed("hello\nworld");
        e.carets.carets = vec![
            Caret {
                selection: Selection {
                    anchor: TextPosition::new(0, 0),
                    cursor: TextPosition::new(0, 5),
                },
                desired_column: None,
            },
            Caret {
                selection: Selection {
                    anchor: TextPosition::new(1, 0),
                    cursor: TextPosition::new(1, 5),
                },
                desired_column: None,
            },
        ];
        e.carets.primary = 0;
        e.carets.normalize();
        e.copy();

        e.carets.carets = vec![
            Caret::new(TextPosition::new(0, 5)),
            Caret::new(TextPosition::new(1, 5)),
        ];
        e.carets.primary = 1;
        e.carets.normalize();
        e.paste();

        assert_eq!(e.test_text(), "hellohello\nworldworld");
    }

    #[test]
    fn test_multicursor_tab_with_selections_indents_lines() {
        let mut e = ed_named("aaa\nbbb\nccc", "test.rs");
        e.carets.carets = vec![
            Caret {
                selection: Selection {
                    anchor: TextPosition::new(0, 0),
                    cursor: TextPosition::new(0, 3),
                },
                desired_column: None,
            },
            Caret {
                selection: Selection {
                    anchor: TextPosition::new(2, 0),
                    cursor: TextPosition::new(2, 3),
                },
                desired_column: None,
            },
        ];
        e.carets.primary = 0;
        e.carets.normalize();

        e.insert_tab();

        assert_eq!(e.test_text(), "  aaa\nbbb\n  ccc");
        assert_eq!(e.carets.len(), 2);
        assert_eq!(
            e.carets.carets[0].selection.ordered(),
            (TextPosition::new(0, 2), TextPosition::new(0, 5))
        );
        assert_eq!(
            e.carets.carets[1].selection.ordered(),
            (TextPosition::new(2, 2), TextPosition::new(2, 5))
        );
    }

    #[test]
    fn test_multicursor_linewise_indent_handles_more_lines_than_carets() {
        let mut e = ed_named("aaa\nbbb\nccc\nddd", "test.rs");
        e.carets.carets = vec![
            Caret {
                selection: Selection {
                    anchor: TextPosition::new(0, 0),
                    cursor: TextPosition::new(1, 3),
                },
                desired_column: None,
            },
            Caret {
                selection: Selection {
                    anchor: TextPosition::new(3, 0),
                    cursor: TextPosition::new(3, 3),
                },
                desired_column: None,
            },
        ];
        e.carets.primary = 1;
        e.carets.normalize();

        e.insert_tab();

        assert_eq!(e.test_text(), "  aaa\n  bbb\nccc\n  ddd");
        assert_eq!(e.cursor(), TextPosition::new(3, 5));
        assert_eq!(
            e.carets.carets[0].selection.ordered(),
            (TextPosition::new(0, 2), TextPosition::new(1, 5))
        );
        assert_eq!(
            e.carets.carets[1].selection.ordered(),
            (TextPosition::new(3, 2), TextPosition::new(3, 5))
        );
    }

    #[test]
    fn test_multicursor_dedent_comment_undo_redo_preserve_carets() {
        let mut e = ed_named("  aaa\n  bbb\nccc", "test.rs");
        e.carets.carets = vec![
            Caret::new(TextPosition::new(0, 2)),
            Caret::new(TextPosition::new(1, 2)),
        ];
        e.carets.primary = 1;
        e.carets.normalize();

        e.dedent();
        assert_eq!(e.test_text(), "aaa\nbbb\nccc");
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
        assert_eq!(
            e.carets
                .iter()
                .map(|caret| caret.selection.cursor)
                .collect::<Vec<_>>(),
            vec![TextPosition::new(0, 0), TextPosition::new(1, 0)]
        );

        e.toggle_comment();
        assert_eq!(e.test_text(), "// aaa\n// bbb\nccc");
        assert_eq!(e.cursor(), TextPosition::new(1, 3));
        assert_eq!(
            e.carets
                .iter()
                .map(|caret| caret.selection.cursor)
                .collect::<Vec<_>>(),
            vec![TextPosition::new(0, 3), TextPosition::new(1, 3)]
        );

        e.undo();
        assert_eq!(e.test_text(), "aaa\nbbb\nccc");
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
        assert_eq!(
            e.carets
                .iter()
                .map(|caret| caret.selection.cursor)
                .collect::<Vec<_>>(),
            vec![TextPosition::new(0, 0), TextPosition::new(1, 0)]
        );

        e.undo();
        assert_eq!(e.test_text(), "  aaa\n  bbb\nccc");
        assert_eq!(e.cursor(), TextPosition::new(1, 2));
        assert_eq!(
            e.carets
                .iter()
                .map(|caret| caret.selection.cursor)
                .collect::<Vec<_>>(),
            vec![TextPosition::new(0, 2), TextPosition::new(1, 2)]
        );

        e.redo();
        assert_eq!(e.test_text(), "aaa\nbbb\nccc");
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
        assert_eq!(
            e.carets
                .iter()
                .map(|caret| caret.selection.cursor)
                .collect::<Vec<_>>(),
            vec![TextPosition::new(0, 0), TextPosition::new(1, 0)]
        );
    }

    #[test]
    fn test_paste_text_replaces_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 6),
                cursor: TextPosition::new(0, 11),
            },
        );
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
        e.set_cursor(TextPosition::new(0, 5));
        e.document.seal_undo();
        e.insert_char('!');
        e.document.seal_undo();
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
        e.set_cursor(TextPosition::new(0, 0));
        e.toggle_comment();
        assert_eq!(e.test_text(), "// hello\nworld");
    }

    #[test]
    fn test_toggle_comment_off_rs_file() {
        let mut e = ed_named("// hello\nworld", "test.rs");
        e.set_cursor(TextPosition::new(0, 0));
        e.toggle_comment();
        assert_eq!(e.test_text(), "hello\nworld");
    }

    #[test]
    fn test_toggle_comment_no_language() {
        let mut e = ed("hello");
        e.toggle_comment();
        assert!(e.status_message.contains("No language"));
    }

    #[test]
    fn test_toggle_comment_selection() {
        let mut e = ed_named("aaa\nbbb\nccc", "test.rs");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(2, 3),
            },
        );
        e.toggle_comment();
        assert_eq!(e.test_text(), "// aaa\n// bbb\n// ccc");
    }

    #[test]
    fn test_dedent_spaces() {
        let mut e = ed("  hello");
        e.set_cursor(TextPosition::new(0, 2));
        e.dedent();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_dedent_tab() {
        let mut e = ed("\thello");
        e.set_cursor(TextPosition::new(0, 1));
        e.dedent();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_dedent_no_indent() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 0));
        e.dedent();
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_indent_selection_skips_blank_lines() {
        let mut e = ed_named("aaa\n\nbbb", "test.rs");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(2, 3),
            },
        );
        e.indent_selection();
        assert_eq!(e.test_text(), "  aaa\n\n  bbb");
    }

    // ========================================================================
    // File I/O scenarios
    // ========================================================================

    #[test]
    fn test_strip_trailing_whitespace() {
        let mut e = ed("hello   \nworld  ");
        e.set_cursor(TextPosition::new(0, 8));
        e.strip_trailing_whitespace();
        assert_eq!(e.test_text(), "hello\nworld");
        // Cursor should be clamped
        assert!(e.cursor().column <= 5);
    }

    #[test]
    fn test_tabs_to_spaces() {
        let mut e = ed("\thello\n\t\tworld");
        e.tabs_to_spaces();
        assert_eq!(e.test_text(), "  hello\n    world");
    }

    #[test]
    fn test_tabs_to_spaces_mid_line() {
        let mut e = ed("a\tb");
        e.tabs_to_spaces();
        assert_eq!(e.test_text(), "a  b");
    }

    #[test]
    fn test_spaces_to_tabs_leading_only() {
        let mut e = ed("    hello  world");
        e.spaces_to_tabs();
        assert_eq!(e.test_text(), "\t\thello  world");
    }

    #[test]
    fn test_spaces_to_tabs_odd_spaces() {
        let mut e = ed("   hello");
        e.spaces_to_tabs();
        assert_eq!(e.test_text(), "\t hello");
    }

    #[test]
    fn test_save_no_filename_opens_prompt() {
        let mut e = ed("hello");
        e.save_file();
        assert!(e.command_buffer.active);
        assert_eq!(e.command_buffer.mode, CommandBufferMode::Prompt);
    }

    #[test]
    fn test_save_to_temp_file() {
        let dir = std::env::temp_dir().join("e_test_save");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        let mut e = ed_named("hello world", path.to_str().unwrap());
        e.document.is_dirty = true;
        e.save_file();
        assert!(!e.document.is_dirty);
        assert!(e.status_message.contains("Saved"));
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
            .document
            .detect_language()
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
        e.document.is_dirty = true;
        let lang_name = e
            .document
            .detect_language()
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
    fn test_cursor_display_col_with_tabs() {
        let mut e = ed("\thello");
        e.set_cursor(TextPosition::new(0, 1));
        assert_eq!(e.cursor_display_col(), 2); // tab = 2 display cols
    }

    #[test]
    fn test_cursor_display_col_no_tabs() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 3));
        assert_eq!(e.cursor_display_col(), 3);
    }

    #[test]
    fn test_find_matching_bracket_none() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 0));
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
        assert!(e.viewport.scroll_line <= 50);
        assert!(e.viewport.scroll_line + e.viewport.text_rows() > 50);
    }

    // ========================================================================
    // draw() smoke test
    // ========================================================================

    #[test]
    fn test_draw_does_not_panic() {
        let mut e = ed_named("hello\nworld", "test.rs");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.find.matches = vec![(TextPosition::new(0, 0), TextPosition::new(0, 5))];
        e.find.active = true;
        let mut output = Vec::new();
        e.draw(&mut output).unwrap();
        assert!(!output.is_empty());
    }

    #[test]
    fn test_draw_with_cmd_buf_active() {
        let mut e = ed("hello");
        e.command_buffer
            .open(CommandBufferMode::Find, "find: ", "test");
        e.command_buffer.completions = vec!["comp1".to_string()];
        let mut output = Vec::new();
        e.draw(&mut output).unwrap();
        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("find: test"));
    }

    #[test]
    fn test_draw_ruler_off() {
        let mut e = ed("hello");
        e.line_numbers_visible = false;
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
        e.set_cursor(TextPosition::new(0, 2));
        e.handle_key(Key::BackTab);
        assert_eq!(e.test_text(), "hello");
    }

    #[test]
    fn test_handle_key_newline() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 5));
        e.handle_key(Key::Char('\n'));
        assert_eq!(e.test_text(), "hello\n");
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
    }

    #[test]
    fn test_newline_empty_buffer() {
        let mut e = ed("");
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
        e.handle_key(Key::Char('\n'));
        assert_eq!(e.test_text(), "\n");
        assert_eq!(
            e.cursor(),
            TextPosition::new(1, 0),
            "cursor should move to line 1"
        );
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
        e.document.is_dirty = true;
        e.handle_key(Key::Ctrl('s'));
        assert!(!e.document.is_dirty);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_keybinding_undo_redo() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 5));
        e.insert_char('!');
        e.document.seal_undo();
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
        e.set_cursor(TextPosition::new(0, 5));
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
        e.set_cursor(TextPosition::new(1, 1));
        e.handle_key(Key::Ctrl('t'));
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
        e.handle_key(Key::Ctrl('g'));
        assert_eq!(e.cursor(), TextPosition::new(2, 3));
    }

    #[test]
    fn test_keybinding_toggle_ruler() {
        let mut e = ed("hello");
        assert!(e.line_numbers_visible);
        e.handle_key(Key::Ctrl('r'));
        assert!(!e.line_numbers_visible);
    }

    #[test]
    fn test_keybinding_command_palette() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('p'));
        assert!(e.command_buffer.active);
        assert_eq!(e.command_buffer.mode, CommandBufferMode::Command);
    }

    #[test]
    fn test_keybinding_goto_line() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('l'));
        assert!(e.command_buffer.active);
        assert_eq!(e.command_buffer.mode, CommandBufferMode::Goto);
    }

    #[test]
    fn test_keybinding_find() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('f'));
        assert!(e.command_buffer.active);
        assert_eq!(e.command_buffer.mode, CommandBufferMode::Find);
    }

    #[test]
    fn test_keybinding_find_prefills_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 6),
                cursor: TextPosition::new(0, 11),
            },
        );
        e.handle_key(Key::Ctrl('f'));
        assert_eq!(e.command_buffer.input, "world");
    }

    #[test]
    fn test_keybinding_ctrl_backspace() {
        let mut e = ed("hello world");
        e.set_cursor(TextPosition::new(0, 11));
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
        e.set_cursor(TextPosition::new(0, 7));
        e.handle_key(Key::Ctrl('w'));
        assert!(!selection(&e).is_empty());
    }

    // ========================================================================
    // desired_column reset
    // ========================================================================

    #[test]
    fn test_desired_col_reset_on_non_vertical_movement() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(0, 3));
        e.handle_key(Key::Down); // sets desired_column
        assert!(e.desired_column().is_some());
        e.handle_key(Key::Char('x')); // non-vertical key should clear it
        assert!(e.desired_column().is_none());
    }

    // ========================================================================
    // mouse double/triple click
    // ========================================================================

    #[test]
    fn test_select_word_at_empty_line() {
        let mut e = ed("hello\n\nworld");
        e.select_word_at(TextPosition::new(1, 0));
        // Empty line should not select anything (early return)
        assert!(selection(&e).is_empty());
    }

    #[test]
    fn test_select_line_at_out_of_bounds() {
        let mut e = ed("hello");
        e.select_line_at(999);
        assert!(selection(&e).is_empty());
    }

    // ========================================================================
    // set_status
    // ========================================================================

    #[test]
    fn test_set_status() {
        let mut e = ed("hello");
        e.set_status("test message".to_string());
        assert_eq!(e.status_message, "test message");
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
        e.viewport.scroll_line = 10;
        e.set_cursor(TextPosition::new(15, 0));
        e.handle_mouse(
            MouseEvent::Press(MouseButton::WheelUp, 1, 1),
            MouseMods::default(),
        );
        assert!(e.viewport.scroll_line < 10);
    }

    #[test]
    fn test_handle_mouse_wheel_down() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.set_cursor(TextPosition::new(5, 0));
        e.handle_mouse(
            MouseEvent::Press(MouseButton::WheelDown, 1, 1),
            MouseMods::default(),
        );
        assert!(e.viewport.scroll_line > 0);
    }

    #[test]
    fn test_handle_mouse_other_button_noop() {
        let mut e = ed("hello");
        e.handle_mouse(
            MouseEvent::Press(MouseButton::Middle, 1, 1),
            MouseMods::default(),
        );
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
    // handle_event dispatches cmd_key when command_buffer active
    // ========================================================================

    #[test]
    fn test_handle_event_dispatches_cmd_key() {
        let mut e = ed("hello");
        e.command_buffer.open(CommandBufferMode::Command, "> ", "");
        e.dispatch_event(EditorEvent::Key(Key::Char('x')));
        assert_eq!(e.command_buffer.input, "x");
    }

    #[test]
    fn test_unsupported_ignored_when_cmd_active() {
        let mut e = ed("hello\nworld");
        e.command_buffer.open(CommandBufferMode::Command, "> ", "");
        e.dispatch_event(EditorEvent::Key(Key::CtrlShiftUp));
        // Should be ignored, cursor unchanged
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
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
        e.set_cursor(TextPosition::new(0, 2));
        e.handle_key(Key::ShiftRight);
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 3));
        e.handle_key(Key::ShiftLeft);
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 2));
        e.handle_key(Key::ShiftDown);
        assert_eq!(selection(&e).cursor, TextPosition::new(1, 2));
        e.handle_key(Key::ShiftUp);
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 2));
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
        e.set_cursor(TextPosition::new(25, 0));
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
        assert_eq!(e.cursor().column, 1);
        e.handle_key(Key::Left);
        assert_eq!(e.cursor().column, 0);
        e.handle_key(Key::End);
        assert_eq!(e.cursor().column, 5);
        e.handle_key(Key::Home);
        assert_eq!(e.cursor().column, 0);
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
        e.line_numbers_visible = false;
        // Start scrolled at line 1
        e.viewport.scroll_line = 1;
        e.viewport.scroll_wrap = 0;
        e.set_cursor(TextPosition::new(1, 0));
        // Scroll up — should go into line 0's wraps
        e.scroll_up();
        assert_eq!(e.viewport.scroll_line, 0);
        assert!(e.viewport.scroll_wrap > 0); // should be partway through wraps
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
        e.line_numbers_visible = false;
        e.viewport.scroll_line = 0;
        e.viewport.scroll_wrap = 0;
        e.set_cursor(TextPosition::new(0, 0));
        e.scroll_down();
        // Should have advanced through wraps within line 0
        assert_eq!(e.viewport.scroll_line, 0);
        assert_eq!(e.viewport.scroll_wrap, 3); // SCROLL_LINES = 3
    }

    // ========================================================================
    // Coverage gap: handle_key Save keybinding (line 478)
    // ========================================================================

    #[test]
    fn test_save_keybinding_no_filename() {
        let mut e = ed("hello");
        e.handle_key(Key::Ctrl('s'));
        // No filename → opens save-as prompt
        assert!(e.command_buffer.active);
    }

    // ========================================================================
    // Coverage gap: handle_key Backspace (line 544)
    // ========================================================================

    #[test]
    fn test_backspace_key_dispatch() {
        let mut e = ed("ab");
        e.set_cursor(TextPosition::new(0, 2));
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
        assert!(!e.line_numbers_visible);
    }

    // ========================================================================
    // Coverage gap: execute_command None action (line 679)
    // ========================================================================

    #[test]
    fn test_execute_unknown_command() {
        let mut e = ed("hello");
        e.execute_command("nonexistent_command");
        // Should set status message about unknown command
        assert!(e.status_message.contains("Unknown"));
    }

    // ========================================================================
    // Coverage gap: kill_line on single-line document (line 731)
    // ========================================================================

    #[test]
    fn test_kill_line_on_last_line() {
        let mut e = ed("hello");
        e.kill_line();
        assert_eq!(e.test_text(), "");
    }

    // ========================================================================
    // Coverage gap: draw with status_message (line 280)
    // ========================================================================

    #[test]
    fn test_draw_with_status_msg() {
        let mut e = ed("hello\nworld");
        e.set_status("Test status".to_string());
        assert!(!e.status_message.is_empty());
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
        e.line_numbers_visible = false;
        e.center_view_on_line(20);
        // Cursor should be somewhere near line 20
        assert!(e.viewport.scroll_line > 0);
    }

    // ========================================================================
    // Coverage gap: find_matching_bracket for quotes (line 393)
    // ========================================================================

    #[test]
    fn test_find_matching_quote() {
        let mut e = ed("let s = \"hello\";\n");
        // Place cursor on the opening quote
        e.set_cursor(TextPosition::new(0, 8));
        let pair = e.find_matching_bracket();
        assert!(pair.is_some());
        let (_, match_pos) = pair.unwrap();
        assert_eq!(match_pos.column, 14); // closing quote
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
        e.line_numbers_visible = false;
        // Start a press first so dragging=true
        e.handle_mouse(
            MouseEvent::Press(MouseButton::Left, 1, 1),
            MouseMods::default(),
        );
        assert!(e.mouse.dragging);
        // Now drag
        e.handle_mouse(MouseEvent::Hold(6, 1), MouseMods::default());
        assert!(!selection(&e).is_empty());
    }

    // ========================================================================
    // Coverage gap: mouse release event
    // ========================================================================

    #[test]
    fn test_mouse_release_stops_drag() {
        let mut e = ed("hello");
        e.mouse.dragging = true;
        e.handle_mouse(MouseEvent::Release(1, 1), MouseMods::default());
        assert!(!e.mouse.dragging);
    }

    // ========================================================================
    // Coverage gap: screen_to_buffer_position ruler off (line 924)
    // ========================================================================

    #[test]
    fn test_screen_to_buffer_position_ruler_off() {
        let mut e = ed("hello\nworld");
        e.line_numbers_visible = false;
        let pos = e.screen_to_buffer_position(1, 1);
        assert_eq!(pos, TextPosition::new(0, 0));
        let pos2 = e.screen_to_buffer_position(1, 2);
        assert_eq!(pos2, TextPosition::new(1, 0));
    }

    // ========================================================================
    // Coverage gap: screen_to_buffer_position text_cols=0 (line 928)
    // ========================================================================

    #[test]
    fn test_screen_to_buffer_position_zero_cols() {
        let mut e = ed("hello");
        e.viewport = crate::viewport::Viewport::new(1, 3); // very narrow
        e.line_numbers_visible = true;
        // With gutter eating all columns, text_cols might be 0
        let pos = e.screen_to_buffer_position(1, 1);
        assert_eq!(pos, TextPosition::zero());
    }

    // ========================================================================
    // Coverage gap: multi-click double/triple (lines 977-994)
    // ========================================================================

    #[test]
    fn test_double_click_selects_word() {
        let mut e = ed("hello world");
        e.line_numbers_visible = false;
        // First click
        e.mouse_press(1, 1, MouseMods::default());
        // Simulate double click by setting last_click_time/pos and calling again
        e.mouse_press(1, 1, MouseMods::default());
        // Should select word "hello"
        assert!(!selection(&e).is_empty());
    }

    #[test]
    fn test_triple_click_selects_line() {
        let mut e = ed("hello world\nsecond");
        e.line_numbers_visible = false;
        // Three clicks at the same spot
        e.mouse_press(1, 1, MouseMods::default());
        e.mouse_press(1, 1, MouseMods::default());
        e.mouse_press(1, 1, MouseMods::default());
        // Should select entire first line
        assert!(!selection(&e).is_empty());
    }

    // ========================================================================
    // Coverage gap: mouse_drag when not dragging (line 1000)
    // ========================================================================

    #[test]
    fn test_mouse_drag_not_dragging_noop() {
        let mut e = ed("hello");
        e.mouse.dragging = false;
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
        e.line_numbers_visible = false;
        e.viewport.scroll_line = 20;
        e.set_cursor(TextPosition::new(20, 0));
        e.scroll_up();
        assert!(e.viewport.scroll_line < 20);
    }

    #[test]
    fn test_scroll_down_ruler_off() {
        let text = (0..50)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut e = ed(&text);
        e.line_numbers_visible = false;
        e.set_cursor(TextPosition::new(0, 0));
        e.scroll_down();
        assert!(e.viewport.scroll_line > 0);
    }

    // ========================================================================
    // Coverage gap: scroll_up with scroll_wrap > 0 (lines 1059-1061)
    // ========================================================================

    #[test]
    fn test_scroll_up_with_wrap() {
        let long_line = "a".repeat(200);
        let mut e = ed(&long_line);
        e.line_numbers_visible = false;
        // Set scroll_wrap to simulate being partway through a wrapped line
        e.viewport.scroll_wrap = 3;
        e.set_cursor(TextPosition::new(0, 0));
        e.scroll_up();
        assert!(e.viewport.scroll_wrap < 3);
    }

    // ========================================================================
    // Coverage gap: scroll_down wrapping (lines 1104-1105)
    // ========================================================================

    #[test]
    fn test_scroll_down_with_wrap() {
        let long_line = "a".repeat(200);
        let text = format!("{}\nshort", long_line);
        let mut e = ed(&text);
        e.line_numbers_visible = false;
        e.set_cursor(TextPosition::new(0, 0));
        // Scroll down — should advance through wraps of the long line
        e.scroll_down();
        assert!(e.viewport.scroll_wrap > 0 || e.viewport.scroll_line > 0);
    }

    // ========================================================================
    // Coverage gap: clamp_cursor_to_viewport zero rows/cols (line 1120)
    // ========================================================================

    #[test]
    fn test_clamp_cursor_zero_rows() {
        let mut e = ed("hello");
        e.viewport = crate::viewport::Viewport::new(80, 2); // only 2 rows = 0 text rows
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
        e.line_numbers_visible = false;
        // Put cursor far below viewport
        e.carets.primary_mut().selection.cursor = TextPosition::new(45, 0);
        e.carets.primary_mut().selection.anchor = TextPosition::new(45, 0);
        e.viewport.scroll_line = 0;
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
        e.set_cursor(TextPosition::new(1, 0));
        e.move_left_extend();
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 5));
    }

    // ========================================================================
    // Coverage gap: move_right_extend wrapping to next line (line 1374)
    // ========================================================================

    #[test]
    fn test_move_right_extend_wraps_to_next_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(0, 5));
        e.move_right_extend();
        assert_eq!(selection(&e).cursor, TextPosition::new(1, 0));
    }

    // ========================================================================
    // Coverage gap: indent_selection end line adjustment (line 1410)
    // ========================================================================

    #[test]
    fn test_indent_selection_skips_trailing_empty_line() {
        let mut e = ed_named("aaa\nbbb\nccc\n", "test.rs");
        // Select lines 0-2 with cursor at column 0 of line 3 (empty trailing)
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(3, 0),
            },
        );
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
        // Select with cursor at column 0 of a later line
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(2, 0),
            },
        );
        e.toggle_comment();
        // Lines 0-1 should be commented (not line 2 since cursor column=0)
        let text = e.test_text();
        assert!(text.starts_with("// aaa\n// bbb\n"));
    }

    // ========================================================================
    // Coverage gap: toggle_comment with empty/whitespace lines (line 1590, 1630)
    // ========================================================================

    #[test]
    fn test_toggle_comment_with_blank_lines() {
        let mut e = ed_named("aaa\n\nbbb\n", "test.rs");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(2, 3),
            },
        );
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
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(2, 3),
            },
        );
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
        // Select with cursor at column 0 of line 2
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(2, 0),
            },
        );
        e.dedent();
        // Lines 0-1 should be dedented
        let text = e.test_text();
        assert!(text.starts_with("aaa\nbbb\n"));
    }

    #[test]
    fn test_dedent_selection_preserves_selection() {
        // Selection should remain after Shift+Tab so the user can dedent multiple times.
        let mut e = ed_named("  aaa\n  bbb\n  ccc", "test.rs");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 2),
                cursor: TextPosition::new(2, 5),
            },
        );
        e.dedent();
        assert_eq!(e.test_text(), "aaa\nbbb\nccc");
        assert!(!selection(&e).is_empty());
        assert_eq!(selection(&e).anchor, TextPosition::new(0, 0));
        assert_eq!(selection(&e).cursor, TextPosition::new(2, 3));
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
        assert_eq!(
            e.document.file_path.as_deref(),
            Some(path.to_str().unwrap())
        );
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
        assert!(e.command_buffer.active);
        assert_eq!(
            e.command_buffer.mode,
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
        e.document.file_path = Some(path.to_str().unwrap().to_string());
        e.document.is_dirty = true;
        e.save_file();
        assert!(!e.document.is_dirty);
        assert!(e.status_message.contains("Saved"));
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
        e.document.file_path = Some(path.to_str().unwrap().to_string());
        e.save_undo_if_named(); // should not panic
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ========================================================================
    // Coverage gap: Find with selection prefill (lines 498-506)
    // ========================================================================

    #[test]
    fn test_find_prefills_from_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.handle_key(Key::Ctrl('f'));
        assert!(e.command_buffer.active);
        assert_eq!(e.command_buffer.input, "hello");
    }

    // ========================================================================
    // Coverage gap: multiple completions with common prefix (lines 665-667)
    // ========================================================================

    #[test]
    fn test_command_completion_common_prefix() {
        let mut e = ed("hello");
        e.command_buffer
            .open(crate::command_buffer::CommandBufferMode::Command, "> ", "");
        e.command_buffer.input = "go".to_string();
        e.command_buffer.cursor = 2;
        // Request tab completion — should find "goto" and complete the common prefix
        let result = e.command_buffer.handle_key(Key::Char('\t'));
        let mode = e.command_buffer.mode;
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
        e.file_modification_time = crate::file_io::file_modification_time(&path);
        assert!(!e.reload_pending);

        // Modify file externally
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"changed").unwrap();

        e.check_external_modification();
        assert!(e.reload_pending);
        assert!(e.status_message.contains("changed on disk"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_reload_file_replaces_buffer() {
        let dir = std::env::temp_dir().join("e_test_reload_buf");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"original\n").unwrap();

        let mut e = ed_named("original\n", path.to_str().unwrap());
        e.file_modification_time = crate::file_io::file_modification_time(&path);

        // Modify file externally
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"new content\n").unwrap();

        e.reload_pending = true;
        e.reload_file();
        assert!(!e.reload_pending);
        assert!(e.test_text().contains("new content"));
        assert!(e.status_message.contains("Reloaded"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dismiss_reload_updates_mtime() {
        let dir = std::env::temp_dir().join("e_test_dismiss");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"original").unwrap();
        let mut e = ed_named("original", path.to_str().unwrap());

        e.file_modification_time = crate::file_io::file_modification_time(&path);

        // Modify file externally
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"changed").unwrap();
        let new_mtime = crate::file_io::file_modification_time(&path);

        e.reload_pending = true;
        e.status_message = "test.txt changed on disk. Reload? (y/n)".to_string();
        e.dismiss_reload();
        assert!(!e.reload_pending);
        assert!(e.status_message.is_empty());
        assert_eq!(e.file_modification_time, new_mtime);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_reload_clamps_cursor() {
        let dir = std::env::temp_dir().join("e_test_reload_clamp");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"line1\nline2\nline3\n").unwrap();

        let mut e = ed_named("line1\nline2\nline3\n", path.to_str().unwrap());
        e.file_modification_time = crate::file_io::file_modification_time(&path);
        // Put cursor on line 2
        set_sel(&mut e, Selection::caret(TextPosition::new(2, 3)));

        // Replace with shorter file
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"short\n").unwrap();

        e.reload_pending = true;
        e.reload_file();
        // Cursor should be clamped to last line
        assert!(selection(&e).cursor.line < e.document.buffer.line_count());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_no_reload_for_unnamed() {
        let mut e = ed("hello");
        e.check_external_modification();
        assert!(!e.reload_pending);
        assert!(e.status_message.is_empty());
    }

    #[test]
    fn test_focus_in_event() {
        let dir = std::env::temp_dir().join("e_test_focus_in");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        std::fs::write(&path, b"original").unwrap();

        let mut e = ed_named("original", path.to_str().unwrap());
        e.file_modification_time = crate::file_io::file_modification_time(&path);

        // Modify file externally
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&path, b"changed").unwrap();

        // Send focus-in event
        e.dispatch_event(EditorEvent::FocusIn);
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
        assert!(e.file_modification_time.is_none()); // ed_named doesn't set mtime

        e.save_file();
        assert!(e.file_modification_time.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ========================================================================
    // Word navigation (Ctrl+Left / Ctrl+Right)
    // ========================================================================

    #[test]
    fn test_word_left_middle_of_line() {
        let mut e = ed("hello world foo");
        e.set_cursor(TextPosition::new(0, 15));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 12));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 6));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    #[test]
    fn test_word_right_middle_of_line() {
        let mut e = ed("hello world foo");
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 6));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 12));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 15));
    }

    #[test]
    fn test_word_left_wraps_to_prev_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(1, 0));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
    }

    #[test]
    fn test_word_right_wraps_to_next_line() {
        let mut e = ed("hello\nworld");
        e.set_cursor(TextPosition::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
    }

    #[test]
    fn test_word_left_collapses_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 2),
                cursor: TextPosition::new(0, 8),
            },
        );
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
        assert!(selection(&e).is_empty());
    }

    #[test]
    fn test_word_right_collapses_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 2),
                cursor: TextPosition::new(0, 8),
            },
        );
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 8));
        assert!(selection(&e).is_empty());
    }

    #[test]
    fn test_word_left_at_origin() {
        let mut e = ed("hello");
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    #[test]
    fn test_word_right_at_end() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
    }

    #[test]
    fn test_word_left_skips_punctuation() {
        // "foo.bar" at end: skip "bar", skip ".", skip "foo" -> 0
        let mut e = ed("foo.bar");
        e.set_cursor(TextPosition::new(0, 7));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 4));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    #[test]
    fn test_word_right_skips_punctuation() {
        // "foo.bar" from 0: skip "foo" to 3, skip "." to 4
        let mut e = ed("foo.bar");
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 4));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 7));
    }

    #[test]
    fn test_word_left_multiple_spaces() {
        let mut e = ed("foo   bar");
        e.set_cursor(TextPosition::new(0, 9));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 6));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    #[test]
    fn test_word_right_multiple_spaces() {
        let mut e = ed("foo   bar");
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 6));
    }

    #[test]
    fn test_word_left_empty_line() {
        let mut e = ed("hello\n\nworld");
        e.set_cursor(TextPosition::new(2, 0));
        e.word_left();
        // wraps to end of empty line 1
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
        e.word_left();
        // wraps to end of line 0
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
    }

    #[test]
    fn test_word_right_empty_line() {
        let mut e = ed("hello\n\nworld");
        e.set_cursor(TextPosition::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(2, 0));
    }

    #[test]
    fn test_word_left_from_middle_of_word() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 3));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    #[test]
    fn test_word_right_from_middle_of_word() {
        // "hello world" from column 3: skip "lo" to 5, skip " " to 6
        let mut e = ed("hello world");
        e.set_cursor(TextPosition::new(0, 3));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 6));
    }

    #[test]
    fn test_word_left_underscores() {
        // underscores are word chars
        let mut e = ed("foo_bar baz");
        e.set_cursor(TextPosition::new(0, 11));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 8));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0)); // foo_bar is one word
    }

    #[test]
    fn test_word_right_underscores() {
        let mut e = ed("foo_bar baz");
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 8)); // skips whole foo_bar + space
    }

    #[test]
    fn test_word_left_at_last_line_end() {
        let mut e = ed("one\ntwo");
        e.set_cursor(TextPosition::new(1, 3));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(1, 0));
    }

    #[test]
    fn test_word_right_at_last_line_end() {
        // at end of last line, no next line, stays put
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
    }

    #[test]
    fn test_word_nav_roundtrip() {
        // word_right then word_left should return near starting region
        let mut e = ed("  fn main() {");
        e.word_right();
        // from column 0: skip word chars (none), skip non-word ("  ") -> lands at 2
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
        e.word_right();
        // from column 2: skip word chars ("fn") to 4, skip non-word (" ") to 5
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
        e.word_left();
        // from column 5: skip non-word (" ") to 4, skip word ("fn") to 2
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0));
    }

    // ========================================================================
    // Bracket-jump on word movement
    // ========================================================================

    #[test]
    fn test_word_right_jumps_to_matching_bracket() {
        let mut e = ed("fn foo() { bar }");
        // Cursor on '(' at column 6 → should jump to ')' at column 7
        e.set_cursor(TextPosition::new(0, 6));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 7));
    }

    #[test]
    fn test_word_left_jumps_to_matching_bracket() {
        let mut e = ed("fn foo() { bar }");
        // Cursor on ')' at column 7 → should jump to '(' at column 6
        e.set_cursor(TextPosition::new(0, 7));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 6));
    }

    #[test]
    fn test_bracket_jump_multiline() {
        let mut e = ed("if x {\n  y\n}");
        // Cursor on '{' at line 0 column 5 → should jump to '}' at line 2 column 0
        e.set_cursor(TextPosition::new(0, 5));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(2, 0));
        // And back
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
    }

    #[test]
    fn test_bracket_jump_nested() {
        let mut e = ed("((inner))");
        // Cursor on outer '(' at column 0 → should jump to outer ')' at column 8
        e.set_cursor(TextPosition::new(0, 0));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 8));
        // Cursor on inner '(' at column 1 → should jump to inner ')' at column 7
        e.set_cursor(TextPosition::new(0, 1));
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 7));
    }

    #[test]
    fn test_bracket_jump_square() {
        let mut e = ed("a[b[c]]");
        e.set_cursor(TextPosition::new(0, 1));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 6)); // outer ]
    }

    #[test]
    fn test_bracket_jump_extend_selection() {
        let mut e = ed("fn foo() {}");
        e.set_cursor(TextPosition::new(0, 6)); // on '('
        e.word_right_extend();
        assert_eq!(selection(&e).cursor, TextPosition::new(0, 7)); // extends to ')'
        assert_eq!(selection(&e).anchor, TextPosition::new(0, 6)); // anchor stays
    }

    #[test]
    fn test_non_bracket_does_normal_word_jump() {
        // When not on a bracket, normal word movement should still work
        let mut e = ed("hello world");
        e.set_cursor(TextPosition::new(0, 0));
        e.word_right();
        assert_eq!(e.cursor(), TextPosition::new(0, 6)); // normal word jump
    }

    #[test]
    fn test_word_left_selection_collapses_before_bracket_check() {
        // With a selection, word_left should collapse the selection, not bracket-jump
        let mut e = ed("(hello)");
        e.set_cursor(TextPosition::new(0, 0)); // on '('
        e.carets.primary_mut().selection.cursor = TextPosition::new(0, 5); // select "hello"
        e.word_left();
        assert_eq!(e.cursor(), TextPosition::new(0, 0)); // collapsed to start
    }

    // ========================================================================
    // Auto-close pairs
    // ========================================================================

    #[test]
    fn test_autoclose_paren() {
        let mut e = ed("");
        e.insert_char('(');
        assert_eq!(e.test_text(), "()");
        assert_eq!(e.cursor(), TextPosition::new(0, 1)); // between parens
    }

    #[test]
    fn test_autoclose_bracket() {
        let mut e = ed("");
        e.insert_char('[');
        assert_eq!(e.test_text(), "[]");
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
    }

    #[test]
    fn test_autoclose_brace() {
        let mut e = ed("");
        e.insert_char('{');
        assert_eq!(e.test_text(), "{}");
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
    }

    #[test]
    fn test_autoclose_double_quote() {
        let mut e = ed("");
        e.insert_char('"');
        assert_eq!(e.test_text(), "\"\"");
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
    }

    #[test]
    fn test_autoclose_single_quote() {
        // Plain Text (no filename): single quote must NOT be auto-closed.
        let mut e = ed("");
        e.insert_char('\'');
        assert_eq!(e.test_text(), "'");
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
    }

    #[test]
    fn test_autoclose_single_quote_in_rust() {
        // Named .rs file: single quote SHOULD be auto-closed.
        let mut e = ed_named("", "/tmp/test.rs");
        e.insert_char('\'');
        assert_eq!(e.test_text(), "''");
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
    }

    #[test]
    fn test_autoclose_skip_closing_paren() {
        let mut e = ed("");
        e.insert_char('(');
        assert_eq!(e.test_text(), "()");
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
        e.insert_char(')'); // should skip over the closing paren
        assert_eq!(e.test_text(), "()");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
    }

    #[test]
    fn test_autoclose_skip_closing_bracket() {
        let mut e = ed("");
        e.insert_char('[');
        e.insert_char(']');
        assert_eq!(e.test_text(), "[]");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
    }

    #[test]
    fn test_autoclose_skip_closing_brace() {
        let mut e = ed("");
        e.insert_char('{');
        e.insert_char('}');
        assert_eq!(e.test_text(), "{}");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
    }

    #[test]
    fn test_autoclose_skip_closing_double_quote() {
        let mut e = ed("");
        e.insert_char('"');
        e.insert_char('"');
        assert_eq!(e.test_text(), "\"\"");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
    }

    #[test]
    fn test_autoclose_skip_closing_single_quote() {
        let mut e = ed("");
        e.insert_char('\'');
        e.insert_char('\'');
        assert_eq!(e.test_text(), "''");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
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
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
    }

    #[test]
    fn test_autoclose_pair_when_next_is_close_char() {
        let mut e = ed(")");
        e.insert_char('(');
        assert_eq!(e.test_text(), "())");
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
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
    fn test_autoclose_backtick() {
        let mut e = ed("");
        e.insert_char('`');
        assert_eq!(e.test_text(), "``");
        assert_eq!(e.cursor(), TextPosition::new(0, 1));
    }

    #[test]
    fn test_autoclose_skip_closing_backtick() {
        let mut e = ed("");
        e.insert_char('`');
        e.insert_char('`');
        assert_eq!(e.test_text(), "``");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
    }

    #[test]
    fn test_autoclose_backspace_deletes_backtick_pair() {
        let mut e = ed("");
        e.insert_char('`');
        e.backspace();
        assert_eq!(e.test_text(), "");
    }

    #[test]
    fn test_autoclose_backspace_only_deletes_pair_when_matched() {
        // "(x" with cursor at 1 — next char is 'x' not ')', so only delete '('
        let mut e = ed("(x");
        e.set_cursor(TextPosition::new(0, 1));
        e.backspace();
        assert_eq!(e.test_text(), "x");
    }

    #[test]
    fn test_autoclose_wraps_selection_paren() {
        let mut e = ed("hello");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.insert_char('(');
        assert_eq!(e.test_text(), "(hello)");
        // Inner text should be selected
        let (s, end) = selection(&e).ordered();
        assert_eq!(s, TextPosition::new(0, 1));
        assert_eq!(end, TextPosition::new(0, 6));
    }

    #[test]
    fn test_autoclose_wraps_selection_bracket() {
        let mut e = ed("world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.insert_char('[');
        assert_eq!(e.test_text(), "[world]");
    }

    #[test]
    fn test_autoclose_wraps_selection_brace() {
        let mut e = ed("abc");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 3),
            },
        );
        e.insert_char('{');
        assert_eq!(e.test_text(), "{abc}");
    }

    #[test]
    fn test_autoclose_wraps_selection_double_quote() {
        let mut e = ed("text");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 4),
            },
        );
        e.insert_char('"');
        assert_eq!(e.test_text(), "\"text\"");
    }

    #[test]
    fn test_autoclose_wraps_selection_single_quote() {
        // Plain Text: single quote replaces selection rather than wrapping it.
        let mut e = ed("text");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 4),
            },
        );
        e.insert_char('\'');
        assert_eq!(e.test_text(), "'");
    }

    #[test]
    fn test_autoclose_wraps_selection_single_quote_in_rust() {
        // Named .rs file: single quote SHOULD wrap the selection.
        let mut e = ed_named("text", "/tmp/test.rs");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 4),
            },
        );
        e.insert_char('\'');
        assert_eq!(e.test_text(), "'text'");
    }

    #[test]
    fn test_autoclose_wraps_partial_selection() {
        let mut e = ed("hello world");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 6),
                cursor: TextPosition::new(0, 11),
            },
        );
        e.insert_char('(');
        assert_eq!(e.test_text(), "hello (world)");
    }

    #[test]
    fn test_autoclose_wraps_multiline_selection() {
        let mut e = ed("foo\nbar");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(1, 3),
            },
        );
        e.insert_char('{');
        assert_eq!(e.test_text(), "{foo\nbar}");
    }

    #[test]
    fn test_autoclose_non_pair_char_replaces_selection() {
        // Typing a regular char with selection should replace it (not wrap)
        let mut e = ed("hello");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 0),
                cursor: TextPosition::new(0, 5),
            },
        );
        e.insert_char('x');
        assert_eq!(e.test_text(), "x");
    }

    #[test]
    fn test_autoclose_type_inside_pair() {
        let mut e = ed("");
        e.insert_char('(');
        e.insert_char('x');
        assert_eq!(e.test_text(), "(x)");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
    }

    #[test]
    fn test_autoclose_nested_pairs() {
        let mut e = ed("");
        e.insert_char('(');
        e.insert_char('[');
        assert_eq!(e.test_text(), "([])");
        assert_eq!(e.cursor(), TextPosition::new(0, 2));
        e.insert_char(']');
        assert_eq!(e.test_text(), "([])");
        assert_eq!(e.cursor(), TextPosition::new(0, 3));
        e.insert_char(')');
        assert_eq!(e.test_text(), "([])");
        assert_eq!(e.cursor(), TextPosition::new(0, 4));
    }

    #[test]
    fn test_autoclose_at_end_of_line() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 5));
        e.insert_char('(');
        assert_eq!(e.test_text(), "hello()");
        assert_eq!(e.cursor(), TextPosition::new(0, 6));
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
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 5),
                cursor: TextPosition::new(0, 0),
            },
        );
        e.insert_char('(');
        assert_eq!(e.test_text(), "(hello)");
    }

    // ========================================================================
    // Smart paste (auto-indent)
    // ========================================================================

    #[test]
    fn test_smart_paste_on_blank_line_clears_autoindent() {
        // Blank line with auto-indent is cleared before paste; pasted text's own
        // indentation is used as-is (indentation is a copy-time property).
        let mut e = ed("    fn main() {\n        ");
        e.set_cursor(TextPosition::new(1, 8));
        e.paste_text("if true {\n    println!(\"hi\");\n}");
        assert_eq!(
            e.test_text(),
            "    fn main() {\nif true {\n    println!(\"hi\");\n}"
        );
    }

    #[test]
    fn test_smart_paste_single_line_no_change() {
        let mut e = ed("    ");
        e.set_cursor(TextPosition::new(0, 4));
        e.paste_text("hello");
        assert_eq!(e.test_text(), "    hello");
    }

    #[test]
    fn test_smart_paste_blank_line_cleared() {
        // Blank line with 2-space auto-indent is cleared; paste goes at column 0.
        let mut e = ed("  ");
        e.set_cursor(TextPosition::new(0, 2));
        e.paste_text("a\n  b\n  c");
        assert_eq!(e.test_text(), "a\n  b\n  c");
    }

    #[test]
    fn test_smart_paste_preserves_relative_indent() {
        // Blank line cleared; pasted text's internal structure is preserved.
        let mut e = ed("    ");
        e.set_cursor(TextPosition::new(0, 4));
        e.paste_text("if true {\n  nested\n}");
        assert_eq!(e.test_text(), "if true {\n  nested\n}");
    }

    #[test]
    fn test_smart_paste_empty_lines_preserved() {
        let mut e = ed("    ");
        e.set_cursor(TextPosition::new(0, 4));
        e.paste_text("a\n\nb");
        // Blank line cleared; empty lines in paste preserved as-is.
        assert_eq!(e.test_text(), "a\n\nb");
    }

    #[test]
    fn test_smart_paste_zero_indent_no_change() {
        // Pasting at column 0 with text that has 0 indent — no change
        let mut e = ed("");
        e.paste_text("a\nb\nc");
        assert_eq!(e.test_text(), "a\nb\nc");
    }

    #[test]
    fn test_smart_paste_with_selection_replaces() {
        let mut e = ed("    old stuff\n    more old");
        set_sel(
            &mut e,
            Selection {
                anchor: TextPosition::new(0, 4),
                cursor: TextPosition::new(0, 13),
            },
        );
        e.paste_text("new\n    thing");
        // After deleting "old stuff" the line becomes blank ("    "), which is
        // cleared; paste goes at column 0 with its own indentation.
        assert_eq!(e.test_text(), "new\n    thing\n    more old");
    }

    #[test]
    fn test_smart_paste_cursor_position() {
        let mut e = ed("");
        e.paste_text("hello\nworld");
        // Cursor should be at end of pasted text
        assert_eq!(e.cursor(), TextPosition::new(1, 5));
    }

    #[test]
    fn test_smart_paste_empty_string() {
        let mut e = ed("hello");
        e.set_cursor(TextPosition::new(0, 5));
        e.paste_text("");
        assert_eq!(e.test_text(), "hello");
        assert_eq!(e.cursor(), TextPosition::new(0, 5));
    }

    #[test]
    fn test_smart_paste_all_empty_continuation_lines() {
        // Blank line cleared; trailing empty lines in paste preserved as-is.
        let mut e = ed("    ");
        e.set_cursor(TextPosition::new(0, 4));
        e.paste_text("a\n\n\n");
        assert_eq!(e.test_text(), "a\n\n\n");
    }

    #[test]
    fn test_smart_paste_tabs_in_pasted_text() {
        // Blank line cleared; paste with tab indentation used as-is.
        let mut e = ed("  ");
        e.set_cursor(TextPosition::new(0, 2));
        e.paste_text("a\n\tb");
        assert_eq!(e.test_text(), "a\n\tb");
    }

    #[test]
    fn test_smart_paste_yaml_structure_preserved() {
        // YAML block pasted on a blank line: auto-indent cleared, structure intact.
        let mut e = ed("  ");
        e.set_cursor(TextPosition::new(0, 2));
        e.paste_text("root:\n  child:\n    grandchild: value");
        assert_eq!(e.test_text(), "root:\n  child:\n    grandchild: value");
    }

    #[test]
    fn test_smart_paste_yaml_at_col_zero_unchanged() {
        // Pasting on an empty line: structure preserved as-is.
        let mut e = ed("");
        e.paste_text("root:\n  child:\n    grandchild: value");
        assert_eq!(e.test_text(), "root:\n  child:\n    grandchild: value");
    }

    #[test]
    fn test_smart_paste_non_blank_line_uses_cursor_col() {
        // Pasting on a non-blank line (mid-content): continuation lines are
        // shifted to cursor.column + their original indent.
        let mut e = ed("    prefix");
        e.set_cursor(TextPosition::new(0, 10));
        e.paste_text("key:\n  val");
        // cursor column = 10, "  val" has ik=2 → new_indent = 10+2 = 12
        assert_eq!(e.test_text(), "    prefixkey:\n            val");
    }
}
