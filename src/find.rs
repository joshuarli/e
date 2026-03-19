use crate::buffer::{self, GapBuffer};
use crate::selection::Pos;
use crate::view::View;

pub struct FindState {
    pub pattern: String,
    /// Viewport-only matches, repopulated each draw() call.
    pub matches: Vec<(Pos, Pos)>,
    /// Compiled regex cached across keystrokes.
    pub re: Option<regex_lite::Regex>,
    /// The currently-navigated match (start, end).
    pub current: Option<(Pos, Pos)>,
    /// 1-based index of the current match within the file.
    pub current_index: usize,
    /// True while browsing find results with up/down arrows.
    pub active: bool,
    /// Total match count across the entire file.
    pub total_count: usize,
}

impl Default for FindState {
    fn default() -> Self {
        Self::new()
    }
}

impl FindState {
    pub fn new() -> Self {
        FindState {
            pattern: String::new(),
            matches: Vec::new(),
            re: None,
            current: None,
            current_index: 0,
            active: false,
            total_count: 0,
        }
    }

    /// Clear all find state (matches, regex, current match).
    pub fn clear(&mut self) {
        self.matches.clear();
        self.re = None;
        self.current = None;
        self.current_index = 0;
        self.total_count = 0;
    }

    /// Update find highlights for a new pattern. Scans viewport and picks
    /// the first match at or after `cursor`.
    pub fn update_highlights(&mut self, pattern: &str, buf: &GapBuffer, view: &View) {
        self.matches.clear();
        self.current = None;
        self.total_count = 0;
        self.pattern = pattern.to_string();
        if pattern.is_empty() {
            self.re = None;
            return;
        }

        let re = match Self::compile_regex(pattern) {
            Some(r) => r,
            None => {
                self.re = None;
                return;
            }
        };
        self.re = Some(re);

        self.total_count = Self::count_all_matches(self.re.as_ref().unwrap(), buf);
        self.refresh_viewport_matches(buf, view);

        // Always land on the first match in the file.
        if let Some(re) = self.re.as_ref() {
            self.current = Self::search_forward(buf, re, Pos::zero());
        }
        if let Some((start, _)) = self.current {
            self.current_index = Self::match_index(self.re.as_ref().unwrap(), buf, start);
        }
    }

    /// Scan only the current viewport lines and populate `matches`.
    pub fn refresh_viewport_matches(&mut self, buf: &GapBuffer, view: &View) {
        self.matches.clear();
        let Some(re) = self.re.as_ref() else { return };
        let line_count = buf.line_count();
        let viewport_end = (view.scroll_line + view.text_rows() + 4).min(line_count);
        let mut line_buf = Vec::new();
        for line_idx in view.scroll_line..viewport_end {
            buf.line_text_into(line_idx, &mut line_buf);
            let Ok(text) = std::str::from_utf8(&line_buf) else {
                continue;
            };
            for m in re.find_iter(text) {
                let start_col = buffer::char_count(&line_buf[..m.start()]);
                let end_col = buffer::char_count(&line_buf[..m.end()]);
                self.matches
                    .push((Pos::new(line_idx, start_col), Pos::new(line_idx, end_col)));
            }
        }
    }

    /// Return the 1-based index of the match starting at `pos` in the file.
    fn match_index(re: &regex_lite::Regex, buf: &GapBuffer, pos: Pos) -> usize {
        let mut index = 0;
        let mut line_buf = Vec::new();
        for line_idx in 0..=pos.line {
            buf.line_text_into(line_idx, &mut line_buf);
            let Ok(text) = std::str::from_utf8(&line_buf) else {
                continue;
            };
            for m in re.find_iter(text) {
                let col = buffer::char_count(&line_buf[..m.start()]);
                if line_idx == pos.line && col >= pos.col {
                    return index + 1;
                }
                index += 1;
            }
        }
        // Fallback — pos is on the last match of its line
        index
    }

    /// Count all matches in the entire file.
    fn count_all_matches(re: &regex_lite::Regex, buf: &GapBuffer) -> usize {
        let line_count = buf.line_count();
        let mut count = 0;
        let mut line_buf = Vec::new();
        for line_idx in 0..line_count {
            buf.line_text_into(line_idx, &mut line_buf);
            let Ok(text) = std::str::from_utf8(&line_buf) else {
                continue;
            };
            count += re.find_iter(text).count();
        }
        count
    }

