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

    /// Adjust scroll so that the cursor line is visible.
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

        // Vertical: scroll just enough to keep cursor on screen
        if cursor_line < self.scroll_line {
            self.scroll_line = cursor_line;
        }
        if cursor_line >= self.scroll_line + rows {
            self.scroll_line = cursor_line - rows + 1;
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

    /// Center the viewport vertically on the given line.
    pub fn center_on_line(&mut self, line: usize) {
        let rows = self.text_rows();
        if rows == 0 {
            return;
        }
        self.scroll_line = line.saturating_sub(rows / 2);
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
        assert_eq!(v.scroll_line, 0); // no scroll needed, line 10 is within 0..22
    }

    #[test]
    fn test_ensure_visible_scrolls_down() {
        let mut v = View::new(80, 24);
        v.ensure_cursor_visible(30, 0, 4);
        // Cursor at line 30, text_rows=22, scroll_line should be 9
        assert_eq!(v.scroll_line, 9);
    }

    #[test]
    fn test_ensure_visible_scrolls_up() {
        let mut v = View::new(80, 24);
        v.scroll_line = 50;
        v.ensure_cursor_visible(10, 0, 4);
        assert_eq!(v.scroll_line, 10);
    }

    #[test]
    fn test_ensure_visible_horizontal_right() {
        let mut v = View::new(80, 24);
        v.ensure_cursor_visible(0, 100, 4);
        assert!(v.scroll_col > 0);
        assert!(v.scroll_col + v.text_cols(4) > 100);
    }

    #[test]
    fn test_ensure_visible_horizontal_left() {
        let mut v = View::new(80, 24);
        v.scroll_col = 50;
        v.ensure_cursor_visible(0, 10, 4);
        assert_eq!(v.scroll_col, 10);
    }

    #[test]
    fn test_ensure_visible_zero_rows() {
        let mut v = View::new(80, 2); // text_rows = 0
        v.ensure_cursor_visible(10, 0, 4);
        assert_eq!(v.scroll_line, 0);
    }

    #[test]
    fn test_ensure_visible_zero_text_cols() {
        let mut v = View::new(5, 24);
        v.ensure_cursor_visible(0, 100, 10);
        assert_eq!(v.scroll_col, 0);
    }

    #[test]
    fn test_ensure_visible_cursor_at_last_row() {
        let mut v = View::new(80, 24);
        // text_rows=22, cursor at line 21 should not scroll
        v.ensure_cursor_visible(21, 0, 4);
        assert_eq!(v.scroll_line, 0);

        // cursor at line 22 should scroll by 1
        v.ensure_cursor_visible(22, 0, 4);
        assert_eq!(v.scroll_line, 1);
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
