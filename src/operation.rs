use std::sync::Arc;
use std::time::Instant;

use crate::selection::{CaretSnapshot, TextPosition};

/// A single atomic text change.
#[derive(Debug, Clone)]
pub enum UndoOperation {
    Insert {
        byte_offset: usize, // byte offset
        bytes: Arc<[u8]>,
    },
    Delete {
        byte_offset: usize, // byte offset
        bytes: Arc<[u8]>,   // the deleted bytes (for undo)
    },
}

impl UndoOperation {
    fn kind(&self) -> UndoOperationKind {
        match self {
            UndoOperation::Insert { .. } => UndoOperationKind::Insert,
            UndoOperation::Delete { .. } => UndoOperationKind::Delete,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum UndoOperationKind {
    Insert,
    Delete,
}

/// A group of operations that are undone/redone together.
#[derive(Debug, Clone)]
pub struct UndoGroup {
    pub operations: Vec<UndoOperation>,
    /// Caret layout before the first op in this group.
    pub carets_before: CaretSnapshot,
    /// Caret layout after the last op in this group.
    pub carets_after: CaretSnapshot,
}

/// Undo/redo stack with grouping heuristics.
pub struct UndoStack {
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
    /// Current accumulating group.
    current: Option<UndoGroup>,
    last_operation_kind: Option<UndoOperationKind>,
    last_operation_time: Option<Instant>,
    last_cursor_position: TextPosition,
    /// When true, all recorded operations go into the same group (no heuristic breaks).
    force_group: bool,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            current: None,
            last_operation_kind: None,
            last_operation_time: None,
            last_cursor_position: TextPosition::zero(),
            force_group: false,
        }
    }

    /// Immediately flush the current group, sealing it as a completed undo step.
    pub fn seal(&mut self) {
        self.flush_current();
    }

    /// Begin a forced group: all operations until `end_group` go into one undo step.
    pub fn begin_group(&mut self) {
        self.flush_current();
        self.force_group = true;
    }

    /// End a forced group and flush it as a single undo step.
    pub fn end_group(&mut self) {
        self.force_group = false;
        self.flush_current();
    }

    /// Returns references to the undo and redo stacks (for serialization).
    pub fn stacks(&self) -> (&[UndoGroup], &[UndoGroup]) {
        (&self.undo, &self.redo)
    }

    /// Replace both stacks with restored bytes, resetting all transient state.
    pub fn restore(&mut self, undo: Vec<UndoGroup>, redo: Vec<UndoGroup>) {
        self.undo = undo;
        self.redo = redo;
        self.current = None;
        self.last_operation_kind = None;
        self.last_operation_time = None;
        self.last_cursor_position = TextPosition::zero();
        self.force_group = false;
    }

    /// Record an operation. Determines whether to extend current group or start new.
    pub fn record(
        &mut self,
        op: UndoOperation,
        carets_before: CaretSnapshot,
        carets_after: CaretSnapshot,
    ) {
        // Clear redo on new edit
        self.redo.clear();

        let should_break = self.should_break_group(&op, carets_before.primary_cursor());

        if should_break {
            self.flush_current();
        }

        let group = self.current.get_or_insert_with(|| UndoGroup {
            operations: Vec::new(),
            carets_before,
            carets_after: carets_after.clone(),
        });
        let kind = op.kind();
        group.operations.push(op);
        group.carets_after = carets_after;

        self.last_operation_kind = Some(kind);
        self.last_operation_time = Some(Instant::now());
        self.last_cursor_position = group.carets_after.primary_cursor();
    }

    fn should_break_group(&self, op: &UndoOperation, cursor_before: TextPosition) -> bool {
        if self.current.is_none() {
            return false;
        }

        if self.force_group {
            return false;
        }

        // Kind change
        if self.last_operation_kind.is_some() && self.last_operation_kind != Some(op.kind()) {
            return true;
        }

        // Time gap > 1s
        if let Some(t) = self.last_operation_time
            && t.elapsed().as_millis() > 1000
        {
            return true;
        }

        // Cursor jumped (not contiguous)
        if cursor_before != self.last_cursor_position {
            return true;
        }

        // Word boundary: space after non-space for inserts
        if let UndoOperation::Insert { bytes, .. } = op
            && (bytes.as_ref() == b" " || bytes.as_ref() == b"\n")
        {
            return true;
        }

        false
    }

    fn flush_current(&mut self) {
        if let Some(group) = self.current.take() {
            self.undo.push(group);
        }
    }

    /// Undo the last group. Calls `apply` for each operation in reverse order.
    /// Returns the cursor position to restore.
    pub fn undo(&mut self, mut apply: impl FnMut(&UndoOperation)) -> Option<CaretSnapshot> {
        self.flush_current();
        let group = self.undo.pop()?;
        let carets = group.carets_before.clone();
        for op in group.operations.iter().rev() {
            apply(op);
        }
        self.redo.push(group);
        Some(carets)
    }

    /// Redo the last undone group. Calls `apply` for each operation in order.
    /// Returns the cursor position to restore.
    pub fn redo(&mut self, mut apply: impl FnMut(&UndoOperation)) -> Option<CaretSnapshot> {
        let group = self.redo.pop()?;
        let carets = group.carets_after.clone();
        for op in &group.operations {
            apply(op);
        }
        self.undo.push(group);
        Some(carets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::Selection;

    fn snap(byte_offset: TextPosition) -> CaretSnapshot {
        CaretSnapshot {
            selections: vec![Selection::caret(byte_offset)],
            primary: 0,
        }
    }

    fn ins(byte_offset: usize, bytes: &[u8]) -> UndoOperation {
        UndoOperation::Insert {
            byte_offset,
            bytes: Arc::from(bytes),
        }
    }

    fn del(byte_offset: usize, bytes: &[u8]) -> UndoOperation {
        UndoOperation::Delete {
            byte_offset,
            bytes: Arc::from(bytes),
        }
    }

    #[test]
    fn test_empty_undo_returns_none() {
        let mut stack = UndoStack::new();
        assert!(stack.undo(|_| {}).is_none());
    }

    #[test]
    fn test_empty_redo_returns_none() {
        let mut stack = UndoStack::new();
        assert!(stack.redo(|_| {}).is_none());
    }

    #[test]
    fn test_record_and_undo() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        let mut count = 0;
        let cursor = stack.undo(|_| count += 1).unwrap();
        assert_eq!(cursor, snap(TextPosition::new(0, 0)));
        assert_eq!(count, 1);
    }

    #[test]
    fn test_undo_then_redo() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.undo(|_| {});
        let mut count = 0;
        let cursor = stack.redo(|_| count += 1).unwrap();
        assert_eq!(cursor, snap(TextPosition::new(0, 1)));
        assert_eq!(count, 1);
    }

    #[test]
    fn test_new_record_clears_redo() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.undo(|_| {});
        // New edit should clear redo
        stack.record(
            ins(0, b"b"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        assert!(stack.redo(|_| {}).is_none());
    }

    #[test]
    fn test_seal_forces_new_group() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.seal();
        stack.record(
            ins(1, b"b"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 2)),
        );

        // Undo should only undo "b"
        let mut count = 0;
        let cursor = stack.undo(|_| count += 1).unwrap();
        assert_eq!(count, 1);
        assert_eq!(cursor, snap(TextPosition::new(0, 1)));

        // Undo should undo "a"
        count = 0;
        let cursor = stack.undo(|_| count += 1).unwrap();
        assert_eq!(count, 1);
        assert_eq!(cursor, snap(TextPosition::new(0, 0)));
    }

