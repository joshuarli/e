use std::io::{self, Write};

use crate::buffer::{self, GapBuffer};
use crate::highlight::{self, HlState, HlType, SyntaxRules};
use crate::selection::{Pos, Selection};
use crate::view::View;

/// Compute gutter width: width of the largest line number + 1 (trailing space).
pub fn gutter_width(line_count: usize) -> usize {
    let digits = if line_count == 0 {
        1
    } else {
        ((line_count as f64).log10().floor() as usize) + 1
    };
    digits + 1
}

/// Expand tabs to display characters. Returns (expanded_bytes, tab_pipe_positions).
/// tab_pipe_positions[i] is true if display column i is the '|' of a tab.
fn expand_tabs(text: &[u8]) -> (Vec<u8>, Vec<bool>) {
    let mut out = Vec::with_capacity(text.len());
    let mut tab_pipes = Vec::with_capacity(text.len());
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
    (out, tab_pipes)
}

pub struct Renderer {
    pub needs_full_redraw: bool,
    syntax: Option<&'static SyntaxRules>,
    /// Cached highlight state at the start of each line.
    hl_cache: Vec<HlState>,
    /// Buffer version the cache was computed for.
    hl_cache_version: u64,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            needs_full_redraw: true,
            syntax: None,
            hl_cache: Vec::new(),
            hl_cache_version: u64::MAX,
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
        find_current: Option<usize>,
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

        // Buffer all output, then write to terminal in one shot to avoid flicker
        let mut frame = Vec::with_capacity(8192);
        let w = &mut frame;

        // Synchronized output: tell terminal to hold rendering until frame is complete
        write!(w, "\x1b[?2026h")?;
        write!(w, "\x1b[?25l")?;

        // Compute per-line highlight states (cached across frames)
        let buf_version = buf.version();
        if let Some(rules) = self.syntax {
            if buf_version != self.hl_cache_version {
                // Buffer changed — recompute all states from scratch
                self.hl_cache.clear();
                let mut state = HlState::Normal;
                for line_idx in 0..line_count {
                    self.hl_cache.push(state);
                    let raw = buf.line_text(line_idx);
                    let (_, next_state) = highlight::highlight_line(&raw, state, rules);
                    state = next_state;
                }
                self.hl_cache_version = buf_version;
            } else if self.hl_cache.len() < line_count {
                // Extend cache if file grew (shouldn't happen without version bump, but safe)
                let mut state = self.hl_cache.last().copied().unwrap_or(HlState::Normal);
                let start = self.hl_cache.len();
                if start > 0 {
                    let raw = buf.line_text(start - 1);
                    let (_, next_state) = highlight::highlight_line(&raw, state, rules);
                    state = next_state;
                }
                for line_idx in start..line_count {
                    self.hl_cache.push(state);
                    let raw = buf.line_text(line_idx);
                    let (_, next_state) = highlight::highlight_line(&raw, state, rules);
                    state = next_state;
                }
            }
        } else {
            self.hl_cache.clear();
        }
        let hl_states = &self.hl_cache;

        // Wrap-aware render loop
        let mut screen_row: usize = 0;
        let mut line_idx = view.scroll_line;
        let first_wrap = view.scroll_wrap;

