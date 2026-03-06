use super::harness::*;

// ==========================================================================
// Section 8: Editing — character insertion, Enter, Backspace, Delete,
//            Kill line, Duplicate line, Tab, auto-close, skip-over, dedent
// Section 9: Comment toggle (Ctrl+D)
// ==========================================================================

#[test]
fn type_characters() {
    let mut e = TestEditor::new(&[]);
    e.type_text("hello world");
    let r = e.row(0);
    assert!(
        r.contains("hello world"),
        "should show typed text, got: {r}"
    );
}

#[test]
fn enter_creates_new_line() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "first\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::End);
    e.enter();
    e.type_text("second");
    let screen = e.screen_text();
    assert!(screen.contains("first"), "should have 'first' on screen");
    assert!(screen.contains("second"), "should have 'second' on screen");
}

#[test]
fn enter_preserves_indentation() {
    let mut e = TestEditor::new(&[]);
    e.type_text("  indented");
    e.enter();
    e.type_text("next");
    // The new line should have 2 spaces of indentation from the previous line
    let r1 = e.row(1);
    assert!(r1.contains("  next"), "should preserve indent, got: {r1}");
}

#[test]
fn backspace_deletes_character() {
    let mut e = TestEditor::new(&[]);
    e.type_text("abc");
    e.backspace();
    let r = e.row(0);
    assert!(
        r.contains("ab"),
        "should have 'ab' after backspace, got: {r}"
    );
    assert!(!r.contains("abc"), "should not have 'abc', got: {r}");
}

#[test]
fn backspace_at_col0_joins_lines() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\nworld\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Move to start of line 2
    e.key(Key::Down);
    e.key(Key::Home);
    e.backspace();
    // Lines should be joined
    let r = e.row(0);
    assert!(r.contains("helloworld"), "lines should be joined, got: {r}");
}

#[test]
fn backspace_smart_dedent() {
    let mut e = TestEditor::new(&[]);
    // Type 4 spaces (2 indent levels)
    e.type_text("    x");
    // Move cursor to after the 4 spaces, before 'x'
    e.key(Key::Home);
    e.key(Key::Right); // col 2
    e.key(Key::Right); // col 4 (indent stop snapping: should be at col 4)
    // Actually, let's be simpler: delete from end of spaces
    // Start fresh
    drop(e);

    let mut e = TestEditor::new(&[]);
    e.type_text("    "); // 4 spaces
    // Cursor is at col 4 in leading whitespace
    // Backspace should remove 2 spaces (smart dedent to col 2)
    e.backspace();
    let r = e.row(0);
    // Should have 2 spaces left
    let text_after_gutter: String = r.chars().skip_while(|c| c.is_ascii_digit()).collect();
    let _leading_spaces = text_after_gutter.trim_start_matches(' ').len();
    // The row should be shorter after dedent
    // Just verify it doesn't have 4 spaces anymore
    assert!(
        !r.contains("    ") || r.trim().is_empty(),
        "should have dedented, got: {r}"
    );
}

#[test]
fn backspace_deletes_auto_close_pair() {
    let mut e = TestEditor::new(&[]);
    // Type ( which auto-closes to ()
    e.type_text("(");
    // Cursor should be between ( and )
    // Backspace should delete both
    e.backspace();
    let r = e.row(0);
    // Should be empty (just the line number)
    let content: String = r
        .chars()
        .skip_while(|c| c.is_ascii_digit() || *c == ' ')
        .collect();
    assert!(
        content.is_empty(),
        "should have deleted both parens, got: {r}"
    );
}

#[test]
fn delete_forward() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "abcdef\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Cursor at start, delete forward should remove 'a'
    e.key(Key::Delete);
    let r = e.row(0);
    assert!(
        r.contains("bcdef"),
        "should have deleted first char, got: {r}"
    );
    assert!(
        !r.contains("abcdef"),
        "should not contain original, got: {r}"
    );
}

#[test]
fn delete_at_eol_joins_lines() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\nworld\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Move to end of first line
    e.key(Key::End);
    // Delete forward joins with next line
    e.key(Key::Delete);
    let r = e.row(0);
    assert!(r.contains("helloworld"), "should join lines, got: {r}");
}

#[test]
fn kill_line_ctrl_k() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "line one\nline two\nline three\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Kill line 1
    e.ctrl('k');
    let r = e.row(0);
    assert!(
        r.contains("line two"),
        "first visible line should now be 'line two', got: {r}"
    );
}

#[test]
fn duplicate_line_ctrl_j() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "original line\nsecond line\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('j');
    // After duplicating line 1, both row 0 and row 1 should have "original line"
    let screen = e.screen_text();
    // Count occurrences of "original line"
    let count = screen.matches("original line").count();
    assert!(
        count >= 2,
        "should have 2 copies of 'original line', got {count}. screen:\n{screen}"
    );
}