    #[test]
    fn test_kind_change_breaks_group() {
        let mut stack = UndoStack::new();
        // Insert then delete should be separate groups
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.record(
            del(0, b"a"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 0)),
        );

        // Undo first undoes the delete
        let cursor = stack.undo(|_| {}).unwrap();
        assert_eq!(cursor, snap(TextPosition::new(0, 1)));

        // Then the insert
        let cursor = stack.undo(|_| {}).unwrap();
        assert_eq!(cursor, snap(TextPosition::new(0, 0)));
    }

    #[test]
    fn test_space_insert_breaks_group() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.record(
            ins(1, b"b"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 2)),
        );
        // Space should start new group
        stack.record(
            ins(2, b" "),
            snap(TextPosition::new(0, 2)),
            snap(TextPosition::new(0, 3)),
        );

        // Undo: first undoes the space
        let mut count = 0;
        stack.undo(|_| count += 1);
        assert_eq!(count, 1);

        // Then undoes "ab" as a group
        count = 0;
        stack.undo(|_| count += 1);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_newline_insert_breaks_group() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.record(
            ins(1, b"\n"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(1, 0)),
        );

        // Newline should be separate
        let mut count = 0;
        stack.undo(|_| count += 1);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_cursor_jump_breaks_group() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        // cursor_before doesn't match last cursor_after_byte — should break
        stack.record(
            ins(5, b"b"),
            snap(TextPosition::new(0, 5)),
            snap(TextPosition::new(0, 6)),
        );

        let mut count = 0;
        stack.undo(|_| count += 1);
        assert_eq!(count, 1); // "b" alone
    }

    #[test]
    fn test_contiguous_inserts_grouped() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.record(
            ins(1, b"b"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 2)),
        );
        stack.record(
            ins(2, b"c"),
            snap(TextPosition::new(0, 2)),
            snap(TextPosition::new(0, 3)),
        );

        // All three should be one group
        let mut count = 0;
        stack.undo(|_| count += 1);
        assert_eq!(count, 3);
    }

    #[test]
    fn test_stacks_empty() {
        let stack = UndoStack::new();
        let (undo, redo) = stack.stacks();
        assert!(undo.is_empty());
        assert!(redo.is_empty());
    }

    #[test]
    fn test_stacks_returns_committed_groups() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.seal();
        stack.record(
            ins(1, b"b"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 2)),
        );
        stack.seal();
        let (undo, redo) = stack.stacks();
        assert_eq!(undo.len(), 2);
        assert!(redo.is_empty());
    }

    #[test]
    fn test_stacks_after_undo() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.seal();
        stack.record(
            ins(1, b"b"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 2)),
        );
        stack.seal();
        stack.undo(|_| {});
        let (undo, redo) = stack.stacks();
        assert_eq!(undo.len(), 1);
        assert_eq!(redo.len(), 1);
    }

    #[test]
    fn test_restore_replaces_stacks() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"x"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.seal();

        let undo_groups = vec![UndoGroup {
            operations: vec![ins(0, b"a")],
            carets_before: snap(TextPosition::new(0, 0)),
            carets_after: snap(TextPosition::new(0, 1)),
        }];
        let redo_groups = vec![UndoGroup {
            operations: vec![ins(1, b"b")],
            carets_before: snap(TextPosition::new(0, 1)),
            carets_after: snap(TextPosition::new(0, 2)),
        }];

        stack.restore(undo_groups, redo_groups);
        let (undo, redo) = stack.stacks();
        assert_eq!(undo.len(), 1);
        assert_eq!(redo.len(), 1);
    }

    #[test]
    fn test_restore_resets_transient_state() {
        let mut stack = UndoStack::new();
        // Build up transient state
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        // Don't seal — there's a current group

        stack.restore(Vec::new(), Vec::new());

        // After restore, recording should work cleanly
        stack.record(
            ins(0, b"b"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.seal();
        let (undo, redo) = stack.stacks();
        assert_eq!(undo.len(), 1);
        assert!(redo.is_empty());
    }

    #[test]
    fn test_begin_end_group() {
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.record(
            ins(1, b" "),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 2)),
        );
        stack.record(
            ins(2, b"b"),
            snap(TextPosition::new(0, 2)),
            snap(TextPosition::new(0, 3)),
        );
        stack.end_group();

        // All three operations in one group, even though space normally breaks
        let mut count = 0;
        stack.undo(|_| count += 1).unwrap();
        assert_eq!(count, 3);
        assert!(stack.undo(|_| {}).is_none());
    }

    #[test]
    fn test_force_group_ignores_kind_change() {
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.record(
            ins(0, b"a"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.record(
            del(0, b"a"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 0)),
        );
        stack.end_group();

        // Insert then delete should still be one group when force_group is on
        let mut count = 0;
        stack.undo(|_| count += 1).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_multiple_undo_redo_cycles() {
        let mut stack = UndoStack::new();
        stack.record(
            ins(0, b"x"),
            snap(TextPosition::new(0, 0)),
            snap(TextPosition::new(0, 1)),
        );
        stack.seal();
        stack.record(
            ins(1, b"y"),
            snap(TextPosition::new(0, 1)),
            snap(TextPosition::new(0, 2)),
        );

        stack.undo(|_| {}); // undo "y"
        stack.undo(|_| {}); // undo "x"
        stack.redo(|_| {}); // redo "x"
        stack.redo(|_| {}); // redo "y"

        // Should be back to having two groups on undo stack
        assert!(stack.undo(|_| {}).is_some()); // "y"
        assert!(stack.undo(|_| {}).is_some()); // "x"
        assert!(stack.undo(|_| {}).is_none());
    }
}
