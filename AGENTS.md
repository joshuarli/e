# `e` — Minimalist Terminal Text Editor

A performant, minimalist text editor in Rust. Single-file editing only — no tabs, no file browser. macOS and Linux.

## Design Constraints

- Rust 2024 edition
- 3 dependencies only: `termion`, `regex-lite`, `libc` — do not add crates without good reason
- Single-file editing — no tabs, no file browser, no split panes
- macOS and Linux only (no Windows)
- Indent: 2 spaces for all files except `.c`, `.h`, `.go`, `Makefile` which use tabs
- Tabs display as 2 spaces

## Architecture

Ownership chain: `main.rs` → `Editor` → `Document` → `GapBuffer`

```
src/
  main.rs            arg parsing (single file only), file safety checks (binary, >5MB), file locking, enter raw mode
  editor.rs          Editor struct: all state, channel-based event loop, action dispatch
  buffer.rs          GapBuffer (Vec<u8> with gap) + lazy line-start index cache
  document.rs        wraps GapBuffer + UndoStack + dirty flag + filename
  selection.rs       Pos (line, col), Selection (anchor+cursor), word/line boundary helpers
  operation.rs       Operation enum (Insert/Delete), OperationGroup, UndoStack with grouping
  view.rs            Viewport: scroll offsets, cursor-to-screen mapping (no scroll margin)
  render.rs          ANSI rendering: gutter, line content, tab pipes, syntax/selection/find highlighting, status bar, completions
  keybind.rs         EditorAction enum, KeybindingTable with defaults, INI config loader
  command.rs         CommandRegistry: HashMap<String, CommandFn>, built-in commands
  command_buffer.rs  Modal mini-editor for command palette, find, goto, save-as prompt, sudo password; tab completion
  clipboard.rs       Platform-detected clipboard: pbcopy/wl-copy/xclip/xsel/internal fallback
  file_io.rs         Read/write files, CRLF→LF normalization, binary detection, trailing whitespace strip, file locking
  language.rs        Language detection by file extension (~45 languages), comment syntax lookup
  signal.rs          SIGWINCH handler via libc::sigaction + AtomicBool polling
  highlight.rs       Syntax highlighting: byte-by-byte highlighter, HlType/HlState types, per-language rules (15 languages), dedicated JSON/YAML/Markdown highlighters, semver detection, bracket matching
```

## Key Data Structures

- **GapBuffer** (`buffer.rs`): `Vec<u8>` with `gap_start`/`gap_end`. Lazy `line_starts` cache (byte offsets of each line start, rebuilt on access after edits). All text stored as UTF-8 bytes.
- **Document** (`document.rs`): Owns `GapBuffer` + `UndoStack` + `dirty: bool` + `filename: Option<String>`. All mutations (`insert`, `delete_range`) record undo operations.
- **Pos** (`selection.rs`): `{ line: usize, col: usize }` — 0-indexed, col is character index not byte offset. Implements `Ord`.
- **Selection** (`selection.rs`): `{ anchor: Pos, cursor: Pos }`. `anchor == cursor` means no selection. `ordered()` returns `(start, end)`.
- **UndoStack** (`operation.rs`): Groups operations automatically by: kind change (insert vs delete), word boundary (space/newline), time gap (>1s), cursor jump, or explicit `seal()`. `seal()` immediately flushes the current group for atomic undo of paste/comment operations.
- **Editor** (`editor.rs`): Owns everything. Event loop uses `mpsc` channels — background thread for stdin. SIGWINCH polled via atomic flag on 500ms timeout. Main thread does `recv_timeout(500ms)` for status message expiry.

## Event Loop

Channel-based (`std::sync::mpsc`). No async runtime.

1. Background thread: reads `stdin.events()` via termion → sends `EditorEvent::Term(Event)`. Detects bracketed paste markers (`\x1b[200~`/`\x1b[201~`) and buffers pasted text into a single `EditorEvent::Paste(String)` for atomic undo.
2. Main thread: `recv_timeout(500ms)` — dispatches events, polls SIGWINCH atomic flag, expires status messages, redraws

## Rendering

