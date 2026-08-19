//! Checked-in unified diff fixtures derived from the `fx` semantic diff.

use std::fs;
use std::path::Path;

use hi_lite::{
    DEFAULT_CONTEXT_LINES, DEFAULT_MAX_DISPLAY_LINES, DiffLine, DiffOp, DiffPreviewLine,
    DiffPreviewOp, DiffPreviewText, diff, diff_preview,
};

const FIXTURES: &[&str] = &["diff-basic", "diff-hunks"];

#[test]
fn unified_diff_fixtures_match_checked_in_goldens() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for &name in FIXTURES {
        assert_fixture(&root, name);
    }
}

#[test]
fn trailing_newline_changes_use_fx_markers() {
    let removed = diff(b"line\n", b"line");
    assert_eq!(removed[1].op, DiffOp::Remove);
    assert_eq!(removed[1].text, b"(trailing newline removed)");

    let added = diff(b"line", b"line\n");
    assert_eq!(added[1].op, DiffOp::Add);
    assert_eq!(added[1].text, b"(trailing newline added)");
}

fn assert_fixture(root: &Path, name: &str) {
    let old = fs::read(root.join(format!("{name}.old"))).expect("read old fixture");
    let new = fs::read(root.join(format!("{name}.new"))).expect("read new fixture");
    let golden = fs::read_to_string(root.join(format!("{name}.golden"))).expect("read golden");
    let (expected_full, expected_preview) = parse_golden(&golden);

    let actual_full = diff(&old, &new);
    assert_rows(name, &actual_full, &expected_full);

    let actual_preview = diff_preview(
        &old,
        &new,
        DEFAULT_CONTEXT_LINES,
        DEFAULT_MAX_DISPLAY_LINES,
    );
    assert_preview_rows(name, &actual_preview, &expected_preview);
}

#[derive(Debug, PartialEq, Eq)]
struct ExpectedRow {
    op: String,
    old_line: Option<u32>,
    new_line: Option<u32>,
    text: Vec<u8>,
}

fn parse_golden(text: &str) -> (Vec<ExpectedRow>, Vec<ExpectedRow>) {
    let mut full = Vec::new();
    let mut preview = Vec::new();
    for raw in text.lines() {
        let raw = raw.trim_end();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let (label, fields) = raw.split_once(": ").expect("golden row separator");
        let row = parse_row(fields);
        if label.starts_with("line ") {
            full.push(row);
        } else if label.starts_with("preview ") {
            preview.push(row);
        } else {
            panic!("unknown golden row {label:?}");
        }
    }
    (full, preview)
}

fn parse_row(fields: &str) -> ExpectedRow {
    let mut parts = fields.splitn(4, ' ');
    let op = parts.next().expect("golden op").to_owned();
    let old_line = parse_line_number(parts.next().expect("golden old line"));
    let new_line = parse_line_number(parts.next().expect("golden new line"));
    let text = parts
        .next()
        .expect("golden text")
        .strip_prefix("text=")
        .expect("golden text field")
        .as_bytes()
        .to_owned();
    ExpectedRow {
        op,
        old_line,
        new_line,
        text,
    }
}

fn parse_line_number(field: &str) -> Option<u32> {
    field
        .strip_prefix("old=")
        .or_else(|| field.strip_prefix("new="))
        .and_then(|number| number.parse().ok())
}

fn assert_rows(name: &str, actual: &[DiffLine<'_>], expected: &[ExpectedRow]) {
    assert_eq!(actual.len(), expected.len(), "{name}: full row count");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(format!("{:?}", actual.op).to_lowercase(), expected.op, "{name}: row {index}");
        assert_eq!(actual.old_line, expected.old_line, "{name}: row {index} old");
        assert_eq!(actual.new_line, expected.new_line, "{name}: row {index} new");
        assert_eq!(actual.text, expected.text, "{name}: row {index} text");
    }
}

fn assert_preview_rows(name: &str, actual: &[DiffPreviewLine<'_>], expected: &[ExpectedRow]) {
    assert_eq!(actual.len(), expected.len(), "{name}: preview row count");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let actual_op = match actual.op {
            DiffPreviewOp::Context => "context",
            DiffPreviewOp::Addition => "addition",
            DiffPreviewOp::Deletion => "deletion",
            DiffPreviewOp::Elision => "elision",
        };
        assert_eq!(actual_op, expected.op, "{name}: preview row {index}");
        assert_eq!(actual.old_line, expected.old_line, "{name}: preview row {index} old");
        assert_eq!(actual.new_line, expected.new_line, "{name}: preview row {index} new");
        match actual.text {
            DiffPreviewText::Source(text) => assert_eq!(text, expected.text, "{name}: preview row {index} text"),
            DiffPreviewText::Elision => assert_eq!(expected.text, "⋯".as_bytes(), "{name}: preview row {index} elision"),
        }
    }
}
