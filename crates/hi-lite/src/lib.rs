//! Dependency-free syntax highlighting for line-oriented terminal and text UIs.
///
/// The crate owns syntax tokenization and state propagation across lines. It does
/// not depend on an editor buffer or renderer; callers provide one line of bytes
/// at a time and choose their own presentation for the semantic `Kind` values.
mod highlight;
mod diff;
mod language;
mod languages;

pub use diff::{
    DiffLine, DiffOp, DiffPreviewLine, DiffPreviewOp, DiffPreviewText, DiffScratch,
    DEFAULT_CONTEXT_LINES, DEFAULT_MAX_DISPLAY_LINES, diff, diff_preview,
    unified_preview,
};
pub use highlight::{
    Highlighter, Kind, Run, State, TextPosition, byte_kinds_to_char_kinds,
    byte_kinds_to_char_kinds_into, find_bracket_match, find_quote_match, highlight_line_into, runs,
};
pub use language::Language;

/// Scan one XSH line for application-defined type names.
pub fn scan_xsh_type_line(line: &[u8], in_continuation: bool) -> (Vec<Vec<u8>>, bool) {
    languages::xsh::scan_type_line(line, in_continuation)
}

#[cfg(test)]
mod buffer {
    /// Minimal line source used only by the XSH scanner's legacy full-file tests.
    pub struct GapBuffer {
        lines: Vec<Vec<u8>>,
        dirty: usize,
    }

    impl GapBuffer {
        pub fn from_bytes(mut bytes: Vec<u8>) -> Self {
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            let lines = bytes
                .split(|&byte| byte == b'\n')
                .map(ToOwned::to_owned)
                .collect();
            Self {
                lines,
                dirty: usize::MAX,
            }
        }

        pub fn line_count(&self) -> usize {
            self.lines.len().max(1)
        }

        pub fn line_text_into(&self, line: usize, out: &mut Vec<u8>) {
            out.clear();
            if let Some(text) = self.lines.get(line) {
                out.extend_from_slice(text);
            }
        }

        pub fn take_dirty_line(&mut self) -> usize {
            let dirty = self.dirty;
            self.dirty = usize::MAX;
            dirty
        }
    }
}
