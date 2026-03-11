use super::harness::*;

// ==========================================================================
// Section 18: Mouse — click, double-click, triple-click, drag, scroll
// ==========================================================================

#[test]
fn click_places_cursor() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let (r0, c0) = e.cursor();
    // Click at a different position (row 0, further right)
    // Gutter is 2 chars, so clicking at screen col 7 = buffer col 5
    e.click(0, 7);
    let (r1, c1) = e.cursor();
    assert_eq!(r1, 0, "click should stay on row 0");
    assert!(
        c1 != c0 || r1 != r0,
        "click should move cursor, was ({r0},{c0}) now ({r1},{c1})"
    );
}

#[test]
fn double_click_selects_word() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Double-click on "hello" (gutter=2, so col 2-6 is "hello")
    e.double_click(0, 3);
    // Should have a selection (cursor hidden)
    assert!(
        !e.cursor_visible(),
        "double-click should select word (cursor hidden)"
    );
    // Copy to verify selection content
    e.ctrl('c');
    e.key(Key::End);
    e.ctrl('v');
    let r = e.row(0);
    assert!(
        r.contains("hello worldhello"),
        "double-click should select 'hello', got: {r}"
    );
}

#[test]
fn triple_click_selects_line() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "line one\nline two\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Triple-click on first line
    e.triple_click(0, 5);
    assert!(
        !e.cursor_visible(),
        "triple-click should select line (cursor hidden)"
    );
    // Copy and paste on line 2 to verify
    e.ctrl('c');
    e.key(Key::Down);
    e.key(Key::End);
    e.ctrl('v');
    let screen = e.screen_text();
    // Should have pasted the entire first line including newline
    assert!(
        screen.contains("line one"),
        "triple-click should select entire line"
    );
}

#[test]
fn drag_creates_selection() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello world\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Drag from col 2 to col 7 (selecting "hello")
    e.drag((0, 2), (0, 7));
    assert!(
        !e.cursor_visible(),
        "drag should create selection (cursor hidden)"
    );
}

#[test]
fn scroll_wheel_scrolls_content() {
    let dir = TempDir::new();
    let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    let r0_before = e.row(0);
    // Scroll down
    e.scroll_down();
    e.scroll_down();
    let r0_after = e.row(0);
    assert_ne!(
        r0_before, r0_after,
        "scroll should change visible content, before: {r0_before}, after: {r0_after}"
    );
}

#[test]
fn scroll_up_from_middle() {
    let dir = TempDir::new();
    let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Scroll down first
    for _ in 0..5 {
        e.scroll_down();
    }
    let r0_scrolled = e.row(0);
    // Scroll back up
    for _ in 0..5 {
        e.scroll_up();
    }
    let r0_back = e.row(0);
    assert_ne!(r0_scrolled, r0_back, "scroll up should change content");
}

#[test]
fn scroll_down_and_back_restores_content() {
    // Regression: skip-clean-lines used to bypass dirty-line checks on rows
    // invalidated by scroll regions, leaving blank rows after scrolling back.
    let dir = TempDir::new();
    let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    let path = create_file(dir.path(), "test.txt", &content);
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);

    // Capture initial screen (first several text rows)
    let initial: Vec<String> = (0..10).map(|r| e.row(r)).collect();

    // Scroll down and back up the same amount
    for _ in 0..4 {
        e.scroll_down();
    }
    for _ in 0..4 {
        e.scroll_up();
    }

    // Every row must match the initial content exactly
    for (r, expected) in initial.iter().enumerate() {
        let after = e.row(r as u16);
        assert_eq!(
            *expected, after,
            "row {r} content mismatch after scroll roundtrip"
        );
    }
}
