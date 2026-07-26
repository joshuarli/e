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
use e::command::CommandRegistry;
use e::command_buffer::{CommandBuffer, CommandBufferMode, CommandBufferResult};
use e::document::Document;
use e::find::FindState;
use e::highlight::{self, SyntaxRules};
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

fn append_python_line(buf: &mut Vec<u8>, index: usize) {
    match index % 5 {
        0 => buf.extend_from_slice(
            format!(
                "benchmark_token_{index:05}: dict[str, int | bool] = {{\"value\": {index}, \"scaled\": {index} * 3, \"even\": {index} % 2 == 0}}\n"
            )
            .as_bytes(),
        ),
        1 => buf.extend_from_slice(
            format!(
                "benchmark_token_{index:05}_list = [value * 2 for value in range({}) if value >= 0]\n",
                index % 7
            )
            .as_bytes(),
        ),
        2 => buf.extend_from_slice(
            format!(
                "benchmark_token_{index:05}_name = f\"item-{{{index}:05d}}-{{{}}}\"\n",
                index % 3
            )
            .as_bytes(),
        ),
        3 => buf.extend_from_slice(
            format!(
                "benchmark_token_{index:05}_result = ({} * 2 if {} % 2 == 0 else {} + 1)\n",
                index, index, index
            )
            .as_bytes(),
        ),
        _ => buf.extend_from_slice(
            format!(
                "benchmark_token_{index:05}_call = sorted({{value: value * value for value in range({})}}.items(), key=lambda pair: pair[1])\n",
                index % 5
            )
            .as_bytes(),
        ),
    }
}

fn make_python_source(lines: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(lines * 128);
    for index in 0..lines {
        append_python_line(&mut buf, index);
    }
    buf
}

fn make_python_source_bytes(target: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(target);
    let mut index = 0;
    while buf.len() < target {
        append_python_line(&mut buf, index);
        index += 1;
    }
    buf
}

#[divan::bench]
fn file_read_1000_lines(bencher: Bencher) {
    let dir = BenchDir::new("read");
    let path = dir.0.join("fixture.rs");
    let data = make_rust_source(1_000);
    fs::write(&path, &data).unwrap();
    bench_with_syscall_trace(bencher, || black_box(e::file_io::read_file(&path).unwrap()));
}

#[divan::bench]
fn file_write_1000_lines(bencher: Bencher) {
    let dir = BenchDir::new("write");
    let path = dir.0.join("fixture.rs");
    let data = make_rust_source(1_000);
    bench_with_syscall_trace(bencher, || {
        e::file_io::write_file(&path, &data).unwrap();
        black_box(())
    });
}

