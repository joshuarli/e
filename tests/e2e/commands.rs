use super::harness::*;

// ==========================================================================
// Section 14: Command palette — Ctrl+P, tab completion, built-in commands
// Section 15: Command buffer — mini-editor, history
// ==========================================================================

#[test]
fn command_palette_opens() {
    let mut e = TestEditor::new(&[]);
    e.ctrl('p');
    let cl = e.command_line();
    assert!(
        cl.contains('>') || !cl.is_empty(),
        "command palette should show prompt, got: {cl}"
    );
    e.escape();
}

#[test]
fn command_palette_tab_shows_completions() {
    let mut e = TestEditor::new(&[]);
    e.ctrl('p');
    e.tab(); // Tab on empty input should show all commands
    let screen = e.screen_text();
    // Should show available commands somewhere on screen
    assert!(
        screen.contains("save") || screen.contains("quit"),
        "tab should show command completions"
    );
    e.escape();
}

#[test]
fn command_palette_partial_tab_completes() {
    let mut e = TestEditor::new(&[]);
    e.ctrl('p');
    e.type_text("sa");
    e.tab(); // Should complete to "save"
    let cl = e.command_line();
    assert!(
        cl.contains("save"),
        "tab should complete 'sa' to 'save', got: {cl}"
    );
    e.escape();
}

#[test]
fn goto_command() {
    let dir = TempDir::new();
    let content: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('p');
    e.type_text("goto 10");
    e.enter();
    // Cursor should be on line 10 (0-indexed: row 9)
    let (row, _) = e.cursor();
    assert_eq!(row, 9, "goto 10 should put cursor on row 9 (0-indexed)");
}

#[test]
fn goto_line_ctrl_l() {
    let dir = TempDir::new();
    let content: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('l');
    e.type_text("15");
    e.enter();
    // Goto centers the viewport, so cursor screen row depends on centering.
    // Just verify the visible content includes line 15.
    let screen = e.screen_text();
    assert!(
        screen.contains("line 15"),
        "goto 15 should show line 15 on screen"
    );
}

#[test]
fn ruler_toggle() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let with_ruler = e.row(0);
    assert!(
        with_ruler.contains('1'),
        "should show line number 1 by default"
    );
    // Toggle ruler off with Ctrl+R
    e.ctrl('r');
    let without_ruler = e.row(0);
    // Without ruler, should not start with a digit
    assert!(
        without_ruler.starts_with('h'),
        "ruler off: content should start at col 0, got: {without_ruler}"
    );
    // Toggle back on
    e.ctrl('r');
    let back_on = e.row(0);
    assert!(back_on.contains('1'), "ruler should be back on");
}

#[test]
fn selectall_command() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\nworld\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('p');
    e.type_text("selectall");
    e.enter();
    assert!(!e.cursor_visible(), "selectall should create a selection");
}

#[test]
fn quit_command() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('p');
    e.type_text("quit");
    e.enter();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(e.has_exited(), "quit command should exit the editor");
}

#[test]
fn q_alias_for_quit() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('p');
    e.type_text("q");
    e.enter();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(e.has_exited(), "q command should exit the editor");
}

#[test]
fn trim_command() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello   \nworld  \n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('p');
    e.type_text("trim");
    e.enter();
    // Save and check file content
    e.ctrl('s');
    std::thread::sleep(std::time::Duration::from_millis(100));
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        content, "hello\nworld\n",
        "trim should strip trailing whitespace"
    );
}

#[test]
fn command_buffer_history() {
    let mut e = TestEditor::new(&[]);
    // Enter a command
    e.ctrl('p');
    e.type_text("ruler");
    e.enter();
    // Open again and press Up to recall previous
    e.ctrl('p');
    e.key(Key::Up);
    let cl = e.command_line();
    assert!(
        cl.contains("ruler"),
        "Up should recall previous command, got: {cl}"
    );
    e.escape();
}

#[test]
fn find_command_via_palette() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('p');
    e.type_text("find hello");
    e.enter();
    // Should enter find browse mode — the status bar or command line should
    // show match info. Check for any find-related content.
    let sb = e.status_bar();
    let cl = e.command_line();
    let combined = format!("{sb} {cl}");
    assert!(
        combined.contains("match")
            || combined.contains("of")
            || combined.contains("1")
            || !e.cursor_visible(),
        "find command should enter browse mode, got sb={sb} cl={cl}"
    );
    e.escape();
}

#[test]
fn goto_line_large_file_repaints_full_screen() {
    let dir = TempDir::new();
    // 60 lines — more than the 24-row terminal can show at once
    let content: String = (1..=60).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('l');
    e.type_text("50");
    e.enter();
    // After goto, the full viewport should be painted — not just line 50.
    // Check that surrounding lines are visible too.
    let screen = e.screen_text();
    assert!(
        screen.contains("line 50"),
        "goto 50 should show line 50: {screen}"
    );
    assert!(
        screen.contains("line 48") || screen.contains("line 49"),
        "goto 50 should repaint surrounding lines: {screen}"
    );
    assert!(
        screen.contains("line 51") || screen.contains("line 52"),
        "goto 50 should repaint lines after target: {screen}"
    );
}
