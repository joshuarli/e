# Benchmark History

## gap_buffer

| Benchmark | `5e6eb76` | `b0d65c2` | `623c291` | `49f13a1` | `6e498d0` | `9e0446f` | `12c67da` |
|---|---:|---:|---:|---:|---:|---:|---:|
| gap_buffer/from_vec/1000 | 18.92 µs | 18.19 µs | 16.68 µs | 21.36 µs | 16.72 µs | 24.30 µs | 23.59 µs |
| gap_buffer/from_vec/10000 | 186.58 µs | 190.81 µs | 169.27 µs | 174.78 µs | 178.69 µs | 244.10 µs | 256.81 µs |
| gap_buffer/from_vec/50000 | 1.129 ms | 1.134 ms | 971.24 µs | 1.069 ms | 985.46 µs | 1.372 ms | 1.327 ms |
| gap_buffer/insert_sequential/1000 | 21.44 µs | 21.66 µs | 20.69 µs | 23.44 µs | 23.00 µs | 32.11 µs | 27.54 µs |
| gap_buffer/insert_sequential/10000 | 198.09 µs | 185.38 µs | 182.99 µs | 196.14 µs | 174.24 µs | 244.78 µs | 237.80 µs |
| gap_buffer/insert_sequential/50000 | 1.139 ms | 1.126 ms | 955.04 µs | 1.034 ms | 1.016 ms | 1.368 ms | 1.341 ms |
| gap_buffer/line_text_all/1000 | 41.13 µs | 41.21 µs | 40.77 µs | 26.89 µs | 27.05 µs | 27.18 µs | 26.40 µs |
| gap_buffer/line_text_all/10000 | 415.75 µs | 414.29 µs | 406.03 µs | 277.38 µs | 273.69 µs | 273.49 µs | 265.25 µs |
| gap_buffer/line_text_all/50000 | 2.076 ms | 2.055 ms | 2.046 ms | 2.161 ms | 1.370 ms | 1.368 ms | 1.343 ms |
| gap_buffer/offset_to_pos_walk/1000 | 635.7 ns | 631.5 ns | 613.2 ns | 622.4 ns | 648.7 ns | 694.3 ns | 673.5 ns |
| gap_buffer/offset_to_pos_walk/10000 | 975.0 ns | 972.7 ns | 941.9 ns | 991.2 ns | 1.00 µs | 1.04 µs | 1.01 µs |
| gap_buffer/offset_to_pos_walk/50000 | 1.16 µs | 1.16 µs | 1.13 µs | 1.80 µs | 1.16 µs | 1.24 µs | 1.25 µs |
| gap_buffer/pos_to_offset_all_lines/1000 | 832.9 ns | 814.8 ns | 787.4 ns | 810.5 ns | 829.2 ns | 966.6 ns | 938.3 ns |
| gap_buffer/pos_to_offset_all_lines/10000 | 8.13 µs | 8.04 µs | 7.85 µs | 8.18 µs | 8.04 µs | 9.66 µs | 9.39 µs |
| gap_buffer/pos_to_offset_all_lines/50000 | 40.21 µs | 40.15 µs | 39.04 µs | 55.35 µs | 40.16 µs | 49.10 µs | 47.07 µs |

## highlight

| Benchmark | `5e6eb76` | `b0d65c2` | `623c291` | `49f13a1` | `6e498d0` | `9e0446f` | `12c67da` |
|---|---:|---:|---:|---:|---:|---:|---:|
| highlight/rust_into/1000 | 1.604 ms | 1.589 ms | 1.534 ms | 1.581 ms | 1.591 ms | 1.582 ms | 1.590 ms |
| highlight/rust_into/10000 | 15.941 ms | 15.914 ms | 15.303 ms | 15.470 ms | 15.741 ms | 15.767 ms | 15.518 ms |

## document

| Benchmark | `5e6eb76` | `b0d65c2` | `623c291` | `49f13a1` | `6e498d0` | `9e0446f` | `12c67da` |
|---|---:|---:|---:|---:|---:|---:|---:|
| document/insert_100_seal_undo_all | 413.18 µs | 417.24 µs | 381.92 µs | 393.86 µs | 389.13 µs | 424.59 µs | 416.01 µs |
| document/insert_delete_interleaved | 180.92 µs | 184.08 µs | 162.75 µs | 164.74 µs | 166.68 µs | 204.76 µs | 210.77 µs |

## search

