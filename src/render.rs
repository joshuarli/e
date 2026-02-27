use std::io::{self, Write};

use crate::buffer::{self, GapBuffer};
use crate::highlight::{self, HlState, HlType, SyntaxRules};
use crate::selection::{Pos, Selection};
use crate::view::View;

/// Compute gutter width: width of the largest line number + 1 (trailing space).
pub fn gutter_width(line_count: usize) -> usize {
    let n = if line_count == 0 { 1 } else { line_count };
    let mut digits = 1usize;
    let mut v = n;
    while v >= 10 {
        v /= 10;
        digits += 1;
    }
    digits + 1
}

/// Expand tabs in `text`, writing expanded bytes into `out` and per-column
/// pipe-markers into `tab_pipes`. Clears both buffers first.
/// Returns `true` when the line contains tabs (caller can skip tab-pipe checks otherwise).
/// When no tabs: `out` receives a verbatim copy of `text`; `tab_pipes` is left empty.
fn expand_tabs_into(text: &[u8], out: &mut Vec<u8>, tab_pipes: &mut Vec<bool>) -> bool {
    out.clear();
    if !text.contains(&b'\t') {
        out.extend_from_slice(text);
        tab_pipes.clear();
        return false;
    }
    tab_pipes.clear();
    for &b in text {
        if b == b'\t' {
            out.push(b'|');
            tab_pipes.push(true);
            out.push(b' ');
            tab_pipes.push(false);
        } else {
            out.push(b);
            tab_pipes.push(false);
        }
    }
    true
}

