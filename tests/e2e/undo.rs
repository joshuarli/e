use super::harness::*;

// ==========================================================================
// Section 11: Undo/Redo
// ==========================================================================

#[test]
fn undo_reverts_typing() {
    let mut e = TestEditor::new(&[]);
    e.type_text("hello");
    let before = e.row(0);
    assert!(before.contains("hello"), "should have typed text");
    // Undo
    e.ctrl('z');
    let after = e.row(0);
    assert!(
        !after.contains("hello"),
        "undo should revert typing, got: {after}"
    );
}

#[test]
fn redo_restores_undone_change() {
    let mut e = TestEditor::new(&[]);
    e.type_text("hello");
    e.ctrl('z'); // undo
    let undone = e.row(0);
    assert!(!undone.contains("hello"), "undo should remove text");
    e.ctrl('y'); // redo
    let redone = e.row(0);
    assert!(
        redone.contains("hello"),
        "redo should restore text, got: {redone}"
    );
}

#[test]
fn new_edit_clears_redo_stack() {
    let mut e = TestEditor::new(&[]);
    e.type_text("first");
    // Wait for group boundary (time-based)
    std::thread::sleep(std::time::Duration::from_millis(1100));
    e.type_text(" second");
    e.ctrl('z'); // undo "second"
    // Type something new — should clear redo stack
    e.type_text(" third");
    e.ctrl('y'); // try redo — should have no effect
    let r = e.row(0);
    assert!(
        r.contains("first third"),
        "redo should be cleared after new edit, got: {r}"
    );
    assert!(!r.contains("second"), "should not have 'second', got: {r}");
}

#[test]
fn undo_groups_by_word_boundary() {
    let mut e = TestEditor::new(&[]);
    e.type_text("hello world");
    // "hello" and "world" are separated by a space, which creates a group boundary
    e.ctrl('z'); // should undo "world" (or "world" + space)
    let r = e.row(0);
    // After one undo, should still have "hello" but not the full "hello world"
    assert!(
        !r.contains("hello world"),
        "first undo should remove last word group, got: {r}"
    );
}

#[test]
fn undo_kill_line_restores_it() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "line one\nline two\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('k'); // kill line
    let after_kill = e.row(0);
    assert!(
        after_kill.contains("line two"),
        "kill should remove first line, got: {after_kill}"
    );
    e.ctrl('z'); // undo
    let restored = e.row(0);
    assert!(
        restored.contains("line one"),
        "undo should restore killed line, got: {restored}"
    );
}

#[test]
fn undo_after_indent() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "aaa\nbbb\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('a'); // select all
    e.tab(); // indent
    let indented = e.row(0);
    assert!(
        indented.contains("  aaa"),
        "should be indented, got: {indented}"
    );
    e.ctrl('z'); // undo entire indent as one group
    let restored = e.row(0);
    assert!(
        !restored.contains("  aaa"),
        "undo should revert indent, got: {restored}"
    );
}

#[test]
fn multiple_undo_redo_steps() {
    let mut e = TestEditor::new(&[]);
    e.type_text("A");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    e.type_text("B");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    e.type_text("C");

    let full = e.row(0);
    assert!(full.contains("ABC"), "should have ABC, got: {full}");

    e.ctrl('z'); // undo C
    let no_c = e.row(0);
    assert!(no_c.contains("AB"), "after 1 undo: {no_c}");

    e.ctrl('z'); // undo B
    let no_bc = e.row(0);
    assert!(no_bc.contains("A"), "after 2 undos: {no_bc}");

    e.ctrl('y'); // redo B
    let redo_b = e.row(0);
    assert!(redo_b.contains("AB"), "after 1 redo: {redo_b}");

    e.ctrl('y'); // redo C
    let redo_c = e.row(0);
    assert!(redo_c.contains("ABC"), "after 2 redos: {redo_c}");
}