| Benchmark | `5e6eb76` | `b0d65c2` | `623c291` | `49f13a1` | `6e498d0` | `9e0446f` | `12c67da` |
|---|---:|---:|---:|---:|---:|---:|---:|
| search/search_backward/1000 | 1.15 µs | — | — | — | — | — | — |
| search/search_backward/10000 | 1.13 µs | — | — | — | — | — | — |
| search/search_backward_miss/1000 | — | 1.132 ms | 1.138 ms | 1.125 ms | 1.114 ms | 1.117 ms | 1.126 ms |
| search/search_backward_miss/10000 | — | 11.322 ms | 11.515 ms | 11.279 ms | 11.318 ms | 11.347 ms | 10.988 ms |
| search/search_forward/1000 | 1.23 µs | — | — | — | — | — | — |
| search/search_forward/10000 | 1.25 µs | — | — | — | — | — | — |
| search/search_forward_miss/1000 | — | 574.58 µs | 558.55 µs | 563.48 µs | 567.51 µs | 558.66 µs | 563.13 µs |
| search/search_forward_miss/10000 | — | 5.693 ms | 5.628 ms | 5.557 ms | 5.575 ms | 5.679 ms | 5.520 ms |

## viewport

| Benchmark | `5e6eb76` | `b0d65c2` | `623c291` | `49f13a1` | `6e498d0` | `9e0446f` | `12c67da` |
|---|---:|---:|---:|---:|---:|---:|---:|
| viewport/ensure_cursor_visible_jump | 103.8 ns | 101.0 ns | 104.8 ns | 104.4 ns | 105.2 ns | 167.7 ns | 171.1 ns |
| viewport/wrapped_rows_sweep | 112.3 ns | 109.0 ns | 113.8 ns | 109.6 ns | 114.3 ns | 108.8 ns | 109.7 ns |

## alloc_audit

| Benchmark | `5e6eb76` | `b0d65c2` | `623c291` | `49f13a1` | `6e498d0` | `9e0446f` | `12c67da` |
|---|---:|---:|---:|---:|---:|---:|---:|
| alloc_audit/highlight_1k_alloc | — | — | 1.613 ms | 1.562 ms | 1.610 ms | 1.548 ms | 1.566 ms |
| alloc_audit/highlight_1k_into | — | — | 1.603 ms | 1.551 ms | 1.589 ms | 1.526 ms | 1.534 ms |
| alloc_audit/highlight_1k_into_allocs | 1.615 ms | 1.534 ms | — | — | — | — | — |
| alloc_audit/pos_to_offset_1k | — | — | 811.0 ns | 791.0 ns | 807.8 ns | 940.2 ns | 949.8 ns |
| alloc_audit/pos_to_offset_allocs | 750.3 ns | 714.7 ns | — | — | — | — | — |
| alloc_audit/single_insert | — | — | 29.66 µs | 30.62 µs | 31.30 µs | 37.89 µs | 36.99 µs |
| alloc_audit/single_insert_allocs | 22.47 µs | 19.97 µs | — | — | — | — | — |

## Allocation Audit

| Benchmark | `5e6eb76` | `b0d65c2` | `623c291` | `49f13a1` | `6e498d0` | `9e0446f` | `12c67da` |
|---|---:|---:|---:|---:|---:|---:|---:|
| GapBuffer::from_vec | — | — | — | 3 / 83,660B | 3 / 83,660B | 4 / 85,401B | 4 / 85,401B |
| doc_100_edit_undo | — | — | — | 315 / 125,952B | 215 / 113,152B | 216 / 114,892B | 216 / 114,892B |
| highlight_1k_alloc | — | — | 1000 / 34,406B | 1000 / 34,406B | 1000 / 34,406B | 1000 / 34,406B | 1000 / 34,406B |
| highlight_1k_into | — | — | 1 / 43B | 1 / 43B | 1 / 43B | 1 / 43B | 1 / 43B |
| line_text_all_1k | — | — | — | 1000 / 34,406B | 1000 / 34,406B | 1000 / 34,406B | 1000 / 34,406B |
| pos_to_offset_1k | — | — | 0 / 0B | 0 / 0B | 0 / 0B | 0 / 0B | 0 / 0B |
| search_backward_miss_1k | — | — | — | 3 / 76B | 3 / 76B | 3 / 76B | 3 / 76B |
| search_forward_miss_1k | — | — | — | 11 / 1,024B | 11 / 1,024B | 11 / 1,024B | 11 / 1,024B |
| single_insert | — | — | 3 / 83,660B | 3 / 83,660B | 3 / 83,660B | 4 / 85,401B | 4 / 85,401B |

### Commit Legend

- `5e6eb76`: Add criterion benchmark harness with counting allocator
- `b0d65c2`: Make highlight_line_into truly zero-alloc and fix benchmarks
- `623c291`: Track peak allocation and live bytes in benchmark allocator
- `49f13a1`: Reduce allocations in search, line_text, and alloc audit
- `6e498d0`: Reduce allocations: undo/redo callback pattern and editor scratch buffer
- `9e0446f`: Per-line ASCII fast path for O(1) char/byte conversion
- `12c67da`: bump version to 0.1.5
