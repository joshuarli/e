//! Dependency-free line diffing and unified diff projection.
//!
//! The implementation follows the semantic diff rules used by `fx`: source
//! lines are borrowed, replacements emit removed rows before added rows, and
//! a unified projection retains a small context window around changed hunks.

/// The operation assigned to a source line in a complete diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffOp {
    Equal,
    Add,
    Remove,
}

/// One line in a complete diff, borrowing its text from either input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffLine<'a> {
    pub op: DiffOp,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: &'a [u8],
}

impl DiffLine<'_> {
    /// Whether this row represents a changed source line.
    pub const fn is_changed(self) -> bool {
        !matches!(self.op, DiffOp::Equal)
    }
}

/// The operation assigned to a row in a unified diff projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffPreviewOp {
    Context,
    Addition,
    Deletion,
    Elision,
}

/// Text carried by a unified row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffPreviewText<'a> {
    Source(&'a [u8]),
    Elision,
}

/// One row in a context-limited unified diff projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffPreviewLine<'a> {
    pub op: DiffPreviewOp,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: DiffPreviewText<'a>,
}

/// The default number of unchanged rows retained around a changed hunk.
pub const DEFAULT_CONTEXT_LINES: usize = 2;

/// The default maximum number of source rows emitted by a unified projection.
/// A final elision row may be appended when the source exceeds this limit.
pub const DEFAULT_MAX_DISPLAY_LINES: usize = 6;

const TRAILING_NEWLINE_ADDED: &[u8] = b"(trailing newline added)";
const TRAILING_NEWLINE_REMOVED: &[u8] = b"(trailing newline removed)";

#[derive(Clone, Copy)]
struct SourceLine {
    start: usize,
    end: usize,
    marker: Option<&'static [u8]>,
}

impl SourceLine {
    fn text<'a>(self, source: &'a [u8]) -> &'a [u8] {
        self.marker.unwrap_or(&source[self.start..self.end])
    }
}

/// Reusable storage for complete diffs and unified projections.
///
/// The first call grows the line-index, LCS matrix, inclusion mask, and output
/// vectors supplied by the caller. Reusing both this scratch value and the
/// output vectors avoids per-call allocations for steady-state highlighting.
#[derive(Default)]
pub struct DiffScratch {
    old_lines: Vec<SourceLine>,
    new_lines: Vec<SourceLine>,
    table: Vec<usize>,
    include: Vec<bool>,
}

impl DiffScratch {
    /// Create empty reusable diff storage.
    pub const fn new() -> Self {
        Self {
            old_lines: Vec::new(),
            new_lines: Vec::new(),
            table: Vec::new(),
            include: Vec::new(),
        }
    }

