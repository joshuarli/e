//! Rustybench workload for the complete checked-in hi-lite golden corpus.
//!
//! Fixture loading and line splitting happen before the benchmark closure. The
//! warm benchmarks therefore measure the highlighter's steady-state work,
//! while the cold variants deliberately include scratch/highlighter setup so
//! allocation costs remain visible in rustybench's allocation report.

use hi_lite::{
    DEFAULT_CONTEXT_LINES, DEFAULT_MAX_DISPLAY_LINES, DiffOp, DiffPreviewOp, DiffPreviewText,
    DiffScratch, Highlighter, Kind, Language, diff, unified_preview,
};
use rustybench::counter::{BytesCount, ItemsCount};
use rustybench::{AllocProfiler, Bencher, black_box};

#[global_allocator]
static ALLOC: AllocProfiler = AllocProfiler::system();

const CORPUS_REPETITIONS: usize = 512;
const DIFF_REPETITIONS: usize = 512;

const DIFF_FIXTURES: &[(&[u8], &[u8])] = &[
    (
        include_bytes!("../tests/fixtures/diff-basic.old"),
        include_bytes!("../tests/fixtures/diff-basic.new"),
    ),
    (
        include_bytes!("../tests/fixtures/diff-hunks.old"),
        include_bytes!("../tests/fixtures/diff-hunks.new"),
    ),
];

struct Fixture {
    language: Language,
    lines: Vec<&'static [u8]>,
    bytes: usize,
}

const FIXTURE_SOURCES: &[(&str, Language, &str)] = &[
    (
        "bash",
        Language::Bash,
        include_str!("../tests/fixtures/bash.snippet"),
    ),
    (
        "c",
        Language::C,
        include_str!("../tests/fixtures/c.snippet"),
    ),
    (
        "css",
        Language::Css,
        include_str!("../tests/fixtures/css.snippet"),
    ),
    (
        "dockerfile",
        Language::Dockerfile,
        include_str!("../tests/fixtures/dockerfile.snippet"),
    ),
    (
        "go",
        Language::Go,
        include_str!("../tests/fixtures/go.snippet"),
    ),
    (
        "html",
        Language::Html,
        include_str!("../tests/fixtures/html.snippet"),
    ),
    (
        "ini",
        Language::Ini,
        include_str!("../tests/fixtures/ini.snippet"),
    ),
    (
        "javascript",
        Language::JavaScript,
        include_str!("../tests/fixtures/javascript.snippet"),
    ),
    (
        "json",
        Language::Json,
        include_str!("../tests/fixtures/json.snippet"),
    ),
    (
        "makefile",
        Language::Makefile,
        include_str!("../tests/fixtures/makefile.snippet"),
    ),
    (
        "markdown",
        Language::Markdown,
        include_str!("../tests/fixtures/markdown.snippet"),
    ),
    (
        "python",
        Language::Python,
        include_str!("../tests/fixtures/python.snippet"),
    ),
    (
        "rust",
        Language::Rust,
        include_str!("../tests/fixtures/rust.snippet"),
    ),
    (
        "toml",
        Language::Toml,
        include_str!("../tests/fixtures/toml.snippet"),
    ),
    (
        "typescript",
        Language::TypeScript,
        include_str!("../tests/fixtures/typescript.snippet"),
    ),
    (
        "yaml",
        Language::Yaml,
        include_str!("../tests/fixtures/yaml.snippet"),
    ),
];

fn load_fixtures() -> Vec<Fixture> {
    FIXTURE_SOURCES
        .iter()
        .map(|&(_name, language, source)| {
            let source = source.strip_suffix('\n').unwrap_or(source);
            let lines: Vec<_> = source.split('\n').map(str::as_bytes).collect();
            let bytes = lines.iter().map(|line| line.len()).sum();
            Fixture {
                language,
                lines,
                bytes,
            }
        })
        .collect()
}

fn corpus_totals(fixtures: &[Fixture]) -> (usize, usize) {
    fixtures.iter().fold((0, 0), |(bytes, lines), fixture| {
        (bytes + fixture.bytes, lines + fixture.lines.len())
    })
}

