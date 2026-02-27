use super::harness::*;

// ==========================================================================
// Section 7: Movement — arrows, Home/End, Page, Ctrl+T/G, word movement,
//            desired column, indent stop snapping, line wrapping
// ==========================================================================

#[test]
fn arrow_down_moves_cursor() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "line one\nline two\nline three\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let (r0, _) = e.cursor();
    e.key(Key::Down);
    let (r1, _) = e.cursor();
    assert_eq!(r1, r0 + 1, "Down should move cursor down one row");
}

#[test]
fn arrow_up_moves_cursor() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "line one\nline two\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::Down);
    let (r1, _) = e.cursor();
    e.key(Key::Up);
    let (r0, _) = e.cursor();
    assert_eq!(r0, r1 - 1, "Up should move cursor up one row");
}

#[test]
fn arrow_right_moves_cursor() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "abcde\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let (_, c0) = e.cursor();
    e.key(Key::Right);
    let (_, c1) = e.cursor();
    assert!(c1 > c0, "Right should move cursor right");
}

#[test]
fn home_goes_to_start() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "abcdef\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Move right a few times
    e.key(Key::Right);
    e.key(Key::Right);
    e.key(Key::Right);
    // Home should go to beginning of content (after gutter)
    e.key(Key::Home);
    let (_, col) = e.cursor();
    // Should be at the gutter column (start of text area)
    // For a 1-line file, gutter is "1 " = 2 chars
    assert_eq!(col, 2, "Home should go to col 0 of text (gutter offset 2)");
}

#[test]
fn end_goes_to_end_of_line() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "abcdef\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::End);
    let (_, col) = e.cursor();
    // For "abcdef" with gutter "1 " (2 chars), end should be at col 2+6=8
    assert_eq!(col, 2 + 6, "End should go to end of line content");
}

#[test]
fn ctrl_t_goes_to_top() {
    let dir = TempDir::new();
    let content: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Move down several lines
    for _ in 0..10 {
        e.key(Key::Down);
    }
    // Ctrl+T should go to top
    e.ctrl('t');
    let (row, _) = e.cursor();
    assert_eq!(row, 0, "Ctrl+T should go to row 0");
}

#[test]
fn ctrl_g_goes_to_end() {
    let dir = TempDir::new();
    let content: String = (1..=5).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Ctrl+G should go to end
    e.ctrl('g');
    let (row, _) = e.cursor();
    // Should be on the last line (line 5 is empty after trailing newline, or line 6)
    assert!(row >= 4, "Ctrl+G should go near end, got row {row}");
}

#[test]
fn word_movement_ctrl_right() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world foo\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let (_, c0) = e.cursor();
    // Ctrl+Right: should jump past "hello" to "world"
    e.key(Key::CtrlRight);
    let (_, c1) = e.cursor();
    assert!(
        c1 > c0 + 1,
        "Ctrl+Right should jump by word, c0={c0} c1={c1}"
    );
}

#[test]
fn word_movement_ctrl_left() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Go to end of line
    e.key(Key::End);
    let (_, ce) = e.cursor();
    // Ctrl+Left: should jump back to start of "world"
    e.key(Key::CtrlLeft);
    let (_, c1) = e.cursor();
    assert!(c1 < ce, "Ctrl+Left should jump back, ce={ce} c1={c1}");
    // Another Ctrl+Left: should jump to start of "hello"
    e.key(Key::CtrlLeft);
    let (_, c2) = e.cursor();
    assert!(
        c2 < c1,
        "Ctrl+Left again should jump to 'hello', c1={c1} c2={c2}"
    );
}

#[test]
fn desired_column_preserved() {
    let dir = TempDir::new();
    let path = create_file(
        dir.path(),
        "test.txt",
        "long line here\nab\nlong line here\n",
    );
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Move to end of first line (col 14 in text)
    e.key(Key::End);
    let (_, col_end) = e.cursor();
    // Move down to short line — cursor clamps to end of "ab" (col 2)
    e.key(Key::Down);
    let (r1, c1) = e.cursor();
    assert_eq!(r1, 1, "should be on line 2");
    // Move down again to long line — should restore desired column
    e.key(Key::Down);
    let (r2, c2) = e.cursor();
    assert_eq!(r2, 2, "should be on line 3");
    assert_eq!(
        c2, col_end,
        "desired column should be restored, c2={c2} col_end={col_end}"
    );
    _ = c1; // used for debugging
}

#[test]
fn left_wraps_to_previous_line() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "abc\nxyz\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Move to start of line 2
    e.key(Key::Down);
    e.key(Key::Home);
    // Left should wrap to end of line 1
    e.key(Key::Left);
    let (row, _) = e.cursor();
    assert_eq!(row, 0, "Left at col 0 should wrap to previous line");
}

#[test]
fn right_wraps_to_next_line() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "abc\nxyz\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Move to end of line 1
    e.key(Key::End);
    // Right should wrap to start of line 2
    e.key(Key::Right);
    let (row, _) = e.cursor();
    assert_eq!(row, 1, "Right at EOL should wrap to next line");
}

#[test]
fn page_down_moves_by_screen() {
    let dir = TempDir::new();
    let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let (r0, _) = e.cursor();
    e.key(Key::PageDown);
    let (r1, _) = e.cursor();
    // Should jump by approximately the number of visible text rows (22 for 24-row terminal)
    assert!(
        r1 > r0 + 10,
        "PageDown should move significantly, r0={r0} r1={r1}"
    );
}

#[test]
fn page_up_moves_by_screen() {
    let dir = TempDir::new();
    let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Go to end first
    e.ctrl('g');
    let (re, _) = e.cursor();
    e.key(Key::PageUp);
    let (r1, _) = e.cursor();
    assert!(
        r1 < re.saturating_sub(10),
        "PageUp should move significantly, re={re} r1={r1}"
    );
}

#[test]
fn indent_stop_snapping_in_leading_whitespace() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "      hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // With 6 leading spaces: cursor starts at col 0 (gutter offset)
    // Move right — should snap to indent stops (every 2 spaces)
    e.key(Key::Right);
    let (_, c1) = e.cursor();
    e.key(Key::Right);
    let (_, c2) = e.cursor();
    // Each right in leading whitespace should advance by 2 (indent stop)
    let gutter = c1 - 2; // first right should be gutter + 2
    assert_eq!(
        c2 - c1,
        2,
        "should snap to 2-space indent stops, c1={c1} c2={c2} gutter={gutter}"
    );
}
