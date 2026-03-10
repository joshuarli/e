//! Benchmark harness for `e`.
//!
//! Tracks wall time (criterion) and heap allocations (counting allocator).
//! Run: `cargo bench`

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

use e::buffer::GapBuffer;
use e::document::Document;
use e::find::FindState;
use e::highlight::{self, HlState};
use e::selection::Pos;
use e::view::{self, View};

// ---------------------------------------------------------------------------
// Counting allocator
// ---------------------------------------------------------------------------

struct CountingAlloc;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn reset_alloc_counters() {
    ALLOC_COUNT.store(0, Relaxed);
    ALLOC_BYTES.store(0, Relaxed);
}

fn alloc_count() -> usize {
    ALLOC_COUNT.load(Relaxed)
}

fn alloc_bytes() -> usize {
    ALLOC_BYTES.load(Relaxed)
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

    // Measure allocations for highlighting (using _into variant should be ~0)
    group.bench_function("highlight_1k_into_allocs", |b| {
        let mut out = Vec::new();
        b.iter(|| {
            reset_alloc_counters();
            let mut state = HlState::default();
            for line in data.split(|&byte| byte == b'\n') {
                state = highlight::highlight_line_into(line, state, rules, &mut out);
            }
            let count = alloc_count();
            let bytes = alloc_bytes();
            black_box((count, bytes));
        });
    });

    // Measure allocations for pos_to_offset (should be 0)
    group.bench_function("pos_to_offset_allocs", |b| {
        let buf = GapBuffer::from_vec(data.clone());
        b.iter(|| {
            reset_alloc_counters();
            for line in 0..buf.line_count() {
                buf.pos_to_offset(line, 0);
            }
            let count = alloc_count();
            black_box(count);
        });
    });

    // Measure allocations for a single insert
    group.bench_function("single_insert_allocs", |b| {
        b.iter(|| {
            let mut buf = GapBuffer::from_vec(data.clone());
            reset_alloc_counters();
            buf.insert(0, b"hello");
            let count = alloc_count();
            let bytes = alloc_bytes();
            black_box((count, bytes));
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_gap_buffer,
    bench_highlight,
    bench_document,
    bench_search,
    bench_viewport,
    bench_alloc_counts,
);
criterion_main!(benches);
