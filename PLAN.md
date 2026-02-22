# Plan: `e` — Minimalist Terminal Text Editor in Rust

## Context

Building a performant, minimalist text editor from scratch. Single-file editing only — no tabs, no file browser. macOS and Linux only.

## Dependencies (3 total)

```toml
[package]
name = "e"
version = "0.1.0"
edition = "2024"

[dependencies]
termion = "4"
regex = "1"
signal-hook = "0.3"
```

## Design Decisions

- **TUI**: termion (lightweight, Unix-only — fits our macOS/Linux constraint)
- **Text buffer**: Gap buffer (`Vec<u8>` with gap). Not a rope — we cap files at 5MB so gap buffer's simplicity and cache locality win.
- **Undo/redo**: Operation-based with grouping. Group boundaries on: kind change, word boundary (space after non-space), >1s gap, cursor jump, explicit seal (paste, kill-line, replace-all, etc.).
- **Config**: Hand-rolled INI parser. Config lives at `~/.config/e/keybindings.ini`. Defaults hardcoded, config overrides.
- **Clipboard**: Shell out to `pbcopy`/`pbpaste` (macOS), `wl-copy`/`xclip`/`xsel` (Linux). Internal fallback if none found.
- **Line endings**: Enforce LF. Auto-convert CRLF on open. Always ensure trailing newline on save.
- **Encoding**: UTF-8 only. Non-UTF-8 files are treated as binary (trigger binary detection prompt).
- **Colors**: Minimal ANSI colors — dim for line numbers, color for status bar, highlight for find matches, reverse for selection. No 256/truecolor.
- **Tab key**: Smart file-type detection. Insert literal `\t` for Makefile/.c/.go files, 2 spaces for everything else. Tabs display as 2 spaces wide.
- **Long lines**: Soft wrap. Arrow up/down navigates visual (screen) rows, not logical lines. Line numbers only on first visual row.
- **Auto-indent**: Enter copies leading whitespace from current line.
- **Smart dedent**: Backspace in leading whitespace at an indent boundary removes a full indent level (2 spaces or 1 tab). Otherwise removes one char.
- **^Backspace**: Deletes previous word (to word boundary).
- **Scroll margin**: 5 lines from top/bottom edge.
- **Scroll wheel**: 3 lines per tick.
- **No-arg launch**: Opens empty scratch buffer. Prompts for filename on first save.
- **Nonexistent file arg**: Opens empty buffer with that filename set. Shows `[new file]` in status bar. ^s creates the file.

## Module Layout

```
src/
  main.rs           — arg parsing, file safety checks, enter raw mode, launch editor
  editor.rs         — Editor struct: owns all state, event loop, action dispatch
  buffer.rs         — GapBuffer (Vec<u8> with gap) + lazy line index cache
  document.rs       — wraps buffer + undo stack + dirty flag + filename
  selection.rs      — Pos, Selection (anchor+cursor), word/line boundary helpers
  operation.rs      — Operation enum (Insert/Delete), OperationGroup, UndoStack with grouping
  view.rs           — Viewport: scroll offsets, cursor-to-screen mapping, ensure-visible, soft wrap
  render.rs         — Dirty-tracked rendering: line hashes, buffered writes, status bar
  input.rs          — Translate termion events → EditorActions based on current mode
  keybind.rs        — Hand-rolled INI parser, KeybindingTable, defaults
  command.rs        — CommandRegistry: HashMap<String, CommandFn>, built-in commands
  command_buffer.rs — Modal mini-editor for ^p palette, ^f find, ^l goto, save-as prompt
  clipboard.rs      — Trait + platform detection, shell out to system clipboard tools
  file_io.rs        — Read/write files, binary detection (null bytes in first 8KB), CRLF→LF, size checks
  signal.rs         — SIGWINCH via signal-hook → sends Resize event on channel
  highlight.rs      — (stub) Highlighter trait for future syntax highlighting
```

## Key Data Structures

