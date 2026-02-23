This file must be updated before each commit.

# `e` — Minimalist Terminal Text Editor

A performant, minimalist text editor in Rust. Single-file editing only — no tabs, no file browser. macOS and Linux.

## Design Constraints

- Rust 2024 edition
- 3 dependencies only: `termion`, `regex`, `signal-hook` — do not add crates without good reason
- Single-file editing — no tabs, no file browser, no split panes
- macOS and Linux only (no Windows)
- Indent: 2 spaces for all files except `.c`, `.h`, `.go`, `Makefile` which use tabs
- Tabs display as 2 spaces

## Architecture

Ownership chain: `main.rs` → `Editor` → `Document` → `GapBuffer`

```
src/
  main.rs            arg parsing (single file only), file safety checks (binary, >5MB), enter raw mode
  editor.rs          Editor struct: all state, channel-based event loop, action dispatch
  buffer.rs          GapBuffer (Vec<u8> with gap) + lazy line-start index cache
  document.rs        wraps GapBuffer + UndoStack + dirty flag + filename
  selection.rs       Pos (line, col), Selection (anchor+cursor), word/line boundary helpers
  operation.rs       Operation enum (Insert/Delete), OperationGroup, UndoStack with grouping
  view.rs            Viewport: scroll offsets, cursor-to-screen mapping (no scroll margin)
  render.rs          ANSI rendering: gutter, line content, tab pipes, selection/find highlighting, status bar, completions
  keybind.rs         EditorAction enum, KeybindingTable with defaults, INI config loader
  command.rs         CommandRegistry: HashMap<String, CommandFn>, built-in commands
  command_buffer.rs  Modal mini-editor for command palette, find, goto, save-as prompt; tab completion
  clipboard.rs       Platform-detected clipboard: pbcopy/wl-copy/xclip/xsel/internal fallback
  file_io.rs         Read/write files, CRLF→LF normalization, binary detection, trailing whitespace strip
  language.rs        Language detection by file extension (~45 languages), comment syntax lookup
  signal.rs          Placeholder (SIGWINCH handled directly in editor.rs via signal-hook)
  highlight.rs       Stub: Highlighter trait + Span/Style types for future syntax highlighting
```

## Key Data Structures

- **GapBuffer** (`buffer.rs`): `Vec<u8>` with `gap_start`/`gap_end`. Lazy `line_starts` cache (byte offsets of each line start, rebuilt on access after edits). All text stored as UTF-8 bytes.
- **Document** (`document.rs`): Owns `GapBuffer` + `UndoStack` + `dirty: bool` + `filename: Option<String>`. All mutations (`insert`, `delete_range`) record undo operations.
- **Pos** (`selection.rs`): `{ line: usize, col: usize }` — 0-indexed, col is character index not byte offset. Implements `Ord`.
- **Selection** (`selection.rs`): `{ anchor: Pos, cursor: Pos }`. `anchor == cursor` means no selection. `ordered()` returns `(start, end)`.
- **UndoStack** (`operation.rs`): Groups operations automatically by: kind change (insert vs delete), word boundary (space/newline), time gap (>1s), cursor jump, or explicit `seal()`. `seal()` immediately flushes the current group for atomic undo of paste/comment operations.
- **Editor** (`editor.rs`): Owns everything. Event loop uses `mpsc` channels — background thread for stdin, another for SIGWINCH. Main thread does `recv_timeout(500ms)` for status message expiry.

## Event Loop

Channel-based (`std::sync::mpsc`). No async runtime.

1. Background thread: reads `stdin.events()` via termion → sends `EditorEvent::Term(Event)`
2. Background thread: listens for `SIGWINCH` via `signal-hook` → sends `EditorEvent::Resize(w, h)`
3. Main thread: `recv_timeout(500ms)` — dispatches events, expires status messages, redraws

## Rendering

