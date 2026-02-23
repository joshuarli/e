use std::time::Instant;

use crate::selection::Pos;

/// A single atomic text change.
#[derive(Debug, Clone)]
pub enum Operation {
    Insert {
        pos: usize, // byte offset
        data: Vec<u8>,
    },
    Delete {
        pos: usize,    // byte offset
        data: Vec<u8>, // the deleted bytes (for undo)
    },
}

impl Operation {
    fn kind(&self) -> OpKind {
        match self {
            Operation::Insert { .. } => OpKind::Insert,
            Operation::Delete { .. } => OpKind::Delete,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OpKind {
    Insert,
    Delete,
}

/// A group of operations that are undone/redone together.
#[derive(Debug, Clone)]
pub struct OperationGroup {
    pub ops: Vec<Operation>,
    /// Cursor position before the first op in this group.
    pub cursor_before: Pos,
    /// Cursor position after the last op in this group.
    pub cursor_after: Pos,
}

/// Undo/redo stack with grouping heuristics.
pub struct UndoStack {
    undo: Vec<OperationGroup>,
    redo: Vec<OperationGroup>,
    /// Current accumulating group.
    current: Option<OperationGroup>,
    last_kind: Option<OpKind>,
    last_time: Option<Instant>,
    last_cursor: Pos,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            current: None,
            last_kind: None,
            last_time: None,
            last_cursor: Pos::zero(),
        }
    }

    /// Immediately flush the current group, sealing it as a completed undo step.
    pub fn seal(&mut self) {
        self.flush_current();
    }

    /// Record an operation. Determines whether to extend current group or start new.
    pub fn record(&mut self, op: Operation, cursor_before: Pos, cursor_after: Pos) {
        // Clear redo on new edit
        self.redo.clear();

        let should_break = self.should_break_group(&op, cursor_before);

        if should_break {
            self.flush_current();
        }

        let group = self.current.get_or_insert_with(|| OperationGroup {
            ops: Vec::new(),
            cursor_before,
            cursor_after,
        });
        group.ops.push(op.clone());
        group.cursor_after = cursor_after;

        self.last_kind = Some(op.kind());
        self.last_time = Some(Instant::now());
        self.last_cursor = cursor_after;
    }

    fn should_break_group(&self, op: &Operation, cursor_before: Pos) -> bool {
        if self.current.is_none() {
            return false;
        }

        // Kind change
        if self.last_kind.is_some() && self.last_kind != Some(op.kind()) {
            return true;
        }

        // Time gap > 1s
        if let Some(t) = self.last_time
            && t.elapsed().as_millis() > 1000
        {
            return true;
        }

        // Cursor jumped (not contiguous)
        if cursor_before != self.last_cursor {
            return true;
        }

        // Word boundary: space after non-space for inserts
        if let Operation::Insert { data, .. } = op
            && (data == b" " || data == b"\n")
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

    /// Undo the last group. Returns the cursor position to restore.
    pub fn undo(&mut self) -> Option<(Vec<Operation>, Pos)> {
        self.flush_current();
        let group = self.undo.pop()?;
        let cursor = group.cursor_before;

        // Build reverse operations for redo
        let mut redo_ops = Vec::new();
        for op in group.ops.iter().rev() {
            redo_ops.push(op.clone());
        }

        self.redo.push(group);

        Some((redo_ops, cursor))
    }

    /// Redo the last undone group. Returns the cursor position to restore.
    pub fn redo(&mut self) -> Option<(Vec<Operation>, Pos)> {
        let group = self.redo.pop()?;
        let cursor = group.cursor_after;

        let ops: Vec<Operation> = group.ops.clone();
        self.undo.push(group);

        Some((ops, cursor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ins(pos: usize, data: &[u8]) -> Operation {
        Operation::Insert {
            pos,
            data: data.to_vec(),
        }
    }

    fn del(pos: usize, data: &[u8]) -> Operation {
        Operation::Delete {
            pos,
            data: data.to_vec(),
        }
    }

    #[test]
    fn test_empty_undo_returns_none() {
        let mut stack = UndoStack::new();
        assert!(stack.undo().is_none());
    }

    #[test]
    fn test_empty_redo_returns_none() {
        let mut stack = UndoStack::new();
        assert!(stack.redo().is_none());
    }

    #[test]
    fn test_record_and_undo() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        let (ops, cursor) = stack.undo().unwrap();
        assert_eq!(cursor, Pos::new(0, 0));
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn test_undo_then_redo() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        stack.undo();
        let (ops, cursor) = stack.redo().unwrap();
        assert_eq!(cursor, Pos::new(0, 1));
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn test_new_record_clears_redo() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        stack.undo();
        // New edit should clear redo
        stack.record(ins(0, b"b"), Pos::new(0, 0), Pos::new(0, 1));
        assert!(stack.redo().is_none());
    }

    #[test]
    fn test_seal_forces_new_group() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        stack.seal();
        stack.record(ins(1, b"b"), Pos::new(0, 1), Pos::new(0, 2));

        // Undo should only undo "b"
        let (ops, cursor) = stack.undo().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(cursor, Pos::new(0, 1));

        // Undo should undo "a"
        let (ops, cursor) = stack.undo().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(cursor, Pos::new(0, 0));
    }

    #[test]
    fn test_kind_change_breaks_group() {
        let mut stack = UndoStack::new();
        // Insert then delete should be separate groups
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        stack.record(del(0, b"a"), Pos::new(0, 1), Pos::new(0, 0));

        // Undo first undoes the delete
        let (_, cursor) = stack.undo().unwrap();
        assert_eq!(cursor, Pos::new(0, 1));

        // Then the insert
        let (_, cursor) = stack.undo().unwrap();
        assert_eq!(cursor, Pos::new(0, 0));
    }

    #[test]
    fn test_space_insert_breaks_group() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        stack.record(ins(1, b"b"), Pos::new(0, 1), Pos::new(0, 2));
        // Space should start new group
        stack.record(ins(2, b" "), Pos::new(0, 2), Pos::new(0, 3));

        // Undo: first undoes the space
        let (ops, _) = stack.undo().unwrap();
        assert_eq!(ops.len(), 1);

        // Then undoes "ab" as a group
        let (ops, _) = stack.undo().unwrap();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn test_newline_insert_breaks_group() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        stack.record(ins(1, b"\n"), Pos::new(0, 1), Pos::new(1, 0));

        // Newline should be separate
        let (ops, _) = stack.undo().unwrap();
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn test_cursor_jump_breaks_group() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        // cursor_before doesn't match last cursor_after — should break
        stack.record(ins(5, b"b"), Pos::new(0, 5), Pos::new(0, 6));

        let (ops, _) = stack.undo().unwrap();
        assert_eq!(ops.len(), 1); // "b" alone
    }

    #[test]
    fn test_contiguous_inserts_grouped() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"a"), Pos::new(0, 0), Pos::new(0, 1));
        stack.record(ins(1, b"b"), Pos::new(0, 1), Pos::new(0, 2));
        stack.record(ins(2, b"c"), Pos::new(0, 2), Pos::new(0, 3));

        // All three should be one group
        let (ops, _) = stack.undo().unwrap();
        assert_eq!(ops.len(), 3);
    }

    #[test]
    fn test_multiple_undo_redo_cycles() {
        let mut stack = UndoStack::new();
        stack.record(ins(0, b"x"), Pos::new(0, 0), Pos::new(0, 1));
        stack.seal();
        stack.record(ins(1, b"y"), Pos::new(0, 1), Pos::new(0, 2));

        stack.undo(); // undo "y"
        stack.undo(); // undo "x"
        stack.redo(); // redo "x"
        stack.redo(); // redo "y"

        // Should be back to having two groups on undo stack
        assert!(stack.undo().is_some()); // "y"
        assert!(stack.undo().is_some()); // "x"
        assert!(stack.undo().is_none());
    }
}