- **GapBuffer**: `Vec<u8>` with `gap_start`/`gap_end` byte offsets. Lazy `LineIndex` (Vec of line-start byte offsets, invalidated from edit point downward).
- **Document**: owns GapBuffer + UndoStack + dirty flag + optional filename.
- **Selection**: anchor `Pos` + cursor `Pos` (0-indexed line/col). Anchor==cursor means no selection.
- **UndoStack**: `Vec<OperationGroup>` for undo/redo. Current group accumulates ops until a boundary.
- **Editor**: owns Document, View, Selection, EditorMode enum (Normal | Command | Find | Prompt), keybindings, clipboard, render state, terminal dimensions.

## Keybindings

| Key | Action |
|-----|--------|
| ^s | Save (prompt for filename if unnamed). On permission denied: prompt `sudo tee`. |
| ^q | Quit (prompt if dirty: "save changes to X? (y/n)") |
| ^z | Undo |
| ^y | Redo |
| ^k | Kill line (deletes entire current line, does NOT copy to clipboard) |
| ^t | Go to top of file |
| ^g | Go to end of file |
| ^f | Find (regex, smart-case, wraps around, Enter to search then cycle, Escape clears highlights and exits) |
| ^p | Open command buffer |
| ^l | Go to line (opens command buffer pre-filled with `goto `) |
| ^r | Toggle ruler (line numbers, default on) |
| ^a | Select all |
| ^c | Copy selection to clipboard (no-op if no selection) |
| ^v | Paste from clipboard (verbatim, no indent adjustment) |
| ^x | Cut selection to clipboard (no-op if no selection) |
| Shift+Arrow | Extend selection |
| Home/End | Start/end of line |
| PageUp/PageDown | Scroll by screenful |
| Tab | Insert `\t` for Makefile/.c/.go, 2 spaces otherwise |
| Backspace | Smart dedent if in leading whitespace at indent boundary, else delete one char |
| ^Backspace | Delete previous word |
| Delete key | No-op (ignored) |
| Escape | Clear selection (in normal mode). Cancel and exit (in command/find/prompt mode). |

All keybindings configurable via `~/.config/e/keybindings.ini`.

## Behavioral Details

- **^c/^x without selection**: No-op. Only operate when there is an active selection.
- **Typing with selection**: Replaces selected text with typed character.
- **Find**: Smart-case (case-insensitive if all-lowercase pattern, case-sensitive if any uppercase). Wraps around. Highlights clear on Escape.
- **`replaceall regex replacement`**: Literal replacement string (no capture group refs). Applies to selection if present, otherwise whole file. Shows count in status bar: "Replaced N occurrences".
- **`goto N`**: Clamps to valid range (1 to line_count). No error on out-of-range.
- **Unknown commands**: Show "Unknown command: X" as timed status message.
- **Status bar**: Full file path + `*` when modified. Right side: `Ln X, Col Y │ N lines`. Colored/inverted background.
- **Empty rows past EOF**: Blank gutter (no tilde, no marker).
- **Trailing whitespace**: Silently stripped on save.
- **Small terminal**: Best-effort rendering, no minimum size enforcement.
- **History**: Session-only (in-memory). Not persisted across editor sessions.
- **Crash recovery**: None (out of scope for v0).

## Event Loop

Channel-based (`mpsc`). Background thread reads `stdin.events()` → sends `TermInput`. Signal handler via signal-hook → sends `Resize`. Main thread does `recv_timeout(500ms)` for ticks (status message expiry). No async runtime.

## Rendering

Buffer all output to `Vec<u8>`, flush once. Dirty flags: FULL, LINES, STATUS_BAR, CURSOR, SELECTION. Per-line hashes to skip unchanged lines.

- **Ruler** (gutter): line numbers in dim/gray, `│` separator. Only on first visual row of wrapped lines.
- **Selection**: reverse video
- **Status bar**: colored/inverted background. Left: full path + `*`. Right: `Ln X, Col Y │ N lines`
- **Current line**: No special indicator (cursor is sufficient)
- **Find matches**: highlighted with ANSI color
- **Command buffer**: last row when active, `> ` prompt
- **Empty lines past EOF**: blank gutter, blank text area

