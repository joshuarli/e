use super::harness::*;

// ==========================================================================
// Section 6: Selection — Shift+arrows, Ctrl+Shift, Ctrl+A, Ctrl+W, collapse
// ==========================================================================

#[test]
fn shift_right_creates_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Shift+Right should create a selection
    e.key(Key::ShiftRight);
    e.key(Key::ShiftRight);
    e.key(Key::ShiftRight);
    // Cursor should be hidden during selection
    assert!(
        !e.cursor_visible(),
        "cursor should be hidden during selection"
    );
}

#[test]
fn shift_left_extends_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "abcdef\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::End);
    e.key(Key::ShiftLeft);
    e.key(Key::ShiftLeft);
    assert!(
        !e.cursor_visible(),
        "cursor should be hidden during selection"
    );
}

#[test]
fn escape_clears_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::ShiftRight);
    e.key(Key::ShiftRight);
    assert!(!e.cursor_visible(), "selection should hide cursor");
    e.escape();
    assert!(
        e.cursor_visible(),
        "Esc should clear selection and show cursor"
    );
}

#[test]
fn ctrl_a_selects_all() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\nworld\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('a');
    assert!(!e.cursor_visible(), "select all should hide cursor");
}

#[test]
fn ctrl_w_selects_word() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Ctrl+W should select the word under cursor ("hello")
    e.ctrl('w');
    assert!(!e.cursor_visible(), "word selection should hide cursor");
    // Copy to verify what was selected
    e.ctrl('c');
    // Clear selection
    e.escape();
    // Move to end and paste to verify
    e.key(Key::End);
    e.ctrl('v');
    let r = e.row(0);
    assert!(
        r.contains("hello worldhello"),
        "should have pasted 'hello', got: {r}"
    );
}

#[test]
fn selection_renders_with_inverse_video() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "abcdef\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Select first few characters
    e.key(Key::ShiftRight);
    e.key(Key::ShiftRight);
    e.key(Key::ShiftRight);
    // The selected cells should have inverse video
    // Gutter is 2 chars wide for single-digit line numbers
    let inv = e.cell_inverse(0, 2);
    assert!(inv, "selected cell should be inverse video");
}

#[test]
fn ctrl_shift_up_selects_to_start() {
    let dir = TempDir::new();
    let content = "line 1\nline 2\nline 3\nline 4\n";
    let path = create_file(dir.path(), "test.txt", content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Move to line 3
    e.key(Key::Down);
    e.key(Key::Down);
    // Ctrl+Shift+Up selects from cursor to start of file
    e.key(Key::CtrlShiftUp);
    assert!(!e.cursor_visible(), "should have a selection");
}

#[test]
fn ctrl_shift_down_selects_to_end() {
    let dir = TempDir::new();
    let content = "line 1\nline 2\nline 3\nline 4\n";
    let path = create_file(dir.path(), "test.txt", content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Ctrl+Shift+Down selects from cursor to end of file
    e.key(Key::CtrlShiftDown);
    assert!(!e.cursor_visible(), "should have a selection");
}

#[test]
fn movement_collapses_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Create selection
    e.key(Key::ShiftRight);
    e.key(Key::ShiftRight);
    assert!(!e.cursor_visible(), "should have selection");
    // Regular arrow should collapse selection
    e.key(Key::Right);
    assert!(e.cursor_visible(), "movement should collapse selection");
}

#[test]
fn typing_replaces_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Select "hello"
    e.ctrl('w');
    // Type replacement
    e.type_text("goodbye");
    let r = e.row(0);
    assert!(
        r.contains("goodbye world"),
        "typing should replace selection, got: {r}"
    );
}

#[test]
fn shift_up_down_extends_selection() {
    let dir = TempDir::new();
    let content = "line 1\nline 2\nline 3\n";
    let path = create_file(dir.path(), "test.txt", content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::ShiftDown);
    e.key(Key::ShiftDown);
    assert!(!e.cursor_visible(), "Shift+Down should extend selection");
    e.key(Key::ShiftUp);
    // Should still have a selection (just smaller)
    assert!(
        !e.cursor_visible(),
        "Shift+Up should still have selection (reduced)"
    );
}

#[test]
fn ctrl_shift_left_right_word_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world test\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Ctrl+Shift+Right should select a word
    e.key(Key::CtrlShiftRight);
    assert!(
        !e.cursor_visible(),
        "Ctrl+Shift+Right should create selection"
    );
    // Copy and paste to verify
    e.ctrl('c');
    e.key(Key::End);
    e.ctrl('v');
    let r = e.row(0);
    assert!(
        r.contains("hello world testhello"),
        "should have selected 'hello', got: {r}"
    );
}
