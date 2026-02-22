/// Viewport: tracks scroll offsets and maps cursor position to screen coordinates.
pub struct View {
    /// First visible line (0-indexed logical line).
    pub scroll_line: usize,
    /// Horizontal scroll offset in characters (for long lines).
    pub scroll_col: usize,
    /// Terminal width in columns.
    pub width: u16,
    /// Terminal height in rows.
    pub height: u16,
}

/// How many lines from top/bottom before we start scrolling.
const SCROLL_MARGIN: usize = 5;

impl View {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            scroll_line: 0,
            scroll_col: 0,
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

    /// Adjust scroll so that the cursor line is visible with margin.
    pub fn ensure_cursor_visible(
        &mut self,
        cursor_line: usize,
        cursor_col: usize,
        gutter_width: usize,
    ) {
        let rows = self.text_rows();
        if rows == 0 {
            return;
        }
        let margin = SCROLL_MARGIN.min(rows / 2);

        // Vertical
        if cursor_line < self.scroll_line + margin {
            self.scroll_line = cursor_line.saturating_sub(margin);
        }
        if cursor_line >= self.scroll_line + rows - margin {
            self.scroll_line = cursor_line.saturating_sub(rows - margin - 1);
        }

        // Horizontal
        let text_cols = self.text_cols(gutter_width);
        if text_cols == 0 {
            return;
        }
        if cursor_col < self.scroll_col {
            self.scroll_col = cursor_col;
        }
        if cursor_col >= self.scroll_col + text_cols {
            self.scroll_col = cursor_col - text_cols + 1;
        }
    }

