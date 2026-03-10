//! Benchmark harness for `e`.
//!
//! Tracks wall time (criterion) and heap allocations (counting allocator).
//! Run: `cargo bench`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

use e::buffer::GapBuffer;
use e::document::Document;
use e::find::FindState;
use e::highlight::{self, HlState, SyntaxRules};
use e::selection::Pos;
use e::view::{self, View};

// ---------------------------------------------------------------------------
// Counting allocator — tracks allocations, live bytes, and peak (high-water)
// ---------------------------------------------------------------------------

struct CountingAlloc;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Update peak to be the max of current peak and current live bytes.
fn update_peak() {
    let live = LIVE_BYTES.load(Relaxed);
    let mut peak = PEAK_BYTES.load(Relaxed);
    while live > peak {
        match PEAK_BYTES.compare_exchange_weak(peak, live, Relaxed, Relaxed) {
            Ok(_) => break,
            Err(actual) => peak = actual,
        }
    }
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Relaxed);
        LIVE_BYTES.fetch_add(layout.size(), Relaxed);
        update_peak();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Relaxed);
        if new_size > layout.size() {
            let delta = new_size - layout.size();
            ALLOC_BYTES.fetch_add(delta, Relaxed);
            LIVE_BYTES.fetch_add(delta, Relaxed);
        } else {
            let delta = layout.size() - new_size;
            LIVE_BYTES.fetch_sub(delta, Relaxed);
        }
        update_peak();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn reset_alloc_counters() {
    ALLOC_COUNT.store(0, Relaxed);
    ALLOC_BYTES.store(0, Relaxed);
    PEAK_BYTES.store(LIVE_BYTES.load(Relaxed), Relaxed);
}

fn alloc_count() -> usize {
    ALLOC_COUNT.load(Relaxed)
}

fn alloc_bytes() -> usize {
    ALLOC_BYTES.load(Relaxed)
}

fn peak_bytes() -> usize {
    PEAK_BYTES.load(Relaxed)
}

/// Snapshot of allocation counters for a measured region.
#[derive(Clone, Copy)]
struct AllocStats {
    count: usize,
    bytes: usize,
    peak_delta: usize,
}

impl std::fmt::Display for AllocStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn human(n: usize) -> String {
            if n >= 1_048_576 {
                format!("{:.1} MiB", n as f64 / 1_048_576.0)
            } else if n >= 1024 {
                format!("{:.1} KiB", n as f64 / 1024.0)
            } else {
                format!("{n} B")
            }
        }
        write!(
            f,
            "{} allocs, {} total, {} peak",
            self.count,
            human(self.bytes),
            human(self.peak_delta)
        )
    }
}

/// Reset counters, run `f`, return stats for the measured region.
/// Peak is reported as a delta above the live-bytes baseline at entry.
fn measure_allocs<F: FnOnce()>(f: F) -> AllocStats {
    let baseline = LIVE_BYTES.load(Relaxed);
    reset_alloc_counters();
    f();
    let peak = peak_bytes();
    AllocStats {
        count: alloc_count(),
        bytes: alloc_bytes(),
        peak_delta: peak.saturating_sub(baseline),
    }
}

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

/// Generate a realistic Rust-like source file of `n` lines.
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

// ---------------------------------------------------------------------------
// Gap buffer benchmarks
// ---------------------------------------------------------------------------