pub struct Renderer {
    pub needs_full_redraw: bool,
    syntax: Option<&'static SyntaxRules>,
    /// Cached highlight state at the start of each line.
    hl_cache: Vec<HlState>,
    /// Buffer version the cache was computed for.
    hl_cache_version: u64,
    /// First line whose cached state may be stale (set by caller via dirty tracking).
    hl_dirty_from: usize,
    /// Scratch buffer reused each line for line_text_into.
    line_buf: Vec<u8>,
    /// Scratch buffer reused each line for tab-expanded output.
    expanded_scratch: Vec<u8>,
    /// Scratch buffer reused each line for tab-pipe column markers.
    tab_pipes_scratch: Vec<bool>,
    /// Scratch buffer reused each line for byte-indexed highlight output.
    hl_scratch: Vec<HlType>,
    /// Scratch buffer reused each line for char-indexed highlight output.
    char_hl_scratch: Vec<HlType>,
    /// Scratch buffer reused each line for find-range display columns.
    find_scratch: Vec<(usize, usize, bool)>,
    /// Frame buffer reused each draw to avoid per-frame allocation.
    frame_buf: Vec<u8>,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            needs_full_redraw: true,
            syntax: None,
            hl_cache: Vec::new(),
            hl_cache_version: u64::MAX,
            hl_dirty_from: 0,
            line_buf: Vec::new(),
            expanded_scratch: Vec::new(),
            tab_pipes_scratch: Vec::new(),
            hl_scratch: Vec::new(),
            char_hl_scratch: Vec::new(),
            find_scratch: Vec::new(),
            frame_buf: Vec::new(),
        }
    }

    pub fn set_syntax(&mut self, rules: Option<&'static SyntaxRules>) {
        let same = match (self.syntax, rules) {
            (Some(a), Some(b)) => std::ptr::eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.syntax = rules;
            self.hl_cache.clear();
            self.hl_cache_version = u64::MAX;
            self.hl_dirty_from = 0;
        }
    }

    pub fn force_full_redraw(&mut self) {
        self.needs_full_redraw = true;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        out: &mut impl Write,
        buf: &mut GapBuffer,
        view: &View,
        cursor_line: usize,
        cursor_col: usize,
        ruler_on: bool,
        status_left: &str,
        status_right: &str,
        command_line: Option<&str>,
        selection: Option<Selection>,
        find_matches: Option<&[(Pos, Pos)]>,
        find_current: Option<(Pos, Pos)>,
        completions: &[String],
        cmd_cursor: Option<usize>,
        find_active: bool,
        bracket_pair: Option<(Pos, Pos)>,
    ) -> io::Result<()> {
        let line_count = buf.line_count();
        let gw = if ruler_on {
            gutter_width(line_count)
        } else {
            0
        };
        let completion_rows = completions.len();
        let text_rows = view.text_rows().saturating_sub(completion_rows);
        let text_cols = view.text_cols(gw);

        let (sel_start, sel_end) = selection
            .map(|s| s.ordered())
            .unwrap_or((Pos::zero(), Pos::zero()));
        let has_sel = selection.is_some_and(|s| !s.is_empty());

        // Buffer all output, then write to terminal in one shot to avoid flicker.
        // Take the frame buffer out of self so we can still call self.X methods below.
        let mut frame = std::mem::take(&mut self.frame_buf);
        frame.clear();
        let w = &mut frame;

        // Synchronized output: tell terminal to hold rendering until frame is complete
        write!(w, "\x1b[?2026h")?;
        write!(w, "\x1b[?25l")?;

        // Compute per-line highlight states (lazy: only as far as the viewport needs)
        let buf_version = buf.version();
        // One line past the last visible line — that's all we need to cover.
        let visible_end = (view.scroll_line + text_rows + 1).min(line_count);
        if let Some(rules) = self.syntax {
            if buf_version != self.hl_cache_version {
                let dirty = buf.take_dirty_line();
                self.hl_dirty_from = self.hl_dirty_from.min(dirty);
                if self.hl_cache.is_empty() {
                    self.hl_dirty_from = 0;
                }
                self.hl_cache_version = buf_version;
            }
            self.refresh_hl_cache(visible_end, view.scroll_line, line_count, buf, rules);
            self.hl_dirty_from = usize::MAX;
        } else {
            self.hl_cache.clear();
        }
        let hl_states = &self.hl_cache;

        // Wrap-aware render loop
        let mut screen_row: usize = 0;
        let mut line_idx = view.scroll_line;
        let first_wrap = view.scroll_wrap;

        while screen_row < text_rows && line_idx < line_count {
            buf.line_text_into(line_idx, &mut self.line_buf);
            let raw_text: &[u8] = &self.line_buf;
            let has_tabs = expand_tabs_into(
                raw_text,
                &mut self.expanded_scratch,
                &mut self.tab_pipes_scratch,
            );
            let line_str = String::from_utf8_lossy(&self.expanded_scratch);

            // One-pass scan: count chars, find trailing-whitespace boundary.
            // Replaces the former `chars: Vec<char>` allocation.
            let mut char_count = 0usize;
            let mut trailing_ws_start = 0usize;
            let mut any_nonws = false;
            for (i, c) in line_str.chars().enumerate() {
                char_count += 1;
                if !c.is_ascii_whitespace() {
                    trailing_ws_start = i + 1;
                    any_nonws = true;
                }
            }
            let has_trailing = any_nonws && trailing_ws_start < char_count;
            let total_wraps = crate::view::wrapped_rows(char_count, text_cols);
            let start_wrap = if line_idx == view.scroll_line {
                first_wrap
            } else {
                0
            };

            // Compute per-char syntax highlights for this line (once per logical line).
            // Results go into self.char_hl_scratch; has_char_hl tracks whether it's valid.
            let has_char_hl =
                if let (Some(rules), Some(&state)) = (self.syntax, hl_states.get(line_idx)) {
                    highlight::highlight_line_into(raw_text, state, rules, &mut self.hl_scratch);
                    highlight::byte_hl_to_char_hl_into(
                        raw_text,
                        &self.hl_scratch,
                        &mut self.char_hl_scratch,
                    );
                    true
                } else {
                    false
                };

            // Bracket match: at most 2 display columns on this line (open + close).
            // Using two Option<usize> locals avoids a Vec allocation entirely.
            let mut bracket_col_0 = None::<usize>;
            let mut bracket_col_1 = None::<usize>;
            if let Some((open, close)) = bracket_pair {
                if open.line == line_idx {
                    bracket_col_0 = Some(display_col_for_char_col(raw_text, open.col));
                }
                if close.line == line_idx {
                    bracket_col_1 = Some(display_col_for_char_col(raw_text, close.col));
                }
            }
            let has_bracket = bracket_col_0.is_some() || bracket_col_1.is_some();

            // Per-character highlight info
            let need_per_char = has_sel && line_idx >= sel_start.line && line_idx <= sel_end.line;
            let has_find = find_matches.is_some_and(|m| {
                m.iter()
                    .any(|(s, e)| line_idx >= s.line && line_idx <= e.line)
            });

            // Pre-compute selection and find ranges once per logical line
            let (line_sel_start, line_sel_end) = if need_per_char {
                let s = if line_idx == sel_start.line {
                    display_col_for_char_col(raw_text, sel_start.col)
                } else {
                    0
                };
                let e = if line_idx == sel_end.line {
                    display_col_for_char_col(raw_text, sel_end.col)
                } else {
                    char_count
                };
                (s, e)
            } else {
                (0, 0)
            };

            // Build find ranges into the reusable scratch buffer (no alloc after warm-up).
            self.find_scratch.clear();
            if let Some(matches) = find_matches {
                for (s, e) in matches
                    .iter()
                    .filter(|(s, e)| line_idx >= s.line && line_idx <= e.line)
                {
                    let fs = if line_idx == s.line {
                        display_col_for_char_col(raw_text, s.col)
                    } else {
                        0
                    };
                    let fe = if line_idx == e.line {
                        display_col_for_char_col(raw_text, e.col)
                    } else {
                        char_count
                    };
                    let is_current = find_current.is_some_and(|(cs, ce)| cs == *s && ce == *e);
                    self.find_scratch.push((fs, fe, is_current));
                }
            }

            for wrap in start_wrap..total_wraps {
                if screen_row >= text_rows {
                    break;
                }

                write!(w, "\x1b[{};1H", screen_row + 1)?;

                // Gutter: line number on first wrap row, blank on continuations
                if ruler_on {
                    let is_cursor_line = line_idx == cursor_line;
                    if wrap == 0 {
                        let num_str = format!("{}", line_idx + 1);
                        let pad = gw - 1;
                        if is_cursor_line {
                            write!(w, "\x1b[0;47;30m{:>width$}\x1b[0m ", num_str, width = pad)?;
                        } else {
                            write!(w, "\x1b[0;2m{:>width$} \x1b[0m", num_str, width = pad)?;
                        }
                    } else if is_cursor_line {
                        write!(w, "\x1b[0;47;30m{:>width$}\x1b[0m ", "", width = gw - 1)?;
                    } else {
                        write!(w, "\x1b[0;2m{:>width$} \x1b[0m", "", width = gw - 1)?;
                    }
                }

                let chunk_start = wrap * text_cols;
                let chunk_end = ((wrap + 1) * text_cols).min(char_count);

                if need_per_char || has_find || has_bracket {
                    for (i, ch) in line_str
                        .chars()
                        .enumerate()
                        .take(chunk_end)
                        .skip(chunk_start)
                    {
                        let in_sel = need_per_char && i >= line_sel_start && i < line_sel_end;
                        let find_hit = self
                            .find_scratch
                            .iter()
                            .find(|(fs, fe, _)| i >= *fs && i < *fe);
                        let is_tab_pipe = has_tabs
                            && i < self.tab_pipes_scratch.len()
                            && self.tab_pipes_scratch[i];
                        let is_bracket_match = bracket_col_0 == Some(i) || bracket_col_1 == Some(i);
                        let is_trailing_ws = has_trailing && i >= trailing_ws_start;

                        if in_sel {
                            if is_tab_pipe {
                                write!(w, "\x1b[7;90m{}\x1b[0m", ch)?;
                            } else {
                                write!(w, "\x1b[7m{}\x1b[0m", ch)?;
                            }
                        } else if let Some((_, _, is_current)) = find_hit {
                            if *is_current {
                                write!(w, "\x1b[42;30m{}\x1b[0m", ch)?;
                            } else {
                                write!(w, "\x1b[43;30m{}\x1b[0m", ch)?;
                            }
                        } else if is_bracket_match {
                            write!(w, "\x1b[45;30m{}\x1b[0m", ch)?;
                        } else if is_trailing_ws {
                            write!(w, "\x1b[41m{}\x1b[0m", ch)?;
                        } else if is_tab_pipe {
                            write!(w, "\x1b[90m{}\x1b[0m", ch)?;
                        } else {
                            let ht = if has_char_hl {
                                self.char_hl_scratch
                                    .get(i)
                                    .copied()
                                    .unwrap_or(HlType::Normal)
                            } else {
                                HlType::Normal
                            };
                            let code = ht.ansi_code();
                            if code.is_empty() {
                                write!(w, "{}", ch)?;
                            } else {
                                write!(w, "{}{}\x1b[0m", code, ch)?;
                            }
                        }
                    }
                } else {
                    // Fast path: syntax highlighting only
                    let mut current_hl = HlType::Normal;
                    for (i, ch) in line_str
                        .chars()
                        .enumerate()
                        .take(chunk_end)
                        .skip(chunk_start)
                    {
                        let is_tab_pipe = has_tabs
                            && i < self.tab_pipes_scratch.len()
                            && self.tab_pipes_scratch[i];
                        let is_trailing_ws = has_trailing && i >= trailing_ws_start;
                        if is_trailing_ws {
                            if current_hl != HlType::Normal {
                                write!(w, "\x1b[0m")?;
                                current_hl = HlType::Normal;
                            }
                            write!(w, "\x1b[41m{}\x1b[0m", ch)?;
                        } else if is_tab_pipe {
                            if current_hl != HlType::Normal {
                                write!(w, "\x1b[0m")?;
                                current_hl = HlType::Normal;
                            }
                            write!(w, "\x1b[90m{}\x1b[0m", ch)?;
                        } else {
                            let ht = if has_char_hl {
                                self.char_hl_scratch
                                    .get(i)
                                    .copied()
                                    .unwrap_or(HlType::Normal)
                            } else {
                                HlType::Normal
                            };
                            if ht != current_hl {
                                if ht == HlType::Normal {
                                    write!(w, "\x1b[0m")?;
                                } else {
                                    write!(w, "{}", ht.ansi_code())?;
                                }
                                current_hl = ht;
                            }
                            write!(w, "{}", ch)?;
                        }
                    }
                    if current_hl != HlType::Normal {
                        write!(w, "\x1b[0m")?;
                    }
                }

                write!(w, "\x1b[0m\x1b[K")?;
                screen_row += 1;
            }

            line_idx += 1;
        }

        // Fill remaining screen rows with empty lines
        while screen_row < text_rows {
            write!(w, "\x1b[{};1H", screen_row + 1)?;
            if ruler_on {
                let pad = gw - 1;
                write!(w, "\x1b[0;2m{:>width$} \x1b[0m\x1b[K", "", width = pad)?;
            } else {
                write!(w, "\x1b[K")?;
            }
            screen_row += 1;
        }

        // Completions area
        for (i, comp) in completions.iter().enumerate() {
            let row = text_rows + i + 1;
            write!(w, "\x1b[{};1H", row)?;
            write!(w, "\x1b[2m  {}\x1b[0m\x1b[K", comp)?;
        }

        // Status bar
        let status_row = text_rows + completion_rows + 1;
        write!(w, "\x1b[{};1H", status_row)?;
        write!(w, "\x1b[0;7m")?;
        let width = view.width as usize;
        let left_len = status_left.len().min(width);
        let right_len = status_right.len();
        let padding = width.saturating_sub(left_len + right_len);
        write!(
            w,
            "{}{}{}",
            &status_left[..left_len],
            " ".repeat(padding),
            status_right,
        )?;
        write!(w, "\x1b[0m")?;

        // Command line
        let cmd_row = text_rows + completion_rows + 2;
        write!(w, "\x1b[{};1H", cmd_row)?;
        if let Some(cmd) = command_line {
            write!(w, "\x1b[30;43m{}\x1b[K\x1b[0m", cmd)?;
        } else {
            write!(w, "\x1b[K")?;
        }

        // Position cursor
        if find_active || has_sel {
            // Hide cursor while browsing find results or when selection is active
            write!(w, "\x1b[?25l")?;
        } else if let Some(col) = cmd_cursor {
            // Blinking cursor in command buffer
            write!(w, "\x1b[{};{}H", cmd_row, col + 1)?;
            write!(w, "\x1b[?25h")?;
        } else {
            let screen_col = (cursor_col % text_cols.max(1)) + gw;
            // Count screen rows from scroll position to cursor
            let cursor_wrap = cursor_col / text_cols.max(1);
            let screen_row = {
                let mut sr = 0usize;
                if cursor_line == view.scroll_line {
                    sr = cursor_wrap.saturating_sub(view.scroll_wrap);
                } else {
                    // Remaining wraps of scroll_line
                    buf.line_text_into(view.scroll_line, &mut self.line_buf);
                    let first_wraps = crate::view::wrapped_rows(
                        display_col_for_char_col(&self.line_buf, usize::MAX),
                        text_cols,
                    );
                    sr += first_wraps.saturating_sub(view.scroll_wrap);
                    for l in (view.scroll_line + 1)..cursor_line {
                        buf.line_text_into(l, &mut self.line_buf);
                        sr += crate::view::wrapped_rows(
                            display_col_for_char_col(&self.line_buf, usize::MAX),
                            text_cols,
                        );
                    }
                    sr += cursor_wrap;
                }
                sr
            };
            write!(w, "\x1b[{};{}H", screen_row + 1, screen_col + 1)?;
            write!(w, "\x1b[?25h")?;
        }

        // End synchronized output
        write!(w, "\x1b[?2026l")?;

        out.write_all(&frame)?;
        out.flush()?;
        self.needs_full_redraw = false;
        self.frame_buf = frame;
        Ok(())
    }

    /// Ensure `hl_cache` covers lines `0..end`, computing only what's needed.
    ///
    /// - Truncates stale entries if the file shrank.
    /// - Extends to `end` lazily when the user scrolls past current coverage.
    ///   To extend, we reprocess the last already-computed line to recover its
    ///   output state (= input state for the next line), then continue forward.
    /// - When `hl_dirty_from` is set (edit occurred), recomputes forward from
    ///   that line, stopping early once the cached output state matches.
    /// - **Large-jump fast path**: when the viewport jumped far past the computed
    ///   range (e.g. select-all on a 1M-line file), the intermediate entries are
    ///   filled with `HlState::Normal` and only the `scroll_line-200..end` range
    ///   is actually computed.  Multi-line constructs starting in the skipped gap
    ///   will be cosmetically wrong until the user scrolls back through them.
    fn refresh_hl_cache(
        &mut self,
        end: usize,
        scroll_line: usize,
        line_count: usize,
        buf: &mut GapBuffer,
        rules: &'static SyntaxRules,
    ) {
        let end = end.min(line_count);

        // Truncate if the file shrank; clamp dirty marker too.
        if self.hl_cache.len() > line_count {
            self.hl_cache.truncate(line_count);
            self.hl_dirty_from = self.hl_dirty_from.min(line_count);
        }

        let computed = self.hl_cache.len(); // valid entries before this call

        // Grow to cover the viewport.  Entries beyond the old `computed` point
        // are initialised to Normal (fast memset).
        if computed < end {
            self.hl_cache.resize(end, HlState::Normal);
        }

        // Where to start recomputing:
        //   dirty_from  — explicit invalidation from an edit
        //   computed-1  — reprocess last known line to recover its output state
        //                 so we can extend the cache forward from there
        //
        // Where to start recomputing:
        //   dirty_from  — explicit invalidation from an edit
        //   computed-1  — reprocess last known line to recover its output state
        //                 so we can extend the cache forward from there
        //
        // Large-jump optimisation: when the viewport moved far past the computed
        // range (e.g. select-all jumps scroll_line from 0 to 999 978), skip the
        // intermediate gap (already filled Normal by resize) and only compute the
        // ~200 lines just before the visible area.  The condition
        //   scroll_line > computed + 50
        // identifies this: a normal one-line scroll keeps scroll_line ≤ computed.
        let start = if computed < end {
            let dirty_from = self.hl_dirty_from.min(computed.saturating_sub(1));
            if scroll_line > computed + 50 {
                // Far jump: compute only near the new viewport; accept Normal
                // state approximation for the skipped gap.
                scroll_line.saturating_sub(200)
            } else {
                dirty_from
            }
        } else {
            self.hl_dirty_from.min(end)
        };

        if start >= end {
            return;
        }

        let mut state = if start == 0 {
            HlState::Normal
        } else {
            self.hl_cache[start]
        };

        let mut line = start;
        while line < end {
            buf.line_text_into(line, &mut self.line_buf);
            let next_state =
                highlight::highlight_line_into(&self.line_buf, state, rules, &mut self.hl_scratch);
            state = next_state;

            if line + 1 < end {
                // Early exit: within the previously-computed range, stop as soon
                // as the output state already matches what's cached.
                if line + 1 < computed && self.hl_cache[line + 1] == next_state {
                    break;
                }
                self.hl_cache[line + 1] = next_state;
            }
            line += 1;
        }
    }
}

