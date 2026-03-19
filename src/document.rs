use std::sync::Arc;

use crate::buffer::GapBuffer;
use crate::language::{self, Language};
use crate::operation::{Operation, UndoStack};
use crate::selection::Pos;

/// Wraps a GapBuffer with undo/redo and dirty tracking.
pub struct Document {
    pub buf: GapBuffer,
    pub undo_stack: UndoStack,
    pub dirty: bool,
    pub filename: Option<String>,
}

impl Document {
    pub fn new(text: Vec<u8>, filename: Option<String>) -> Self {
        let buf = if text.is_empty() {
            GapBuffer::new()
        } else {
            GapBuffer::from_vec(text) // takes ownership, no copy
        };
        Self {
            buf,
            undo_stack: UndoStack::new(),
            dirty: false,
            filename,
        }
    }

    /// Insert bytes at (line, col), recording an undo operation.
    pub fn insert(&mut self, line: usize, col: usize, bytes: &[u8]) -> Pos {
        let offset = self.buf.pos_to_offset(line, col);
        let cursor_before = Pos::new(line, col);
        self.buf.insert(offset, bytes);
        let (new_line, new_col) = self.buf.offset_to_pos(offset + bytes.len());
        let cursor_after = Pos::new(new_line, new_col);

        self.undo_stack.record(
            Operation::Insert {
                pos: offset,
                data: Arc::from(bytes),
            },
            cursor_before,
            cursor_after,
        );
        self.dirty = true;
        cursor_after
    }

    /// Delete `count` bytes starting at byte offset, recording an undo operation.
    pub fn delete_range(&mut self, start_pos: Pos, end_pos: Pos) -> Pos {
        let start_offset = self.buf.pos_to_offset(start_pos.line, start_pos.col);
        let end_offset = self.buf.pos_to_offset(end_pos.line, end_pos.col);
        if start_offset >= end_offset {
            return start_pos;
        }
        let deleted = self.buf.slice(start_offset, end_offset);
        self.buf.delete(start_offset, end_offset - start_offset);

        self.undo_stack.record(
            Operation::Delete {
                pos: start_offset,
                data: Arc::from(deleted.as_slice()),
            },
            end_pos,
            start_pos,
        );
        self.dirty = true;
        start_pos
    }

    /// Seal the current undo group (force a boundary).
    pub fn seal_undo(&mut self) {
        self.undo_stack.seal();
    }

    pub fn begin_undo_group(&mut self) {
        self.undo_stack.begin_group();
    }

    pub fn end_undo_group(&mut self) {
        self.undo_stack.end_group();
    }

    /// Undo the last operation group. Returns new cursor position.
    pub fn undo(&mut self) -> Option<Pos> {
        let cursor = self.undo_stack.undo(|op| match op {
            Operation::Insert { pos, data } => self.buf.delete(*pos, data.len()),
            Operation::Delete { pos, data } => self.buf.insert(*pos, data.as_ref()),
        })?;
        self.dirty = true;
        Some(cursor)
    }

    /// Redo the last undone group. Returns new cursor position.
    pub fn redo(&mut self) -> Option<Pos> {
        let cursor = self.undo_stack.redo(|op| match op {
            Operation::Insert { pos, data } => self.buf.insert(*pos, data.as_ref()),
            Operation::Delete { pos, data } => self.buf.delete(*pos, data.len()),
        })?;
        self.dirty = true;
        Some(cursor)
    }

    /// Insert bytes at a raw byte offset (avoids line-cache lookups).
    /// `cursor_before`/`cursor_after` are recorded for undo.
    pub fn insert_at_byte(
        &mut self,
        offset: usize,
        bytes: &[u8],
        cursor_before: Pos,
        cursor_after: Pos,
    ) {
        self.buf.insert(offset, bytes);
        self.undo_stack.record(
            Operation::Insert {
                pos: offset,
                data: Arc::from(bytes),
            },
            cursor_before,
            cursor_after,
        );
        self.dirty = true;
    }