fn bench_gap_buffer(c: &mut Criterion) {
    let mut group = c.benchmark_group("gap_buffer");

    for &size in &[1_000, 10_000, 50_000] {
        let data = make_rust_source(size);

        group.throughput(Throughput::Bytes(data.len() as u64));

        group.bench_with_input(BenchmarkId::new("from_vec", size), &data, |b, data| {
            b.iter(|| {
                black_box(GapBuffer::from_vec(data.clone()));
            });
        });

        group.bench_with_input(
            BenchmarkId::new("insert_sequential", size),
            &data,
            |b, data| {
                b.iter(|| {
                    let mut buf = GapBuffer::from_vec(data.clone());
                    let end = buf.len();
                    // Simulate typing 100 chars at end of file
                    for i in 0..100 {
                        buf.insert(end + i, b"x");
                    }
                    black_box(&buf);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("pos_to_offset_all_lines", size),
            &data,
            |b, data| {
                let buf = GapBuffer::from_vec(data.clone());
                b.iter(|| {
                    for line in 0..buf.line_count() {
                        black_box(buf.pos_to_offset(line, 0));
                    }
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("offset_to_pos_walk", size),
            &data,
            |b, data| {
                let buf = GapBuffer::from_vec(data.clone());
                let len = buf.len();
                let step = len / 100;
                b.iter(|| {
                    let mut offset = 0;
                    while offset < len {
                        black_box(buf.offset_to_pos(offset));
                        offset += step.max(1);
                    }
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("line_text_all", size), &data, |b, data| {
            let buf = GapBuffer::from_vec(data.clone());
            b.iter(|| {
                for line in 0..buf.line_count() {
                    black_box(buf.line_text(line));
                }
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Highlight benchmarks
// ---------------------------------------------------------------------------

fn bench_highlight(c: &mut Criterion) {
    let mut group = c.benchmark_group("highlight");

    for &size in &[1_000, 10_000] {
        let rust_src = make_rust_source(size);
        let json_src = make_json(size);

        // Rust highlighting
        group.throughput(Throughput::Bytes(rust_src.len() as u64));
        group.bench_with_input(BenchmarkId::new("rust", size), &rust_src, |b, data| {
            let rules = highlight::rules_for_language("Rust").unwrap();
            b.iter(|| {
                let mut state = HlState::default();
                for line in data.split(|&b| b == b'\n') {
                    let (hl, next) = highlight::highlight_line(line, state, rules);
                    state = next;
                    black_box(&hl);
                }
            });
        });

        // JSON highlighting
        group.throughput(Throughput::Bytes(json_src.len() as u64));
        group.bench_with_input(BenchmarkId::new("json", size), &json_src, |b, data| {
            let rules = highlight::rules_for_language("JSON").unwrap();
            b.iter(|| {
                let mut state = HlState::default();
                for line in data.split(|&b| b == b'\n') {
                    let (hl, next) = highlight::highlight_line(line, state, rules);
                    state = next;
                    black_box(&hl);
                }
            });
        });

        // Rust highlighting (non-allocating path)
        group.throughput(Throughput::Bytes(rust_src.len() as u64));
        group.bench_with_input(BenchmarkId::new("rust_into", size), &rust_src, |b, data| {
            let rules = highlight::rules_for_language("Rust").unwrap();
            let mut out = Vec::new();
            b.iter(|| {
                let mut state = HlState::default();
                for line in data.split(|&b| b == b'\n') {
                    state = highlight::highlight_line_into(line, state, rules, &mut out);
                    black_box(&out);
                }
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Document (edit + undo/redo) benchmarks
// ---------------------------------------------------------------------------

fn bench_document(c: &mut Criterion) {
    let mut group = c.benchmark_group("document");

    let data = make_rust_source(5_000);

    group.bench_function("insert_100_seal_undo_all", |b| {
        b.iter(|| {
            let mut doc = Document::new(data.clone(), None);
            // Insert 100 characters at various positions
            for i in 0..100 {
                let line = i % doc.buf.line_count();
                doc.insert(line, 0, b"// ");
                doc.seal_undo();
            }
            // Undo everything
            while doc.undo().is_some() {}
            black_box(&doc);
        });
    });

    group.bench_function("insert_delete_interleaved", |b| {
        b.iter(|| {
            let mut doc = Document::new(data.clone(), None);
            for _ in 0..50 {
                let lc = doc.buf.line_count();
                let line = lc / 2;
                let pos = doc.insert(line, 0, b"new line\n");
                doc.seal_undo();
                doc.delete_range(Pos::new(pos.line, 0), pos);
                doc.seal_undo();
            }
            black_box(&doc);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Search benchmarks
// ---------------------------------------------------------------------------

fn bench_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("search");

    for &size in &[1_000, 10_000] {
        let data = make_rust_source(size);
        let buf = GapBuffer::from_vec(data.clone());

        group.throughput(Throughput::Bytes(data.len() as u64));

        // Use a pattern that doesn't exist to force a full scan of the buffer
        group.bench_with_input(
            BenchmarkId::new("search_forward_miss", size),
            &buf,
            |b, buf| {
                let re = regex_lite::Regex::new("ZZNOTFOUND").expect("valid regex");
                b.iter(|| {
                    black_box(FindState::search_forward(buf, &re, Pos::zero()));
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("search_backward_miss", size),
            &buf,
            |b, buf| {
                let re = regex_lite::Regex::new("ZZNOTFOUND").expect("valid regex");
                let last = Pos::new(buf.line_count().saturating_sub(1), 0);
                b.iter(|| {
                    black_box(FindState::search_backward(buf, &re, last));
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Viewport benchmarks
// ---------------------------------------------------------------------------

fn bench_viewport(c: &mut Criterion) {
    let mut group = c.benchmark_group("viewport");

    let data = make_rust_source(10_000);
    let buf = GapBuffer::from_vec(data);

    group.bench_function("ensure_cursor_visible_jump", |b| {
        b.iter(|| {
            let mut v = View::new(120, 40);
            let mut widths = |line: usize| -> usize { buf.display_col_at(line, usize::MAX) };
            // Jump cursor to various positions
            for line in (0..buf.line_count()).step_by(100) {
                v.ensure_cursor_visible(line, 0, 5, &mut widths);
            }
            black_box(&v);
        });
    });

    group.bench_function("wrapped_rows_sweep", |b| {
        b.iter(|| {
            for w in (0..500).step_by(3) {
                black_box(view::wrapped_rows(w, 80));
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Allocation tracking benchmarks
// ---------------------------------------------------------------------------

fn bench_alloc_counts(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_audit");

    let data = make_rust_source(1_000);
    let rules = highlight::rules_for_language("Rust").unwrap();

    // One-shot allocation reports (printed, not benchmarked for speed)
    {
        eprintln!();
        eprintln!("  ── allocation audit (1k-line Rust source) ──");

        // GapBuffer construction
        let stats = measure_allocs(|| {
            black_box(GapBuffer::from_vec(data.clone()));
        });
        eprintln!("  [alloc] GapBuffer::from_vec:       {stats}");

        let buf = GapBuffer::from_vec(data.clone());

        // Single insert into existing buffer
        let stats = measure_allocs(|| {
            let mut buf2 = GapBuffer::from_vec(data.clone());
            buf2.insert(0, b"hello");
            black_box(&buf2);
        });
        eprintln!("  [alloc] single_insert:             {stats}");

        // Document create + 100 edits + full undo
        let stats = measure_allocs(|| {
            let mut doc = Document::new(data.clone(), None);
            for i in 0..100 {
                let line = i % doc.buf.line_count();
                doc.insert(line, 0, b"// ");
                doc.seal_undo();
            }
            while doc.undo().is_some() {}
            black_box(&doc);
        });
        eprintln!("  [alloc] doc_100_edit_undo:         {stats}");

        // Highlight (non-allocating path)
        let mut out = Vec::new();
        let stats = measure_allocs(|| {
            let mut state = HlState::default();
            for line in data.split(|&byte| byte == b'\n') {
                state = highlight::highlight_line_into(line, state, rules, &mut out);
            }
        });
        eprintln!("  [alloc] highlight_1k_into:         {stats}");

        // Highlight (allocating path)
        let stats = measure_allocs(|| {
            let mut state = HlState::default();
            for line in data.split(|&byte| byte == b'\n') {
                let (hl, next) = highlight::highlight_line(line, state, rules);
                state = next;
                black_box(&hl);
            }
        });
        eprintln!("  [alloc] highlight_1k_alloc:        {stats}");

        // pos_to_offset (should be 0)
        let stats = measure_allocs(|| {
            for line in 0..buf.line_count() {
                buf.pos_to_offset(line, 0);
            }
        });
        eprintln!("  [alloc] pos_to_offset_1k:          {stats}");

        // line_text for all lines (allocates a String per line)
        let stats = measure_allocs(|| {
            for line in 0..buf.line_count() {
                black_box(buf.line_text(line));
            }
        });
        eprintln!("  [alloc] line_text_all_1k:          {stats}");

        // Search forward (full scan, miss)
        let re = regex_lite::Regex::new("ZZNOTFOUND").expect("valid regex");
        let stats = measure_allocs(|| {
            black_box(FindState::search_forward(&buf, &re, Pos::zero()));
        });
        eprintln!("  [alloc] search_forward_miss_1k:    {stats}");

        // Search backward (full scan, miss)
        let last = Pos::new(buf.line_count().saturating_sub(1), 0);
        let stats = measure_allocs(|| {
            black_box(FindState::search_backward(&buf, &re, last));
        });
        eprintln!("  [alloc] search_backward_miss_1k:   {stats}");

        eprintln!();
    }

    // Criterion timing benchmarks for the same operations
    group.bench_function("highlight_1k_into", |b| {
        let mut out = Vec::new();
        b.iter(|| {
            let mut state = HlState::default();
            for line in data.split(|&byte| byte == b'\n') {
                state = highlight::highlight_line_into(line, state, rules, &mut out);
            }
            black_box(&out);
        });
    });

    group.bench_function("highlight_1k_alloc", |b| {
        b.iter(|| {
            let mut state = HlState::default();
            for line in data.split(|&byte| byte == b'\n') {
                let (hl, next) = highlight::highlight_line(line, state, rules);
                state = next;
                black_box(&hl);
            }
        });
    });

    group.bench_function("pos_to_offset_1k", |b| {
        let buf = GapBuffer::from_vec(data.clone());
        b.iter(|| {
            for line in 0..buf.line_count() {
                black_box(buf.pos_to_offset(line, 0));
            }
        });
    });

    group.bench_function("single_insert", |b| {
        b.iter(|| {
            let mut buf = GapBuffer::from_vec(data.clone());
            buf.insert(0, b"hello");
            black_box(&buf);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Render benchmarks
// ---------------------------------------------------------------------------

use e::render::Renderer;
use e::selection::Selection;

/// Helper: set up a Renderer + GapBuffer + View for a given terminal size and
/// file, with the cursor at the middle of the document.
fn render_setup(
    data: &[u8],
    width: u16,
    height: u16,
    syntax: Option<&'static SyntaxRules>,
) -> (Renderer, GapBuffer, View) {
    let mut r = Renderer::new();
    r.set_syntax(syntax);
    let buf = GapBuffer::from_vec(data.to_vec());
    let mut v = View::new(width, height);
    // Scroll to middle of file so we're not benchmarking a trivial top-of-file case
    v.scroll_line = buf.line_count() / 2;
    (r, buf, v)
}

fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("render");

    let rust_1k = make_rust_source(1_000);
    let rust_10k = make_rust_source(10_000);
    let rust_rules = highlight::rules_for_language("Rust").unwrap();

    // Full frame, 80x24 terminal, 1k-line file, syntax highlighted
    group.bench_function("frame_80x24_1k_syntax", |b| {
        let (mut r, mut buf, view) = render_setup(&rust_1k, 80, 24, Some(rust_rules));
        let cursor_line = view.scroll_line;
        let mut sink = Vec::with_capacity(16 * 1024);
        b.iter(|| {
            sink.clear();
            r.needs_full_redraw = true;
            r.render(
                &mut sink,
                &mut buf,
                &view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
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
    });

    // Full frame, 120x40 terminal (common modern size), 10k-line file
    group.bench_function("frame_120x40_10k_syntax", |b| {
        let (mut r, mut buf, view) = render_setup(&rust_10k, 120, 40, Some(rust_rules));
        let cursor_line = view.scroll_line;
        let mut sink = Vec::with_capacity(32 * 1024);
        b.iter(|| {
            sink.clear();
            r.needs_full_redraw = true;
            r.render(
                &mut sink,
                &mut buf,
                &view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
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
    });

    // With active selection (triggers slow per-character path)
    group.bench_function("frame_120x40_10k_selection", |b| {
        let (mut r, mut buf, view) = render_setup(&rust_10k, 120, 40, Some(rust_rules));
        let cursor_line = view.scroll_line;
        let sel = Selection {
            anchor: Pos::new(cursor_line, 0),
            cursor: Pos::new(cursor_line + 5, 10),
        };
        let mut sink = Vec::with_capacity(32 * 1024);
        b.iter(|| {
            sink.clear();
            r.needs_full_redraw = true;
            r.render(
                &mut sink,
                &mut buf,
                &view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                Some(sel),
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
    });

    // No syntax highlighting (plain text)
    group.bench_function("frame_120x40_10k_plain", |b| {
        let (mut r, mut buf, view) = render_setup(&rust_10k, 120, 40, None);
        let cursor_line = view.scroll_line;
        let mut sink = Vec::with_capacity(32 * 1024);
        b.iter(|| {
            sink.clear();
            r.needs_full_redraw = true;
            r.render(
                &mut sink,
                &mut buf,
                &view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
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
    });

    // Incremental: no-op redraw (identical state — should skip all rows)
    group.bench_function("incr_noop_120x40", |b| {
        let (mut r, mut buf, view) = render_setup(&rust_10k, 120, 40, Some(rust_rules));
        let cursor_line = view.scroll_line;
        let mut sink = Vec::with_capacity(32 * 1024);
        // Prime the cache with a full frame
        r.needs_full_redraw = true;
        r.render(
            &mut sink,
            &mut buf,
            &view,
            cursor_line,
            0,
            true,
            " test.rs",
            " e v0.1.5 ",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();
        b.iter(|| {
            sink.clear();
            // NOT setting needs_full_redraw — incremental path
            r.render(
                &mut sink,
                &mut buf,
                &view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
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
    });

    // Incremental: cursor moved down 1 line (old + new cursor lines change)
    group.bench_function("incr_cursor_move_120x40", |b| {
        let (mut r, mut buf, view) = render_setup(&rust_10k, 120, 40, Some(rust_rules));
        let cursor_line = view.scroll_line;
        let mut sink = Vec::with_capacity(32 * 1024);
        r.needs_full_redraw = true;
        r.render(
            &mut sink,
            &mut buf,
            &view,
            cursor_line,
            0,
            true,
            " test.rs",
            " e v0.1.5 ",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();
        let mut cur = cursor_line;
        b.iter(|| {
            sink.clear();
            // Alternate cursor between two lines
            cur = if cur == cursor_line {
                cursor_line + 1
            } else {
                cursor_line
            };
            r.render(
                &mut sink,
                &mut buf,
                &view,
                cur,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
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
    });

    // Incremental: scroll down by 3 rows (scroll region shifts, only 3 new rows drawn)
    group.bench_function("incr_scroll_120x40", |b| {
        let (mut r, mut buf, view) = render_setup(&rust_10k, 120, 40, Some(rust_rules));
        let cursor_line = view.scroll_line;
        let mut sink = Vec::with_capacity(32 * 1024);
        r.needs_full_redraw = true;
        r.render(
            &mut sink,
            &mut buf,
            &view,
            cursor_line,
            0,
            true,
            " test.rs",
            " e v0.1.5 ",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();
        b.iter(|| {
            sink.clear();
            // Simulate scroll down then back up (so state resets each pair)
            let mut v = view.clone();
            v.scroll_line += 3;
            r.render(
                &mut sink,
                &mut buf,
                &v,
                cursor_line + 3,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
                None,
                None,
                &[],
                None,
                false,
                None,
            )
            .unwrap();
            sink.clear();
            r.render(
                &mut sink,
                &mut buf,
                &view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
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
    });

    // Measure output bytes per frame (allocation audit style)
    {
        let (mut r, mut buf, view) = render_setup(&rust_10k, 120, 40, Some(rust_rules));
        let cursor_line = view.scroll_line;
        let mut sink = Vec::with_capacity(32 * 1024);
        r.needs_full_redraw = true;
        r.render(
            &mut sink,
            &mut buf,
            &view,
            cursor_line,
            0,
            true,
            " test.rs",
            " e v0.1.5 ",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();
        eprintln!();
        eprintln!("  ── render output size (120x40, 10k-line Rust, syntax on) ──");
        eprintln!(
            "  [bytes] frame: {} B ({:.1} KiB)",
            sink.len(),
            sink.len() as f64 / 1024.0
        );

        let stats = measure_allocs(|| {
            sink.clear();
            r.needs_full_redraw = true;
            r.render(
                &mut sink,
                &mut buf,
                &view,
                cursor_line,
                0,
                true,
                " test.rs",
                " e v0.1.5 ",
                None,
                None,
                None,
                None,
                &[],
                None,
                false,
                None,
            )
            .unwrap();
        });
        eprintln!("  [alloc] render_frame_120x40:       {stats}");

        // Incremental byte counts — the real TTY bandwidth savings
        // No-op: same state, should emit only sync markers + cursor
        sink.clear();
        r.render(
            &mut sink,
            &mut buf,
            &view,
            cursor_line,
            0,
            true,
            " test.rs",
            " e v0.1.5 ",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();
        eprintln!("  [bytes] incr_noop: {} B", sink.len());

        // Cursor move: 2 rows change (old + new cursor line) + status bar
        sink.clear();
        r.render(
            &mut sink,
            &mut buf,
            &view,
            cursor_line + 1,
            0,
            true,
            " test.rs",
            " e v0.1.5 ",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();
        eprintln!("  [bytes] incr_cursor_move: {} B", sink.len());

        // Scroll down 3: scroll region + 3 new rows
        sink.clear();
        let mut v2 = view.clone();
        v2.scroll_line += 3;
        r.render(
            &mut sink,
            &mut buf,
            &v2,
            cursor_line + 3,
            0,
            true,
            " test.rs",
            " e v0.1.5 ",
            None,
            None,
            None,
            None,
            &[],
            None,
            false,
            None,
        )
        .unwrap();
        eprintln!("  [bytes] incr_scroll_3: {} B", sink.len());
        eprintln!();
    }

    group.finish();
}

// ---------------------------------------------------------------------------

fn fast_config() -> Criterion {
    Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
}

criterion_group!(
    name = benches;
    config = fast_config();
    targets =
        bench_gap_buffer,
        bench_highlight,
        bench_document,
        bench_search,
        bench_viewport,
        bench_alloc_counts,
        bench_render,
);
criterion_main!(benches);
