use super::harness::*;

// ==========================================================================
// Section 16: Save — Ctrl+S, save-as, mkdir -p, trailing whitespace strip
// Section 17: Quit — Ctrl+Q, dirty confirmation
// ==========================================================================

#[test]
fn save_writes_file() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "original\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    // Type something to modify
    e.key(Key::End);
    e.type_text(" modified");
    e.ctrl('s');
    std::thread::sleep(std::time::Duration::from_millis(100));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.contains("original modified"),
        "save should write modified content, got: {content}"
    );
}

#[test]
fn save_strips_trailing_whitespace() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::End);
    e.type_text("   "); // add trailing spaces
    e.ctrl('s');
    std::thread::sleep(std::time::Duration::from_millis(100));
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        content, "hello\n",
        "save should strip trailing whitespace, got: {content:?}"
    );
}

#[test]
fn save_ensures_trailing_newline() {
    let dir = TempDir::new();
    // File without trailing newline
    let path = create_file(dir.path(), "test.txt", "no newline");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('s');
    std::thread::sleep(std::time::Duration::from_millis(100));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.ends_with('\n'),
        "save should ensure trailing newline, got: {content:?}"
    );
}

#[test]
fn save_creates_parent_dirs() {
    let dir = TempDir::new();
    let path = dir.path().join("sub").join("dir").join("test.txt");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.type_text("hello");
    e.ctrl('s');
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(path.exists(), "save should create parent directories");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("hello"), "saved content: {content}");
}

#[test]
fn save_unnamed_prompts_for_filename() {
    let mut e = TestEditor::new(&[]);
    e.type_text("content");
    e.ctrl('s');
    // Should show a prompt for filename
    let cl = e.command_line();
    assert!(
        !cl.is_empty(),
        "save on unnamed buffer should prompt for filename, got: {cl}"
    );
    e.escape(); // cancel
}

#[test]
fn save_as_with_filename() {
    let dir = TempDir::new();
    let path = dir.path().join("output.txt");
    let mut e = TestEditor::new(&[]);
    e.type_text("save-as content");
    e.ctrl('s');
    // Type the filename in the prompt
    e.type_text(path.to_str().unwrap());
    e.enter();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(path.exists(), "file should be created by save-as");
}

#[test]
fn quit_clean_exits() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.ctrl('q');
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(e.has_exited(), "Ctrl+Q on clean buffer should exit");
}

#[test]
fn quit_dirty_prompts() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.type_text("x"); // make dirty
    e.ctrl('q');
    // Should show quit confirmation
    let sb = e.status_bar();
    assert!(
        sb.contains("Save") || sb.contains("save") || sb.contains("(y/n)"),
        "should show save confirmation, got: {sb}"
    );
    // Answer 'n' to not save
    e.send_raw(b"n");
    e.wait();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(e.has_exited(), "should exit after answering 'n'");
}

#[test]
fn quit_dirty_save_and_exit() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.key(Key::End);
    e.type_text(" modified");
    e.ctrl('q');
    e.send_raw(b"y"); // save
    e.wait();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.contains("hello modified"),
        "should save before exiting, got: {content}"
    );
}

#[test]
fn quit_dirty_cancel() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.type_text("x"); // make dirty
    e.ctrl('q');
    // Press some other key to cancel
    e.escape();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        !e.has_exited(),
        "Esc should cancel quit and keep editor running"
    );
}

#[test]
fn save_marks_buffer_clean() {
    let dir = TempDir::new();
    let path = create_file(dir.path(), "test.txt", "hello\n");
    let mut e = TestEditor::new(&[path.to_str().unwrap()]);
    e.type_text("x");
    let sb = e.status_bar();
    assert!(sb.contains('*'), "should be dirty after edit");
    e.ctrl('s');
    let sb = e.status_bar();
    assert!(!sb.contains('*'), "should be clean after save, got: {sb}");
}