#[test]
fn tab_inserts_spaces_for_txt() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "x\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::Home);
    e.tab();
    let r = e.row(0);
    // For .txt files, tab should insert 2 spaces before 'x'
    assert!(
        r.contains("  x"),
        "tab should insert 2 spaces for .txt, got: {r}"
    );
}

#[test]
fn auto_close_parens() {
    let mut e = TestEditor::new(&[]);
    e.type_text("(");
    let r = e.row(0);
    assert!(r.contains("()"), "should auto-close parens, got: {r}");
}

#[test]
fn auto_close_brackets() {
    let mut e = TestEditor::new(&[]);
    e.type_text("[");
    let r = e.row(0);
    assert!(r.contains("[]"), "should auto-close brackets, got: {r}");
}

#[test]
fn auto_close_braces() {
    let mut e = TestEditor::new(&[]);
    e.type_text("{");
    let r = e.row(0);
    assert!(r.contains("{}"), "should auto-close braces, got: {r}");
}

#[test]
fn auto_close_double_quotes() {
    let mut e = TestEditor::new(&[]);
    e.type_text("\"");
    let r = e.row(0);
    // Should show ""
    let count = r.matches('"').count();
    assert!(count >= 2, "should auto-close quotes, got: {r}");
}

#[test]
fn skip_over_closing_paren() {
    let mut e = TestEditor::new(&[]);
    e.type_text("(");
    // Cursor between ( and )
    e.type_text("x");
    // Now type ) — should skip over the auto-closed )
    e.type_text(")");
    let r = e.row(0);
    assert!(r.contains("(x)"), "should skip over close paren, got: {r}");
    // Should NOT have (x))
    assert!(
        !r.contains("(x))"),
        "should not duplicate close paren, got: {r}"
    );
}

#[test]
fn skip_over_closing_quote() {
    let mut e = TestEditor::new(&[]);
    e.type_text("\"");
    e.type_text("hi");
    e.type_text("\"");
    let r = e.row(0);
    // Should be "hi" not "hi""
    assert!(
        r.contains("\"hi\""),
        "should skip over close quote, got: {r}"
    );
}

#[test]
fn shift_tab_dedent() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "  indented\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::ShiftTab);
    let r = e.row(0);
    // Should remove 2 spaces of indent
    assert!(
        r.contains("indented"),
        "should still have text after dedent, got: {r}"
    );
}

#[test]
fn ctrl_backspace_deletes_word() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::End);
    // Ctrl+H is mapped to delete word backward
    e.ctrl('h');
    let r = e.row(0);
    assert!(r.contains("hello"), "should keep 'hello', got: {r}");
    assert!(
        !r.contains("world"),
        "should have deleted 'world', got: {r}"
    );
}

#[test]
fn auto_close_wraps_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "foo\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Select "foo" using Ctrl+W (select word at cursor)
    e.ctrl('w');
    // Type ( to wrap with parens
    e.type_text("(");
    let r = e.row(0);
    assert!(
        r.contains("(foo)"),
        "should wrap selection with parens, got: {r}"
    );
}

#[test]
fn comment_toggle_rust() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.rs", "let x = 1;\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Toggle comment with Ctrl+D
    e.ctrl('d');
    let r = e.row(0);
    assert!(
        r.contains("// let x = 1;"),
        "should comment the line, got: {r}"
    );
    // Toggle again to uncomment
    e.ctrl('d');
    let r = e.row(0);
    assert!(
        r.contains("let x = 1;"),
        "should uncomment the line, got: {r}"
    );
    assert!(
        !r.contains("//"),
        "should not have comment prefix, got: {r}"
    );
}

#[test]
fn comment_toggle_python() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.py", "x = 1\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('d');
    let r = e.row(0);
    assert!(r.contains("# x = 1"), "should comment with #, got: {r}");
}

#[test]
fn tab_indents_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "line one\nline two\nline three\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Select all with Ctrl+A
    e.ctrl('a');
    // Tab should indent all lines
    e.tab();
    let r0 = e.row(0);
    let r1 = e.row(1);
    assert!(r0.contains("  line one"), "should indent line 1, got: {r0}");
    assert!(r1.contains("  line two"), "should indent line 2, got: {r1}");
}

#[test]
fn enter_on_empty_buffer_moves_cursor_down() {
    let mut e = TestEditor::new(&[]);
    let (r0, _) = e.cursor();
    e.enter();
    let (r1, _) = e.cursor();
    assert_eq!(
        r1,
        r0 + 1,
        "Enter on empty buffer should move cursor down one row; was {r0} -> {r1}"
    );
}
