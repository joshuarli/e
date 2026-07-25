//! Wall-time benchmarks for editor hot paths.

#[cfg(target_os = "linux")]
use std::ffi::CStr;
use std::fs;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use divan::{AllocProfiler, Bencher, black_box};

#[global_allocator]
static ALLOC: AllocProfiler = AllocProfiler::system();

use e::buffer::GapBuffer;
use e::document::Document;
use e::find::FindState;
use e::highlight::{self, HlState, SyntaxRules};
use e::render::Renderer;
use e::selection::{Pos, Selection};
use e::view::View;

#[cfg(target_os = "linux")]
const TRACE_BEGIN: &CStr = c"BENCH_BEGIN";
#[cfg(target_os = "linux")]
const TRACE_END: &CStr = c"BENCH_END";

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn prctl(option: i32, ...) -> i32;
}

#[cfg(target_os = "linux")]
fn syscall_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("SYSCALL_TRACE").is_some())
}

#[cfg(target_os = "linux")]
fn trace_marker(marker: &CStr) {
    if syscall_trace_enabled() {
        unsafe {
            let _ = prctl(15, marker.as_ptr(), 0, 0, 0);
        }
    }
}

#[cfg(target_os = "linux")]
fn bench_with_syscall_trace<O>(bencher: Bencher, mut operation: impl FnMut() -> O) {
    bencher.bench_local(|| {
        trace_marker(TRACE_BEGIN);
        let result = operation();
        trace_marker(TRACE_END);
        black_box(result);
    });
}

#[cfg(not(target_os = "linux"))]
fn bench_with_syscall_trace<O>(bencher: Bencher, operation: impl FnMut() -> O) {
    bencher.bench_local(operation);
}

struct BenchDir(PathBuf);