fn scratch_for(fixtures: &[Fixture]) -> Vec<Kind> {
    let capacity = fixtures
        .iter()
        .flat_map(|fixture| fixture.lines.iter().map(|line| line.len()))
        .max()
        .unwrap_or(0);
    Vec::with_capacity(capacity)
}

fn highlight_once(highlighter: &mut Highlighter, lines: &[&[u8]], scratch: &mut Vec<Kind>) -> u64 {
    let mut checksum = 0u64;
    for line in lines {
        for &kind in highlighter.highlight_into(line, scratch) {
            checksum = checksum
                .wrapping_mul(31)
                .wrapping_add(kind as u8 as u64 + 1);
        }
    }
    checksum
}

fn warm_fixture(
    fixture: &Fixture,
    repetitions: usize,
    highlighter: &mut Highlighter,
    scratch: &mut Vec<Kind>,
) -> u64 {
    let mut checksum = 0;
    for _ in 0..repetitions {
        highlighter.reset();
        checksum ^= highlight_once(&mut *highlighter, &fixture.lines, scratch);
    }
    checksum
}

fn cold_fixture(fixture: &Fixture, repetitions: usize) -> u64 {
    let mut checksum = 0;
    for _ in 0..repetitions {
        let mut highlighter = Highlighter::new(fixture.language);
        let mut scratch = Vec::new();
        checksum ^= highlight_once(&mut highlighter, &fixture.lines, &mut scratch);
    }
    checksum
}

fn warm_corpus(
    fixtures: &[Fixture],
    highlighters: &mut [Highlighter],
    repetitions: usize,
    scratch: &mut Vec<Kind>,
) -> u64 {
    let mut checksum = 0;
    for (fixture, highlighter) in fixtures.iter().zip(highlighters) {
        checksum ^= warm_fixture(fixture, repetitions, highlighter, scratch);
    }
    checksum
}

fn cold_corpus(fixtures: &[Fixture], repetitions: usize) -> u64 {
    fixtures
        .iter()
        .fold(0, |checksum, fixture| checksum ^ cold_fixture(fixture, repetitions))
}

fn configure_corpus_bench<'a, 'b>(
    bencher: Bencher<'a, 'b>,
    fixtures: &[Fixture],
) -> Bencher<'a, 'b> {
    let (bytes, lines) = corpus_totals(fixtures);
    bencher
        .counter(BytesCount::new(bytes * CORPUS_REPETITIONS))
        .counter(ItemsCount::new(lines * CORPUS_REPETITIONS))
}

// Keep one representative non-empty line per language so the small-input
// benchmark measures call latency without making fixture setup part of it.
fn single_lines(fixtures: &[Fixture]) -> Vec<&'static [u8]> {
    fixtures
        .iter()
        .map(|fixture| {
            fixture
                .lines
                .iter()
                .copied()
                .find(|line| !line.is_empty())
                .unwrap_or(&[])
        })
        .collect()
}

fn single_line_totals(lines: &[&[u8]]) -> (usize, usize) {
    (
        lines.iter().map(|line| line.len()).sum(),
        lines.len(),
    )
}

fn scratch_for_lines(lines: &[&[u8]]) -> Vec<Kind> {
    let capacity = lines.iter().map(|line| line.len()).max().unwrap_or(0);
    Vec::with_capacity(capacity)
}

