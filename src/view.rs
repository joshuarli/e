/// Viewport: tracks scroll offsets and maps cursor position to screen coordinates.
pub struct View {
    /// First visible line (0-indexed logical line).
    pub scroll_line: usize,
    /// Which wrapped sub-row of `scroll_line` is at top of screen (0 = first row).
    pub scroll_wrap: usize,
    /// Terminal width in columns.
    pub width: u16,
    /// Terminal height in rows.
    pub height: u16,
}

/// How many screen rows a line of given display width occupies (minimum 1).
pub fn wrapped_rows(display_width: usize, text_cols: usize) -> usize {
    if text_cols == 0 || display_width == 0 {
        return 1;
    }
    display_width.div_ceil(text_cols)
}

impl View {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            scroll_line: 0,
            scroll_wrap: 0,
            width,
            height,
        }
    }

    /// Number of lines available for text (total height minus status bar and command line).
    pub fn text_rows(&self) -> usize {
        (self.height as usize).saturating_sub(2)
    }

    /// Width available for text after the gutter.
    pub fn text_cols(&self, gutter_width: usize) -> usize {
        (self.width as usize).saturating_sub(gutter_width)
    }

    /// Adjust scroll so that the cursor line/col is visible, with soft-wrap.
    ///
    /// `line_display_width` returns the total display width for a given line index.
    pub fn ensure_cursor_visible(
        &mut self,
        cursor_line: usize,
        cursor_col: usize,
        gutter_width: usize,
        line_display_width: &mut dyn FnMut(usize) -> usize,
    ) {
        let rows = self.text_rows();
        if rows == 0 {
            return;
        }
        let text_cols = self.text_cols(gutter_width);
        if text_cols == 0 {
            return;
        }

        // Which wrapped sub-row the cursor is on within its line
        let cursor_wrap = cursor_col / text_cols;

        // Cursor above viewport?
        if cursor_line < self.scroll_line
            || (cursor_line == self.scroll_line && cursor_wrap < self.scroll_wrap)
        {
            self.scroll_line = cursor_line;
            self.scroll_wrap = cursor_wrap;
            return;
        }

        // Fast path: when the cursor is far off-screen (more than 2× viewport heights
        // away), skip the O(distance) per-line wrap scan and jump directly.  The slow
        // path is still used for small overscrolls so minimal-scroll semantics are exact.
        if cursor_line.saturating_sub(self.scroll_line) > rows * 2 {
            // Place cursor at the bottom of the viewport (1 row per line assumption).
            self.scroll_line = cursor_line + 1 - rows;
            self.scroll_wrap = 0;
            return;
        }

        // Count screen rows from (scroll_line, scroll_wrap) to (cursor_line, cursor_wrap) inclusive
        let mut screen_rows = 0usize;

        if cursor_line == self.scroll_line {
            // Same line: just the difference in wrap rows + 1
            screen_rows = cursor_wrap - self.scroll_wrap + 1;
        } else {
            // Remaining rows of scroll_line
            let first_line_wraps = wrapped_rows(line_display_width(self.scroll_line), text_cols);
            screen_rows += first_line_wraps.saturating_sub(self.scroll_wrap);

            // Full lines between scroll_line+1 and cursor_line-1
            for line in (self.scroll_line + 1)..cursor_line {
                screen_rows += wrapped_rows(line_display_width(line), text_cols);
            }

            // Rows needed on cursor_line (through cursor_wrap, inclusive)
            screen_rows += cursor_wrap + 1;
        }

        // If cursor is beyond bottom of viewport, scroll forward
        if screen_rows > rows {
            let overshoot = screen_rows - rows;
            self.scroll_forward(overshoot, text_cols, line_display_width);
        }
    }

    /// Scroll forward by `n` screen rows.
    fn scroll_forward(
        &mut self,
        mut n: usize,
        text_cols: usize,
        line_display_width: &mut dyn FnMut(usize) -> usize,
    ) {
        while n > 0 {
            let wraps = wrapped_rows(line_display_width(self.scroll_line), text_cols);
            let remaining_in_line = wraps.saturating_sub(self.scroll_wrap);
            if n < remaining_in_line {
                self.scroll_wrap += n;
                return;
            }
            n -= remaining_in_line;
            self.scroll_line += 1;
            self.scroll_wrap = 0;
        }
    }

    /// Center the viewport vertically on the given line.
    pub fn center_on_line(
        &mut self,
        line: usize,
        line_display_width: &mut dyn FnMut(usize) -> usize,
        gutter_width: usize,
    ) {
        let rows = self.text_rows();
        if rows == 0 {
            return;
        }
        let text_cols = self.text_cols(gutter_width);
        if text_cols == 0 {
            return;
        }

        // Walk backwards from `line` accumulating screen rows until we've used ~half
        let half = rows / 2;
        let mut accum = 0usize;
        let mut start_line = line;
        let mut start_wrap = 0usize;

        // Include wraps of the target line itself (up to the midpoint)
        let target_wraps = wrapped_rows(line_display_width(line), text_cols);
        // Put the first row of the target line roughly at center
        accum += 1; // at least 1 row for the target line's first row

        if accum < half && start_line > 0 {
            // Walk backwards through preceding lines
            let mut remaining = half - accum;
            let mut l = line;
            while l > 0 && remaining > 0 {
                l -= 1;
                let w = wrapped_rows(line_display_width(l), text_cols);
                if w <= remaining {
                    remaining -= w;
                    start_line = l;
                    start_wrap = 0;
                } else {
                    // Partial: start at a sub-row within this line
                    start_line = l;
                    start_wrap = w - remaining;
                    remaining = 0;
                }
            }
            if remaining > 0 {
                start_line = 0;
                start_wrap = 0;
            }
        } else {
            // Target line wraps enough to fill half the screen on its own
            let _ = target_wraps; // target line starts at top
            start_line = line;
            start_wrap = 0;
        }

        self.scroll_line = start_line;
        self.scroll_wrap = start_wrap;
    }

    /// Convert a buffer (line, col) to screen (row, col). Returns None if off-screen.
    #[allow(dead_code)]
    pub fn buffer_to_screen(
        &self,
        line: usize,
        col: usize,
        gutter_width: usize,
        line_display_width: &mut dyn FnMut(usize) -> usize,
    ) -> Option<(u16, u16)> {
        let text_cols = self.text_cols(gutter_width);
        if text_cols == 0 {
            return None;
        }
        let rows = self.text_rows();

        // Compute screen row by counting wrapped rows from scroll position
        let mut screen_row: usize = 0;

        if line < self.scroll_line {
            return None;
        }

        if line == self.scroll_line {
            let wrap = col / text_cols;
            if wrap < self.scroll_wrap {
                return None;
            }
            screen_row = wrap - self.scroll_wrap;
        } else {
            // Remaining rows from scroll_line
            let first_wraps = wrapped_rows(line_display_width(self.scroll_line), text_cols);
            screen_row += first_wraps.saturating_sub(self.scroll_wrap);

            for l in (self.scroll_line + 1)..line {
                screen_row += wrapped_rows(line_display_width(l), text_cols);
            }

            let wrap = col / text_cols;
            screen_row += wrap;
        }

        if screen_row >= rows {
            return None;
        }

        let screen_col = (col % text_cols) + gutter_width;
        if screen_col >= self.width as usize {
            return None;
        }

        Some((screen_row as u16, screen_col as u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trivial_width(_line: usize) -> usize {
        0
    }

    // -- new / basic accessors ------------------------------------------------

    #[test]
    fn test_new() {
        let v = View::new(80, 24);
        assert_eq!(v.scroll_line, 0);
        assert_eq!(v.scroll_wrap, 0);
        assert_eq!(v.width, 80);
        assert_eq!(v.height, 24);
    }

    #[test]
    fn test_text_rows() {
        let v = View::new(80, 24);
        assert_eq!(v.text_rows(), 22); // 24 - 2 (status bar + command line)
    }

    #[test]
    fn test_text_rows_small_terminal() {
        let v = View::new(80, 3);
        assert_eq!(v.text_rows(), 1);
    }

    #[test]
    fn test_text_rows_minimum() {
        let v = View::new(80, 2);
        assert_eq!(v.text_rows(), 0);
    }

    #[test]
    fn test_text_rows_very_small() {
        let v = View::new(80, 1);
        assert_eq!(v.text_rows(), 0); // saturating_sub
    }

    #[test]
    fn test_text_cols() {
        let v = View::new(80, 24);
        assert_eq!(v.text_cols(5), 75); // 80 - 5 gutter
    }

    #[test]
    fn test_text_cols_no_gutter() {
        let v = View::new(80, 24);
        assert_eq!(v.text_cols(0), 80);
    }

    #[test]
    fn test_text_cols_large_gutter() {
        let v = View::new(10, 24);
        assert_eq!(v.text_cols(15), 0); // saturating_sub
    }

    // -- wrapped_rows ---------------------------------------------------------

    #[test]
    fn test_wrapped_rows_short_line() {
        assert_eq!(wrapped_rows(10, 80), 1);
    }

    #[test]
    fn test_wrapped_rows_exact_fit() {
        assert_eq!(wrapped_rows(80, 80), 1);
    }

    #[test]
    fn test_wrapped_rows_overflow() {
        assert_eq!(wrapped_rows(81, 80), 2);
    }

    #[test]
    fn test_wrapped_rows_double() {
        assert_eq!(wrapped_rows(160, 80), 2);
    }

    #[test]
    fn test_wrapped_rows_empty() {
        assert_eq!(wrapped_rows(0, 80), 1);
    }

    #[test]
    fn test_wrapped_rows_zero_cols() {
        assert_eq!(wrapped_rows(100, 0), 1);
    }

    // -- ensure_cursor_visible -----------------------------------------------

    #[test]
    fn test_ensure_visible_cursor_already_visible() {
        let mut v = View::new(80, 24);
        v.ensure_cursor_visible(10, 5, 4, &mut trivial_width);
        assert_eq!(v.scroll_line, 0); // no scroll needed, line 10 is within 0..22
    }

    #[test]
    fn test_ensure_visible_scrolls_down() {
        let mut v = View::new(80, 24);
        v.ensure_cursor_visible(30, 0, 4, &mut trivial_width);
        // Cursor at line 30, text_rows=22, scroll_line should be 9
        assert_eq!(v.scroll_line, 9);
    }

    #[test]
    fn test_ensure_visible_scrolls_up() {
        let mut v = View::new(80, 24);
        v.scroll_line = 50;
        v.ensure_cursor_visible(10, 0, 4, &mut trivial_width);
        assert_eq!(v.scroll_line, 10);
    }

    #[test]
    fn test_ensure_visible_zero_rows() {
        let mut v = View::new(80, 2); // text_rows = 0
        v.ensure_cursor_visible(10, 0, 4, &mut trivial_width);
        assert_eq!(v.scroll_line, 0);
    }

    #[test]
    fn test_ensure_visible_zero_text_cols() {
        let mut v = View::new(5, 24);
        v.ensure_cursor_visible(0, 100, 10, &mut trivial_width);
        // text_cols = 0, early return
        assert_eq!(v.scroll_line, 0);
    }

    #[test]
    fn test_ensure_visible_cursor_at_last_row() {
        let mut v = View::new(80, 24);
        // text_rows=22, cursor at line 21 should not scroll
        v.ensure_cursor_visible(21, 0, 4, &mut trivial_width);
        assert_eq!(v.scroll_line, 0);

        // cursor at line 22 should scroll by 1
        v.ensure_cursor_visible(22, 0, 4, &mut trivial_width);
        assert_eq!(v.scroll_line, 1);
    }

    #[test]
    fn test_ensure_visible_at_line_zero() {
        let mut v = View::new(80, 24);
        v.scroll_line = 10;
        v.ensure_cursor_visible(0, 0, 4, &mut trivial_width);
        assert_eq!(v.scroll_line, 0);
    }

    #[test]
    fn test_ensure_visible_wrapped_line() {
        // 10 cols text area, line 0 is 25 chars wide → 3 wrapped rows
        let mut v = View::new(14, 7); // text_rows=5, text_cols=10 (14-4 gutter)
        let mut widths = |_line: usize| -> usize { 25 };
        // cursor at col 22 → wrap row 2 (0-indexed)
        v.ensure_cursor_visible(0, 22, 4, &mut widths);
        assert_eq!(v.scroll_line, 0);
        // cursor wrap=2, scroll_wrap=0, screen_rows=3 ≤ 5, no scroll needed
        assert_eq!(v.scroll_wrap, 0);
    }

    #[test]
    fn test_ensure_visible_wrapped_scrolls_down() {
        // 10 cols text area, terminal 5 text rows
        let mut v = View::new(14, 7); // text_rows=5, text_cols=10
        // Line 0 is 60 chars → 6 wrapped rows. Cursor at col 55 → wrap 5
        let mut widths = |_line: usize| -> usize { 60 };
        v.ensure_cursor_visible(0, 55, 4, &mut widths);
        // Need wrap 5 visible, 5 rows available → scroll_wrap should be 1
        assert_eq!(v.scroll_line, 0);
        assert_eq!(v.scroll_wrap, 1);
    }

    // -- buffer_to_screen ----------------------------------------------------

    #[test]
    fn test_buffer_to_screen_basic() {
        let v = View::new(80, 24);
        let result = v.buffer_to_screen(0, 0, 4, &mut trivial_width);
        assert_eq!(result, Some((0, 4))); // row 0, col 0 + gutter
    }

    #[test]
    fn test_buffer_to_screen_line_above_viewport() {
        let mut v = View::new(80, 24);
        v.scroll_line = 10;
        assert_eq!(v.buffer_to_screen(5, 0, 4, &mut trivial_width), None);
    }

    #[test]
    fn test_buffer_to_screen_line_below_viewport() {
        let v = View::new(80, 24);
        // text_rows = 22, so lines 0..22 are visible
        assert_eq!(v.buffer_to_screen(22, 0, 4, &mut trivial_width), None);
    }

    #[test]
    fn test_buffer_to_screen_no_gutter() {
        let v = View::new(80, 24);
        let result = v.buffer_to_screen(5, 10, 0, &mut trivial_width);
        assert_eq!(result, Some((5, 10)));
    }

    #[test]
    fn test_buffer_to_screen_last_visible_line() {
        let v = View::new(80, 24);
        // text_rows = 22, last visible line is 21
        let result = v.buffer_to_screen(21, 0, 4, &mut trivial_width);
        assert_eq!(result, Some((21, 4)));
    }

    // -- center_on_line -------------------------------------------------------

    #[test]
    fn test_center_on_line_middle() {
        let mut v = View::new(80, 24);
        let mut widths = |_: usize| -> usize { 10 }; // short lines
        v.center_on_line(50, &mut widths, 4);
        // scroll_line should be roughly 50 - 11 = 39
        assert!(v.scroll_line <= 50);
        assert!(v.scroll_line + v.text_rows() > 50);
    }

    #[test]
    fn test_center_on_line_near_start() {
        let mut v = View::new(80, 24);
        let mut widths = |_: usize| -> usize { 10 };
        v.center_on_line(3, &mut widths, 4);
        assert_eq!(v.scroll_line, 0);
    }

    #[test]
    fn test_center_on_line_zero_rows() {
        let mut v = View::new(80, 2); // text_rows = 0
        let mut widths = |_: usize| -> usize { 10 };
        v.center_on_line(10, &mut widths, 4);
        assert_eq!(v.scroll_line, 0);
    }

    #[test]
    fn test_center_on_line_zero_text_cols() {
        let mut v = View::new(4, 24); // text_cols = 0 with gutter 4
        let mut widths = |_: usize| -> usize { 10 };
        v.center_on_line(10, &mut widths, 4);
        assert_eq!(v.scroll_line, 0);
    }

    #[test]
    fn test_center_on_line_wrapped_target() {
        // Target line is very wide → wraps, center_on_line should still work
        let mut v = View::new(14, 7); // text_rows=5, text_cols=10
        let mut widths = |line: usize| -> usize { if line == 50 { 100 } else { 10 } };
        v.center_on_line(50, &mut widths, 4);
        assert!(v.scroll_line <= 50);
    }

    // -- scroll_forward -------------------------------------------------------

    #[test]
    fn test_scroll_forward_multi_line() {
        let mut v = View::new(80, 24);
        let mut widths = |_: usize| -> usize { 10 };
        v.scroll_forward(3, 76, &mut widths);
        assert_eq!(v.scroll_line, 3);
        assert_eq!(v.scroll_wrap, 0);
    }

    #[test]
    fn test_scroll_forward_wrapping() {
        let mut v = View::new(14, 24); // text_cols = 10
        // Line 0 has 25 display cols → 3 wraps
        let mut widths = |_: usize| -> usize { 25 };
        v.scroll_forward(2, 10, &mut widths);
        assert_eq!(v.scroll_line, 0);
        assert_eq!(v.scroll_wrap, 2);
    }

    // -- buffer_to_screen edge cases -----------------------------------------

    #[test]
    fn test_buffer_to_screen_zero_text_cols() {
        let v = View::new(4, 24); // text_cols = 0 with gutter 4
        assert_eq!(v.buffer_to_screen(0, 0, 4, &mut trivial_width), None);
    }

    #[test]
    fn test_buffer_to_screen_scroll_wrap_above() {
        let mut v = View::new(14, 24); // text_cols=10
        v.scroll_line = 0;
        v.scroll_wrap = 2; // scrolled past first 2 wrap rows
        // Col 5 is on wrap 0, which is before scroll_wrap → should return None
        assert_eq!(v.buffer_to_screen(0, 5, 4, &mut |_| 30), None);
    }

    #[test]
    fn test_buffer_to_screen_col_beyond_width() {
        let v = View::new(10, 24); // width = 10, text_cols = 6 with gutter 4
        // col 8 → wrap 1, screen_col = 8%6 + 4 = 6, within width 10
        let result = v.buffer_to_screen(0, 8, 4, &mut trivial_width);
        assert_eq!(result, Some((1, 6)));
        // screen_col = col % text_cols + gutter_width. If that >= width, returns None
        // With text_cols=6, the max screen_col is 4+5=9 < 10, so it never exceeds.
        // Test a case where col would land out of viewport rows instead
        let v_small = View::new(10, 4); // text_rows = 2
        let result = v_small.buffer_to_screen(0, 20, 4, &mut trivial_width);
        // wrap 20/6 = 3, screen_row = 3 >= text_rows(2) → None
        assert_eq!(result, None);
    }

    #[test]
    fn test_center_on_line_partial_wrap() {
        // Triggers the partial wrap branch (lines 158-161) in center_on_line
        let mut v = View::new(14, 14); // text_rows=12, text_cols=10 (with gutter 4)
        // half = 6, target = 5 with width 10 → 1 wrap → remaining = 5
        // Lines 4..1 each have 1 wrap → remaining = 1
        // Line 0 has 100 display cols → 10 wraps. w=10 > remaining=1 → partial
        let mut widths = |line: usize| -> usize { if line == 0 { 100 } else { 10 } };
        v.center_on_line(5, &mut widths, 4);
        // start_line should be 0 with start_wrap = 10 - 1 = 9
        assert_eq!(v.scroll_line, 0);
        assert_eq!(v.scroll_wrap, 9);
    }

    #[test]
    fn test_buffer_to_screen_wrapped() {
        // Line 0 is 20 chars wide, text_cols=10 → 2 wrapped rows
        let v = View::new(14, 24); // text_cols=10 with gutter 4
        let mut widths = |_line: usize| -> usize { 20 };
        // col 5 → wrap 0, screen col = 5 + 4 = 9
        assert_eq!(v.buffer_to_screen(0, 5, 4, &mut widths), Some((0, 9)));
        // col 12 → wrap 1, screen row 1, screen col = 2 + 4 = 6
        assert_eq!(v.buffer_to_screen(0, 12, 4, &mut widths), Some((1, 6)));
    }
}
