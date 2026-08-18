use super::harness::*;

/// Exercise one intentionally narrow, deterministic user-facing workload for
/// PGO collection. The fixture and weights are engineering assumptions for the
/// separation proof of concept, not a model of measured editor usage.
#[test]
#[ignore = "run only through the PGO profile-collection target"]
fn profile_editor_startup_find_edit() {
    let fixture_dir = TempDir::new();
    let mut source = String::with_capacity(256 * 1024);
    for index in 0..3_000 {
        source.push_str(&format!(
            "fn profile_function_{index:04}(value: usize) -> usize {{ profile_token_{:02} + value }}\n",
            index % 10
        ));
    }
    let path = create_file(fixture_dir.path(), "profile.rs", &source);

    let mut editor = TestEditor::with_profile_size(&[path.to_str().unwrap()], 40, 120);

    editor.ctrl('f');
    editor.type_text("profile_token_0");
    editor.backspace();
    editor.type_text("7");
    editor.enter();
    for _ in 0..4 {
        editor.key(Key::Down);
    }
    editor.escape();

    editor.key(Key::PageDown);
    editor.key(Key::PageUp);
    editor.key(Key::End);
    editor.type_text(" // profile edit");
    editor.ctrl('z');
    editor.ctrl('y');
    editor.ctrl('z');

    let status = editor.status_bar();
    assert!(
        status.contains("profile.rs"),
        "fixture should remain open: {status}"
    );

    editor.quit_no_save();
    let exit_status = editor.wait_for_exit();
    assert_eq!(
        exit_status,
        ptytest::ExitStatus::Code(0),
        "profile workload child failed: {exit_status:?}"
    );
}