#[divan::bench]
fn document_insert_10_seal_undo_all(b: Bencher) {
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

#[divan::bench]
fn edit_and_render_python_10k(b: Bencher) {
    let data = make_python_source(10_000);
    let rules = highlight::rules_for_language("Python");
    let mut doc = Document::new(data, Some("fixture.py".to_string()));
    let mut renderer = Renderer::new();
    renderer.set_syntax(rules);
    let mut view = View::new(120, 40);
    let cursor = Pos::new(5_000, 12);
    view.scroll_line = cursor.line;
    let mut sink = Vec::with_capacity(32 * 1024);
    render_viewport(&mut renderer, &mut doc.buf, &view, cursor, None, &mut sink);

    bench_with_syscall_trace(b, || {
        let after = doc.insert(cursor.line, cursor.col, b"x");
        render_viewport(&mut renderer, &mut doc.buf, &view, after, None, &mut sink);
        doc.undo();
        black_box((after, sink.len()));
    });
}

#[divan::bench]
fn edit_and_render_python_short(b: Bencher) {
    let data = make_python_source(40);
    let rules = highlight::rules_for_language("Python");
    let mut doc = Document::new(data, Some("fixture.py".to_string()));
    let mut renderer = Renderer::new();
    renderer.set_syntax(rules);
    let view = View::new(120, 40);
    let cursor = Pos::new(5, 12);
    let mut sink = Vec::with_capacity(32 * 1024);
    render_viewport(&mut renderer, &mut doc.buf, &view, cursor, None, &mut sink);

    bench_with_syscall_trace(b, || {
        let after = doc.insert(cursor.line, cursor.col, b"x");
        render_viewport(&mut renderer, &mut doc.buf, &view, after, None, &mut sink);
        doc.undo();
        black_box((after, sink.len()));
    });
}

#[divan::bench]
fn find_update_python_10k(b: Bencher) {
    let buf = GapBuffer::from_vec(make_python_source(10_000));
    let mut view = View::new(120, 40);
    view.scroll_line = 5_000;
    let mut find = FindState::new();
    bench_with_syscall_trace(b, || {
        find.update_highlights_lazy(r"benchmark_token_\d+", &buf, &view);
        black_box((find.total_count, find.matches.len()));
    });
}

#[divan::bench]
fn paste_multiline_python_100k_into_10k(b: Bencher) {
    let mut doc = Document::new(make_python_source(10_000), Some("fixture.py".to_string()));
    let paste = make_python_source_bytes(100 * 1024);
    let cursor = Pos::new(5_000, 12);
    bench_with_syscall_trace(b, || {
        let after = doc.insert(cursor.line, cursor.col, &paste);
        doc.undo();
        black_box(after);
    });
}

#[divan::bench]
fn replace_all_python_10k(b: Bencher) {
    let data = make_python_source(10_000);
    let pattern = regex_lite::Regex::new(r"benchmark_token_\d+").unwrap();
    bench_with_syscall_trace(b, || {
        let mut doc = Document::new(data.clone(), Some("fixture.py".to_string()));
        let last_line = doc.buf.line_count().saturating_sub(1);
        let end = Pos::new(last_line, doc.buf.line_char_len(last_line));
        let text_bytes = doc.text_in_range(Pos::zero(), end);
        let text = String::from_utf8_lossy(&text_bytes);
        let count = pattern.find_iter(&text).count();
        let replacement = pattern.replace_all(&text, "replacement_token").into_owned();
        doc.seal_undo();
        doc.replace_range_with_deleted(Pos::zero(), end, replacement.as_bytes(), text_bytes);
        doc.seal_undo();
        black_box((count, doc.buf.len()));
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

fn render_viewport(
    renderer: &mut Renderer,
    buffer: &mut GapBuffer,
    view: &View,
    cursor: Pos,
    selection: Option<Selection>,
    sink: &mut Vec<u8>,
) {
    render_editor_frame(
        renderer, buffer, view, cursor, selection, None, None, None, false, sink,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_editor_frame(
    renderer: &mut Renderer,
    buffer: &mut GapBuffer,
    view: &View,
    cursor: Pos,
    selection: Option<Selection>,
    command_line: Option<&str>,
    find_matches: Option<&[(Pos, Pos)]>,
    find_current: Option<(Pos, Pos)>,
    find_active: bool,
    sink: &mut Vec<u8>,
) {
    sink.clear();
    renderer
        .render(
            sink,
            buffer,
            view,
            cursor.line,
            cursor.col,
            true,
            " fixture.py",
            " e v0.1.13 ",
            command_line,
            selection,
            &[],
            &[],
            find_matches,
            find_current,
            &[],
            command_line.map(|line| line.len()),
            find_active,
            None,
        )
        .unwrap();
}

#[divan::bench]
fn command_palette_trace_python_10k(b: Bencher) {
    let data = make_python_source(10_000);
    let mut doc = Document::new(data, Some("fixture.py".to_string()));
    let mut renderer = Renderer::new();
    renderer.set_syntax(highlight::rules_for_language("Python"));
    let view = View::new(120, 40);
    let cursor = Pos::new(5_000, 12);
    let mut command = CommandBuffer::new();
    let registry = CommandRegistry::new();
    let mut sink = Vec::with_capacity(32 * 1024);

    bench_with_syscall_trace(b, || {
        command.open(CommandBufferMode::Command, "> ", "");
        for ch in "goto 5000".chars() {
            let result = command.handle_key(e::input::Key::Char(ch));
            if matches!(result, CommandBufferResult::Changed(_)) {
                let line = command.display_line();
                render_editor_frame(
                    &mut renderer,
                    &mut doc.buf,
                    &view,
                    cursor,
                    None,
                    Some(&line),
                    None,
                    None,
                    false,
                    &mut sink,
                );
            }
        }
        let result = command.handle_key(e::input::Key::Char('\n'));
        if let CommandBufferResult::Submit(input) = result {
            let action = registry.execute(&input);
            command.close();
            render_editor_frame(
                &mut renderer,
                &mut doc.buf,
                &view,
                cursor,
                None,
                None,
                None,
                None,
                false,
                &mut sink,
            );
            black_box(action);
        }
        black_box((&command, &sink));
    });
}

#[divan::bench]
fn find_command_trace_python_10k(b: Bencher) {
    let data = make_python_source(10_000);
    let mut doc = Document::new(data, Some("fixture.py".to_string()));
    let mut renderer = Renderer::new();
    renderer.set_syntax(highlight::rules_for_language("Python"));
    let view = View::new(120, 40);
    let cursor = Pos::new(5_000, 12);
    let mut command = CommandBuffer::new();
    let mut find = FindState::new();
    let mut sink = Vec::with_capacity(32 * 1024);

    bench_with_syscall_trace(b, || {
        command.open(CommandBufferMode::Find, "find: ", "");
        find.clear();
        for ch in "benchmark_token_123".chars() {
            let result = command.handle_key(e::input::Key::Char(ch));
            if let CommandBufferResult::Changed(pattern) = result {
                find.update_highlights_lazy(&pattern, &doc.buf, &view);
                let line = command.display_line();
                render_editor_frame(
                    &mut renderer,
                    &mut doc.buf,
                    &view,
                    cursor,
                    None,
                    Some(&line),
                    Some(&find.matches),
                    find.current,
                    false,
                    &mut sink,
                );
            }
        }
        find.update_highlights(command.input.as_str(), &doc.buf, &view);
        command.close();
        render_editor_frame(
            &mut renderer,
            &mut doc.buf,
            &view,
            cursor,
            None,
            None,
            Some(&find.matches),
            find.current,
            true,
            &mut sink,
        );
        black_box((&find, &sink));
    });
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
        renderer.needs_full_redraw = true;
        render_viewport(
            &mut renderer,
            &mut buffer,
            &view,
            Pos::new(cursor_line, 0),
            selection,
            &mut sink,
        );
        black_box(&sink);
    });
}

fn open_and_render_python(b: Bencher, data: &[u8], label: &str) {
    let dir = BenchDir::new(label);
    let path = dir.0.join("fixture.py");
    fs::write(&path, data).unwrap();
    let rules = highlight::rules_for_language("Python");
    bench_with_syscall_trace(b, || {
        let loaded = e::file_io::read_file(&path).unwrap();
        let mut buffer = GapBuffer::from_vec(loaded);
        let mut renderer = Renderer::new();
        renderer.set_syntax(rules);
        let mut view = View::new(120, 40);
        view.scroll_line = buffer.line_count() / 2;
        let cursor = Pos::new(view.scroll_line, 0);
        let mut sink = Vec::with_capacity(32 * 1024);
        render_viewport(&mut renderer, &mut buffer, &view, cursor, None, &mut sink);
        black_box(sink.len());
    });
}

#[divan::bench]
fn open_and_render_python_1mb(b: Bencher) {
    let data = make_python_source_bytes(1024 * 1024);
    open_and_render_python(b, &data, "open-1mb");
}

#[divan::bench]
fn open_and_render_python_10mb(b: Bencher) {
    let data = make_python_source_bytes(10 * 1024 * 1024);
    open_and_render_python(b, &data, "open-10mb");
}

#[divan::bench]
fn render_frame_120x40_viewport_syntax(b: Bencher) {
    let data = make_rust_source(1_000);
    let rules = highlight::rules_for_language("Rust");
    render_frame(b, &data, 120, 40, None, rules);
}

#[divan::bench]
fn render_frame_120x40_viewport_selection(b: Bencher) {
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
fn render_frame_120x40_viewport_plain(b: Bencher) {
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
