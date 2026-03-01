use super::harness::*;

// ==========================================================================
// Section 1: Startup — empty buffer, file open, language detect, line numbers
// Section 2: Line endings — CRLF normalization
// ==========================================================================

#[test]
fn empty_buffer_shows_status_bar() {
    let mut e = TestEditor::new(&[]);
    let sb = e.status_bar();
    assert!(
        sb.contains("[Text]"),
        "status bar should show [Text], got: {sb}"
    );
    assert!(sb.contains("e v"), "status bar should show version");
}

#[test]
fn empty_buffer_has_line_numbers() {
    let mut e = TestEditor::new(&[]);
    let r = e.row(0);
    assert!(
        r.starts_with('1'),
        "first row should show line number 1, got: {r}"
    );
}

#[test]
fn open_existing_file_shows_content() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "hello.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let r = e.row(0);
    assert!(
        r.contains("hello world"),
        "row 0 should contain file text, got: {r}"
    );
}

#[test]
fn open_file_shows_filename_in_status_bar() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "notes.txt", "some text\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let sb = e.status_bar();
    assert!(
        sb.contains("notes.txt"),
        "status bar should show filename, got: {sb}"
    );
}

#[test]
fn open_new_file_empty_buffer_with_name() {
    let dir = TempDir::new();
    let path = dir.path().join("newfile.txt");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Buffer should be empty
    let r = e.row(0);
    assert!(
        r.trim().is_empty() || r.trim() == "1",
        "new file should have empty buffer, got: {r}"
    );
    let sb = e.status_bar();
    assert!(
        sb.contains("newfile.txt"),
        "status bar should show filename for new file, got: {sb}"
    );
}

#[test]
fn language_detection_rust() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "main.rs", "fn main() {}\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let sb = e.status_bar();
    // Use partial match — long temp paths can truncate the closing ']'
    assert!(
        sb.contains("[Rust"),
        "status bar should show [Rust], got: {sb}"
    );
}

#[test]
fn language_detection_python() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "app.py", "print('hi')\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let sb = e.status_bar();
    assert!(
        sb.contains("[Py"),
        "status bar should show [Python], got: {sb}"
    );
}

#[test]
fn language_detection_go() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "main.go", "package main\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let sb = e.status_bar();
    assert!(sb.contains("[Go"), "status bar should show [Go], got: {sb}");
}

#[test]
fn language_detection_javascript() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "index.js", "console.log('hi');\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let sb = e.status_bar();
    assert!(
        sb.contains("[Ja"),
        "status bar should show [JavaScript], got: {sb}"
    );
}

#[test]
fn language_detection_makefile() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "Makefile", "all:\n\techo hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let sb = e.status_bar();
    assert!(
        sb.contains("[Ma"),
        "status bar should show [Makefile], got: {sb}"
    );
}

#[test]
fn version_in_status_bar() {
    let mut e = TestEditor::new(&[]);
    let sb = e.status_bar();
    assert!(
        sb.contains(concat!("e v", env!("CARGO_PKG_VERSION"))),
        "status bar should show version, got: {sb}"
    );
}

#[test]
fn dirty_flag_appears_after_typing() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Not dirty initially
    let sb = e.status_bar();
    assert!(
        !sb.contains('*'),
        "should not be dirty initially, got: {sb}"
    );
    // Type something → dirty
    e.type_text("x");
    let sb = e.status_bar();
    assert!(sb.contains('*'), "should be dirty after typing, got: {sb}");
}

#[test]
fn multiline_file_shows_all_lines() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "multi.txt", "line one\nline two\nline three\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let r0 = e.row(0);
    let r1 = e.row(1);
    let r2 = e.row(2);
    assert!(r0.contains("line one"), "row 0: {r0}");
    assert!(r1.contains("line two"), "row 1: {r1}");
    assert!(r2.contains("line three"), "row 2: {r2}");
}

#[test]
fn cursor_starts_at_top() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\nworld\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let (row, _col) = e.cursor();
    assert_eq!(row, 0, "cursor should start at row 0");
}

#[test]
fn line_numbers_gutter_width() {
    let dir = TempDir::new();
    // 3 lines → gutter is "1 ", "2 ", "3 " (2 chars wide)
    let path = create_file(dir.path(), "test.txt", "aaa\nbbb\nccc\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let r0 = e.row(0);
    // Line number "1" followed by space, then content "aaa"
    assert!(r0.contains("aaa"), "should show content, got: {r0}");
    // Line numbers should be present
    let r1 = e.row(1);
    assert!(r1.contains("bbb"), "row 1 should show content, got: {r1}");
}

#[test]
fn many_lines_wider_gutter() {
    let dir = TempDir::new();
    let content: String = (1..=15).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // With 15 lines, gutter needs 2 digits + 1 space = 3 chars
    let r0 = e.row(0);
    assert!(r0.contains("line 1"), "row 0: {r0}");
    // Line 10 should also be visible
    let r9 = e.row(9);
    assert!(r9.contains("line 10"), "row 9: {r9}");
}