    /// Compute a complete diff into caller-owned output storage.
    pub fn diff_into<'a>(
        &mut self,
        old_text: &'a [u8],
        new_text: &'a [u8],
        output: &mut Vec<DiffLine<'a>>,
    ) {
        split_lines(old_text, &mut self.old_lines);
        split_lines(new_text, &mut self.new_lines);
        append_trailing_newline_marker(
            &mut self.old_lines,
            &mut self.new_lines,
            old_text,
            new_text,
        );

        let old_len = self.old_lines.len();
        let new_len = self.new_lines.len();
        let stride = new_len + 1;
        let cells = (old_len + 1) * stride;
        self.table.resize(cells, 0);
        self.table.fill(0);

        for old_index in 1..=old_len {
            for new_index in 1..=new_len {
                let value = if self.old_lines[old_index - 1].text(old_text)
                    == self.new_lines[new_index - 1].text(new_text)
                {
                    self.table[(old_index - 1) * stride + new_index - 1] + 1
                } else {
                    self.table[old_index * stride + new_index - 1]
                        .max(self.table[(old_index - 1) * stride + new_index])
                };
                self.table[old_index * stride + new_index] = value;
            }
        }

        output.clear();
        let required_output = old_len + new_len;
        if output.capacity() < required_output {
            output.reserve(required_output - output.capacity());
        }
        let mut old_cursor = old_len;
        let mut new_cursor = new_len;
        while old_cursor > 0 || new_cursor > 0 {
            if old_cursor > 0
                && new_cursor > 0
                && self.old_lines[old_cursor - 1].text(old_text)
                    == self.new_lines[new_cursor - 1].text(new_text)
            {
                output.push(DiffLine {
                    op: DiffOp::Equal,
                    old_line: Some(old_cursor as u32),
                    new_line: Some(new_cursor as u32),
                    text: self.old_lines[old_cursor - 1].text(old_text),
                });
                old_cursor -= 1;
                new_cursor -= 1;
            } else if new_cursor > 0
                && (old_cursor == 0
                    || self.table[old_cursor * stride + new_cursor - 1]
                        >= self.table[(old_cursor - 1) * stride + new_cursor])
            {
                output.push(DiffLine {
                    op: DiffOp::Add,
                    old_line: None,
                    new_line: Some(new_cursor as u32),
                    text: self.new_lines[new_cursor - 1].text(new_text),
                });
                new_cursor -= 1;
            } else {
                output.push(DiffLine {
                    op: DiffOp::Remove,
                    old_line: Some(old_cursor as u32),
                    new_line: None,
                    text: self.old_lines[old_cursor - 1].text(old_text),
                });
                old_cursor -= 1;
            }
        }
        output.reverse();
    }

    /// Project a complete diff into caller-owned unified output storage.
    pub fn unified_preview_into<'a>(
        &mut self,
        lines: &[DiffLine<'a>],
        context_lines: usize,
        max_display_lines: usize,
        output: &mut Vec<DiffPreviewLine<'a>>,
    ) {
        let max_display_lines = max_display_lines.max(1);
        self.include.resize(lines.len(), false);
        self.include.fill(false);
        for (index, line) in lines.iter().enumerate() {
            if !line.is_changed() {
                continue;
            }
            let start = index.saturating_sub(context_lines);
            let end = (index + context_lines + 1).min(lines.len());
            self.include[start..end].fill(true);
        }

        output.clear();
        let mut previous_included = false;
        let mut emitted_source_rows = 0;
        for (index, line) in lines.iter().enumerate() {
            if !self.include[index] {
                previous_included = false;
                continue;
            }
            if emitted_source_rows >= max_display_lines {
                output.push(DiffPreviewLine {
                    op: DiffPreviewOp::Elision,
                    old_line: None,
                    new_line: None,
                    text: DiffPreviewText::Elision,
                });
                break;
            }
            if !previous_included && !output.is_empty() {
                output.push(DiffPreviewLine {
                    op: DiffPreviewOp::Elision,
                    old_line: None,
                    new_line: None,
                    text: DiffPreviewText::Elision,
                });
            }
            output.push(preview_line(*line));
            previous_included = true;
            emitted_source_rows += 1;
        }
    }
}

/// Compute a complete line diff using the same LCS tie-breaking as `fx`.
///
/// The inputs must remain alive while the returned rows are used. Empty input
/// has no source rows, while a trailing newline is represented by the same
/// explicit marker used by `fx` when only one side ends in a newline.
pub fn diff<'a>(old_text: &'a [u8], new_text: &'a [u8]) -> Vec<DiffLine<'a>> {
    let mut scratch = DiffScratch::new();
    let mut result = Vec::new();
    scratch.diff_into(old_text, new_text, &mut result);
    result
}

