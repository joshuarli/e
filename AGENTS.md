`e` is a minimalist terminal text editor in Rust for macOS and Linux.
It edits one file at a time: no tabs, file browser, split panes, or async runtime.

The ownership chain is:

```text
main.rs → Editor → Document → GapBuffer
```

## Working rules

- Prefer the simplest correct solution; avoid speculative abstractions and dependencies.
- Preserve unrelated user changes and keep patches focused on the requested behavior.
- Fix root causes, update callers/tests/terminology together, and do not leave stale references.
- Put important explanations above the definition they describe. Comments should explain why,
  invariants, ownership, platform constraints, or intentional absences.
- Do not add banner/separator comments. Preserve useful existing comments.
- Do not run pre-commit hooks, create commits, or push remotes.

## Navigation map

- `src/main.rs` — CLI parsing, file safety, locks, piped stdin, and startup.
- `src/editor.rs` — event loop, key dispatch, editing commands, caret state, and resize handling.
- `src/document.rs` — document mutations, `TextEdit`, undo/redo, dirty state, and file path.
- `src/buffer.rs` — `GapBuffer`, byte storage, line index, UTF-8 conversion, and ASCII fast paths.
- `src/selection.rs` — `TextPosition`, `Selection`, `Caret`, `CaretSet`, and word boundaries.
- `src/viewport.rs` — `Viewport`, soft-wrap scrolling, screen mapping, and resize anchors.
- `src/render.rs` — ANSI rendering, syntax-state caching, dirty rows, and reusable scratch buffers.
- `src/find.rs` — `FindState`, regex compilation, viewport match caching, and navigation.
- `src/highlight.rs` — `HighlightKind`, `HighlightState`, syntax rules, and bracket matching.
- `src/operation.rs` — `UndoOperation`, `UndoGroup`, and `UndoStack`.
- `src/file_io.rs` — file reads/writes, modification times, persistent undo, and cursor state.
- `benches/bench.rs` — executable performance and allocation benchmarks.
- `tests/e2e/` — terminal behavior tests named by user-facing scenario.

## Vocabulary

Use one spelling per concept:

| Concept | Canonical name | Meaning |
|---|---|---|
| Text location | `TextPosition` | Zero-based logical `line` and character `column`. |
| Selection | `Selection` | `anchor` plus `cursor`; `ordered()` gives start/end. |
| Caret collection | `CaretSet` | One or more `Caret` values and a `primary` index. |
| Persistent text model | `Document` | `GapBuffer` plus undo state, dirty state, and `file_path`. |
| Text storage | `GapBuffer` | UTF-8 bytes with a movable gap and incremental line metadata. |
| Planned edit | `TextEdit` | A byte range plus `inserted_bytes` and `deleted_bytes`. |
| Undo item | `UndoOperation` | Insert/delete at `byte_offset` with retained `bytes`. |
| Screen layout | `Viewport` | Scroll offsets, terminal dimensions, and soft-wrap mapping. |
| Resize context | `ViewportAnchor` | Logical line/display-column position preserved across layout changes. |
| Find controller | `FindState` | Pattern, compiled regex, current match, and viewport cache. |

Do not use abbreviated replacements such as `Pos`, `View`, `RawEdit`, `buf` fields,
`col` fields, or `sel` fields for these concepts. Local names may be short only when
their type and scope make the meaning unambiguous.

## Core invariants

### `GapBuffer` (`src/buffer.rs`)

- `storage` contains text and a gap; logical offsets ignore the gap.
- `line_starts` and `line_ascii` are always valid and parallel. A trailing newline
  is a terminator, not an extra loaded line entry.
- `position_to_byte_offset()` and `byte_offset_to_position()` translate between
  `TextPosition` and internal byte offsets; `line_character_count()` handles UTF-8.
- `display_column_at()` and `character_column_from_display()` use tab width 2.
- `version()` increases on every edit. `take_dirty_line()` returns and clears the
  earliest line whose syntax state may be stale.
- ASCII lines use the O(1) byte/character fast path. Invalid UTF-8 is preserved as bytes.
- `from_bytes()` takes ownership of loaded file bytes; do not add a copying load path.

### `Document` and edits (`src/document.rs`)

- All mutations go through `Document` methods so undo history and `is_dirty` stay aligned.
- `Document::apply_batch()` consumes sorted `TextEdit` values in reverse order and records
  one undo group. Byte ranges are half-open: `[start_byte, end_byte)`.
- Undo/redo uses callbacks through `UndoStack`; do not allocate an intermediate operation list.
- `file_path` is optional for scratch buffers. File I/O and external-modification checks
  use `file_modification_time()`.

### `Selection` and carets (`src/selection.rs`)

- `TextPosition::column` is a character index, never a byte offset.
- `Selection::anchor` stays fixed during extension; `Selection::cursor` moves.
- `Caret::desired_column` is sticky only for vertical/page movement and resets otherwise.
- `CaretSet::primary` identifies the caret controlling viewport focus and status behavior.

### `Viewport` and rendering (`src/viewport.rs`, `src/render.rs`)

- `scroll_line` and `scroll_wrap` identify the first visible soft-wrapped row.
- `Viewport::ensure_cursor_visible()` must remain safe for empty, narrow, and huge files.
- `Viewport::center_anchor()` plus `center_on_anchor()` preserve logical content across resize.
- `Renderer::render()` uses `GapBuffer::version()` and `take_dirty_line()` to invalidate
  syntax state, then compares cached rows before emitting ANSI output.
- Keep persistent caches on `Renderer`; reuse scratch buffers instead of per-frame allocations.

### Find (`src/find.rs`)

- Live find uses `update_highlights_lazy()` and scans only the visible window plus lookahead.
- `refresh_viewport_matches()` caches by buffer version and viewport geometry.
- `search_forward()` and `search_backward()` perform navigation scans on demand.
- Multi-line regex matches are intentionally unsupported.

## Runtime boundaries

- `Editor::run()` enters raw mode and the alternate screen, starts the terminal input thread,
  drains bursts of queued events, polls SIGWINCH, and restores terminal state on exit.
- Command input is modal through `CommandBuffer`; normal editing operates on `CaretSet`.
- File reload is prompted after external modification detection; dirty quit asks before discarding.
- Clipboard detection is platform-specific with an internal fallback.

## Benchmarks

Use `make bench` for host-keyed timing/allocation baselines.

## Configuration and persistence

- `~/.config/e/keybindings.ini` — Ctrl-key action overrides.
- `~/.config/e/undo.bin` — persistent undo history.
- `~/.config/e/cursor.bin` — cursor persistence.
- `~/.config/e/locks/<encoded_path>.elock` — single-editor file locks.

Keep public behavior and these paths stable unless the task explicitly changes the contract.

