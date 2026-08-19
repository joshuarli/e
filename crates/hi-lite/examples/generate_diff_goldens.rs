use std::fmt::Write;
use std::fs;
use std::path::Path;

use hi_lite::{
    DEFAULT_CONTEXT_LINES, DEFAULT_MAX_DISPLAY_LINES, DiffLine, DiffOp, DiffPreviewLine,
    DiffPreviewOp, DiffPreviewText, diff_preview,
};

const FIXTURES: &[&str] = &["diff-basic", "diff-hunks"];

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for &name in FIXTURES {
        let old = fs::read(root.join(format!("{name}.old"))).expect("read old fixture");
        let new = fs::read(root.join(format!("{name}.new"))).expect("read new fixture");
        let full = hi_lite::diff(&old, &new);
        let preview = diff_preview(
            &old,
            &new,
            DEFAULT_CONTEXT_LINES,
            DEFAULT_MAX_DISPLAY_LINES,
        );
        let mut golden = String::from("# hi-lite-diff-v1\n");
        for (index, line) in full.iter().enumerate() {
            write_full(&mut golden, index + 1, *line);
        }
        golden.push('\n');
        for (index, line) in preview.iter().enumerate() {
            write_preview(&mut golden, index + 1, *line);
        }
        fs::write(root.join(format!("{name}.golden")), golden).expect("write golden");
    }
}

fn write_full(output: &mut String, index: usize, line: DiffLine<'_>) {
    let _ = writeln!(
        output,
        "line {index}: {} old={} new={} text={}",
        full_op(line.op),
        line_number(line.old_line),
        line_number(line.new_line),
        String::from_utf8_lossy(line.text),
    );
}

fn write_preview(output: &mut String, index: usize, line: DiffPreviewLine<'_>) {
    let text = match line.text {
        DiffPreviewText::Source(text) => String::from_utf8_lossy(text),
        DiffPreviewText::Elision => "⋯".into(),
    };
    let _ = writeln!(
        output,
        "preview {index}: {} old={} new={} text={text}",
        preview_op(line.op),
        line_number(line.old_line),
        line_number(line.new_line),
    );
}

fn line_number(number: Option<u32>) -> String {
    number.map_or_else(|| "-".to_owned(), |number| number.to_string())
}

fn full_op(op: DiffOp) -> &'static str {
    match op {
        DiffOp::Equal => "equal",
        DiffOp::Add => "add",
        DiffOp::Remove => "remove",
    }
}

fn preview_op(op: DiffPreviewOp) -> &'static str {
    match op {
        DiffPreviewOp::Context => "context",
        DiffPreviewOp::Addition => "addition",
        DiffPreviewOp::Deletion => "deletion",
        DiffPreviewOp::Elision => "elision",
    }
}