fn warm_single_line_corpus(
    lines: &[&'static [u8]],
    highlighters: &mut [Highlighter],
    repetitions: usize,
    scratch: &mut Vec<Kind>,
) -> u64 {
    let mut checksum = 0;
    for _ in 0..repetitions {
        for (highlighter, line) in highlighters.iter_mut().zip(lines) {
            highlighter.reset();
            checksum ^= highlight_once(highlighter, std::slice::from_ref(line), scratch);
        }
    }
    checksum
}

fn cold_single_line_corpus(
    fixtures: &[Fixture],
    lines: &[&'static [u8]],
    repetitions: usize,
) -> u64 {
    let mut checksum = 0;
    for _ in 0..repetitions {
        for (fixture, line) in fixtures.iter().zip(lines) {
            let mut highlighter = Highlighter::new(fixture.language);
            let mut scratch = Vec::new();
            checksum ^= highlight_once(&mut highlighter, std::slice::from_ref(line), &mut scratch);
        }
    }
    checksum
}

fn configure_single_line_bench<'a, 'b>(
    bencher: Bencher<'a, 'b>,
    lines: &[&[u8]],
) -> Bencher<'a, 'b> {
    let (bytes, line_count) = single_line_totals(lines);
    bencher
        .counter(BytesCount::new(bytes * CORPUS_REPETITIONS))
        .counter(ItemsCount::new(line_count * CORPUS_REPETITIONS))
}

fn diff_totals() -> (usize, usize) {
    DIFF_FIXTURES.iter().fold((0, 0), |(bytes, lines), (old, new)| {
        let line_count = old.iter().chain(new.iter()).filter(|&&byte| byte == b'\n').count();
        (bytes + old.len() + new.len(), lines + line_count)
    })
}

fn diff_checksum(
    lines: &[hi_lite::DiffLine<'_>],
    preview: &[hi_lite::DiffPreviewLine<'_>],
) -> u64 {
    let mut checksum = 0u64;
    for line in lines {
        let op = match line.op {
            DiffOp::Equal => 1,
            DiffOp::Add => 2,
            DiffOp::Remove => 3,
        };
        checksum = checksum.wrapping_mul(31).wrapping_add(op);
        for &byte in line.text {
            checksum = checksum.wrapping_mul(31).wrapping_add(byte as u64);
        }
    }
    for line in preview {
        let op = match line.op {
            DiffPreviewOp::Context => 4,
            DiffPreviewOp::Addition => 5,
            DiffPreviewOp::Deletion => 6,
            DiffPreviewOp::Elision => 7,
        };
        checksum = checksum.wrapping_mul(31).wrapping_add(op);
        if let DiffPreviewText::Source(text) = line.text {
            for &byte in text {
                checksum = checksum.wrapping_mul(31).wrapping_add(byte as u64);
            }
        }
    }
    checksum
}

fn configure_diff_bench<'a, 'b>(bencher: Bencher<'a, 'b>) -> Bencher<'a, 'b> {
    let (bytes, lines) = diff_totals();
    bencher
        .counter(BytesCount::new(bytes * DIFF_REPETITIONS))
        .counter(ItemsCount::new(lines * DIFF_REPETITIONS))
}

fn warm_diff_corpus(
    scratch: &mut DiffScratch,
    lines: &mut Vec<hi_lite::DiffLine<'static>>,
    preview: &mut Vec<hi_lite::DiffPreviewLine<'static>>,
) -> u64 {
    let mut checksum = 0;
    for _ in 0..DIFF_REPETITIONS {
        for &(old, new) in DIFF_FIXTURES {
            scratch.diff_into(old, new, lines);
            scratch.unified_preview_into(
                lines,
                DEFAULT_CONTEXT_LINES,
                DEFAULT_MAX_DISPLAY_LINES,
                preview,
            );
            checksum ^= diff_checksum(lines, preview);
        }
    }
    checksum
}

fn cold_diff_corpus() -> u64 {
    let mut checksum = 0;
    for _ in 0..DIFF_REPETITIONS {
        for &(old, new) in DIFF_FIXTURES {
            let lines = diff(old, new);
            let preview = unified_preview(
                &lines,
                DEFAULT_CONTEXT_LINES,
                DEFAULT_MAX_DISPLAY_LINES,
            );
            checksum ^= diff_checksum(&lines, &preview);
        }
    }
    checksum
}

#[rustybench::bench]
fn hi_lite_highlight_all_goldens_warm(bencher: Bencher) {
    let fixtures = load_fixtures();
    let mut highlighters: Vec<_> = fixtures
        .iter()
        .map(|fixture| Highlighter::new(fixture.language))
        .collect();
    let mut scratch = scratch_for(&fixtures);
    configure_corpus_bench(bencher, &fixtures).bench_local(|| {
        black_box(warm_corpus(
            &fixtures,
            &mut highlighters,
            CORPUS_REPETITIONS,
            &mut scratch,
        ));
    });
}

#[rustybench::bench]
fn hi_lite_highlight_all_goldens_cold(bencher: Bencher) {
    let fixtures = load_fixtures();
    configure_corpus_bench(bencher, &fixtures).bench_local(|| {
        black_box(cold_corpus(&fixtures, CORPUS_REPETITIONS));
    });
}

#[rustybench::bench]
fn hi_lite_highlight_single_lines_warm(bencher: Bencher) {
    let fixtures = load_fixtures();
    let lines = single_lines(&fixtures);
    let mut highlighters: Vec<_> = fixtures
        .iter()
        .map(|fixture| Highlighter::new(fixture.language))
        .collect();
    let mut scratch = scratch_for_lines(&lines);
    configure_single_line_bench(bencher, &lines).bench_local(|| {
        black_box(warm_single_line_corpus(
            &lines,
            &mut highlighters,
            CORPUS_REPETITIONS,
            &mut scratch,
        ));
    });
}

#[rustybench::bench]
fn hi_lite_highlight_single_lines_cold(bencher: Bencher) {
    let fixtures = load_fixtures();
    let lines = single_lines(&fixtures);
    configure_single_line_bench(bencher, &lines).bench_local(|| {
        black_box(cold_single_line_corpus(&fixtures, &lines, CORPUS_REPETITIONS));
    });
}

#[rustybench::bench]
fn hi_lite_unified_diff_warm(bencher: Bencher) {
    let mut scratch = DiffScratch::new();
    let mut lines = Vec::new();
    let mut preview = Vec::new();
    for &(old, new) in DIFF_FIXTURES {
        scratch.diff_into(old, new, &mut lines);
        scratch.unified_preview_into(
            &lines,
            DEFAULT_CONTEXT_LINES,
            DEFAULT_MAX_DISPLAY_LINES,
            &mut preview,
        );
    }
    configure_diff_bench(bencher).bench_local(|| {
        black_box(warm_diff_corpus(&mut scratch, &mut lines, &mut preview));
    });
}

#[rustybench::bench]
fn hi_lite_unified_diff_cold(bencher: Bencher) {
    configure_diff_bench(bencher).bench_local(|| {
        black_box(cold_diff_corpus());
    });
}

macro_rules! fixture_benchmarks {
    ($(($warm_function:ident, $cold_function:ident, $index:expr)),+ $(,)?) => {
        $(
            #[rustybench::bench]
            fn $warm_function(bencher: Bencher) {
                let fixtures = load_fixtures();
                let fixture = &fixtures[$index];
                let mut highlighter = Highlighter::new(fixture.language);
                let mut scratch = scratch_for(std::slice::from_ref(fixture));
                configure_corpus_bench(bencher, std::slice::from_ref(fixture))
                    .bench_local(|| {
                        black_box(warm_fixture(
                            fixture,
                            CORPUS_REPETITIONS,
                            &mut highlighter,
                            &mut scratch,
                        ));
                    });
            }

            #[rustybench::bench]
            fn $cold_function(bencher: Bencher) {
                let fixtures = load_fixtures();
                let fixture = &fixtures[$index];
                configure_corpus_bench(bencher, std::slice::from_ref(fixture))
                    .bench_local(|| {
                        black_box(cold_fixture(fixture, CORPUS_REPETITIONS));
                    });
            }
        )+
    };
}

fixture_benchmarks!(
    (hi_lite_bash, hi_lite_bash_cold, 0),
    (hi_lite_c, hi_lite_c_cold, 1),
    (hi_lite_css, hi_lite_css_cold, 2),
    (hi_lite_dockerfile, hi_lite_dockerfile_cold, 3),
    (hi_lite_go, hi_lite_go_cold, 4),
    (hi_lite_html, hi_lite_html_cold, 5),
    (hi_lite_ini, hi_lite_ini_cold, 6),
    (hi_lite_javascript, hi_lite_javascript_cold, 7),
    (hi_lite_json, hi_lite_json_cold, 8),
    (hi_lite_makefile, hi_lite_makefile_cold, 9),
    (hi_lite_markdown, hi_lite_markdown_cold, 10),
    (hi_lite_python, hi_lite_python_cold, 11),
    (hi_lite_rust, hi_lite_rust_cold, 12),
    (hi_lite_toml, hi_lite_toml_cold, 13),
    (hi_lite_typescript, hi_lite_typescript_cold, 14),
    (hi_lite_yaml, hi_lite_yaml_cold, 15),
);

fn main() {
    rustybench::main();
}
