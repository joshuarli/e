use std::sync::Arc;

use crate::buffer::GapBuffer;
use crate::language::{self, Language};
use crate::operation::{UndoOperation, UndoStack};
use crate::selection::{CaretSnapshot, Selection, TextPosition};

/// Wraps a GapBuffer with undo/redo and dirty tracking.
pub struct Document {
    pub buffer: GapBuffer,
    pub undo_stack: UndoStack,
    pub is_dirty: bool,
    pub file_path: Option<String>,
}

pub struct TextEdit {
    pub start_byte: usize,
    pub end_byte: usize,
    pub inserted_bytes: Vec<u8>,
    pub deleted_bytes: Vec<u8>,
}

impl Document {
    fn snapshot_for_position(pos: TextPosition) -> CaretSnapshot {
        CaretSnapshot {
            selections: vec![Selection::caret(pos)],
            primary: 0,
        }
    }

    pub fn new(text: Vec<u8>, file_path: Option<String>) -> Self {
        let buffer = if text.is_empty() {
            GapBuffer::new()
        } else {
            GapBuffer::from_bytes(text) // takes ownership, no copy
        };
        Self {
            buffer,
            undo_stack: UndoStack::new(),
            is_dirty: false,
            file_path,
        }
    }

    /// Insert bytes at (line, column), recording an undo operation.
    pub fn insert(&mut self, line: usize, column: usize, bytes: &[u8]) -> TextPosition {
        let offset = self.buffer.position_to_byte_offset(line, column);
        let cursor_before = TextPosition::new(line, column);
        self.buffer.insert(offset, bytes);
        let (new_line, new_col) = self.buffer.byte_offset_to_position(offset + bytes.len());
        let cursor_after = TextPosition::new(new_line, new_col);

        self.undo_stack.record(
            UndoOperation::Insert {
                byte_offset: offset,
                bytes: Arc::from(bytes),
            },
            Self::snapshot_for_position(cursor_before),
            Self::snapshot_for_position(cursor_after),
        );
        self.is_dirty = true;
        cursor_after
    }

    /// Delete `count` bytes starting at byte offset, recording an undo operation.
    pub fn delete_range(&mut self, start_pos: TextPosition, end_pos: TextPosition) -> TextPosition {
        let start_offset = self
            .buffer
            .position_to_byte_offset(start_pos.line, start_pos.column);
        let end_offset = self
            .buffer
            .position_to_byte_offset(end_pos.line, end_pos.column);
        if start_offset >= end_offset {
            return start_pos;
        }
        let deleted = self.buffer.slice(start_offset, end_offset);
        self.buffer.delete(start_offset, end_offset - start_offset);

        self.undo_stack.record(
            UndoOperation::Delete {
                byte_offset: start_offset,
                bytes: Arc::from(deleted.as_slice()),
            },
            Self::snapshot_for_position(end_pos),
            Self::snapshot_for_position(start_pos),
        );
        self.is_dirty = true;
        start_pos
    }

