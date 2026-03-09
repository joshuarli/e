use std::time::Instant;

use crate::buffer::GapBuffer;
use crate::render::gutter_width;
use crate::selection::Pos;
use crate::view::{self, View};

pub struct MouseState {
    pub last_click_time: Option<Instant>,
    pub last_click_pos: Option<(u16, u16)>,
    pub click_count: u8,
    pub dragging: bool,
}

impl Default for MouseState {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseState {
    pub fn new() -> Self {
        MouseState {
            last_click_time: None,
            last_click_pos: None,
            click_count: 0,
            dragging: false,
        }
    }

    /// Process a mouse press. Returns the click count (1=single, 2=double, 3=triple).
    pub fn press(&mut self, x: u16, y: u16) -> u8 {
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
        self.click_count
    }

    pub fn release(&mut self) {
        self.dragging = false;
    }
}

/// Map terminal screen coordinates (1-indexed) to buffer position.
pub fn screen_to_buffer_pos(x: u16, y: u16, buf: &GapBuffer, view: &View, ruler_on: bool) -> Pos {
    let line_count = buf.line_count();
    let gw = if ruler_on {
        gutter_width(line_count)
    } else {
        0
    };
    let text_cols = view.text_cols(gw);
    if text_cols == 0 {
        return Pos::zero();
    }

    let target_row = (y as usize).saturating_sub(1);
    let click_col = (x as usize).saturating_sub(1).saturating_sub(gw);

    let mut screen_row: usize = 0;
    let mut line_idx = view.scroll_line;
    let first_wrap = view.scroll_wrap;

    while line_idx < line_count {
        let display_width = buf.display_col_at(line_idx, usize::MAX);
        let char_len = buf.line_char_len(line_idx);
        let total_wraps = view::wrapped_rows(display_width, text_cols);
        let start_wrap = if line_idx == view.scroll_line {
            first_wrap
        } else {
            0
        };

        for wrap in start_wrap..total_wraps {
            if screen_row == target_row {
                let display_col = wrap * text_cols + click_col;
                let char_col = buf.char_col_from_display(line_idx, display_col);
                return Pos::new(line_idx, char_col.min(char_len));
            }
            screen_row += 1;
        }

        line_idx += 1;
    }

    let last_line = line_count.saturating_sub(1);
    let last_col = buf.line_char_len(last_line);
    Pos::new(last_line, last_col)
}