All output buffered to a `Write` impl, flushed once per frame. Status bar (reverse video) on second-to-last row shows `Language │ Ln X, Col Y` on the right. Command buffer on last row when active. Tab completions render above the status bar. Selection rendered as reverse video, find matches as yellow background. Line numbers in dim text (no separator). Tabs display as dark grey `|` pipe followed by space.

## Keybindings

Configurable via `~/.config/e/keybindings.ini`. Format: `ctrl+key = action`.

| Key | Action |
|---|---|
| `^s` | Save |
| `^q` | Quit (confirms if dirty) |
| `^z` | Undo |
| `^y` | Redo |
| `^a` | Select all |
| `^c` | Copy |
| `^x` | Cut |
| `^v` | Paste |
| `^f` | Find (regex, smart-case) |
| `^p` | Command palette |
| `^l` | Goto line |
| `^k` | Kill line |
| `^t` | Goto top |
| `^g` | Goto end |
| `^d` | Toggle comment |
| `^r` | Toggle ruler |
| `^h` | Ctrl+Backspace (delete word) |
| `Shift+Arrows` | Extend selection |
| `Esc` | Clear selection / find highlights |

Mouse: click to place cursor, drag to select, double-click selects word (space-delineated), triple-click selects line, scroll wheel scrolls.

## Commands

Entered via `^p` command palette. Available commands:

| Command | Description |
|---|---|
| `save [filename]` | Save current file, or save-as if filename given |
| `quit` / `q` | Quit |
| `goto <line>` | Jump to line number |
| `ruler` | Toggle line number ruler |
| `replaceall <regex> <replacement>` | Replace all matches (in selection if active, else whole file) |
| `comment` | Toggle line comments (language-aware) |

## Development Guidelines

- Run `cargo clippy && cargo test` before every commit — zero warnings, all tests pass
- All modules have inline `#[cfg(test)] mod tests` — 232 tests total
- Prefer `&self` over `&mut self` for read-only operations (the line cache uses interior mutability via `Option<Vec<usize>>`)
- Minimize heap allocations in hot paths (render loop, cursor movement)
- No `unwrap()` on user-facing I/O — propagate errors or show in status bar
- Keep the dependency count at 3 — solve problems with std
- Tests should be self-contained with no external file dependencies (use `std::env::temp_dir()` for integration tests)
- When adding new keybindings, update `KeybindingTable::with_defaults()` in `keybind.rs` and `parse_action()` match arm

## v0 Feature Status

- [x] Gap buffer with lazy line index
- [x] Undo/redo with automatic grouping heuristics
- [x] Selection (shift+arrows, mouse drag, double/triple click)
- [x] System clipboard (platform-detected: pbcopy, wl-copy, xclip, xsel)
- [x] Regex find with smart-case and live highlighting
- [x] Replace all (selection-aware)
- [x] Command palette (`^p`)
- [x] Goto line (`^l`)
- [x] Configurable keybindings (INI file)
- [x] SIGWINCH window resize handling
- [x] File safety checks (binary detection, >5MB confirmation)
- [x] CRLF→LF normalization on read
- [x] Trailing whitespace strip on save (adjusts cursor position) + ensure newline on save
- [x] Quit confirmation when dirty
- [x] Save-as prompt for unnamed buffers
- [x] Mouse support (click, drag, double/triple click, scroll wheel)
- [x] Horizontal scrolling for long lines
- [x] Timed status messages
- [x] Toggle ruler (`^r`)
- [x] Language detection (~45 languages by file extension)
- [x] Comment toggle (`^d` / `comment` command, language-aware)
- [x] Tab completion in command palette
- [x] Tab display as dark grey pipes

## Future Work

- [ ] Syntax highlighting (foundation: `highlight.rs` stub with `Highlighter` trait)
- [ ] `mkdir -p` prompt when saving to non-existent parent directories
- [ ] Permission denied handling on save
- [ ] Differential rendering with per-line hashes (field exists in `Renderer`, not yet wired up)