    /// Search forward from `from` (inclusive), wrapping around the file.
    pub fn search_forward(
        buf: &GapBuffer,
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
                // On the starting line, search from the byte offset of from.col
                let byte_start = if pass == 0 && line_idx == from.line {
                    buffer::char_to_byte(&line_buf, from.col)
                } else {
                    0
                };
                if let Some(m) = re.find_at(text, byte_start) {
                    let start_col = buffer::char_count(&line_buf[..m.start()]);
                    let end_col = buffer::char_count(&line_buf[..m.end()]);
                    return Some((Pos::new(line_idx, start_col), Pos::new(line_idx, end_col)));
                }
            }
        }
        None
    }

    /// Search backward from `from` (exclusive), wrapping around the file.
    pub fn search_backward(
        buf: &GapBuffer,
        re: &regex_lite::Regex,
        from: Pos,
    ) -> Option<(Pos, Pos)> {
        let line_count = buf.line_count();
        let mut line_buf = Vec::new();
        for pass in 0..2 {
            let (start, end) = if pass == 0 {
                (0, from.line + 1)
            } else {
                (0, line_count)
            };
            for line_idx in (start..end).rev() {
                buf.line_text_into(line_idx, &mut line_buf);
                let Ok(text) = std::str::from_utf8(&line_buf) else {
                    continue;
                };
                // Walk re.find_at() to find the last match on this line
                let mut best: Option<(Pos, Pos)> = None;
                let mut at = 0;
                while let Some(m) = re.find_at(text, at) {
                    let start_col = buffer::char_count(&line_buf[..m.start()]);
                    let end_col = buffer::char_count(&line_buf[..m.end()]);
                    if pass == 0 && line_idx == from.line && end_col >= from.col {
                        break;
                    }
                    best = Some((Pos::new(line_idx, start_col), Pos::new(line_idx, end_col)));
                    // Advance past this match (at least 1 byte to avoid infinite loop)
                    at = if m.end() > m.start() {
                        m.end()
                    } else {
                        m.end() + 1
                    };
                    if at >= text.len() {
                        break;
                    }
                }
                if best.is_some() {
                    return best;
                }
            }
        }
        None
    }

    /// Navigate to the next match. Returns the match position if found.
    pub fn find_next(&mut self, buf: &GapBuffer, cursor: Pos) -> Option<(Pos, Pos)> {
        let re = self.re.as_ref()?;
        let result = Self::search_forward(buf, re, cursor);
        if let Some(m) = result {
            self.current = Some(m);
            self.current_index = Self::match_index(re, buf, m.0);
        }
        result
    }

    /// Navigate to the previous match. Returns the match position if found.
    pub fn find_prev(&mut self, buf: &GapBuffer, cursor: Pos) -> Option<(Pos, Pos)> {
        let re = self.re.as_ref()?;
        let result = Self::search_backward(buf, re, cursor);
        if let Some(m) = result {
            self.current = Some(m);
            self.current_index = Self::match_index(re, buf, m.0);
        }
        result
    }

    /// Exit find mode. Returns the current match (if any) for selection.
    pub fn exit(&mut self) -> Option<(Pos, Pos)> {
        let current = self.current;
        self.active = false;
        self.matches.clear();
        self.re = None;
        self.current = None;
        self.current_index = 0;
        self.total_count = 0;
        current
    }

    /// Format the find status text.
    pub fn status_text(&self) -> String {
        if self.total_count == 0 {
            return format!("Find: {} (no matches)", self.pattern);
        }
        if self.current_index > 0 {
            format!(
                "Find: {} (match {} of {})",
                self.pattern, self.current_index, self.total_count
            )
        } else {
            format!(
                "Find: {} ({} match{})",
                self.pattern,
                self.total_count,
                if self.total_count == 1 { "" } else { "es" }
            )
        }
    }

    /// Compile a regex with smart-case: case-insensitive if pattern is all lowercase.
    fn compile_regex(pattern: &str) -> Option<regex_lite::Regex> {
        let case_insensitive = pattern.chars().all(|c| !c.is_uppercase());
        let result = if case_insensitive {
            regex_lite::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
        } else {
            regex_lite::Regex::new(pattern)
        };
        result.ok()
    }
}
