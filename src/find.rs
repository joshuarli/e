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
    /// True while browsing find results with up/down arrows.
    pub active: bool,
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
            active: false,
        }
    }

    /// Clear all find state (matches, regex, current match).
    pub fn clear(&mut self) {
        self.matches.clear();
        self.re = None;
        self.current = None;
    }

    /// Update find highlights for a new pattern. Scans viewport and picks
    /// the first match at or after `cursor`.
    pub fn update_highlights(&mut self, pattern: &str, buf: &GapBuffer, view: &View, cursor: Pos) {
        self.matches.clear();
        self.current = None;
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

        self.refresh_viewport_matches(buf, view);

        let in_viewport = self
            .matches
            .iter()
            .find(|(s, _)| *s >= cursor)
            .or_else(|| self.matches.first())
            .copied();
        if let Some(m) = in_viewport {
            self.current = Some(m);
        } else {
            // No match in viewport — search forward through the rest of the file.
            if let Some(re) = self.re.take() {
                self.current = Self::search_forward(buf, &re, cursor);
                self.re = Some(re);
            }
        }
    }

    /// Scan only the current viewport lines and populate `matches`.
    pub fn refresh_viewport_matches(&mut self, buf: &GapBuffer, view: &View) {
        self.matches.clear();
        let re = match self.re.take() {
            Some(r) => r,
            None => return,
        };
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
        self.re = Some(re);
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
                for m in re.find_iter(text) {
                    let start_col = buffer::char_count(&line_buf[..m.start()]);
                    // On the starting line (pass 0) skip matches before from.col.
                    if pass == 0 && line_idx == from.line && start_col < from.col {
                        continue;
                    }
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
            let range: Box<dyn Iterator<Item = usize>> = if pass == 0 {
                Box::new((0..=from.line).rev())
            } else {
                Box::new((0..line_count).rev())
            };
            for line_idx in range {
                buf.line_text_into(line_idx, &mut line_buf);
                let Ok(text) = std::str::from_utf8(&line_buf) else {
                    continue;
                };
                let mut best: Option<(Pos, Pos)> = None;
                for m in re.find_iter(text) {
                    let start_col = buffer::char_count(&line_buf[..m.start()]);
                    let end_col = buffer::char_count(&line_buf[..m.end()]);
                    if pass == 0 && line_idx == from.line && end_col >= from.col {
                        continue;
                    }
                    best = Some((Pos::new(line_idx, start_col), Pos::new(line_idx, end_col)));
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
        let re = self.re.take()?;
        let result = Self::search_forward(buf, &re, cursor);
        self.re = Some(re);
        if let Some(m) = result {
            self.current = Some(m);
        }
        result
    }

    /// Navigate to the previous match. Returns the match position if found.
    pub fn find_prev(&mut self, buf: &GapBuffer, cursor: Pos) -> Option<(Pos, Pos)> {
        let re = self.re.take()?;
        let result = Self::search_backward(buf, &re, cursor);
        self.re = Some(re);
        if let Some(m) = result {
            self.current = Some(m);
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
        current
    }

    /// Format the find status text.
    pub fn status_text(&self) -> String {
        let n = self.matches.len();
        format!(
            "Find: {} ({} match{})",
            self.pattern,
            n,
            if n == 1 { "" } else { "es" }
        )
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