impl BenchDir {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("e-bench-{label}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for BenchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn make_rust_source(n: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(n * 40);
    for i in 0..n {
        match i % 5 {
            0 => buf.extend_from_slice(b"    fn example_function(x: usize) -> bool {\n"),
            1 => buf.extend_from_slice(b"        let result = x * 2 + 1; // compute\n"),
            2 => buf.extend_from_slice(b"        if result > 100 { return false; }\n"),
            3 => buf.extend_from_slice(b"        println!(\"value: {}\", result);\n"),
            _ => buf.extend_from_slice(b"    }\n"),
        }
    }
    buf
}

fn make_json(n: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(n * 30);
    buf.extend_from_slice(b"{\n");
    for i in 0..n.saturating_sub(2) {
        let comma = if i + 1 < n.saturating_sub(2) { "," } else { "" };
        buf.extend_from_slice(format!("  \"key_{}\": \"value_{}\" {}\n", i, i, comma).as_bytes());
    }
    buf.extend_from_slice(b"}\n");
    buf
}

#[divan::bench]
fn file_read_1k(bencher: Bencher) {
    let dir = BenchDir::new("read");
    let path = dir.0.join("fixture.rs");
    let data = make_rust_source(1_000);
    fs::write(&path, &data).unwrap();
    bench_with_syscall_trace(bencher, || black_box(e::file_io::read_file(&path).unwrap()));
}

#[divan::bench]
fn file_write_1k(bencher: Bencher) {
    let dir = BenchDir::new("write");
    let path = dir.0.join("fixture.rs");
    let data = make_rust_source(1_000);
    bench_with_syscall_trace(bencher, || {
        black_box(e::file_io::write_file(&path, &data).unwrap())
    });
}

macro_rules! gap_benchmarks {
    ($($name:ident: $operation:expr),+ $(,)?) => {
        $(
            #[divan::bench]
            fn $name(b: Bencher) {
                let data = make_rust_source($operation.0);
                $operation.1(b, &data);
            }
        )+
    };
}

fn gap_from_vec(b: Bencher, data: &[u8]) {
    bench_with_syscall_trace(b, || black_box(GapBuffer::from_vec(data.to_vec())));
}

fn gap_insert_sequential(b: Bencher, data: &[u8]) {
    bench_with_syscall_trace(b, || {
        let mut buf = GapBuffer::from_vec(data.to_vec());
        let end = buf.len();
        for i in 0..100 {
            buf.insert(end + i, b"x");
        }
        black_box(&buf);
    });
}

fn gap_pos_to_offset_all_lines(b: Bencher, data: &[u8]) {
    let buf = GapBuffer::from_vec(data.to_vec());
    bench_with_syscall_trace(b, || {
        for line in 0..buf.line_count() {
            black_box(buf.pos_to_offset(line, 0));
        }
    });
}

fn gap_offset_to_pos_walk(b: Bencher, data: &[u8]) {
    let buf = GapBuffer::from_vec(data.to_vec());
    let len = buf.len();
    let step = len / 100;
    bench_with_syscall_trace(b, || {
        let mut offset = 0;
        while offset < len {
            black_box(buf.offset_to_pos(offset));
            offset += step.max(1);
        }
    });
}

fn gap_line_text_all(b: Bencher, data: &[u8]) {
    let buf = GapBuffer::from_vec(data.to_vec());
    bench_with_syscall_trace(b, || {
        for line in 0..buf.line_count() {
            black_box(buf.line_text(line));
        }
    });
}

macro_rules! gap_size_set {
    ($size:literal, $from_vec:ident, $insert:ident, $pos:ident, $offset:ident, $line:ident) => {
        gap_benchmarks! {
            $from_vec: ($size, gap_from_vec),
            $insert: ($size, gap_insert_sequential),
            $pos: ($size, gap_pos_to_offset_all_lines),
            $offset: ($size, gap_offset_to_pos_walk),
            $line: ($size, gap_line_text_all),
        }
    };
}

gap_size_set!(
    1000,
    gap_from_vec_1000,
    gap_insert_sequential_1000,
    gap_pos_to_offset_all_lines_1000,
    gap_offset_to_pos_walk_1000,
    gap_line_text_all_1000
);
gap_size_set!(
    5000,
    gap_from_vec_5000,
    gap_insert_sequential_5000,
    gap_pos_to_offset_all_lines_5000,
    gap_offset_to_pos_walk_5000,
    gap_line_text_all_5000
);

macro_rules! highlight_benchmarks {
    ($rust_name:ident, $json_name:ident, $into_name:ident, $size:literal) => {
        #[divan::bench]
        fn $rust_name(b: Bencher) {
            let data = make_rust_source($size);
            let rules = highlight::rules_for_language("Rust").unwrap();
            bench_with_syscall_trace(b, || {
                let mut state = HlState::default();
                for line in data.split(|&byte| byte == b'\n') {
                    let (highlighted, next) = highlight::highlight_line(line, state, rules);
                    state = next;
                    black_box(&highlighted);
                }
            });
        }

        #[divan::bench]
        fn $json_name(b: Bencher) {
            let data = make_json($size);
            let rules = highlight::rules_for_language("JSON").unwrap();
            bench_with_syscall_trace(b, || {
                let mut state = HlState::default();
                for line in data.split(|&byte| byte == b'\n') {
                    let (highlighted, next) = highlight::highlight_line(line, state, rules);
                    state = next;
                    black_box(&highlighted);
                }
            });
        }

        #[divan::bench]
        fn $into_name(b: Bencher) {
            let data = make_rust_source($size);
            let rules = highlight::rules_for_language("Rust").unwrap();
            let mut output = Vec::new();
            bench_with_syscall_trace(b, || {
                let mut state = HlState::default();
                for line in data.split(|&byte| byte == b'\n') {
                    state = highlight::highlight_line_into(line, state, rules, &[], &mut output);
                    black_box(&output);
                }
            });
        }
    };
}

highlight_benchmarks!(
    highlight_rust_1000,
    highlight_json_1000,
    highlight_rust_into_1000,
    1000
);

#[divan::bench]
fn document_insert_100_seal_undo_all(b: Bencher) {
    let data = make_rust_source(500);
    bench_with_syscall_trace(b, || {
        let mut doc = Document::new(data.clone(), None);
        for i in 0..10 {
            let line = i % doc.buf.line_count();
            doc.insert(line, 0, b"// ");
            doc.seal_undo();
        }
        while doc.undo().is_some() {}
        black_box(&doc);
    });
}

#[divan::bench]
fn document_insert_delete_interleaved(b: Bencher) {
    let data = make_rust_source(500);
    bench_with_syscall_trace(b, || {
        let mut doc = Document::new(data.clone(), None);
        for _ in 0..5 {
            let line = doc.buf.line_count() / 2;
            let pos = doc.insert(line, 0, b"new line\n");
            doc.seal_undo();
            doc.delete_range(Pos::new(pos.line, 0), pos);
            doc.seal_undo();
        }
        black_box(&doc);
    });
}

macro_rules! search_benchmarks {
    ($forward:ident, $backward:ident, $size:literal) => {
        #[divan::bench]
        fn $forward(b: Bencher) {
            let data = make_rust_source($size);
            let buf = GapBuffer::from_vec(data.clone());
            let re = regex_lite::Regex::new("ZZNOTFOUND").unwrap();
            bench_with_syscall_trace(b, || {
                black_box(FindState::search_forward(&buf, &re, Pos::zero()))
            });
        }

        #[divan::bench]
        fn $backward(b: Bencher) {
            let data = make_rust_source($size);
            let buf = GapBuffer::from_vec(data.clone());
            let re = regex_lite::Regex::new("ZZNOTFOUND").unwrap();
            let last = Pos::new(buf.line_count().saturating_sub(1), 0);
            bench_with_syscall_trace(b, || black_box(FindState::search_backward(&buf, &re, last)));
        }
    };
}

search_benchmarks!(search_forward_miss_1000, search_backward_miss_1000, 1000);

#[divan::bench]
fn viewport_ensure_cursor_visible_jump(b: Bencher) {
    let buf = GapBuffer::from_vec(make_rust_source(1_000));
    bench_with_syscall_trace(b, || {
        let mut view = View::new(120, 40);
        let mut widths = |line: usize| buf.display_col_at(line, usize::MAX);
        for line in (0..buf.line_count()).step_by(10) {
            view.ensure_cursor_visible(line, 0, 5, &mut widths);
        }
        black_box(&view);
    });
}

fn render_setup(
    data: &[u8],
    width: u16,
    height: u16,
    syntax: Option<&'static SyntaxRules>,
) -> (Renderer, GapBuffer, View) {
    let mut renderer = Renderer::new();
    renderer.set_syntax(syntax);
    let buffer = GapBuffer::from_vec(data.to_vec());
    let mut view = View::new(width, height);
    view.scroll_line = buffer.line_count() / 2;
    (renderer, buffer, view)
}

fn render_frame(
    b: Bencher,
    data: &[u8],
    width: u16,
    height: u16,
    selection: Option<Selection>,
    syntax: Option<&'static SyntaxRules>,
) {
    let (mut renderer, mut buffer, view) = render_setup(data, width, height, syntax);
    let cursor_line = view.scroll_line;
    let mut sink = Vec::with_capacity(32 * 1024);
    bench_with_syscall_trace(b, || {
        sink.clear();
        renderer.needs_full_redraw = true;
        renderer
            .render(
                &mut sink,
                &mut buffer,
                &view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                selection,
                &[],
                &[],
                None,
                None,
                &[],
                None,
                false,
                None,
            )
            .unwrap();
        black_box(&sink);
    });
}

#[divan::bench]
fn render_frame_120x40_1k_syntax(b: Bencher) {
    let data = make_rust_source(1_000);
    let rules = highlight::rules_for_language("Rust");
    render_frame(b, &data, 120, 40, None, rules);
}

#[divan::bench]
fn render_frame_120x40_1k_selection(b: Bencher) {
    let data = make_rust_source(1_000);
    let rules = highlight::rules_for_language("Rust");
    let line = 1_000 / 2;
    let selection = Selection {
        anchor: Pos::new(line, 0),
        cursor: Pos::new(line + 5, 10),
    };
    render_frame(b, &data, 120, 40, Some(selection), rules);
}

#[divan::bench]
fn render_frame_120x40_1k_plain(b: Bencher) {
    let data = make_rust_source(1_000);
    render_frame(b, &data, 120, 40, None, None);
}

fn render_incremental(
    b: Bencher,
    mut movement: impl FnMut(&mut Renderer, &mut GapBuffer, &mut Vec<u8>, &View, usize),
) {
    let data = make_rust_source(1_000);
    let rules = highlight::rules_for_language("Rust");
    let (mut renderer, mut buffer, view) = render_setup(&data, 120, 40, rules);
    let cursor_line = view.scroll_line;
    let mut sink = Vec::with_capacity(32 * 1024);
    renderer.needs_full_redraw = true;
    renderer
        .render(
            &mut sink,
            &mut buffer,
            &view,
            cursor_line,
            0,
            true,
            " test.rs",
            " e v0.1.5 ",
            None,
            None,
            &[],
            &[],
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();
    bench_with_syscall_trace(b, || {
        movement(&mut renderer, &mut buffer, &mut sink, &view, cursor_line)
    });
}

#[divan::bench]
fn render_incr_noop_120x40(b: Bencher) {
    render_incremental(b, |renderer, buffer, sink, view, cursor_line| {
        sink.clear();
        renderer
            .render(
                sink,
                buffer,
                view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
                &[],
                &[],
                None,
                None,
                &[],
                None,
                false,
                None,
            )
            .unwrap();
        black_box(sink);
    });
}

#[divan::bench]
fn render_incr_cursor_move_120x40(b: Bencher) {
    let mut current = None;
    render_incremental(b, |renderer, buffer, sink, view, cursor_line| {
        let line = current.take().unwrap_or(cursor_line + 1);
        current = Some(if line == cursor_line {
            cursor_line + 1
        } else {
            cursor_line
        });
        sink.clear();
        renderer
            .render(
                sink,
                buffer,
                view,
                line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
                &[],
                &[],
                None,
                None,
                &[],
                None,
                false,
                None,
            )
            .unwrap();
        black_box(sink);
    });
}

#[divan::bench]
fn render_incr_scroll_120x40(b: Bencher) {
    let mut down = true;
    render_incremental(b, |renderer, buffer, sink, view, cursor_line| {
        let mut shifted = view.clone();
        if down {
            shifted.scroll_line += 3;
        }
        down = !down;
        sink.clear();
        renderer
            .render(
                sink,
                buffer,
                &shifted,
                if shifted.scroll_line == view.scroll_line {
                    cursor_line
                } else {
                    cursor_line + 3
                },
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
                &[],
                &[],
                None,
                None,
                &[],
                None,
                false,
                None,
            )
            .unwrap();
        black_box(sink);
    });
}

fn main() {
    divan::main();
}