pub(crate) fn display_col_for_char_col(raw_text: &[u8], char_col: usize) -> usize {
    let mut display = 0;
    let mut ci = 0;
    let mut bi = 0;
    while ci < char_col && bi < raw_text.len() {
        if raw_text[bi] == b'\t' {
            display += 2;
        } else {
            display += 1;
        }
        bi += buffer::utf8_char_len(raw_text[bi]);
        ci += 1;
    }
    display
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- gutter_width ---------------------------------------------------------

    #[test]
    fn test_gutter_width_zero_lines() {
        assert_eq!(gutter_width(0), 2); // 1 digit + 1
    }

    #[test]
    fn test_gutter_width_single_digit() {
        assert_eq!(gutter_width(1), 2); // 1 digit + 1
        assert_eq!(gutter_width(9), 2);
    }

    #[test]
    fn test_gutter_width_two_digits() {
        assert_eq!(gutter_width(10), 3); // 2 digits + 1
        assert_eq!(gutter_width(99), 3);
    }

    #[test]
    fn test_gutter_width_three_digits() {
        assert_eq!(gutter_width(100), 4); // 3 digits + 1
        assert_eq!(gutter_width(999), 4);
    }

    #[test]
    fn test_gutter_width_four_digits() {
        assert_eq!(gutter_width(1000), 5);
        assert_eq!(gutter_width(9999), 5);
    }

    #[test]
    fn test_gutter_width_large() {
        assert_eq!(gutter_width(100000), 7); // 6 digits + 1
    }

    // -- expand_tabs_into -----------------------------------------------------

    fn call_expand(text: &[u8]) -> (Vec<u8>, Vec<bool>, bool) {
        let mut out = Vec::new();
        let mut pipes = Vec::new();
        let has_tabs = expand_tabs_into(text, &mut out, &mut pipes);
        (out, pipes, has_tabs)
    }

    #[test]
    fn test_expand_tabs_no_tabs() {
        let (bytes, pipes, has_tabs) = call_expand(b"hello");
        assert_eq!(bytes, b"hello");
        assert!(!has_tabs);
        assert!(pipes.is_empty());
    }

    #[test]
    fn test_expand_tabs_single_tab() {
        let (bytes, pipes, has_tabs) = call_expand(b"\thello");
        assert_eq!(bytes, b"| hello");
        assert!(has_tabs);
        assert_eq!(pipes, vec![true, false, false, false, false, false, false]);
    }

    #[test]
    fn test_expand_tabs_multiple_tabs() {
        let (bytes, pipes, has_tabs) = call_expand(b"\t\t");
        assert_eq!(bytes, b"| | ");
        assert!(has_tabs);
        assert_eq!(pipes, vec![true, false, true, false]);
    }

    #[test]
    fn test_expand_tabs_mixed() {
        let (bytes, pipes, has_tabs) = call_expand(b"a\tb\tc");
        assert_eq!(bytes, b"a| b| c");
        assert!(has_tabs);
        assert_eq!(pipes, vec![false, true, false, false, true, false, false]);
    }

    #[test]
    fn test_expand_tabs_empty() {
        let (bytes, pipes, has_tabs) = call_expand(b"");
        assert_eq!(bytes, b"");
        assert!(!has_tabs);
        assert!(pipes.is_empty());
    }

    // -- display_col_for_char_col ---------------------------------------------

    #[test]
    fn test_display_col_plain_ascii() {
        // No tabs, ASCII text: display col == char col
        assert_eq!(display_col_for_char_col(b"hello", 0), 0);
        assert_eq!(display_col_for_char_col(b"hello", 3), 3);
        assert_eq!(display_col_for_char_col(b"hello", 5), 5);
    }

    #[test]
    fn test_display_col_with_tab() {
        // Tab expands to 2 display cols
        assert_eq!(display_col_for_char_col(b"\thello", 0), 0);
        assert_eq!(display_col_for_char_col(b"\thello", 1), 2); // past the tab
        assert_eq!(display_col_for_char_col(b"\thello", 2), 3); // past tab + 'h'
    }

    #[test]
    fn test_display_col_multiple_tabs() {
        assert_eq!(display_col_for_char_col(b"\t\thello", 2), 4); // 2 tabs = 4 display cols
    }

    #[test]
    fn test_display_col_utf8() {
        // "é" is 2 bytes but 1 char
        let text = "héllo".as_bytes();
        assert_eq!(display_col_for_char_col(text, 0), 0);
        assert_eq!(display_col_for_char_col(text, 1), 1); // 'h'
        assert_eq!(display_col_for_char_col(text, 2), 2); // 'é'
        assert_eq!(display_col_for_char_col(text, 5), 5); // full string
    }

    #[test]
    fn test_display_col_past_end() {
        // char_col beyond text length: stops at end
        assert_eq!(display_col_for_char_col(b"ab", 10), 2);
    }

    #[test]
    fn test_display_col_empty() {
        assert_eq!(display_col_for_char_col(b"", 0), 0);
        assert_eq!(display_col_for_char_col(b"", 5), 0);
    }

    // -- Renderer basic -------------------------------------------------------

    #[test]
    fn test_renderer_new() {
        let r = Renderer::new();
        assert!(r.needs_full_redraw);
    }

    #[test]
    fn test_renderer_force_full_redraw() {
        let mut r = Renderer::new();
        r.needs_full_redraw = false;
        r.force_full_redraw();
        assert!(r.needs_full_redraw);
    }

    #[test]
    fn test_render_basic_output() {
        // Verify render produces output without panicking
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"hello\nworld");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "test.txt",
            "Ln 1, Col 1",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("hello"));
        assert!(s.contains("world"));
        assert!(s.contains("test.txt"));
        assert!(s.contains("Ln 1, Col 1"));
    }

    #[test]
    fn test_render_no_ruler() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"hello");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            false, // ruler off
            "test.txt",
            "Ln 1, Col 1",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("hello"));
        // No line numbers should appear
        // (gutter_width is 0 when ruler is off)
    }

    #[test]
    fn test_render_with_command_line() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"hello");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            Some("find: test"),
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("find: test"));
    }

    #[test]
    fn test_render_clears_full_redraw_flag() {
        let mut r = Renderer::new();
        assert!(r.needs_full_redraw);

        let mut buf = GapBuffer::from_text(b"hello");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        assert!(!r.needs_full_redraw);
    }

    #[test]
    fn test_render_with_selection() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"hello world");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        let sel = Selection {
            anchor: Pos::new(0, 2),
            cursor: Pos::new(0, 7),
        };

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            None,
            Some(sel),
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        // Should contain reverse video escape codes for selection
        assert!(s.contains("\x1b[7m"));
    }

    #[test]
    fn test_render_with_find_matches() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"hello world hello");
        let view = View::new(80, 24);
        let mut output = Vec::new();
        let matches = [
            (Pos::new(0, 0), Pos::new(0, 5)),
            (Pos::new(0, 12), Pos::new(0, 17)),
        ];

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            None,
            None,
            Some(&matches),
            Some((Pos::new(0, 0), Pos::new(0, 5))), // current = first match
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        // Current match → green bg \x1b[42;30m, other → yellow bg \x1b[43;30m
        assert!(s.contains("\x1b[42;30m"));
        assert!(s.contains("\x1b[43;30m"));
    }

    #[test]
    fn test_render_with_bracket_pair() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"(hello)");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            Some((Pos::new(0, 0), Pos::new(0, 6))),
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        // Bracket match → magenta bg \x1b[45;30m
        assert!(s.contains("\x1b[45;30m"));
    }

    #[test]
    fn test_render_with_completions() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"hello");
        let view = View::new(80, 24);
        let mut output = Vec::new();
        let comps = vec!["save".to_string(), "quit".to_string()];

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            None,
            None,
            None,
            None,
            &comps,
            None,
            false,
            None,
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("save"));
        assert!(s.contains("quit"));
    }

    #[test]
    fn test_render_with_cmd_cursor() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"hello");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            Some("find: test"),
            None,
            None,
            None,
            &[],
            Some(10),
            false,
            None,
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        // Cursor shown in command line area
        assert!(s.contains("\x1b[?25h"));
    }

    #[test]
    fn test_render_find_active_hides_cursor() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::from_text(b"hello");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            None,
            None,
            None,
            None,
            &[],
            None,
            true,
            None,
        )
        .unwrap();

        // find_active=true should hide cursor
        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("\x1b[?25l"));
        // And NOT contain show cursor at the end positioning
        // (the only ?25h should not appear because find_active is true)
    }

    #[test]
    fn test_render_syntax_cache_invalidation() {
        let mut r = Renderer::new();
        let rules = crate::highlight::rules_for_language("Rust");
        r.set_syntax(rules);

        let mut buf = GapBuffer::from_text(b"fn main() {}");
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        // Modify buffer (version changes) and render again
        buf.insert(0, b"// ");
        output.clear();
        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "",
            "",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        // Should contain comment coloring (grey = \x1b[90m)
        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("\x1b[90m"));
    }

    #[test]
    fn test_set_syntax_changes_rules() {
        let mut r = Renderer::new();
        assert!(r.syntax.is_none());
        let rules = crate::highlight::rules_for_language("Rust");
        r.set_syntax(rules);
        assert!(r.syntax.is_some());
        r.set_syntax(None);
        assert!(r.syntax.is_none());
    }

    #[test]
    fn test_render_empty_buffer() {
        let mut r = Renderer::new();
        let mut buf = GapBuffer::new();
        let view = View::new(80, 24);
        let mut output = Vec::new();

        r.render(
            &mut output,
            &mut buf,
            &view,
            0,
            0,
            true,
            "[No Name]",
            "Ln 1, Col 1",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("[No Name]"));
    }
}