    /// Replace a range when its deleted bytes have already been extracted.
    /// This avoids copying the same deleted range a second time for undo.
    pub fn replace_range_with_deleted(
        &mut self,
        start_pos: TextPosition,
        end_pos: TextPosition,
        replacement: &[u8],
        deleted_bytes: Vec<u8>,
    ) -> TextPosition {
        let start_offset = self
            .buffer
            .position_to_byte_offset(start_pos.line, start_pos.column);
        let end_offset = self
            .buffer
            .position_to_byte_offset(end_pos.line, end_pos.column);
        if start_offset >= end_offset {
            return start_pos;
        }
        debug_assert_eq!(deleted_bytes.len(), end_offset - start_offset);

        self.buffer.delete(start_offset, end_offset - start_offset);
        self.undo_stack.record(
            UndoOperation::Delete {
                byte_offset: start_offset,
                bytes: Arc::from(deleted_bytes),
            },
            Self::snapshot_for_position(end_pos),
            Self::snapshot_for_position(start_pos),
        );

        self.buffer.insert(start_offset, replacement);
        let cursor_after = self
            .buffer
            .byte_offset_to_position(start_offset + replacement.len());
        self.undo_stack.record(
            UndoOperation::Insert {
                byte_offset: start_offset,
                bytes: Arc::from(replacement),
            },
            Self::snapshot_for_position(start_pos),
            Self::snapshot_for_position(TextPosition::new(cursor_after.0, cursor_after.1)),
        );
        self.is_dirty = true;
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
    pub fn undo(&mut self) -> Option<CaretSnapshot> {
        let carets = self.undo_stack.undo(|op| match op {
            UndoOperation::Insert { byte_offset, bytes } => {
                self.buffer.delete(*byte_offset, bytes.len())
            }
            UndoOperation::Delete { byte_offset, bytes } => {
                self.buffer.insert(*byte_offset, bytes.as_ref())
            }
        })?;
        self.is_dirty = true;
        Some(carets)
    }

    /// Redo the last undone group. Returns new cursor position.
    pub fn redo(&mut self) -> Option<CaretSnapshot> {
        let carets = self.undo_stack.redo(|op| match op {
            UndoOperation::Insert { byte_offset, bytes } => {
                self.buffer.insert(*byte_offset, bytes.as_ref())
            }
            UndoOperation::Delete { byte_offset, bytes } => {
                self.buffer.delete(*byte_offset, bytes.len())
            }
        })?;
        self.is_dirty = true;
        Some(carets)
    }

    /// Insert bytes at a raw byte offset (avoids line-cache lookups).
    /// `cursor_before`/`cursor_after` are recorded for undo.
    pub fn insert_at_byte(
        &mut self,
        offset: usize,
        bytes: &[u8],
        cursor_before: TextPosition,
        cursor_after: TextPosition,
    ) {
        self.buffer.insert(offset, bytes);
        self.undo_stack.record(
            UndoOperation::Insert {
                byte_offset: offset,
                bytes: Arc::from(bytes),
            },
            Self::snapshot_for_position(cursor_before),
            Self::snapshot_for_position(cursor_after),
        );
        self.is_dirty = true;
    }

    /// Delete bytes at a raw byte offset (avoids line-cache lookups).
    /// `cursor_before`/`cursor_after` are recorded for undo.
    pub fn delete_at_byte(
        &mut self,
        offset: usize,
        count: usize,
        cursor_before: TextPosition,
        cursor_after: TextPosition,
    ) {
        let deleted = self.buffer.slice(offset, offset + count);
        self.buffer.delete(offset, count);
        self.undo_stack.record(
            UndoOperation::Delete {
                byte_offset: offset,
                bytes: Arc::from(deleted.as_slice()),
            },
            Self::snapshot_for_position(cursor_before),
            Self::snapshot_for_position(cursor_after),
        );
        self.is_dirty = true;
    }

    pub fn insert_at_byte_with_carets(
        &mut self,
        offset: usize,
        bytes: &[u8],
        carets_before: CaretSnapshot,
        carets_after: CaretSnapshot,
    ) {
        self.buffer.insert(offset, bytes);
        self.undo_stack.record(
            UndoOperation::Insert {
                byte_offset: offset,
                bytes: Arc::from(bytes),
            },
            carets_before,
            carets_after,
        );
        self.is_dirty = true;
    }

    pub fn delete_at_byte_with_carets(
        &mut self,
        offset: usize,
        count: usize,
        carets_before: CaretSnapshot,
        carets_after: CaretSnapshot,
    ) {
        let deleted = self.buffer.slice(offset, offset + count);
        self.buffer.delete(offset, count);
        self.undo_stack.record(
            UndoOperation::Delete {
                byte_offset: offset,
                bytes: Arc::from(deleted.as_slice()),
            },
            carets_before,
            carets_after,
        );
        self.is_dirty = true;
    }

    pub fn apply_batch(
        &mut self,
        edits: &[TextEdit],
        carets_before: &CaretSnapshot,
        carets_after_offsets: &[(usize, usize)],
        primary: usize,
    ) -> CaretSnapshot {
        if edits.is_empty() {
            return carets_before.clone();
        }
        debug_assert_eq!(carets_before.selections.len(), carets_after_offsets.len());
        debug_assert!(
            edits
                .windows(2)
                .all(|window| window[0].end_byte <= window[1].start_byte)
        );

        for edit in edits.iter().rev() {
            if edit.end_byte > edit.start_byte {
                self.buffer
                    .delete(edit.start_byte, edit.end_byte - edit.start_byte);
            }
            if !edit.inserted_bytes.is_empty() {
                self.buffer.insert(edit.start_byte, &edit.inserted_bytes);
            }
        }

        let carets_after = CaretSnapshot {
            selections: carets_after_offsets
                .iter()
                .map(|(anchor, cursor)| {
                    let (anchor_line, anchor_col) = self.buffer.byte_offset_to_position(*anchor);
                    let (cursor_line, cursor_col) = self.buffer.byte_offset_to_position(*cursor);
                    Selection {
                        anchor: TextPosition::new(anchor_line, anchor_col),
                        cursor: TextPosition::new(cursor_line, cursor_col),
                    }
                })
                .collect(),
            primary,
        };

        self.undo_stack.begin_group();
        for edit in edits.iter().rev() {
            if !edit.deleted_bytes.is_empty() {
                self.undo_stack.record(
                    UndoOperation::Delete {
                        byte_offset: edit.start_byte,
                        bytes: Arc::from(edit.deleted_bytes.as_slice()),
                    },
                    carets_before.clone(),
                    carets_after.clone(),
                );
            }
            if !edit.inserted_bytes.is_empty() {
                self.undo_stack.record(
                    UndoOperation::Insert {
                        byte_offset: edit.start_byte,
                        bytes: Arc::from(edit.inserted_bytes.as_slice()),
                    },
                    carets_before.clone(),
                    carets_after.clone(),
                );
            }
        }
        self.undo_stack.end_group();
        self.is_dirty = true;
        carets_after
    }

    /// Detect language from filename, falling back to shebang on the first line.
    pub fn detect_language(&self) -> Option<Language> {
        self.file_path
            .as_deref()
            .and_then(language::detect)
            .or_else(|| language::detect_from_shebang(&self.buffer.line_text(0)))
    }

    /// Get text in a range (for clipboard, etc.).
    pub fn text_in_range(&mut self, start: TextPosition, end: TextPosition) -> Vec<u8> {
        let start_offset = self
            .buffer
            .position_to_byte_offset(start.line, start.column);
        let end_offset = self.buffer.position_to_byte_offset(end.line, end.column);
        self.buffer.slice(start_offset, end_offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(pos: TextPosition) -> CaretSnapshot {
        CaretSnapshot {
            selections: vec![Selection::caret(pos)],
            primary: 0,
        }
    }

    #[test]
    fn test_new_empty() {
        let doc = Document::new(Vec::new(), None);
        assert!(!doc.is_dirty);
        assert!(doc.file_path.is_none());
        assert_eq!(doc.buffer.line_count(), 1);
    }

    #[test]
    fn test_new_with_text() {
        let doc = Document::new(b"hello\nworld".to_vec(), Some("test.txt".to_string()));
        assert!(!doc.is_dirty);
        assert_eq!(doc.file_path.as_deref(), Some("test.txt"));
        assert_eq!(doc.buffer.line_count(), 2);
    }

    #[test]
    fn test_insert_sets_dirty() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        assert!(!doc.is_dirty);
        doc.insert(0, 5, b" world");
        assert!(doc.is_dirty);
    }

    #[test]
    fn test_insert_returns_cursor() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        let pos = doc.insert(0, 5, b" world");
        assert_eq!(pos, TextPosition::new(0, 11));
    }

    #[test]
    fn test_insert_newline_moves_cursor_to_next_line() {
        let mut doc = Document::new(b"helloworld".to_vec(), None);
        let pos = doc.insert(0, 5, b"\n");
        assert_eq!(pos, TextPosition::new(1, 0));
        assert_eq!(doc.buffer.line_count(), 2);
        assert_eq!(doc.buffer.line_text(0), b"hello");
        assert_eq!(doc.buffer.line_text(1), b"world");
    }

    #[test]
    fn test_delete_range_sets_dirty() {
        let mut doc = Document::new(b"hello world".to_vec(), None);
        doc.delete_range(TextPosition::new(0, 5), TextPosition::new(0, 11));
        assert!(doc.is_dirty);
    }

    #[test]
    fn test_delete_range_returns_start() {
        let mut doc = Document::new(b"hello world".to_vec(), None);
        let pos = doc.delete_range(TextPosition::new(0, 5), TextPosition::new(0, 11));
        assert_eq!(pos, TextPosition::new(0, 5));
        assert_eq!(doc.buffer.contents(), b"hello");
    }

    #[test]
    fn test_delete_range_across_lines() {
        let mut doc = Document::new(b"hello\nworld".to_vec(), None);
        let pos = doc.delete_range(TextPosition::new(0, 3), TextPosition::new(1, 2));
        assert_eq!(pos, TextPosition::new(0, 3));
        assert_eq!(doc.buffer.contents(), b"helrld");
    }

    #[test]
    fn test_delete_range_noop_when_equal() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        let pos = doc.delete_range(TextPosition::new(0, 3), TextPosition::new(0, 3));
        assert_eq!(pos, TextPosition::new(0, 3));
        assert!(!doc.is_dirty);
    }