/// Project a complete diff into a context-limited unified view.
///
/// Unchanged rows outside the requested context are omitted. Separate hunks
/// receive an elision row, and once `max_display_lines` source rows have been
/// emitted a final elision row marks the remaining output. A zero-row limit
/// is treated as one source row so the projection always makes progress.
pub fn unified_preview<'a>(
    lines: &[DiffLine<'a>],
    context_lines: usize,
    max_display_lines: usize,
) -> Vec<DiffPreviewLine<'a>> {
    let mut scratch = DiffScratch::new();
    let mut result = Vec::new();
    scratch.unified_preview_into(lines, context_lines, max_display_lines, &mut result);
    result
}

/// Compute and project a complete diff in one call.
pub fn diff_preview<'a>(
    old_text: &'a [u8],
    new_text: &'a [u8],
    context_lines: usize,
    max_display_lines: usize,
) -> Vec<DiffPreviewLine<'a>> {
    let mut scratch = DiffScratch::new();
    let mut lines = Vec::new();
    scratch.diff_into(old_text, new_text, &mut lines);
    let mut result = Vec::new();
    scratch.unified_preview_into(&lines, context_lines, max_display_lines, &mut result);
    result
}

fn preview_line(line: DiffLine<'_>) -> DiffPreviewLine<'_> {
    let op = match line.op {
        DiffOp::Equal => DiffPreviewOp::Context,
        DiffOp::Add => DiffPreviewOp::Addition,
        DiffOp::Remove => DiffPreviewOp::Deletion,
    };
    DiffPreviewLine {
        op,
        old_line: line.old_line,
        new_line: line.new_line,
        text: DiffPreviewText::Source(line.text),
    }
}

fn split_lines(text: &[u8], lines: &mut Vec<SourceLine>) {
    lines.clear();
    let mut start = 0;
    for (index, &byte) in text.iter().enumerate() {
        if byte == b'\n' {
            lines.push(SourceLine {
                start,
                end: index,
                marker: None,
            });
            start = index + 1;
        }
    }
    if start < text.len() {
        lines.push(SourceLine {
            start,
            end: text.len(),
            marker: None,
        });
    }
}

fn append_trailing_newline_marker<'a>(
    old_lines: &mut Vec<SourceLine>,
    new_lines: &mut Vec<SourceLine>,
    old_text: &'a [u8],
    new_text: &'a [u8],
) {
    if old_text.is_empty() || new_text.is_empty() {
        return;
    }
    let old_has_newline = old_text.ends_with(b"\n");
    let new_has_newline = new_text.ends_with(b"\n");
    if old_has_newline == new_has_newline {
        return;
    }
    if old_has_newline {
        old_lines.push(SourceLine {
            start: 0,
            end: 0,
            marker: Some(TRAILING_NEWLINE_REMOVED),
        });
    } else {
        new_lines.push(SourceLine {
            start: 0,
            end: 0,
            marker: Some(TRAILING_NEWLINE_ADDED),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_prefers_removal_before_addition() {
        let lines = diff(b"a\nold\nc\n", b"a\nnew\nc\n");
        assert_eq!(lines[1].op, DiffOp::Remove);
        assert_eq!(lines[1].old_line, Some(2));
        assert_eq!(lines[2].op, DiffOp::Add);
        assert_eq!(lines[2].new_line, Some(2));
    }

    #[test]
    fn trailing_newline_change_is_an_explicit_row() {
        let lines = diff(b"a\n", b"a");
        assert_eq!(lines[1].op, DiffOp::Remove);
        assert_eq!(lines[1].text, TRAILING_NEWLINE_REMOVED);
    }

    #[test]
    fn unified_preview_keeps_context_and_elides_distant_hunks() {
        let old = b"one\ntwo\nold-a\nfour\nfive\nsix\nold-b\neight\n";
        let new = b"one\ntwo\nnew-a\nfour\nfive\nsix\nnew-b\neight\n";
        let lines = diff(old, new);
        let preview = unified_preview(&lines, 1, 6);
        assert!(preview.iter().any(|line| line.op == DiffPreviewOp::Elision));
        assert!(preview.iter().any(|line| line.text == DiffPreviewText::Source(b"new-a")));
    }
}
