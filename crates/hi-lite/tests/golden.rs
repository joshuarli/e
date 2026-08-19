//! Fixture comparison against checked-in semantic highlight goldens.
//!
//! The `.snippet` files are syntax-complete examples. Their `.golden` siblings
//! contain one `start..end=kind` run list per source line. Goldens are generated
//! from syntect by the standalone tool documented in `tools/hi-lite-goldens.md`;
//! normal tests intentionally need only this crate and the checked-in files.

use std::fs;
use std::path::{Path, PathBuf};

use hi_lite::{Highlighter, Kind, Language, Run, runs};

const FIXTURES: &[(&str, Language)] = &[
    ("bash", Language::Bash),
    ("c", Language::C),
    ("css", Language::Css),
    ("dockerfile", Language::Dockerfile),
    ("go", Language::Go),
    ("html", Language::Html),
    ("ini", Language::Ini),
    ("javascript", Language::JavaScript),
    ("json", Language::Json),
    ("makefile", Language::Makefile),
    ("markdown", Language::Markdown),
    ("python", Language::Python),
    ("rust", Language::Rust),
    ("toml", Language::Toml),
    ("typescript", Language::TypeScript),
    ("yaml", Language::Yaml),
];

#[test]
fn syntax_complete_fixtures_match_syntect_goldens() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for &(name, language) in FIXTURES {
        assert_fixture(&root, name, language);
    }
}

fn assert_fixture(root: &Path, name: &str, language: Language) {
    let source_path = root.join(format!("{name}.snippet"));
    let golden_path = root.join(format!("{name}.golden"));
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("{}: {error}", source_path.display()));
    let golden = fs::read_to_string(&golden_path)
        .unwrap_or_else(|error| panic!("{}: {error}", golden_path.display()));
    let expected = parse_golden(&golden, &golden_path);

    let source = source.strip_suffix('\n').unwrap_or(&source);
    let mut highlighter = Highlighter::new(language);
    let mut scratch = Vec::new();
    for (line_index, line) in source.split('\n').enumerate() {
        let actual: Vec<_> = runs(highlighter.highlight_into(line.as_bytes(), &mut scratch)).collect();
        let expected_line = expected.get(line_index).cloned().unwrap_or_default();
        assert_eq!(actual, expected_line, "{name} line {}: {line:?}", line_index + 1);
    }
    assert_eq!(expected.len(), source.split('\n').count(), "{name}: golden line count");
}

fn parse_golden(text: &str, path: &Path) -> Vec<Vec<Run>> {
    let mut lines = Vec::new();
    for (line_number, raw) in text.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let (line_label, runs_text) = raw
            .split_once(':')
            .unwrap_or_else(|| panic!("{}:{}: expected `line N:`", path.display(), line_number + 1));
        let source_line = line_label
            .strip_prefix("line ")
            .and_then(|number| number.parse::<usize>().ok())
            .unwrap_or_else(|| panic!("{}:{}: invalid line label", path.display(), line_number + 1));
        while lines.len() < source_line {
            lines.push(Vec::new());
        }
        let mut parsed: Vec<Run> = Vec::new();
        for item in runs_text.split_whitespace() {
            let (range, kind) = item
                .split_once('=')
                .unwrap_or_else(|| panic!("{}:{}: invalid run {item:?}", path.display(), line_number + 1));
            let (start, end) = range
                .split_once("..")
                .and_then(|(start, end)| Some((start.parse().ok()?, end.parse().ok()?)))
                .unwrap_or_else(|| panic!("{}:{}: invalid range {range:?}", path.display(), line_number + 1));
            let kind = parse_kind(kind, path, line_number + 1);
            if let Some(previous) = parsed.last_mut()
                && previous.end == start
                && previous.kind == kind
            {
                previous.end = end;
            } else {
                parsed.push(Run { start, end, kind });
            }
        }
        lines[source_line - 1] = parsed;
    }
    lines
}

fn parse_kind(kind: &str, path: &Path, line: usize) -> Kind {
    match kind {
        "normal" => Kind::Normal,
        "keyword" => Kind::Keyword,
        "type" => Kind::Type,
        "string" => Kind::String,
        "comment" => Kind::Comment,
        "number" => Kind::Number,
        "bracket" => Kind::Bracket,
        "operator" => Kind::Operator,
        "function" => Kind::Function,
        "constant" => Kind::Constant,
        "macro" => Kind::Macro,
        _ => panic!("{}:{}: unknown kind {kind:?}", path.display(), line),
    }
}

#[allow(dead_code)]
fn _fixture_paths(root: &Path) -> Vec<PathBuf> {
    FIXTURES.iter().map(|(name, _)| root.join(format!("{name}.snippet"))).collect()
}
