use super::harness::*;

// ==========================================================================
// Section 10: Clipboard — copy, cut, paste, smart paste, bracketed paste
// ==========================================================================

#[test]
fn copy_paste_roundtrip() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Select "hello"
    e.ctrl('w');
    // Copy
    e.ctrl('c');
    // Move to end of line
    e.key(Key::End);
    // Paste
    e.ctrl('v');
    let r = e.row(0);
    assert!(
        r.contains("hello worldhello"),
        "should paste copied text at cursor, got: {r}"
    );
}

#[test]
fn cut_removes_and_copies() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Select "hello"
    e.ctrl('w');
    // Cut
    e.ctrl('x');
    let after_cut = e.row(0);
    assert!(
        !after_cut.contains("hello"),
        "cut should remove selected text, got: {after_cut}"
    );
    // Paste at end
    e.key(Key::End);
    e.ctrl('v');
    let after_paste = e.row(0);
    assert!(
        after_paste.contains("hello"),
        "should be able to paste cut text, got: {after_paste}"
    );
}

#[test]
fn copy_no_selection_is_noop() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Copy with no selection — should be no-op
    e.ctrl('c');
    // File should be unchanged
    let r = e.row(0);
    assert!(
        r.contains("hello"),
        "copy with no selection should be noop, got: {r}"
    );
}

#[test]
fn paste_replaces_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "aaa bbb ccc\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Select and copy "aaa"
    e.ctrl('w');
    e.ctrl('c');
    e.escape();
    // Select "bbb" by word-navigating there
    e.key(Key::CtrlRight);
    e.ctrl('w');
    // Paste "aaa" over "bbb"
    e.ctrl('v');
    let r = e.row(0);
    assert!(
        r.contains("aaa aaa ccc"),
        "paste should replace selection, got: {r}"
    );
}

#[test]
fn bracketed_paste_single_undo() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Send a bracketed paste
    e.paste("hello world");
    let r = e.row(0);
    assert!(
        r.contains("hello world"),
        "bracketed paste should insert text, got: {r}"
    );
    // Undo should remove the entire paste in one step
    e.ctrl('z');
    let after_undo = e.row(0);
    assert!(
        !after_undo.contains("hello world"),
        "undo should revert entire paste at once, got: {after_undo}"
    );
}

#[test]
fn bracketed_paste_multiline() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.paste("line 1\nline 2\nline 3");
    let r0 = e.row(0);
    let r1 = e.row(1);
    let r2 = e.row(2);
    assert!(r0.contains("line 1"), "row 0: {r0}");
    assert!(r1.contains("line 2"), "row 1: {r1}");
    assert!(r2.contains("line 3"), "row 2: {r2}");
}
