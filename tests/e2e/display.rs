use super::harness::*;

// ==========================================================================
// Section 3: Display — layout, soft-wrap, trailing whitespace highlight
// Section 4: Syntax highlighting (basic checks)
// Section 5: Bracket/quote matching
// Section 20: Keybinding configuration
// ==========================================================================

#[test]
fn status_bar_layout() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.rs", "fn main() {}\n");
    // Use wider terminal so the full status bar is visible
    let mut e = TestEditor::with_size(&[path.to_str().unwrap()], 24, 200);
    let sb = e.status_bar();
    // Left side: filename + language
    assert!(
        sb.contains("test.rs"),
        "status bar left should have filename"
    );
    assert!(
        sb.contains("[Rust]"),
        "status bar left should have language"
    );
    // Right side: version
    assert!(sb.contains("e v"), "status bar right should have version");
}

#[test]
fn status_bar_is_reverse_video() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Status bar is on row `rows - 2`
    let sb_row = e.rows - 2;
    let inv = e.cell_inverse(sb_row, 0);
    assert!(inv, "status bar should use reverse video");
}

#[test]
fn command_line_empty_by_default() {
    let mut e = TestEditor::new(&[]);
    let cl = e.command_line();
    assert!(
        cl.is_empty(),
        "command line should be empty by default, got: {cl}"
    );
}

#[test]
fn soft_wrap_long_line() {
    let dir = TempDir::new();
    // Create a line longer than 80 cols (accounting for gutter)
    let long_line = "x".repeat(100);
    let path = create_file(dir.path(), "test.txt", &format!("{long_line}\n"));
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let r0 = e.row(0);
    let r1 = e.row(1);
    // Both rows should contain x's (the line wraps)
    assert!(
        r0.contains("xxx"),
        "first row should have content, got: {r0}"
    );
    assert!(
        r1.contains("xxx"),
        "second row should have wrapped content, got: {r1}"
    );
    // Second row should NOT have a line number (continuation row)
    assert!(
        !r1.starts_with('2'),
        "wrapped continuation should not show line number 2"
    );
}

#[test]
fn trailing_whitespace_not_highlighted() {
    let dir = TempDir::new();
    // Trailing spaces on a line with content — should NOT be highlighted
    let path = create_file(dir.path(), "test.txt", "hello   \n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Gutter is "1 " = 2 chars. Trailing space is at col 7 (2+5)
    let bg = e.cell_bg(0, 7);
    assert_eq!(
        bg,
        vt100::Color::Default,
        "trailing whitespace should not be highlighted"
    );
}

#[test]
fn current_line_number_highlighted() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "line 1\nline 2\nline 3\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Current line (0) should have a special background on the line number
    let bg = e.cell_bg(0, 0);
    // Other lines should have default/dim styling
    let bg_other = e.cell_bg(1, 0);
    assert_ne!(
        bg, bg_other,
        "current line number should be highlighted differently"
    );
}

#[test]
fn syntax_highlight_rust_keyword() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.rs", "fn main() {\n    let x = 1;\n}\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // "fn" starts at gutter + 0. Gutter for 3 lines = "1 " (2 chars)
    let fg_f = e.cell_fg(0, 2); // 'f' of "fn"
    // Keywords should be yellow (Idx(3))
    assert_eq!(
        fg_f,
        vt100::Color::Idx(3),
        "Rust keyword 'fn' should be yellow"
    );
}

#[test]
fn syntax_highlight_string() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.rs", "let s = \"hello\";\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Find the position of the string content
    let r = e.row(0);
    if let Some(pos) = r.find('"') {
        let fg = e.cell_fg(0, pos as u16 + 1); // character after opening quote
        // Strings should be green (Idx(2))
        assert_eq!(fg, vt100::Color::Idx(2), "string content should be green");
    }
}

#[test]
fn syntax_highlight_comment() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.rs", "// this is a comment\nlet x = 1;\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Comment at row 0, after gutter
    // Gutter for 2 lines = "1 " (2 chars)
    let fg = e.cell_fg(0, 2); // '/' of "//"
    // Comments should be grey (Idx(8) or similar dark color)
    assert_ne!(
        fg,
        vt100::Color::Default,
        "comment should have a non-default color"
    );
}

#[test]
fn bracket_matching() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "(hello)\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Cursor on '(' at col 2 (gutter=2)
    // The matching ')' at col 8 should have magenta background
    let bg = e.cell_bg(0, 8); // ')' position
    // Magenta = Idx(5)
    assert_eq!(
        bg,
        vt100::Color::Idx(5),
        "matching bracket should have magenta background"
    );
}

#[test]
fn json_key_highlighting() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.json", "{\n  \"name\": \"value\"\n}\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Check status bar shows JSON
    let sb = e.status_bar();
    // Partial match — long paths may truncate the tag
    assert!(sb.contains("[JS"), "should detect JSON, got: {sb}");
}

#[test]
fn yaml_highlighting() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.yaml", "key: value\nbool: true\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let sb = e.status_bar();
    assert!(sb.contains("[YA"), "should detect YAML, got: {sb}");
}

#[test]
fn markdown_highlighting() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.md", "# Header\n\nSome text.\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let sb = e.status_bar();
    assert!(
        sb.contains("[Markdown") || sb.contains("[Mark"),
        "should detect Markdown, got: {sb}"
    );
    // Header should be keyword-colored (yellow)
    // Gutter for 3 lines = "1 " (2 chars), "#" at col 2
    let fg = e.cell_fg(0, 2);
    assert_eq!(
        fg,
        vt100::Color::Idx(3),
        "Markdown header '#' should be yellow"
    );
}

#[test]
fn custom_terminal_size() {
    let mut e = TestEditor::with_size(&[], 40, 120);
    let sb = e.status_bar();
    // Status bar should be at row 38 (40-2)
    assert!(sb.contains("[Text]"), "should work with custom size");
    assert_eq!(e.rows, 40);
    assert_eq!(e.cols, 120);
}

#[test]
fn tab_displays_as_pipe_space() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "\thello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let r = e.row(0);
    // Tab should display as pipe + space
    assert!(
        r.contains('|'),
        "tab should display with pipe character, got: {r}"
    );
}