    /// Delete bytes at a raw byte offset (avoids line-cache lookups).
    /// `cursor_before`/`cursor_after` are recorded for undo.
    pub fn delete_at_byte(
        &mut self,
        offset: usize,
        count: usize,
        cursor_before: Pos,
        cursor_after: Pos,
    ) {
        let deleted = self.buf.slice(offset, offset + count);
        self.buf.delete(offset, count);
        self.undo_stack.record(
            Operation::Delete {
                pos: offset,
                data: Arc::from(deleted.as_slice()),
            },
            cursor_before,
            cursor_after,
        );
        self.dirty = true;
    }

    /// Detect language from filename, falling back to shebang on the first line.
    pub fn detect_language(&self) -> Option<Language> {
        self.filename
            .as_deref()
            .and_then(language::detect)
            .or_else(|| language::detect_from_shebang(&self.buf.line_text(0)))
    }

    /// Get text in a range (for clipboard, etc.).
    pub fn text_in_range(&mut self, start: Pos, end: Pos) -> Vec<u8> {
        let start_offset = self.buf.pos_to_offset(start.line, start.col);
        let end_offset = self.buf.pos_to_offset(end.line, end.col);
        self.buf.slice(start_offset, end_offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_empty() {
        let doc = Document::new(Vec::new(), None);
        assert!(!doc.dirty);
        assert!(doc.filename.is_none());
        assert_eq!(doc.buf.line_count(), 1);
    }

    #[test]
    fn test_new_with_text() {
        let doc = Document::new(b"hello\nworld".to_vec(), Some("test.txt".to_string()));
        assert!(!doc.dirty);
        assert_eq!(doc.filename.as_deref(), Some("test.txt"));
        assert_eq!(doc.buf.line_count(), 2);
    }

    #[test]
    fn test_insert_sets_dirty() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        assert!(!doc.dirty);
        doc.insert(0, 5, b" world");
        assert!(doc.dirty);
    }

    #[test]
    fn test_insert_returns_cursor() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        let pos = doc.insert(0, 5, b" world");
        assert_eq!(pos, Pos::new(0, 11));
    }

    #[test]
    fn test_insert_newline_moves_cursor_to_next_line() {
        let mut doc = Document::new(b"helloworld".to_vec(), None);
        let pos = doc.insert(0, 5, b"\n");
        assert_eq!(pos, Pos::new(1, 0));
        assert_eq!(doc.buf.line_count(), 2);
        assert_eq!(doc.buf.line_text(0), b"hello");
        assert_eq!(doc.buf.line_text(1), b"world");
    }

    #[test]
    fn test_delete_range_sets_dirty() {
        let mut doc = Document::new(b"hello world".to_vec(), None);
        doc.delete_range(Pos::new(0, 5), Pos::new(0, 11));
        assert!(doc.dirty);
    }

    #[test]
    fn test_delete_range_returns_start() {
        let mut doc = Document::new(b"hello world".to_vec(), None);
        let pos = doc.delete_range(Pos::new(0, 5), Pos::new(0, 11));
        assert_eq!(pos, Pos::new(0, 5));
        assert_eq!(doc.buf.contents(), b"hello");
    }

    #[test]
    fn test_delete_range_across_lines() {
        let mut doc = Document::new(b"hello\nworld".to_vec(), None);
        let pos = doc.delete_range(Pos::new(0, 3), Pos::new(1, 2));
        assert_eq!(pos, Pos::new(0, 3));
        assert_eq!(doc.buf.contents(), b"helrld");
    }