    /// Convert a buffer (line, col) to screen (row, col). Returns None if off-screen.
    #[allow(dead_code)]
    pub fn buffer_to_screen(
        &self,
        line: usize,
        col: usize,
        gutter_width: usize,
    ) -> Option<(u16, u16)> {
        if line < self.scroll_line || line >= self.scroll_line + self.text_rows() {
            return None;
        }
        if col < self.scroll_col {
            return None;
        }
        let screen_col = col - self.scroll_col + gutter_width;
        if screen_col >= self.width as usize {
            return None;
        }
        let screen_row = line - self.scroll_line;
        Some((screen_row as u16, screen_col as u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- new / basic accessors ------------------------------------------------

    #[test]
    fn test_new() {
        let v = View::new(80, 24);
        assert_eq!(v.scroll_line, 0);
        assert_eq!(v.scroll_col, 0);
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

    // -- ensure_cursor_visible -----------------------------------------------

    #[test]
    fn test_ensure_visible_cursor_already_visible() {
        let mut v = View::new(80, 24);
        v.ensure_cursor_visible(10, 5, 4);
        // Cursor at line 10 is within 0..22 range, should scroll down to maintain margin
        assert!(v.scroll_line <= 10);
        assert!(v.scroll_line + v.text_rows() > 10);
    }

    #[test]
    fn test_ensure_visible_scrolls_down() {
        let mut v = View::new(80, 24);
        v.ensure_cursor_visible(30, 0, 4);
        // Cursor at line 30 must be visible
        assert!(v.scroll_line + v.text_rows() > 30);
        assert!(v.scroll_line <= 30);
    }

    #[test]
    fn test_ensure_visible_scrolls_up() {
        let mut v = View::new(80, 24);
        v.scroll_line = 50;
        v.ensure_cursor_visible(10, 0, 4);
        // Should scroll up so line 10 is visible
        assert!(v.scroll_line <= 10);
    }

    #[test]
    fn test_ensure_visible_horizontal_right() {
        let mut v = View::new(80, 24);
        v.ensure_cursor_visible(0, 100, 4);
        // Cursor at col 100 should cause horizontal scroll
        assert!(v.scroll_col > 0);
        assert!(v.scroll_col + v.text_cols(4) > 100);
    }

    #[test]
    fn test_ensure_visible_horizontal_left() {
        let mut v = View::new(80, 24);
        v.scroll_col = 50;
        v.ensure_cursor_visible(0, 10, 4);
        // Should scroll left to show col 10
        assert!(v.scroll_col <= 10);
    }

    #[test]
    fn test_ensure_visible_zero_rows() {
        let mut v = View::new(80, 2); // text_rows = 0
        v.ensure_cursor_visible(10, 0, 4);
        // Should return early without panic
        assert_eq!(v.scroll_line, 0);
    }

    #[test]
    fn test_ensure_visible_zero_text_cols() {
        let mut v = View::new(5, 24);
        v.ensure_cursor_visible(0, 100, 10); // gutter > width
        // Should handle gracefully (text_cols = 0)
        assert_eq!(v.scroll_col, 0);
    }

    #[test]
    fn test_ensure_visible_margin_respected() {
        let mut v = View::new(80, 24);
        // Place cursor near bottom of visible area
        v.ensure_cursor_visible(16, 0, 4);
        // Margin is 5, text_rows is 22. cursor at 16 should be fine at scroll_line=0
        // 16 < 0 + 22 - 5 = 17, so no scroll needed
        assert_eq!(v.scroll_line, 0);

        // Now place cursor at line 17 — right at the margin
        v.ensure_cursor_visible(17, 0, 4);
        // 17 >= 0 + 22 - 5 = 17, so should scroll
        assert!(v.scroll_line > 0);
    }

    #[test]
    fn test_ensure_visible_at_line_zero() {
        let mut v = View::new(80, 24);
        v.scroll_line = 10;
        v.ensure_cursor_visible(0, 0, 4);
        assert_eq!(v.scroll_line, 0);
    }

    // -- buffer_to_screen ----------------------------------------------------

    #[test]
    fn test_buffer_to_screen_basic() {
        let v = View::new(80, 24);
        let result = v.buffer_to_screen(0, 0, 4);
        assert_eq!(result, Some((0, 4))); // row 0, col 0 + gutter
    }

    #[test]
    fn test_buffer_to_screen_with_scroll() {
        let mut v = View::new(80, 24);
        v.scroll_line = 10;
        v.scroll_col = 5;
        let result = v.buffer_to_screen(12, 8, 4);
        assert_eq!(result, Some((2, 7))); // row 12-10=2, col 8-5+4=7
    }

    #[test]
    fn test_buffer_to_screen_line_above_viewport() {
        let mut v = View::new(80, 24);
        v.scroll_line = 10;
        assert_eq!(v.buffer_to_screen(5, 0, 4), None);
    }

    #[test]
    fn test_buffer_to_screen_line_below_viewport() {
        let v = View::new(80, 24);
        // text_rows = 22, so lines 0..22 are visible
        assert_eq!(v.buffer_to_screen(22, 0, 4), None);
    }

    #[test]
    fn test_buffer_to_screen_col_left_of_scroll() {
        let mut v = View::new(80, 24);
        v.scroll_col = 10;
        assert_eq!(v.buffer_to_screen(0, 5, 4), None);
    }

    #[test]
    fn test_buffer_to_screen_col_past_width() {
        let v = View::new(80, 24);
        // col 200 + gutter 4 = 204 >= width 80
        assert_eq!(v.buffer_to_screen(0, 200, 4), None);
    }

    #[test]
    fn test_buffer_to_screen_no_gutter() {
        let v = View::new(80, 24);
        let result = v.buffer_to_screen(5, 10, 0);
        assert_eq!(result, Some((5, 10)));
    }

    #[test]
    fn test_buffer_to_screen_last_visible_line() {
        let v = View::new(80, 24);
        // text_rows = 22, last visible line is 21
        let result = v.buffer_to_screen(21, 0, 4);
        assert_eq!(result, Some((21, 4)));
    }

    #[test]
    fn test_buffer_to_screen_edge_of_width() {
        let v = View::new(80, 24);
        // col 75 + gutter 4 = 79, which is < 80 so visible
        let result = v.buffer_to_screen(0, 75, 4);
        assert_eq!(result, Some((0, 79)));

        // col 76 + gutter 4 = 80, which is >= 80 so NOT visible
        let result = v.buffer_to_screen(0, 76, 4);
        assert_eq!(result, None);
    }
}