    #[test]
    fn test_undo_insert() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        doc.insert(0, 5, b" world");
        assert_eq!(doc.buffer.contents(), b"hello world");

        let carets = doc.undo().unwrap();
        assert_eq!(carets, snap(TextPosition::new(0, 5)));
        assert_eq!(doc.buffer.contents(), b"hello");
    }

    #[test]
    fn test_undo_delete() {
        let mut doc = Document::new(b"hello world".to_vec(), None);
        doc.delete_range(TextPosition::new(0, 5), TextPosition::new(0, 11));
        assert_eq!(doc.buffer.contents(), b"hello");

        let carets = doc.undo().unwrap();
        assert_eq!(carets, snap(TextPosition::new(0, 11)));
        assert_eq!(doc.buffer.contents(), b"hello world");
    }

    #[test]
    fn test_redo() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        doc.insert(0, 5, b" world");
        doc.undo();
        assert_eq!(doc.buffer.contents(), b"hello");

        let carets = doc.redo().unwrap();
        assert_eq!(carets, snap(TextPosition::new(0, 11)));
        assert_eq!(doc.buffer.contents(), b"hello world");
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
        assert_eq!(doc.buffer.contents(), b"abc");

        doc.undo();
        assert_eq!(doc.buffer.contents(), b"ab");
        doc.undo();
        assert_eq!(doc.buffer.contents(), b"a");

        doc.redo();
        assert_eq!(doc.buffer.contents(), b"ab");
        doc.redo();
        assert_eq!(doc.buffer.contents(), b"abc");
    }

    #[test]
    fn test_seal_undo_creates_separate_groups() {
        let mut doc = Document::new(b"".to_vec(), None);
        doc.insert(0, 0, b"a");
        doc.seal_undo();
        doc.insert(0, 1, b"b");
        doc.seal_undo();
        assert_eq!(doc.buffer.contents(), b"ab");

        // Undo should only undo "b"
        doc.undo();
        assert_eq!(doc.buffer.contents(), b"a");

        // Undo "a"
        doc.undo();
        assert_eq!(doc.buffer.contents(), b"");
    }

    #[test]
    fn test_text_in_range() {
        let mut doc = Document::new(b"hello\nworld\nfoo".to_vec(), None);
        let text = doc.text_in_range(TextPosition::new(0, 2), TextPosition::new(1, 3));
        assert_eq!(text, b"llo\nwor");
    }

    #[test]
    fn test_text_in_range_single_line() {
        let mut doc = Document::new(b"hello world".to_vec(), None);
        let text = doc.text_in_range(TextPosition::new(0, 6), TextPosition::new(0, 11));
        assert_eq!(text, b"world");
    }

    #[test]
    fn test_text_in_range_empty() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        let text = doc.text_in_range(TextPosition::new(0, 3), TextPosition::new(0, 3));
        assert_eq!(text, b"");
    }

    #[test]
    fn test_begin_end_undo_group() {
        let mut doc = Document::new(b"hello".to_vec(), None);
        doc.begin_undo_group();
        doc.insert(0, 0, b"// ");
        doc.insert(0, 8, b"\n");
        doc.end_undo_group();

        assert_eq!(doc.buffer.contents(), b"// hello\n");
        // Undo should revert both ops at once
        doc.undo();
        assert_eq!(doc.buffer.contents(), b"hello");
    }

    #[test]
    fn test_text_in_range_full_document() {
        let mut doc = Document::new(b"hello\nworld".to_vec(), None);
        let text = doc.text_in_range(TextPosition::new(0, 0), TextPosition::new(1, 5));
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

    fn clamp_pos(doc: &Document, line_frac: f64, col_frac: f64) -> TextPosition {
        let lc = doc.buffer.line_count();
        let line = (line_frac.abs().fract() * lc as f64) as usize % lc;
        let char_len = doc.buffer.line_character_count(line);
        let column = if char_len == 0 {
            0
        } else {
            (col_frac.abs().fract() * (char_len + 1) as f64) as usize % (char_len + 1)
        };
        TextPosition::new(line, column)
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
                        doc.insert(pos.line, pos.column, data);
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
                let lc = doc.buffer.line_count();
                prop_assert!(lc >= 1);
                prop_assert_eq!(doc.buffer.line_start(0), 0);
                prop_assert_eq!(doc.buffer.line_end(lc - 1), doc.buffer.len());
                prop_assert_eq!(doc.buffer.len(), doc.buffer.contents().len());
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
            let original = doc.buffer.contents();

            for (l, c, data) in &inserts {
                doc.seal_undo();
                let pos = clamp_pos(&doc, *l, *c);
                doc.insert(pos.line, pos.column, data);
            }
            doc.seal_undo();

            // Undo everything
            let mut undo_count = 0;
            while doc.undo().is_some() {
                undo_count += 1;
                prop_assert!(undo_count <= inserts.len() + 1, "infinite undo loop");
            }

            prop_assert_eq!(doc.buffer.contents(), original);
        }
    }
}