## File Safety

Before opening: check size (>5MB → confirm), read first 8KB for null bytes (binary → confirm). Non-UTF-8 files treated as binary. On load: CRLF→LF in-place conversion. On save: always write LF, ensure trailing newline, strip trailing whitespace.

## Implementation Phases

### Phase 1: Skeleton — get text on screen
1. `Cargo.toml` with deps
2. `main.rs` — arg parsing (optional filename), raw mode + alternate screen, cleanup on exit
3. `buffer.rs` — GapBuffer with insert/delete/slice, line index
4. `file_io.rs` — read file, CRLF normalize, write file, binary detection, size checks
5. `view.rs` — viewport basics, soft wrap calculation, scroll margin (5 lines)
6. `render.rs` — full repaint: visible lines with ruler (soft-wrapped), status bar
7. `editor.rs` — minimal event loop: arrow keys (visual-line nav on wrapped lines), Home/End, PageUp/PageDown, char insert, backspace, ^q quit

**Checkpoint**: open a file (or empty buffer), see content with line numbers, soft-wrapped long lines, move cursor, type, quit.

### Phase 2: Core editing
1. `selection.rs` — Selection type, word/line boundaries
2. Shift+Arrow selection, ^a select all, Escape clears selection, typing replaces selection
3. `operation.rs` — Operation, UndoStack with grouping heuristics
4. `document.rs` — all mutations record operations
5. ^z undo, ^y redo
6. Smart Tab key (file-type detection for tab vs spaces)
7. Auto-indent on Enter (copy leading whitespace)
8. Smart dedent on Backspace
9. ^Backspace word delete

**Checkpoint**: full editing with undo/redo, selection, tab, indent, and word-delete behavior.

### Phase 3: Mouse and clipboard
1. Enable termion MouseTerminal
2. Click to place cursor, drag to select
3. Double-click word select, triple-click line select (timing/position tracking)
4. `clipboard.rs` — platform detect + shell out to system clipboard tools (internal fallback)
5. ^c copy, ^v paste (verbatim), ^x cut, scroll wheel (3 lines/tick)

**Checkpoint**: mouse interaction + system clipboard.

### Phase 4: Commands and find
1. `command.rs` — registry with built-in commands (save, quit, goto, replaceall, ruler)
2. `command_buffer.rs` — modal input line, session history (up/down), render
3. ^p command palette, ^l goto line (pre-fills `goto `, clamps to range), ^f regex find (smart-case, wrap, Enter cycles, Escape clears highlights)
4. ^k kill line (delete entire line, no clipboard), ^t top, ^g end
5. ^s save (prompt for filename if unnamed, sudo tee on permission denied)
6. `replaceall regex replacement` (literal replacement, selection-aware, shows count)
7. Unknown command error in status bar

**Checkpoint**: all v0 keybindings and commands working.

### Phase 5: Polish
1. `keybind.rs` — INI config parser, load from `~/.config/e/keybindings.ini`, fall back to defaults
2. `signal.rs` — SIGWINCH handling for window resize
3. File open safety checks with confirmation prompts (binary, >5MB, non-UTF-8)
4. Save: strip trailing whitespace, ensure trailing newline
5. Quit confirmation when dirty ("save changes to /path/file? (y/n)")
6. ^r toggle ruler (default on)
7. Differential rendering with dirty line tracking + line hashes
8. Timed status messages (e.g., "File saved", "Find: no matches", "Replaced N occurrences")

**Checkpoint**: polished v0 with all features.

### Phase 6: Future stubs
1. `highlight.rs` — `Highlighter` trait with `highlight_line()` signature, no implementations
2. `comment` command signature in registry (returns "not yet implemented")

## Verification

After each phase:
- `cargo build` compiles clean
- Manual test: open a real file, exercise the features from that phase
- After phase 5: full end-to-end test of every keybinding and command listed in AGENTS.md