    #[test]
    fn test_delete_range_noop_when_equal() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        let pos = doc.delete_range(Pos::new(0, 3), Pos::new(0, 3));
        assert_eq!(pos, Pos::new(0, 3));
        assert!(!doc.dirty);
    }

    #[test]
    fn test_undo_insert() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        doc.insert(0, 5, b" world");
        assert_eq!(doc.buf.contents(), b"hello world");

        let pos = doc.undo().unwrap();
        assert_eq!(pos, Pos::new(0, 5));
        assert_eq!(doc.buf.contents(), b"hello");
    }

    #[test]
    fn test_undo_delete() {
        let mut doc = Document::new(b"hello world".to_vec(), None);
        doc.delete_range(Pos::new(0, 5), Pos::new(0, 11));
        assert_eq!(doc.buf.contents(), b"hello");

        let pos = doc.undo().unwrap();
        assert_eq!(pos, Pos::new(0, 11));
        assert_eq!(doc.buf.contents(), b"hello world");
    }

    #[test]
    fn test_redo() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        doc.insert(0, 5, b" world");
        doc.undo();
        assert_eq!(doc.buf.contents(), b"hello");

        let pos = doc.redo().unwrap();
        assert_eq!(pos, Pos::new(0, 11));
        assert_eq!(doc.buf.contents(), b"hello world");
    }

    #[test]
    fn test_undo_nothing_returns_none() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        assert!(doc.undo().is_none());
    }

    #[test]
    fn test_redo_nothing_returns_none() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        assert!(doc.redo().is_none());
    }

    #[test]
    fn test_redo_cleared_after_new_edit() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        doc.insert(0, 5, b"1");
        doc.undo();
        // Now insert something new — redo should be gone
        doc.insert(0, 5, b"2");
        assert!(doc.redo().is_none());
    }

    #[test]
    fn test_multiple_undo_redo() {
        let mut doc = Document::new(b"a".to_vec(), None);
        doc.seal_undo();
        doc.insert(0, 1, b"b");
        doc.seal_undo();
        doc.insert(0, 2, b"c");
        doc.seal_undo();
        assert_eq!(doc.buf.contents(), b"abc");

        doc.undo();
        assert_eq!(doc.buf.contents(), b"ab");
        doc.undo();
        assert_eq!(doc.buf.contents(), b"a");

        doc.redo();
        assert_eq!(doc.buf.contents(), b"ab");
        doc.redo();
        assert_eq!(doc.buf.contents(), b"abc");
    }

    #[test]
    fn test_seal_undo_creates_separate_groups() {
        let mut doc = Document::new(b"".to_vec(), None);
        doc.insert(0, 0, b"a");
        doc.seal_undo();
        doc.insert(0, 1, b"b");
        doc.seal_undo();
        assert_eq!(doc.buf.contents(), b"ab");

        // Undo should only undo "b"
        doc.undo();
        assert_eq!(doc.buf.contents(), b"a");

        // Undo "a"
        doc.undo();
        assert_eq!(doc.buf.contents(), b"");
    }

    #[test]
    fn test_text_in_range() {
        let mut doc = Document::new(b"hello\nworld\nfoo".to_vec(), None);
        let text = doc.text_in_range(Pos::new(0, 2), Pos::new(1, 3));
        assert_eq!(text, b"llo\nwor");
    }

    #[test]
    fn test_text_in_range_single_line() {
        let mut doc = Document::new(b"hello world".to_vec(), None);
        let text = doc.text_in_range(Pos::new(0, 6), Pos::new(0, 11));
        assert_eq!(text, b"world");
    }

    #[test]
    fn test_text_in_range_empty() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        let text = doc.text_in_range(Pos::new(0, 3), Pos::new(0, 3));
        assert_eq!(text, b"");
    }

    #[test]
    fn test_begin_end_undo_group() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        doc.begin_undo_group();
        doc.insert(0, 0, b"// ");
        doc.insert(0, 8, b"\n");
        doc.end_undo_group();

        assert_eq!(doc.buf.contents(), b"// hello\n");
        // Undo should revert both ops at once
        doc.undo();
        assert_eq!(doc.buf.contents(), b"hello");
    }

    #[test]
    fn test_text_in_range_full_document() {
        let mut doc = Document::new(b"hello\nworld".to_vec(), None);
        let text = doc.text_in_range(Pos::new(0, 0), Pos::new(1, 5));
        assert_eq!(text, b"hello\nworld");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum DocOp {
        Insert {
            line_frac: f64,
            col_frac: f64,
            data: Vec<u8>,
        },
        Delete {
            l1_frac: f64,
            c1_frac: f64,
            l2_frac: f64,
            c2_frac: f64,
        },
        Undo,
        Redo,
        Seal,
    }

    fn arb_doc_op() -> impl Strategy<Value = DocOp> {
        prop_oneof![
            3 => (any::<f64>(), any::<f64>(), prop::collection::vec(any::<u8>(), 1..32))
                .prop_map(|(l, c, d)| DocOp::Insert { line_frac: l, col_frac: c, data: d }),
            2 => (any::<f64>(), any::<f64>(), any::<f64>(), any::<f64>())
                .prop_map(|(l1, c1, l2, c2)| DocOp::Delete {
                    l1_frac: l1, c1_frac: c1, l2_frac: l2, c2_frac: c2
                }),
            2 => Just(DocOp::Undo),
            1 => Just(DocOp::Redo),
            1 => Just(DocOp::Seal),
        ]
    }

    fn clamp_pos(doc: &Document, line_frac: f64, col_frac: f64) -> Pos {
        let lc = doc.buf.line_count();
        let line = (line_frac.abs().fract() * lc as f64) as usize % lc;
        let char_len = doc.buf.line_char_len(line);
        let col = if char_len == 0 {
            0
        } else {
            (col_frac.abs().fract() * (char_len + 1) as f64) as usize % (char_len + 1)
        };
        Pos::new(line, col)
    }

    proptest! {
        /// After any sequence of edits/undos/redos, buffer invariants hold.
        #[test]
        fn document_edit_undo_redo_consistency(
            initial in prop::collection::vec(any::<u8>(), 0..128),
            ops in prop::collection::vec(arb_doc_op(), 0..40),
        ) {
            let mut doc = Document::new(initial, None);

            for op in &ops {
                match op {
                    DocOp::Insert { line_frac, col_frac, data } => {
                        let pos = clamp_pos(&doc, *line_frac, *col_frac);
                        doc.insert(pos.line, pos.col, data);
                    }
                    DocOp::Delete { l1_frac, c1_frac, l2_frac, c2_frac } => {
                        let start = clamp_pos(&doc, *l1_frac, *c1_frac);
                        let end = clamp_pos(&doc, *l2_frac, *c2_frac);
                        if start < end {
                            doc.delete_range(start, end);
                        }
                    }
                    DocOp::Undo => { doc.undo(); }
                    DocOp::Redo => { doc.redo(); }
                    DocOp::Seal => { doc.seal_undo(); }
                }

                // Buffer must be internally consistent after every operation.
                let lc = doc.buf.line_count();
                prop_assert!(lc >= 1);
                prop_assert_eq!(doc.buf.line_start(0), 0);
                prop_assert_eq!(doc.buf.line_end(lc - 1), doc.buf.len());
                prop_assert_eq!(doc.buf.len(), doc.buf.contents().len());
            }
        }

        /// Full undo restores original content.
        #[test]
        fn full_undo_restores_original(
            initial in prop::collection::vec(any::<u8>(), 0..128),
            inserts in prop::collection::vec(
                (any::<f64>(), any::<f64>(), prop::collection::vec(any::<u8>(), 1..16)),
                1..10,
            ),
        ) {
            let mut doc = Document::new(initial.clone(), None);
            let original = doc.buf.contents();

            for (l, c, data) in &inserts {
                doc.seal_undo();
                let pos = clamp_pos(&doc, *l, *c);
                doc.insert(pos.line, pos.col, data);
            }
            doc.seal_undo();

            // Undo everything
            let mut undo_count = 0;
            while doc.undo().is_some() {
                undo_count += 1;
                prop_assert!(undo_count <= inserts.len() + 1, "infinite undo loop");
            }

            prop_assert_eq!(doc.buf.contents(), original);
        }
    }
}