        while screen_row < text_rows && line_idx < line_count {
            let raw_text = buf.line_text(line_idx);
            let (expanded, tab_pipes) = expand_tabs(&raw_text);
            let line_str = String::from_utf8_lossy(&expanded);
            let chars: Vec<char> = line_str.chars().collect();
            let total_wraps = crate::view::wrapped_rows(chars.len(), text_cols);
            let start_wrap = if line_idx == view.scroll_line {
                first_wrap
            } else {
                0
            };

            // Compute per-char syntax highlights for this line (once per logical line)
            let char_hl: Option<Vec<HlType>> = self.syntax.and_then(|rules| {
                hl_states.get(line_idx).map(|&state| {
                    let (byte_hl, _) = highlight::highlight_line(&raw_text, state, rules);
                    highlight::byte_hl_to_char_hl(&raw_text, &byte_hl)
                })
            });

            // Bracket match display columns for this line
            let bracket_cols: Vec<usize> = bracket_pair
                .iter()
                .filter(|(_, match_pos)| match_pos.line == line_idx)
                .map(|(_, match_pos)| display_col_for_char_col(&raw_text, match_pos.col))
                .collect();

            // Per-character highlight info
            let need_per_char = has_sel && line_idx >= sel_start.line && line_idx <= sel_end.line;
            let has_find = find_matches.is_some_and(|m| {
                m.iter()
                    .any(|(s, e)| line_idx >= s.line && line_idx <= e.line)
            });
            let has_bracket = !bracket_cols.is_empty();

            // Pre-compute selection and find ranges once per logical line
            let (line_sel_start, line_sel_end) = if need_per_char {
                let s = if line_idx == sel_start.line {
                    display_col_for_char_col(&raw_text, sel_start.col)
                } else {
                    0
                };
                let e = if line_idx == sel_end.line {
                    display_col_for_char_col(&raw_text, sel_end.col)
                } else {
                    chars.len()
                };
                (s, e)
            } else {
                (0, 0)
            };

            let find_ranges: Vec<(usize, usize, bool)> = find_matches
                .map(|matches| {
                    matches
                        .iter()
                        .enumerate()
                        .filter(|(_, (s, e))| line_idx >= s.line && line_idx <= e.line)
                        .map(|(idx, (s, e))| {
                            let fs = if line_idx == s.line {
                                display_col_for_char_col(&raw_text, s.col)
                            } else {
                                0
                            };
                            let fe = if line_idx == e.line {
                                display_col_for_char_col(&raw_text, e.col)
                            } else {
                                chars.len()
                            };
                            let is_current = find_current == Some(idx);
                            (fs, fe, is_current)
                        })
                        .collect()
                })
                .unwrap_or_default();

            for wrap in start_wrap..total_wraps {
                if screen_row >= text_rows {
                    break;
                }

                write!(w, "\x1b[{};1H", screen_row + 1)?;

                // Gutter: line number on first wrap row, blank on continuations
                if ruler_on {
                    if wrap == 0 {
                        let num_str = format!("{}", line_idx + 1);
                        let pad = gw - 1;
                        write!(w, "\x1b[0;2m{:>width$} \x1b[0m", num_str, width = pad)?;
                    } else {
                        write!(w, "\x1b[0;2m{:>width$} \x1b[0m", "", width = gw - 1)?;
                    }
                }

                let chunk_start = wrap * text_cols;
                let chunk_end = ((wrap + 1) * text_cols).min(chars.len());

                if need_per_char || has_find || has_bracket {
                    for (i, ch) in chars.iter().enumerate().take(chunk_end).skip(chunk_start) {
                        let in_sel = need_per_char && i >= line_sel_start && i < line_sel_end;
                        let find_hit = find_ranges.iter().find(|(fs, fe, _)| i >= *fs && i < *fe);
                        let is_tab_pipe = i < tab_pipes.len() && tab_pipes[i];
                        let is_bracket_match = bracket_cols.contains(&i);

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
                        } else if is_tab_pipe {
                            write!(w, "\x1b[90m{}\x1b[0m", ch)?;
                        } else {
                            let ht = char_hl
                                .as_ref()
                                .and_then(|h| h.get(i).copied())
                                .unwrap_or(HlType::Normal);
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
                    for (i, ch) in chars.iter().enumerate().take(chunk_end).skip(chunk_start) {
                        let is_tab_pipe = i < tab_pipes.len() && tab_pipes[i];
                        if is_tab_pipe {
                            if current_hl != HlType::Normal {
                                write!(w, "\x1b[0m")?;
                                current_hl = HlType::Normal;
                            }
                            write!(w, "\x1b[90m{}\x1b[0m", ch)?;
                        } else {
                            let ht = char_hl
                                .as_ref()
                                .and_then(|h| h.get(i).copied())
                                .unwrap_or(HlType::Normal);
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
                    let first_wraps = crate::view::wrapped_rows(
                        display_col_for_char_col(
                            &buf.line_text(view.scroll_line),
                            buf.line_char_len(view.scroll_line),
                        ),
                        text_cols,
                    );
                    sr += first_wraps.saturating_sub(view.scroll_wrap);
                    for l in (view.scroll_line + 1)..cursor_line {
                        sr += crate::view::wrapped_rows(
                            display_col_for_char_col(&buf.line_text(l), buf.line_char_len(l)),
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
        Ok(())
    }
}

/// Convert a display column back to a char column (inverse of display_col_for_char_col).
pub(crate) fn char_col_for_display_col(raw_text: &[u8], target_display: usize) -> usize {
    let mut display = 0;
    let mut ci = 0;
    let mut bi = 0;
    while bi < raw_text.len() {
        let width = if raw_text[bi] == b'\t' { 2 } else { 1 };
        if display + width > target_display {
            break;
        }
        display += width;
        bi += buffer::utf8_char_len(raw_text[bi]);
        ci += 1;
    }
    ci
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

    // -- expand_tabs ----------------------------------------------------------

    #[test]
    fn test_expand_tabs_no_tabs() {
        let (bytes, pipes) = expand_tabs(b"hello");
        assert_eq!(bytes, b"hello");
        assert_eq!(pipes, vec![false; 5]);
    }

    #[test]
    fn test_expand_tabs_single_tab() {
        let (bytes, pipes) = expand_tabs(b"\thello");
        assert_eq!(bytes, b"| hello");
        assert_eq!(pipes, vec![true, false, false, false, false, false, false]);
    }

    #[test]
    fn test_expand_tabs_multiple_tabs() {
        let (bytes, pipes) = expand_tabs(b"\t\t");
        assert_eq!(bytes, b"| | ");
        assert_eq!(pipes, vec![true, false, true, false]);
    }

    #[test]
    fn test_expand_tabs_mixed() {
        let (bytes, pipes) = expand_tabs(b"a\tb\tc");
        assert_eq!(bytes, b"a| b| c");
        assert_eq!(pipes, vec![false, true, false, false, true, false, false]);
    }

    #[test]
    fn test_expand_tabs_empty() {
        let (bytes, pipes) = expand_tabs(b"");
        assert_eq!(bytes, b"");
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
