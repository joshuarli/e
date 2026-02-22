use std::io::{self, Write};

use crate::buffer::{self, GapBuffer};
use crate::selection::{Pos, Selection};
use crate::view::View;

/// Compute gutter width: width of the largest line number + 2.
pub fn gutter_width(line_count: usize) -> usize {
    let digits = if line_count == 0 {
        1
    } else {
        ((line_count as f64).log10().floor() as usize) + 1
    };
    digits + 2
}

fn expand_tabs(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for &b in text {
        if b == b'\t' {
            out.push(b' ');
            out.push(b' ');
        } else {
            out.push(b);
        }
    }
    out
}

pub struct Renderer {
    #[allow(dead_code)]
    line_hashes: Vec<u64>,
    pub needs_full_redraw: bool,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            line_hashes: Vec::new(),
            needs_full_redraw: true,
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
    ) -> io::Result<()> {
        let line_count = buf.line_count();
        let gw = if ruler_on {
            gutter_width(line_count)
        } else {
            0
        };
        let text_rows = view.text_rows();
        let text_cols = view.text_cols(gw);

        let (sel_start, sel_end) = selection
            .map(|s| s.ordered())
            .unwrap_or((Pos::zero(), Pos::zero()));
        let has_sel = selection.is_some_and(|s| !s.is_empty());

        write!(out, "\x1b[?25l")?;

        for row in 0..text_rows {
            let logical_line = view.scroll_line + row;
            write!(out, "\x1b[{};1H\x1b[2K", row + 1)?;

            if logical_line < line_count {
                if ruler_on {
                    let num_str = format!("{}", logical_line + 1);
                    let pad = gw - 2;
                    write!(
                        out,
                        "\x1b[2m{:>width$} \u{2502}\x1b[0m",
                        num_str,
                        width = pad
                    )?;
                }

                let raw_text = buf.line_text(logical_line);
                let expanded = expand_tabs(&raw_text);
                let line_str = String::from_utf8_lossy(&expanded);
                let chars: Vec<char> = line_str.chars().collect();
                let visible_start = view.scroll_col.min(chars.len());
                let visible_end = (view.scroll_col + text_cols).min(chars.len());

                // Build per-character highlight info: selection or find match
                let need_per_char =
                    has_sel && logical_line >= sel_start.line && logical_line <= sel_end.line;
                let has_find = find_matches.is_some_and(|m| {
                    m.iter()
                        .any(|(s, e)| logical_line >= s.line && logical_line <= e.line)
                });

                if need_per_char || has_find {
                    // Selection range (display cols)
                    let (line_sel_start, line_sel_end) = if need_per_char {
                        let s = if logical_line == sel_start.line {
                            display_col_for_char_col(&raw_text, sel_start.col)
                        } else {
                            0
                        };
                        let e = if logical_line == sel_end.line {
                            display_col_for_char_col(&raw_text, sel_end.col)
                        } else {
                            chars.len()
                        };
                        (s, e)
                    } else {
                        (0, 0)
                    };

                    // Find match ranges (display cols) for this line
                    let find_ranges: Vec<(usize, usize)> = find_matches
                        .map(|matches| {
                            matches
                                .iter()
                                .filter(|(s, e)| logical_line >= s.line && logical_line <= e.line)
                                .map(|(s, e)| {
                                    let fs = if logical_line == s.line {
                                        display_col_for_char_col(&raw_text, s.col)
                                    } else {
                                        0
                                    };
                                    let fe = if logical_line == e.line {
                                        display_col_for_char_col(&raw_text, e.col)
                                    } else {
                                        chars.len()
                                    };
                                    (fs, fe)
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    for (i, ch) in chars
                        .iter()
                        .enumerate()
                        .take(visible_end)
                        .skip(visible_start)
                    {
                        let in_sel = need_per_char && i >= line_sel_start && i < line_sel_end;
                        let in_find = find_ranges.iter().any(|(fs, fe)| i >= *fs && i < *fe);

                        if in_sel {
                            // Selection: reverse video
                            write!(out, "\x1b[7m{}\x1b[0m", ch)?;
                        } else if in_find {
                            // Find match: yellow background
                            write!(out, "\x1b[43;30m{}\x1b[0m", ch)?;
                        } else {
                            write!(out, "{}", ch)?;
                        }
                    }
                } else {
                    let visible: String = chars[visible_start..visible_end].iter().collect();
                    write!(out, "{}", visible)?;
                }
            } else if ruler_on {
                let pad = gw - 2;
                write!(out, "\x1b[2m{:>width$} \u{2502}\x1b[0m", "", width = pad)?;
            }
        }

        // Status bar
        let status_row = text_rows + 1;
        write!(out, "\x1b[{};1H\x1b[2K", status_row)?;
        write!(out, "\x1b[7m")?;
        let width = view.width as usize;
        let left_len = status_left.len().min(width);
        let right_len = status_right.len();
        let padding = width.saturating_sub(left_len + right_len);
        write!(
            out,
            "{}{}{}",
            &status_left[..left_len],
            " ".repeat(padding),
            status_right,
        )?;
        write!(out, "\x1b[0m")?;

        // Command line
        let cmd_row = text_rows + 2;
        write!(out, "\x1b[{};1H\x1b[2K", cmd_row)?;
        if let Some(cmd) = command_line {
            write!(out, "{}", cmd)?;
        }

        // Position cursor
        let screen_col = if ruler_on {
            cursor_col.saturating_sub(view.scroll_col) + gw
        } else {
            cursor_col.saturating_sub(view.scroll_col)
        };
        let screen_row = cursor_line.saturating_sub(view.scroll_line);
        write!(out, "\x1b[{};{}H", screen_row + 1, screen_col + 1)?;
        write!(out, "\x1b[?25h")?;

        out.flush()?;
        self.needs_full_redraw = false;
        Ok(())
    }
}

fn display_col_for_char_col(raw_text: &[u8], char_col: usize) -> usize {
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
        assert_eq!(gutter_width(0), 3); // 1 digit + 2
    }

    #[test]
    fn test_gutter_width_single_digit() {
        assert_eq!(gutter_width(1), 3); // 1 digit + 2
        assert_eq!(gutter_width(9), 3);
    }

    #[test]
    fn test_gutter_width_two_digits() {
        assert_eq!(gutter_width(10), 4); // 2 digits + 2
        assert_eq!(gutter_width(99), 4);
    }

    #[test]
    fn test_gutter_width_three_digits() {
        assert_eq!(gutter_width(100), 5); // 3 digits + 2
        assert_eq!(gutter_width(999), 5);
    }

    #[test]
    fn test_gutter_width_four_digits() {
        assert_eq!(gutter_width(1000), 6);
        assert_eq!(gutter_width(9999), 6);
    }

    #[test]
    fn test_gutter_width_large() {
        assert_eq!(gutter_width(100000), 8); // 6 digits + 2
    }

    // -- expand_tabs ----------------------------------------------------------

    #[test]
    fn test_expand_tabs_no_tabs() {
        assert_eq!(expand_tabs(b"hello"), b"hello");
    }

    #[test]
    fn test_expand_tabs_single_tab() {
        assert_eq!(expand_tabs(b"\thello"), b"  hello");
    }

    #[test]
    fn test_expand_tabs_multiple_tabs() {
        assert_eq!(expand_tabs(b"\t\t"), b"    ");
    }

    #[test]
    fn test_expand_tabs_mixed() {
        assert_eq!(expand_tabs(b"a\tb\tc"), b"a  b  c");
    }

    #[test]
    fn test_expand_tabs_empty() {
        assert_eq!(expand_tabs(b""), b"");
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
        )
        .unwrap();

        let s = String::from_utf8_lossy(&output);
        assert!(s.contains("[No Name]"));
    }
}
