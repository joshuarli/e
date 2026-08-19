use hi_lite::{
    Highlighter, Kind, Language, Run, State, byte_kinds_to_char_kinds, highlight_line_into, runs,
};

#[test]
fn public_api_streams_opaque_state_and_reuses_output() {
    let mut highlighter = Highlighter::new(Language::Rust);
    let mut output = Vec::new();

    let first = highlighter.highlight_into(b"/* start", &mut output);
    assert!(first.iter().all(|&kind| kind == Kind::Comment));
    assert!(!highlighter.state().is_normal());

    let second = highlighter.highlight_into(b"end */ let", &mut output);
    assert_eq!(second[8], Kind::Keyword);
    assert!(highlighter.state().is_normal());

    highlighter.reset();
    assert_eq!(highlighter.state(), State::default());
}

#[test]
fn public_api_clears_reused_slots_between_equal_length_lines() {
    let mut highlighter = Highlighter::new(Language::Rust);
    let mut output = Vec::new();

    let first = highlighter.highlight_into(b"//x", &mut output);
    assert!(first.iter().all(|&kind| kind == Kind::Comment));

    let second = highlighter.highlight_into(b"let", &mut output);
    assert!(second.iter().all(|&kind| kind == Kind::Keyword));
}

#[test]
fn public_api_supports_stateless_lines_runs_and_char_mapping() {
    let line = b"let x = 1";
    let mut kinds = vec![Kind::Normal; line.len()];
    assert!(highlight_line_into(Language::Rust, State::default(), line, &mut kinds).is_normal());
    assert_eq!(kinds[0], Kind::Keyword);

    let grouped: Vec<_> = runs(&[Kind::Keyword, Kind::Keyword, Kind::Normal]).collect();
    assert_eq!(
        grouped,
        vec![
            Run {
                start: 0,
                end: 2,
                kind: Kind::Keyword,
            },
            Run {
                start: 2,
                end: 3,
                kind: Kind::Normal,
            },
        ]
    );

    let raw = "a\té".as_bytes();
    let byte_kinds = vec![Kind::Comment; raw.len()];
    let chars = byte_kinds_to_char_kinds(raw, &byte_kinds);
    assert_eq!(chars.len(), 4);
}

#[test]
fn language_aliases_are_small_and_explicit() {
    assert_eq!(Language::from_name("rs"), Some(Language::Rust));
    assert_eq!(Language::from_name("shell"), Some(Language::Bash));
    assert_eq!(Language::from_extension(".md"), Some(Language::Markdown));
    assert_eq!(Language::from_name("unknown"), None);
}