All output buffered to a `Vec<u8>`, written to terminal in a single `write_all` per frame. Synchronized output protocol (`\x1b[?2026h`/`\x1b[?2026l`) wraps each frame so supporting terminals (kitty, iTerm2, WezTerm, ghostty, foot) hold rendering until complete; unsupporting terminals ignore the sequences. Lines are overwritten in-place with `\x1b[K` (erase to end of line) after content rather than `\x1b[2K` (erase entire line) before, eliminating clear-then-draw flicker. Scroll at document boundaries short-circuits (no redraw). Syntax highlighting: per-line HlState computed from line 0 through last visible line each frame; per-char HlType mapped from byte highlights; ANSI colors emitted with minimal escape changes on the fast path. Selection/find highlights override syntax colors. Bracket matching: when cursor is on a bracket `()[]{}`, the matching bracket is highlighted with magenta background/black text. Status bar (reverse video) on second-to-last row shows `Language │ Ln X, Col Y` on the right. Command buffer on last row when active with blinking cursor. Tab completions render above the status bar. Selection rendered as reverse video, find matches as yellow background (current match green). Line numbers in dim text (no separator). Tabs display as dark grey `|` pipe followed by space. Cursor hidden during find navigation mode.

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
| `^f` | Find (regex, smart-case); Enter → browse with up/down, Esc exits |
| `^p` | Command palette |
| `^l` | Goto line |
| `^k` | Kill line |
| `^t` | Goto top |
| `^g` | Goto end |
| `^d` | Toggle comment |
| `^r` | Toggle ruler |
| `^h` | Ctrl+Backspace (delete word) |
| `^j` | Duplicate line |
| `^w` | Select word at cursor |
| `Tab` | Indent selected lines (or insert tab/spaces) |
| `Shift+Tab` | Dedent line(s) |
| `Delete` | Forward delete (non-configurable) |
| `Left/Right` | Move cursor; snaps to 2-space indent stops in leading whitespace |
| `Shift+Arrows` | Extend selection (left/right also snap to indent stops) |
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

## Building

- `cargo build` — debug build
- `just release` — optimized release build (~313KB), requires nightly + rust-src
- `just install` — release build + copy to `/usr/local/bin/e`

## Development Guidelines

- Run `cargo clippy && cargo test` before every commit — zero warnings, all tests pass
- All modules have inline `#[cfg(test)] mod tests` — 264 tests total
- Prefer `&self` over `&mut self` for read-only operations (the line cache uses interior mutability via `Option<Vec<usize>>`)
- Minimize heap allocations in hot paths (render loop, cursor movement)
- No `unwrap()` on user-facing I/O — propagate errors or show in status bar
- Keep the dependency count at 3 — solve problems with std
- Tests should be self-contained with no external file dependencies (use `std::env::temp_dir()` for integration tests)
- When adding new keybindings, update `KeybindingTable::with_defaults()` in `keybind.rs`, `parse_action()` match arm, and `~/.config/e/keybindings.ini`

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
- [x] Find navigation mode (up/down browse matches, "match X of Y", current match green, exits to selection)
- [x] Shift+Tab dedent (removes leading tab or 2 spaces from current/selected lines)
- [x] File locking (`~/.config/e/buffers/<encoded_path>.elock`) to prevent concurrent edits
- [x] Automatic `mkdir -p` on save when parent directories don't exist
- [x] Sudo save on permission denied (password prompt with asterisk masking, pipes to `sudo -S`)
- [x] Bracketed paste mode (terminal paste detected as single atomic undo operation)
- [x] Tab indents selected lines (instead of deleting selection)
- [x] Duplicate line (`^j`)
- [x] Forward delete key
- [x] Select word at cursor (`^w`)
- [x] Syntax highlighting (15 languages: Rust, Python, Go, TypeScript, JavaScript, Shell, C, TOML, JSON, YAML, Makefile, HTML, CSS, Dockerfile, Markdown)
- [x] Purple bracket highlighting for `()[]{}` (magenta, not inside strings/comments)
- [x] Bracket matching (cursor on bracket highlights matching bracket with magenta bg, scans up to 1000 lines)
- [x] Markdown highlighting (headers, bold, italic, fenced code blocks, inline code, blockquotes, lists, horizontal rules, HTML comments)
- [x] JSON key/value distinction (keys yellow, string values green, brackets purple)
- [x] YAML key/value distinction (keys yellow, quoted strings green, anchors/aliases cyan, comments grey)
- [x] Semver version highlighting (v1.2.3, 0.3.5-beta.1 → cyan, works inside strings, skips comments, all languages)
