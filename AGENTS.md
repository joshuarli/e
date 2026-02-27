# `e` — Design Document

A performant, minimalist, intuitive terminal text editor in Rust.

## 1. Constraints

- Rust 2024 edition
- Exactly 3 dependencies: `termion` (4), `regex-lite` (0.1), `libc` (0.2)
- Single-file editing only — no tabs, no file browser, no split panes
- macOS and Linux only (no Windows)
- Binary name: `e`, package version: `0.1.0`

### Build profile

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

Release build uses nightly with `-Zunstable-options -Cpanic=immediate-abort` and `-Z build-std=std -Z build-std-features=` to rebuild std with LTO.

## 2. Architecture

Ownership chain: `main.rs` → `Editor` → `Document` → `GapBuffer`

```
src/
  main.rs            Entry point, arg parsing, file safety, file locking, piped stdin
  editor.rs          Editor struct: all state, event loop, action dispatch
  buffer.rs          GapBuffer: Vec<u8> with gap, incremental line-start index
  document.rs        Wraps GapBuffer + UndoStack + dirty flag + filename
  selection.rs       Pos (line, col), Selection (anchor+cursor), word boundary helpers
  operation.rs       Operation enum (Insert/Delete), OperationGroup, UndoStack with grouping
  view.rs            Viewport: scroll offsets, soft-wrap, cursor-to-screen mapping
  render.rs          ANSI rendering: gutter, content, highlighting, status bar
  keybind.rs         EditorAction enum, KeybindingTable with defaults, INI config loader
  command.rs         CommandRegistry: HashMap<String, CommandFn>, built-in commands
  command_buffer.rs  Modal mini-editor for command palette, find, goto, save-as, sudo password
  clipboard.rs       Platform-detected clipboard: pbcopy/wl-copy/xclip/xsel/internal fallback
  file_io.rs         File read/write, CRLF normalization, binary detection, file locking,
                     persistent undo (~/.config/e/undo.bin), cursor persistence (~/.config/e/cursor.bin)
  language.rs        Language detection by file extension (~45 languages), comment syntax
  signal.rs          SIGWINCH handler via libc::sigaction + AtomicBool polling
  highlight.rs       Syntax highlighting: byte-by-byte highlighter, 16 language rule sets,
                     bracket/quote matching, semver detection
```

## 3. Data Structures

### 3.1 GapBuffer (`buffer.rs`)

```rust
pub struct GapBuffer {
    data: Vec<u8>,              // Raw buffer with gap
    gap_start: usize,           // Start of gap
    gap_end: usize,             // End of gap
    line_starts: Vec<usize>,    // Always-valid byte offsets of line starts
    min_dirty_line: usize,      // Min line touched since last take_dirty_line(); usize::MAX = clean
    version: u64,               // Monotonically increasing, bumped on every insert/delete
}
```

- Constant `MIN_GAP: usize = 128` — minimum gap size; used as the growth unit.
- Text stored as UTF-8 bytes. Gap sits between `gap_start` and `gap_end` in `data`.
- Insertions fill the gap. Deletions widen the gap.
- `move_gap_to(pos)` shifts the gap to a logical byte offset using `copy_within`.
- `ensure_gap(needed)` grows the buffer if gap is too small, shifting the tail right.
- `from_vec(data: Vec<u8>) -> Self` — production fast path: takes ownership of the data Vec, pre-allocates `line_starts` with heuristic `content_len / 20 + 16` (avoids ~20 growth doublings for large files), extends the Vec in-place for the gap. Only one allocation total for the whole file-open path.
- Line index: always valid, updated incrementally on every insert/delete. `from_vec()` scans bytes once at load. Each edit uses `binary_search` to locate the affected region and shifts/inserts/removes only the changed entries. A trailing `\n` does NOT create a new line entry.
- `take_dirty_line() -> usize` — returns and resets `min_dirty_line` (used by renderer for incremental highlight invalidation).
- All line-query methods (`line_count`, `line_start`, `line_end`, `line_text`, `pos_to_offset`, `offset_to_pos`, `line_char_len`) take `&self` — no mutable borrow needed.
- `pos_to_offset(line, col)` walks UTF-8 chars from line start to find byte offset.
- `offset_to_pos(offset)` binary searches `line_starts` for the line, then counts UTF-8 chars for the column.
- `display_col_at(line, char_col) -> usize` — display column at `char_col` chars into the line (tabs = 2, other = 1). Pass `usize::MAX` for total line display width. Does not allocate.
- `char_col_from_display(line, target_display) -> usize` — inverse of `display_col_at`: maps a display column back to a char column. Does not allocate.

Free functions:
- `utf8_char_len(first_byte: u8) -> usize` — returns 1-4 based on leading byte.
- `char_count(bytes: &[u8]) -> usize` — counts UTF-8 characters.

### 3.2 Document (`document.rs`)

```rust
pub struct Document {
    pub buf: GapBuffer,
    pub undo_stack: UndoStack,
    pub dirty: bool,
    pub filename: Option<String>,
}
```

All mutations go through Document methods that record undo operations:

- `insert(line, col, bytes) -> Pos` — converts to byte offset, inserts, records `Operation::Insert`, returns new cursor position.
- `delete_range(start_pos, end_pos) -> Pos` — extracts deleted bytes first, deletes, records `Operation::Delete`, returns start_pos. No-op if start >= end.
- `insert_at_byte(offset, bytes, cursor_before, cursor_after)` — raw byte-offset insert. Avoids line-cache lookups. Used by indent/comment operations.
- `delete_at_byte(offset, count, cursor_before, cursor_after)` — raw byte-offset delete.
- `undo() -> Option<Pos>` — pops group from undo stack. Reverses each operation: Insert → delete, Delete → insert (operating directly on GapBuffer).
- `redo() -> Option<Pos>` — pops group from redo stack, replays each operation forward.
- `seal_undo()`, `begin_undo_group()`, `end_undo_group()` — delegate to UndoStack.
- `text_in_range(start, end) -> Vec<u8>` — extracts text between two Pos values.

### 3.3 Pos & Selection (`selection.rs`)

```rust
pub struct Pos {
    pub line: usize,  // 0-indexed
    pub col: usize,   // 0-indexed, CHARACTER index (not byte offset)
}
```
- Implements `Ord` (line first, then col), `PartialEq`, `Eq`, `Clone`, `Copy`.
- `new(line, col)`, `zero()` constructors.

```rust
pub struct Selection {
    pub anchor: Pos,
    pub cursor: Pos,
}
```
- `caret(pos)` — creates a selection with anchor == cursor (no selection).
- `is_empty()` — true when anchor == cursor.
- `ordered() -> (Pos, Pos)` — returns (start, end) where start <= end.

Word boundary helpers:
- `is_word_char(c: u8) -> bool` — ASCII alphanumeric or `_`.
- `prev_word_boundary(line_bytes, col) -> usize` — scans backward: skip non-word chars, then skip word chars.
- `next_word_boundary(line_bytes, col) -> usize` — scans forward: skip word chars, then skip non-word chars.

### 3.4 Operation & UndoStack (`operation.rs`)

```rust
pub enum Operation {
    Insert { pos: usize, data: Arc<[u8]> },  // byte offset + inserted data
    Delete { pos: usize, data: Arc<[u8]> },  // byte offset + deleted data (for undo)
}

pub struct OperationGroup {
    pub ops: Vec<Operation>,
    pub cursor_before: Pos,
    pub cursor_after: Pos,
}

pub struct UndoStack {
    undo: Vec<OperationGroup>,
    redo: Vec<OperationGroup>,
    current: Option<OperationGroup>,  // accumulating group
    last_kind: Option<OpKind>,        // Insert or Delete
    last_time: Option<Instant>,
    last_cursor: Pos,
    force_group: bool,                // when true, all ops go into same group
}
```

**Grouping heuristics** (`should_break_group`): A new group starts when any of these are true (unless `force_group` is set):
1. Kind change (insert after delete or vice versa).
2. Time gap > 1 second since last operation.
3. Cursor jump (`cursor_before != last_cursor` from previous op).
4. Space or newline insert (word boundary).

Key methods:
- `seal()` — flushes current group to undo stack.
- `begin_group()` / `end_group()` — force all operations between into one undo step.
- `record(op, cursor_before, cursor_after)` — clears redo, checks grouping, extends or creates group.
- `undo()` — flushes current, pops from undo, pushes to redo. Returns reversed ops + cursor_before.
- `redo()` — pops from redo, pushes to undo. Returns ops + cursor_after.
- `stacks()` / `restore()` — for serialization/deserialization.

### 3.5 View (`view.rs`)

```rust
pub struct View {
    pub scroll_line: usize,   // First visible logical line
    pub scroll_wrap: usize,   // Which wrapped sub-row of scroll_line is at top
    pub width: u16,            // Terminal width
    pub height: u16,           // Terminal height
}
```

- `text_rows()` — `height - 2` (status bar + command line).
- `text_cols(gutter_width)` — `width - gutter_width`.
- `wrapped_rows(display_width, text_cols) -> usize` — `display_width.div_ceil(text_cols)`, minimum 1. Free function.
- `ensure_cursor_visible(...)` — adjusts scroll so cursor is visible. Computes cursor's wrapped sub-row, counts screen rows from scroll position. Scrolls forward if cursor is below viewport, snaps to cursor if above. **Fast path**: if the cursor is more than `2 × text_rows` logical lines below the scroll position it is definitely off-screen; sets `scroll_line = cursor_line + 1 - rows` (cursor at bottom, 1-row-per-line assumption) without scanning intervening lines. This makes large jumps (e.g. Ctrl+A select-all on a 1M-line file) O(1) instead of O(file_size).
- `scroll_forward(n, ...)` — scrolls forward by `n` screen rows, advancing through wrapped lines.
- `center_on_line(line, ...)` — centers viewport vertically on a line by walking backward, accumulating screen rows until half the viewport is filled.
- `buffer_to_screen(line, col, ...) -> Option<(u16, u16)>` — converts buffer position to screen coordinates. Returns None if off-screen.

### 3.6 Editor (`editor.rs`)

```rust
pub struct Editor {
    doc: Document,
    sel: Selection,
    desired_col: Option<usize>,       // Sticky column for up/down movement
    view: View,
    renderer: Renderer,
    clipboard: Clipboard,
    commands: CommandRegistry,
    keybindings: KeybindingTable,
    cmd_buf: CommandBuffer,
    ruler_on: bool,                    // Line number ruler visible (default: true)
    status_msg: String,
    status_time: Option<Instant>,
    running: bool,
    quit_pending: bool,                // Waiting for y/n on dirty quit
    last_click_time: Option<Instant>,  // For double/triple click detection
    last_click_pos: Option<(u16, u16)>,
    click_count: u8,                   // 1=single, 2=double, 3=triple
    dragging: bool,
    find_pattern: String,
    find_matches: Vec<(Pos, Pos)>,     // Viewport-only matches, refreshed each draw()
    find_re: Option<regex_lite::Regex>, // Compiled regex cached across keystrokes
    find_current: Option<(Pos, Pos)>,  // Currently-navigated match
    find_active: bool,                 // Browsing find results
    sudo_save_tmp: Option<String>,
    piped_stdin: bool,
    file_mtime: Option<SystemTime>,
    reload_pending: bool,
    status_left_cache: String,         // Reused buffer for status-bar left string (no per-frame alloc)
}
```

## 4. Entry Point (`main.rs`)

### Command-line interface

```
Usage: e [file]
```

- More than 1 argument after the binary name → error with `"Usage: e [file]"`.
- If file exists: check size (>5MB prompts y/n confirmation via stderr), read file, check for binary content (null bytes in first 8KB → prompts y/n).
- If file doesn't exist: empty buffer with filename set.
- No file argument + piped stdin: read all stdin data, create unnamed buffer.
- No file argument + no pipe: empty unnamed buffer.

### Piped stdin detection

Uses `libc::isatty(0) == 0`. If piped, reads all stdin data into `Vec<u8>` before anything else. The editor's background input thread then reads from `/dev/tty` instead of stdin.

### File locking

Before creating the Editor, acquires a file lock via `file_io::acquire_lock()`. On exit, releases via `file_io::release_lock()`. Lock files live at `~/.config/e/buffers/<encoded_path>.elock` where path encoding replaces `/` with `%2F` and `%` with `%25`.

## 5. Event Loop

Channel-based (`std::sync::mpsc`). No async runtime.

```rust
enum EditorEvent {
    Term(Event),      // Terminal event from termion
    Paste(String),    // Bracketed paste (accumulated text)
    Tick,             // Unused
}
```

### Background input thread

Spawned in `Editor::run()`. Reads events from `stdin.events()` (or `File::open("/dev/tty").events()` when stdin was piped).

Bracketed paste detection: when the thread sees `\x1b[200~` (PASTE_START), it enters paste mode. All subsequent `Key::Char(c)` events accumulate into a String. `Key::Backspace` appends `\x7f`. On `\x1b[201~` (PASTE_END), the accumulated string is sent as a single `EditorEvent::Paste(String)`.

### Main loop

1. Expire status messages after 3 seconds.
2. Call `draw()` to render the frame.
3. `recv_timeout(500ms)`:
   - `Term(ev)` → `handle_event(ev)`
   - `Paste(text)` → if command buffer active: `insert_str(text)`; else `paste_text(text)`
   - Timeout → poll SIGWINCH via `take_sigwinch()`, update terminal size if changed.
   - Disconnected → break.

### Terminal modes

On entry (in order):
```
\x1b[?1000h  — Enable mouse tracking (button events)
\x1b[?1002h  — Enable button-motion tracking (drag events)
\x1b[?1006h  — Enable SGR mouse mode (for coordinates >223)
\x1b[?2004h  — Enable bracketed paste mode
\x1b[?1004h  — Enable focus events
```

On exit (reverse order):
```
\x1b[?1004l  — Disable focus events
\x1b[?2004l  — Disable bracketed paste mode
\x1b[?1006l  — Disable SGR mouse mode
\x1b[?1002l  — Disable button-motion tracking
\x1b[?1000l  — Disable mouse tracking
```

Uses `termion::raw::IntoRawMode` and `termion::screen::IntoAlternateScreen`.

### Special escape sequences parsed from `Event::Unsupported`

| Bytes | Meaning |
|---|---|
| `\x1b[1;6A` | Ctrl+Shift+Up → `select_above()` |
| `\x1b[1;6B` | Ctrl+Shift+Down → `select_below()` |
| `\x1b[1;5D` | Ctrl+Left → `word_left()` |
| `\x1b[1;5C` | Ctrl+Right → `word_right()` |
| `\x1bOd` | Ctrl+Left (rxvt/tmux) → `word_left()` |
| `\x1bOc` | Ctrl+Right (rxvt/tmux) → `word_right()` |
| `\x1b[1;6D` | Ctrl+Shift+Left → `word_left_extend()` |
| `\x1b[1;6C` | Ctrl+Shift+Right → `word_right_extend()` |
| `\x1b[127;5u` | Ctrl+Backspace (CSI u / kitty/ghostty) → `ctrl_backspace()` |
| `\x1b[I` | Focus in → `check_external_modification()` |

## 6. Event Dispatch

`handle_event(ev)` dispatches:
- `Event::Key(key)` → if command buffer active: `handle_cmd_key(key)`; else `handle_key(key)`.
- `Event::Mouse(mouse)` → if not in command buffer: exit find mode if active, then `handle_mouse(mouse)`.
- `Event::Unsupported(bytes)` → check for special sequences above.

`handle_key(key)` flow:
1. **Quit pending check** — if `quit_pending`: `y`/`Y` → save and quit (if no filename, opens "Save as:" prompt and defers quit until prompt is confirmed); `n`/`N` → save undo history and quit; anything else → cancel.
2. **Reload pending check** — if `reload_pending`: `y`/`Y` → reload file; anything else → dismiss.
3. **Find active check** — if `find_active`: Up → find_prev; Down → find_next; Esc → exit find and clear selection; anything else → exit find and fall through.
4. **Desired column reset** — preserved only for Up/Down/PageUp/PageDown; reset to None for all other keys.
5. **Keybinding table lookup** — check configurable bindings.
6. **Non-configurable keys** — Shift+arrows, arrows, Home, End, Esc, Delete, Backspace, Tab, BackTab, Enter, printable chars.

## 7. Keybindings (`keybind.rs`)

### Configurable bindings (defaults)

| Key | Action | Method |
|---|---|---|
| Ctrl+S | Save | `save_file()` |
| Ctrl+Q | Quit | `try_quit()` |
| Ctrl+Z | Undo | `undo()` |
| Ctrl+Y | Redo | `redo()` |
| Ctrl+A | SelectAll | `select_all()` |
| Ctrl+C | Copy | `copy()` |
| Ctrl+X | Cut | `cut()` |
| Ctrl+V | Paste | `paste()` |
| Ctrl+K | KillLine | `kill_line()` |
| Ctrl+T | GotoTop | `goto_top()` |
| Ctrl+G | GotoEnd | `goto_end()` |
| Ctrl+R | ToggleRuler | toggle `ruler_on`, force full redraw |
| Ctrl+P | CommandPalette | open command buffer in Command mode, prompt "> " |
| Ctrl+L | GotoLine | open command buffer in Goto mode, prompt "goto: " |
| Ctrl+F | Find | open command buffer in Find mode, prompt "find: ", prefill from selection if <= 100 chars |
| Ctrl+H | CtrlBackspace | `ctrl_backspace()` |
| Ctrl+D | ToggleComment | `toggle_comment()` |
| Ctrl+J | DuplicateLine | `duplicate_line()` |
| Ctrl+W | SelectWord | `select_word_at(cursor())` |

### Non-configurable keys

| Key | Action |
|---|---|
| Up/Down/Left/Right | Move cursor (Left/Right: collapse selection first, snap to 2-space indent stops in leading whitespace) |
| Shift+Up/Down/Left/Right | Extend selection (Left/Right also snap to indent stops) |
| Home | Column 0 |
| End | End of line |
| Ctrl+Left/Right | Word left/right (collapse selection, wrap across lines) |
| Ctrl+Shift+Left/Right | Word left/right (extend selection, wrap across lines) |
| Ctrl+Shift+Up | Select from cursor to start of file |
| Ctrl+Shift+Down | Select from cursor to end of file |
| PageUp/PageDown | Move by `text_rows` lines (preserves desired_col) |
| Tab | Indent selected lines or insert tab/2-spaces |
| Shift+Tab (BackTab) | Dedent line(s) |
| Delete | Forward delete |
| Backspace | Smart backward delete |
| Enter | Insert newline with auto-indent |
| Esc | Clear selection, clear find highlights |
| Any printable char | Insert (with auto-close pairs) |

### Configuration file

Path: `~/.config/e/keybindings.ini`

Format:
```ini
[keybindings]
ctrl+s = save
ctrl+q = quit
```

- Lines starting with `#` or `[` are ignored.
- Keys lowercased before parsing.
- Only `ctrl+<single_char>` supported.
- Action names: `save`, `quit`, `undo`, `redo`, `selectall`, `copy`, `cut`, `paste`, `killline`, `gototop`, `gotoend`, `toggleruler`, `commandpalette`, `gotoline`, `find`, `ctrlbackspace`, `togglecomment`, `duplicateline`, `selectword`.

## 8. Editing Behaviors

### Insert character (`insert_char`)

1. If selection active and char has auto-close pair: wrap selection with pair chars. E.g., selecting `foo` and typing `(` → `(foo)` with inner text selected.
2. If selection active otherwise: delete selection first.
3. **Skip-over**: if char is a closing char (`)]}"'`) and the next char in buffer matches → just advance cursor (don't insert).
4. **Auto-close**: if char has auto-close pair and next char is a boundary (space, tab, close-char, or end-of-line) → insert both chars, place cursor between them.
5. Otherwise: insert character normally.

Auto-close pairs: `(→)`, `[→]`, `{→}`, `"→"`, `'→'`.

### Insert tab (`insert_tab`)

- If selection active: indent all selected lines.
- Otherwise: insert `\t` for `.c`, `.h`, `.go`, `Makefile` files; `  ` (2 spaces) for everything else.

### Insert newline (`insert_newline`)

1. Delete selection if active.
2. Copy leading whitespace (spaces + tabs) from current line.
3. Insert `\n` + copied indent.
4. Seal undo before and after.

### Backspace

1. If selection active: delete selection.
2. If in leading whitespace, col >= 2, col is even, all spaces before cursor: delete 2 spaces (smart dedent).
3. If cursor is between an auto-close pair (prev char's close matches next char): delete both characters.
4. Otherwise: delete one character backward.
5. At column 0: join with previous line (delete the newline at end of previous line).

### Ctrl+Backspace (`ctrl_backspace`)

1. If selection active: delete selection.
2. At col 0 line 0: no-op.
3. At col 0: join with previous line.
4. Otherwise: use `prev_word_boundary` to find target, delete range. Sealed undo around the operation.

### Forward delete (`delete_forward`)

- If selection active: delete selection.
- If not at end of line: delete one character forward.
- If at end of line but not last line: join with next line.

### Kill line (`kill_line`)

Deletes the entire current line including its newline. Seals undo before and after.

### Duplicate line (`duplicate_line`)

Inserts `\n` + current line text after the current line end. Moves cursor to same column on new line. Seals undo before and after.

### Movement

**Up/Down**: Use `desired_col` for sticky column. Clamp to line length.

**Left/Right**: Collapse selection first if active. Wrap across lines. In leading spaces, snap to 2-space indent stops:
- `indent_snap_left(line, col)`: If col is in leading spaces and all spaces, snap to `(col-1)/2*2`. Otherwise `col-1`.
- `indent_snap_right(line, col)`: If col is in leading spaces and all spaces, snap to `(col/2+1)*2` clamped to leading whitespace length. Otherwise `col+1`.

**Word left/right**: Collapse selection first. Use `prev_word_boundary`/`next_word_boundary`. Wrap across lines at boundaries.

**Home/End**: Column 0 / end of line.

**PageUp/PageDown**: Move by `text_rows` lines. Preserve `desired_col`.

### Selection

- **Shift+Arrow**: Modify `sel.cursor` only (anchor stays put). Left/Right also snap to indent stops.
- **Ctrl+Shift+Up/Down**: Select from cursor to start/end of file.
- **Word left/right extend**: Like word left/right but extends selection.
- **Select all**: anchor = (0,0), cursor = end of last line.
- **Select word at cursor** (`select_word_at`): If cursor is on a word char, scan backward and forward through word chars. Anchor at end of word, cursor at start.

### Indent/Dedent

**Indent selection** (`indent_selection`):
- Determines line range from selection. If selection end is at col 0 and on a line after start, exclude that last line.
- Pre-reads all line data and byte offsets to avoid O(n^2) cache rebuilds.
- Uses `begin_undo_group`/`end_undo_group` for atomic undo.
- Iterates lines in reverse, inserts tab or 2 spaces at start of non-blank lines using `insert_at_byte`.
- Adjusts cursor column by indent amount if cursor is on an indented line.

**Dedent** (`dedent`):
- Same line range logic and pre-read strategy.
- Iterates lines in reverse, removes leading `\t` (1 byte) or 2 spaces using `delete_at_byte`.
- Adjusts cursor column by amount removed.

### Comment toggle (`comment_impl`)

1. Detect language from filename.
2. Determine line range (selection or current line).
3. Pre-read all line data and byte offsets.
4. Check if all non-blank lines start with `"{comment} "` after their indent → all_commented.
5. `force` parameter: `None` = toggle, `Some(true)` = always comment, `Some(false)` = always uncomment.
6. **Uncomment**: iterate lines in reverse, remove first `"{comment} "` from each indented line using `delete_at_byte`.
7. **Comment**: find minimum indent across non-blank lines. Iterate in reverse, insert `"{comment} "` at minimum indent position on non-blank, non-already-commented lines using `insert_at_byte`.
8. All wrapped in `begin_undo_group`/`end_undo_group`.

## 9. Clipboard (`clipboard.rs`)

Detection priority:
1. macOS: `pbcopy` exists → Pbcopy, else Internal.
2. Linux: `WAYLAND_DISPLAY` env var set + `wl-copy` exists → WlCopy, else `xclip` exists → Xclip, else `xsel` exists → Xsel, else Internal.

Command detection: `which <name>` with null stdout/stderr.

- **Copy**: always stores in `internal: String`. For system backends, pipes to command via spawned process with piped stdin.
- **Paste**: for system backends, reads stdout. For Internal, returns stored string.

| Backend | Copy command | Paste command |
|---|---|---|
| Pbcopy | `pbcopy` | `pbpaste` |
| WlCopy | `wl-copy` | `wl-paste -n` |
| Xclip | `xclip -selection clipboard` | `xclip -selection clipboard -o` |
| Xsel | `xsel --clipboard --input` | `xsel --clipboard --output` |

Test-only: `internal_only()` forces Internal backend.

### Smart paste (`reindent_paste`)

For multi-line pastes (>= 2 lines after splitting on `\n`):
1. Find minimum indentation of non-empty lines in lines 2+ of the pasted text.
2. Get current line's leading whitespace count.
3. Target indent = cursor column if first paste line has content, else current line's indent.
4. If target_indent == min_indent, return unchanged.
5. Otherwise, re-indent: strip min_indent spaces from each non-empty line in lines 2+, prepend target_indent spaces.

## 10. Find & Replace

### Find (`update_find_highlights`) — lazy / viewport-only

**Design**: never scans the whole file on a keystroke. O(viewport) highlighting; O(lines_to_first_match) initial jump.

1. Smart-case: case-insensitive if pattern is all lowercase.
2. Compiles regex once and caches it in `find_re: Option<regex_lite::Regex>`. Invalid regex → silently ignored.
3. Calls `refresh_viewport_matches()` to populate `find_matches` with only the visible lines' matches (scroll_line..scroll_line + text_rows + 4).
4. Sets `find_current` to the first viewport match at/after cursor; if none, calls `search_forward` to scan forward through the file until the first match is found.
5. Multi-line patterns won't match across line boundaries. Stores matches as `Vec<(Pos, Pos)>`.

`refresh_viewport_matches()`: uses `take/restore` on `find_re` to satisfy the borrow checker while calling `buf.line_text_into`. Repopulates `find_matches` from scratch; called every `draw()` so highlights stay correct after scrolling.

### Find workflow

1. **Ctrl+F**: opens find command buffer. Prefills from selection if <= 100 chars. Clears `find_re`, `find_current`.
2. **As user types** (`Changed` result): `update_find_highlights` runs. If `find_current` found, jumps to it and centers viewport. Status shows "Find: {pattern}".
3. **Enter**: activates find browse mode (`find_active = true`). Up/Down navigate matches (wrapping). Status stays "Find: {pattern}".
4. **Esc while browsing**: exits find mode (`exit_find_mode`), selects current match. Clears `find_re`, `find_current`, `find_matches`, status.
5. **Cancel (Esc while command buffer open)**: clears `find_re`, `find_current`, `find_matches`, status.
6. **Any other key while browsing**: exits find mode, processes key normally.

### Find next/prev

- `find_next`: takes `find_re`, calls `search_forward(buf, re, cursor)` which scans forward from cursor (wrapping around end of file). Updates `find_current`. O(lines_to_next_match).
- `find_prev`: takes `find_re`, calls `search_backward(buf, re, cursor)` which scans backward from cursor (wrapping around start of file), returning the last match on each line. O(lines_to_prev_match).
- Both use `take/restore` on `find_re` to avoid borrow conflicts.

### Replace all (`replace_all`)

1. Same smart-case regex construction.
2. If selection active: operates within selection range. Otherwise: whole file.
3. Extracts text in range, applies `re.replace_all`.
4. If unchanged: reports "Replaced 0 occurrences".
5. Counts matches, seals undo, deletes range, inserts new text, seals undo.
6. Clears selection, reports count.

## 11. Commands (`command.rs`)

### Command registry

`CommandRegistry` wraps `HashMap<String, CommandFn>` where `CommandFn = fn(&str, &mut CommandContext)`.

```rust
pub struct CommandContext {
    pub action: CommandAction,
}

pub enum CommandAction {
    None, Save, SaveAs(String), Quit, Goto(usize), ToggleRuler,
    ReplaceAll { pattern: String, replacement: String },
    ToggleComment, CommentOn, CommentOff, Find(String), SelectAll, Trim,
    StatusMsg(String),
}
```

### Registered commands

| Name | Args | Action |
|---|---|---|
| `save` | `[filename]` | Save or SaveAs |
| `quit` / `q` | — | Quit |
| `goto` | `<line>` | Goto(n), error if not a number |
| `ruler` | — | ToggleRuler |
| `find` | `<pattern>` | Find(pattern), parsed via `parse_args` |
| `replaceall` | `<pattern> <replacement>` | ReplaceAll, requires 2 args via `parse_args` |
| `comment` | `[on\|off]` | ToggleComment / CommentOn / CommentOff |
| `selectall` | — | SelectAll |
| `trim` | — | Trim (strip trailing whitespace) |

### Argument parsing (`parse_args`)

Supports single-quoted, double-quoted, and unquoted tokens. No backslash escaping. Whitespace splits unquoted tokens.

### Command execution flow

1. `execute()` splits input on first space into (name, args).
2. Looks up name in HashMap. If found, creates `CommandContext`, calls function, returns action.
3. If not found: returns `StatusMsg("Unknown command: ...")`.

### Tab completion (`complete_command`)

1. Empty input: show all command names (sorted).
2. One match: autocomplete (replace input with full name).
3. Multiple matches: show matches as completions, complete common prefix.

## 12. Command Buffer (`command_buffer.rs`)

```rust
pub struct CommandBuffer {
    pub input: String,
    pub cursor: usize,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
    pub prompt: String,
    pub mode: CommandBufferMode,
    pub active: bool,
    pub completions: Vec<String>,
}

pub enum CommandBufferMode {
    Command,   // ^p command palette
    Find,      // ^f regex find
    Goto,      // ^l goto line
    Prompt,    // save-as, quit confirmation
    SudoSave,  // password prompt for sudo save
}

pub enum CommandBufferResult {
    Submit(String), Cancel, Continue, Changed(String), TabComplete,
}
```

Key handling:
- Enter → Submit
- Esc / Ctrl+Q → Cancel
- Tab → clears completions, returns TabComplete
- Char(c) → insert at cursor, clears completions, returns Changed
- Backspace → delete before cursor, clears completions, returns Changed (Continue if at start)
- Left/Right → move cursor, Continue
- Up → history_prev, Continue
- Down → history_next, Continue

Special behaviors:
- `SudoSave` mode: `display_line()` masks input with `*`.
- `insert_str(s)`: for bracketed paste — strips `\n` and `\r` before inserting.
- History: close saves non-empty input. Up navigates backward (last → first), Down forward. Past-end clears input.
- Close clears completions.

## 13. Rendering (`render.rs`)

### Renderer struct

```rust
pub struct Renderer {
    pub needs_full_redraw: bool,
    syntax: Option<&'static SyntaxRules>,
    hl_cache: Vec<HlState>,              // Cached highlight state at start of each line
    hl_cache_version: u64,               // Buffer version the cache was for
    hl_dirty_from: usize,                // First line needing recompute (usize::MAX = clean)
    // Scratch buffers reused across render iterations (no per-frame allocation):
    line_buf: Vec<u8>,                   // Raw line bytes from line_text_into
    expanded_scratch: Vec<u8>,           // Tab-expanded bytes
    tab_pipes_scratch: Vec<bool>,        // Per-column tab-pipe markers (empty when no tabs)
    hl_scratch: Vec<HlType>,             // Byte-indexed highlight output
    char_hl_scratch: Vec<HlType>,        // Char-indexed highlight output
    find_scratch: Vec<(usize, usize, bool)>, // Per-line find-range display columns
    frame_buf: Vec<u8>,                  // Reused per-draw output buffer (taken via mem::take)
}
```

### Render flow (`render()`)

1. Compute gutter width: integer digit count loop (no floating-point) + 1 trailing space. 0 if ruler is off.
2. Compute text area: `text_rows = view.text_rows() - completion_rows`, `text_cols = view.text_cols(gw)`.
3. Take `frame_buf` out of `self` via `mem::take`, clear it, use as the output accumulator; put back before returning. No per-draw allocation after warm-up.
4. Wrap output in synchronized output protocol (`\x1b[?2026h` / `\x1b[?2026l`).
5. Hide cursor (`\x1b[?25l`).
6. **Syntax cache** (lazy, viewport-bounded): compute `visible_end = scroll_line + text_rows + 1`. If buffer version changed, read `buf.take_dirty_line()` to get `hl_dirty_from`. Call `refresh_hl_cache(visible_end, scroll_line)` which: truncates if file shrank; extends the cache to `visible_end` if the user scrolled past the current coverage (reprocesses the last cached line to recover its output state, then continues forward); recomputes from `hl_dirty_from` on edits with early-exit when the cached state matches. **Large-jump fast path**: when `scroll_line > computed + 50` (viewport jumped far past the end of the computed cache, e.g. select-all on a 1M-line file), the intermediate gap is left as `HlState::Normal` and recomputation starts only from `scroll_line - 200`. Multi-line constructs starting in the skipped gap are cosmetically approximated as Normal. On first open of a 1M-line file only ~`text_rows` lines are highlighted — O(viewport) not O(file).
7. **Wrap-aware render loop**: walk from `(scroll_line, scroll_wrap)` through logical lines:
   - Expand tabs in each line (`\t` → `|` + space, 2 display columns).
   - Convert to chars. Compute wrapped rows.
   - For each wrapped chunk:
     - **Gutter**: line number (right-aligned, formatted via `write_line_num` into a stack `[u8; 20]` — no heap allocation) on first wrap row; blank on continuation rows. Current line: white bg black text (`\x1b[0;47;30m`). Other lines: dim (`\x1b[0;2m`).
     - **Content**: two paths:
       - **Slow path** (selection, find matches, or bracket matches present): per-character rendering with priority: selection (reverse video `\x1b[7m`) > find match (yellow bg `\x1b[43;30m`, current match green bg `\x1b[42;30m`) > bracket match (magenta bg `\x1b[45;30m`) > tab pipe (dark grey `\x1b[90m`) > syntax highlight.
       - **Fast path** (no overlays): streaming syntax highlighting with minimal escape changes. Tab pipes handled inline.
     - Reset and erase to end of line: `\x1b[0m\x1b[K`.
8. Fill remaining rows with empty lines (blank gutters if ruler on).
9. **Completions**: dim text (`\x1b[2m`) on rows above status bar.
10. **Status bar**: reverse video (`\x1b[0;7m`). Left: built by `build_status_left()` into `Editor::status_left_cache` (reused `String`, filled with `push`/`push_str` — no `format!` allocation after warm-up); the `&str` is passed to `render()`. Right: `" e vVERSION "` (`&'static str` via `concat!`, no per-frame allocation). Padded with spaces to fill width.
11. **Command line**: if active, yellow bg black text (`\x1b[30;43m`) + erase to end. If status message, display it. Otherwise blank.
12. **Cursor positioning**:
    - If `find_active` or selection active: cursor stays hidden.
    - If command buffer active: position cursor in command line at `prompt.len() + cursor`, show cursor.
    - Otherwise: compute cursor screen position accounting for soft-wrap (col % text_cols + gutter for column, count wrapped rows from scroll position for row using `line_text_into` + `display_col_for_char_col` — no allocation), show cursor.
13. Write entire frame buffer via single `write_all`.

### Display column conversion

- `display_col_for_char_col(raw_text, char_col) -> usize`: tabs count as 2 display columns.
- `char_col_for_display_col(raw_text, target_display) -> usize`: inverse conversion.

### Tab display

Tabs expand to `|` (dark grey pipe) + space (2 display columns total).

### Trailing whitespace

Highlighted with red background (`\x1b[41m`) on lines that have non-whitespace content. Trailing whitespace = characters after the last non-whitespace character on the line.

### ANSI color scheme

| HlType | ANSI code | Color |
|---|---|---|
| Normal | (none) | default |
| Comment | `\x1b[90m` | grey |
| Keyword | `\x1b[33m` | yellow |
| Type | `\x1b[36m` | cyan |
| String | `\x1b[32m` | green |
| Number | `\x1b[31m` | red |
| Bracket | `\x1b[35m` | magenta |
| Operator | `\x1b[33m` | yellow (same as keyword) |

## 14. Syntax Highlighting (`highlight.rs`)

### Types

```rust
enum HlType { Normal, Keyword, Type, String, Comment, Number, Bracket, Operator }
enum HlState { Normal, BlockComment, MultiLineString(u8), FencedCodeBlock }

struct StringDelim { open: &'static str, close: &'static str, multiline: bool }
struct SyntaxRules {
    line_comment: &'static str,
    block_comment: (&'static str, &'static str),
    string_delims: &'static [StringDelim],
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    operators: &'static [&'static str],
    highlight_numbers: bool,
    is_markdown: bool,
    is_json: bool,
    is_yaml: bool,
    is_ini: bool,
}
```

### Core function

`highlight_line(line, state, rules) -> (Vec<HlType>, HlState)`:
- Dispatches to specialized highlighter based on flags (`is_markdown`, `is_json`, `is_yaml`, `is_ini`, or generic `highlight_line_code`).
- Runs `highlight_semver` post-pass on result.

### Generic code highlighter (`highlight_line_code`)

Byte-by-byte, inspired by kilo/kibi. Produces one `HlType` per byte.

1. Handle entering in multiline state (block comment continuation, multiline string continuation).
2. Main loop checks in order:
   - **Line comment**: if remaining line starts with `line_comment` → rest of line is Comment.
   - **Block comment start**: scan for close on same line. If not found → return BlockComment state.
   - **String delimiters** (longest open first): check each delimiter. Scan for close with backslash escape handling. If multiline and not closed → return MultiLineString(index).
   - **Numbers** (after separator): digit or `.digit` start. Consume digits/alphanumeric/`_`/`.`. Mark as Number.
   - **Keywords/types** (after separator, followed by separator or end): check keyword list then type list.
   - **Operators**: check operator list (multi-char like `&&`, `||`, etc.).
   - **Brackets**: `()[]{}` → Bracket.
   - Track `prev_sep` (whether previous character was a separator).

Separator function: whitespace, null, or `,.()+-./*=~%<>[]{};&|!^@#?`.

### Specialized highlighters

**JSON** (`highlight_line_json`): Strings checked for trailing `:` → keys (yellow/Keyword) vs values (green/String). Numbers, true/false/null as Type. Brackets.

**YAML** (`highlight_line_yaml`): Finds comments (`#` after whitespace, outside quotes). Keys before unquoted `:` as Keyword. Quoted string values as String. Anchors `&name` and aliases `*name` as Type. Boolean true/false/null/yes/no as Type. Numbers.

**INI** (`highlight_line_ini`): Comment lines (`;` or `#` at start). Section headers `[section]` as Keyword. Key=value pairs: key as Keyword, quoted values as String, booleans (true/false/yes/no/on/off) as Type, numbers as Number. Inline comments after values.

**Markdown** (`highlight_line_markdown`): Fenced code blocks (``` delimiters, content as String). HTML block comments (`<!-- -->`). Horizontal rules. Headers (`#`) as Keyword. Blockquotes (`>`) prefix as Comment. List markers as Number. Inline: code (String), bold `**` (Keyword), italic `*` (Type).

### Semver highlighting

Post-pass on every highlighted line. Finds patterns like `v1.2.3` or `0.3.5-beta.1`:
- Optional `v`/`V` prefix.
- Must not be preceded by alphanumeric/underscore.
- MAJOR.MINOR.PATCH (each one or more digits).
- Optional pre-release (`-alpha.1`) and build metadata (`+build.123`).
- Must not be followed by alphanumeric/underscore.
- Skips bytes already marked as Comment.
- Highlighted as Type (cyan).

### Bracket matching (`find_bracket_match`)

Signature: `find_bracket_match(pos, get_line: &mut impl FnMut(usize, &mut Vec<u8>), scratch: &mut Vec<u8>, line_count) -> Option<Pos>`.

`get_line(idx, buf)` fills `buf` with raw bytes for line `idx` (no allocation). `scratch` is a reused buffer supplied by the caller — eliminates the up-to-1000 per-line `Vec<u8>` allocations the previous `FnMut(usize) -> Vec<u8>` design incurred on deep bracket searches.

Given cursor position on a bracket `()[]{}`:
1. Loads the initial line into `scratch`, extracts target bracket and direction.
2. Scans forward or backward counting depth, reloading `scratch` at each line boundary.
3. Limit: 1000 lines.
4. Returns matching bracket position or None.

### Quote matching (`find_quote_match`)

Signature: `find_quote_match(pos, get_line: &mut impl FnMut(usize, &mut Vec<u8>), scratch: &mut Vec<u8>, line_count) -> Option<Pos>`.

Same scratch-buffer pattern as `find_bracket_match`. For `"` or `'` at cursor:
1. Loads the line into `scratch`. Collects all unescaped positions of same quote char.
2. Pairs sequentially: 0↔1, 2↔3, etc.
3. Returns the paired position if cursor is on one.

Escape detection: counts consecutive preceding backslashes; odd count = escaped.

### Byte-to-char mapping (`byte_hl_to_char_hl`)

Tabs expand to 2 display entries (both get the tab's HlType). Multi-byte UTF-8 collapses to 1 entry (uses first byte's HlType).

### Language rule sets (16 total)

| Language | line_comment | block_comment | string_delims | operators | highlight_numbers |
|---|---|---|---|---|---|
| Rust | `//` | `/* */` | `"` `'` | `&& \|\| != == <= >= => ->` | yes |
| Python | `#` | (none) | `"""` `'''` `"` `'` (triple quotes multiline) | `!= == <= >=` | yes |
| Go | `//` | `/* */` | `` ` `` (multiline) `"` `'` | `&& \|\| != == <= >= :=` | yes |
| TypeScript | `//` | `/* */` | `` ` `` (multiline) `"` `'` | `&& \|\| !== === != == <= >= =>` | yes |
| JavaScript | `//` | `/* */` | `` ` `` (multiline) `"` `'` | `&& \|\| !== === != == <= >= =>` | yes |
| Shell | `#` | (none) | `"` `'` | `&& \|\|` | yes |
| C | `//` | `/* */` | `"` `'` | `&& \|\| != == <= >= ->` | yes |
| TOML | `#` | (none) | `"""` `'''` (multiline) `"` `'` | (none) | yes |
| JSON | (none) | (none) | `"` | (none) | yes, custom highlighter |
| YAML | `#` | (none) | `"` `'` | (none) | yes, custom highlighter |
| Makefile | `#` | (none) | `"` `'` | (none) | no |
| HTML | (none) | `<!-- -->` | `"` `'` | (none) | no |
| CSS | (none) | `/* */` | `"` `'` | (none) | yes |
| Dockerfile | `#` | (none) | `"` `'` | (none) | no |
| Markdown | (none) | `<!-- -->` | (none) | (none) | no, custom highlighter |
| Config/INI | `;` | (none) | `"` `'` | (none) | yes, custom highlighter |

`rules_for_language(name)` maps language name to static rules reference.

Each language rule set includes keyword and type lists. See the source for the complete lists per language.

## 15. Language Detection (`language.rs`)

```rust
pub struct Language {
    pub name: &'static str,
    pub comment: &'static str,
}
```

50 languages in a static table. Each entry has an array of file extension patterns.

Detection (`detect(filename) -> Option<Language>`):
- Extracts basename from path.
- For patterns starting with `.`: matches as suffix of filename.
- For patterns without `.` (e.g., `Makefile`, `Dockerfile`): exact basename match, or prefix match followed by `.` (e.g., `Dockerfile.release`).

Languages include (with comment syntax): Rust (//), C (//), C++ (//), Go (//), JavaScript (//), TypeScript (//), Java (//), C# (//), Swift (//), Kotlin (//), Scala (//), Python (#), Ruby (#), Shell (#), Perl (#), R (#), JSON (none), YAML (#), TOML (#), Config (#), Lua (--), SQL (--), Haskell (--), Elm (--), HTML (<!--), XML (<!--), CSS (/*), SCSS (//), Less (//), PHP (//), Elixir (#), Erlang (%), Clojure (;;), Lisp (;;), Vim ("), Zig (//), D (//), Dart (//), Objective-C (//), V (//), Nim (#), Crystal (#), Julia (#), Terraform (#), Makefile (#), Dockerfile (#), CMake (#), Protobuf (//), GraphQL (#), Markdown (<!--).

## 16. File I/O (`file_io.rs`)

### Read/Write

- `read_file(path) -> io::Result<Vec<u8>>` — reads file bytes as-is (`fs::read`). No CRLF normalization. Returns the owned Vec for zero-copy handoff to `GapBuffer::from_vec`.
- `write_file(path, data) -> io::Result<()>` — strips trailing whitespace from each line (via `split('\n')` and `trim_end()`), ensures trailing newline, writes.
- `clean_for_write(data) -> Vec<u8>` — same cleaning as write_file but returns bytes (for sudo save).
- `is_likely_binary(data) -> bool` — checks first 8KB (min of data length and 8192) for null bytes.
- `file_size(path) -> io::Result<u64>` — `fs::metadata(path)?.len()`.
- `file_mtime(path) -> Option<SystemTime>` — `fs::metadata(path).ok()?.modified().ok()`.

### File locking

- Lock directory: `~/.config/e/buffers/`
- `encode_path(path) -> String`: `/` → `%2F`, `%` → `%25`.
- `resolve_absolute(path)`: canonicalizes, or canonicalizes parent + filename for non-existent files.
- `acquire_lock(path) -> Result<(), String>`: creates `<encoded_abs_path>.elock` in buffers dir. Errors if file already exists.
- `release_lock(path)`: removes lock file, ignores errors.

### Persistent undo history

Single binary file: `~/.config/e/undo.bin`

Format:
```
[magic: 4 bytes "eUND"]
[version: 1 byte, value 1]
[entry_count: u32 LE]
For each entry:
  [entry_body_len: u32 LE]
  Entry body:
    [path_len: u32 LE][path_bytes]
    [mtime_secs: i64 LE][mtime_nanos: u32 LE]
    [undo_group_count: u32 LE]
    For each undo group:
      [cursor_before_line: u32][cursor_before_col: u32]
      [cursor_after_line: u32][cursor_after_col: u32]
      [op_count: u32]
      For each op:
        [kind: u8, 0=Insert 1=Delete]
        [pos: u64 LE]
        [data_len: u32 LE][data_bytes]
    [redo_group_count: u32 LE]
    (same group format as undo)
```

Constants: `MAX_GROUPS = 100_000`, `MAX_ENTRIES = 10_000`.

Concurrency: uses `libc::flock` (exclusive for write, shared for read).

**Save**: reads existing DB, keeps entries for other paths whose files still exist and mtimes still match (raw byte copy, no deserialization), replaces/adds entry for current path.

**Load**: scans entries linearly for matching path. Validates mtime matches current file. Stale history silently discarded. Calls `undo_stack.restore(undo, redo)`.

### Cursor position persistence

Single binary file: `~/.config/e/cursor.bin`

Format:
```
[magic: 4 bytes "eCUR"]
[version: 1 byte, value 1]
[entry_count: u32 LE]
For each entry:
  [entry_len: u32 LE]
  [path_len: u32 LE][path_bytes]
  [line: u32 LE][col: u32 LE]
```

- No mtime validation — cursor position is clamped to buffer bounds on load.
- On save: prunes entries for files that no longer exist on disk.
- Uses `flock` for concurrency.

## 17. Signal Handling (`signal.rs`)

Static `AtomicBool` (`SIGWINCH_RECEIVED`).

- `register_sigwinch()`: installs `sigwinch_handler` via `libc::sigaction` with `SA_RESTART` flag.
- `sigwinch_handler(_: c_int)`: stores `true` to atomic bool with `Relaxed` ordering.
- `take_sigwinch() -> bool`: atomically swaps flag to `false`, returns previous value.

Polled in main loop on 500ms timeout. On true: `termion::terminal_size()` updates view dimensions, forces full redraw.

## 18. File Save Flow

### Normal save

1. If filename set: create parent directories if needed (`create_dir_all`). On permission denied → start sudo save.
2. Call `file_io::write_file` (strips trailing whitespace, ensures trailing newline).
3. On success: mark not dirty, seal undo, update cached mtime, save undo history. Status: "Saved {path}".
4. On permission denied → start sudo save.
5. If no filename → open save-as prompt (CommandBufferMode::Prompt, "Save as: ").

### Sudo save

1. Write cleaned content to temp file `/tmp/e_sudo_{pid}`.
2. Open password prompt (`CommandBufferMode::SudoSave`, input masked with `*`).
3. On submit: `sudo -S mkdir -p` for parent dirs if needed (pipes password via stdin), then `sudo -S cp {tmp} {path}` (pipes password via stdin).
4. Clean up temp file. Mark not dirty on success.

### External modification detection

Triggered by terminal focus-in event (`\x1b[I`). Compares disk mtime with cached mtime. If different, shows "{filename} changed on disk. Reload? (y/n)" and sets `reload_pending`.

### Reload

Re-reads file, creates new Document, clamps cursor to valid position, clears find state, forces full redraw.

### Quit flow

1. If dirty: shows "Save changes to {name}? (y/n)", sets `quit_pending`.
2. `y` → `save_file()` then quit. If the buffer has no filename, `save_file()` opens the "Save as:" prompt and does NOT quit yet (`quit_pending` stays true, `running` stays true). After the filename is confirmed, the Prompt handler saves and then clears `quit_pending` and sets `running = false`.
3. `n` → save undo history and cursor position, then quit.
4. Anything else → cancel quit.
5. Before quitting (in all paths): `save_undo_if_named()` saves cursor position and undo history to disk.

## 19. Mouse Handling

### Click detection

- Single click (within 400ms at same screen position resets multi-click counter, cycles 1→2→3→1):
  - **Single**: place cursor, start drag.
  - **Double**: select word (scan backward/forward through `is_word_char`). Anchor at word end, cursor at word start.
  - **Triple**: select entire line (anchor at line start, cursor at next line start or end of last line).

### Drag

Updates `sel.cursor` to new buffer position on each `MouseEvent::Hold`.

### Scroll

`SCROLL_LINES = 3` screen rows per wheel event. Handles wrapped lines correctly. After scroll, clamps cursor to visible viewport.

### Screen-to-buffer mapping (`screen_to_buffer_pos`)

Walks from `(scroll_line, scroll_wrap)` through wrapped lines counting screen rows to find which logical line/col was clicked. Uses `char_col_for_display_col` for tab handling. If clicked below all content, returns end of last line.

## 20. Status Bar & Command Line

### Status bar (second-to-last row)

Reverse video (`\x1b[0;7m`). Full width.
- Left: `" {filename}{*} [{language}]"` — `*` if dirty, language from detection or "Text".
- Right: `" e v{VERSION} "` — version from `env!("CARGO_PKG_VERSION")`.
- Padding with spaces between left and right.

### Command line (last row)

- When command buffer active: yellow background black text (`\x1b[30;43m`).
- When status message present (and command buffer inactive): displays message as plain text.
- Otherwise: blank.

### Status messages

- Set via `set_status(msg)` which records `Instant::now()`.
- Expired after 3 seconds in main loop (checks `elapsed().as_secs() >= 3`).
- Some messages don't expire: quit confirmation, reload prompt, find status (status_time set to None).

## 21. Testing

- Run: `cargo clippy && cargo test`
- All modules have inline `#[cfg(test)] mod tests`.
- Test helper: `ed("text")` / `ed_named("text", "file.rs")` creates an 80x24 Editor with internal-only clipboard, no disk I/O, default keybindings.
- Integration tests use `std::env::temp_dir()` for file I/O, clean up with `remove_dir_all`.
- Coverage: `cargo tarpaulin`.
- Philosophy: prefer integration-style scenario tests over tiny unit tests.

## 22. Configuration Paths Summary

| Path | Purpose |
|---|---|
| `~/.config/e/keybindings.ini` | Keybinding overrides |
| `~/.config/e/undo.bin` | Persistent undo history (all files) |
| `~/.config/e/cursor.bin` | Cursor position persistence (all files) |
| `~/.config/e/buffers/<encoded>.elock` | File lock files |

## 23. Indent Rules

- 2 spaces for all files except `.c`, `.h`, `.go`, `Makefile` which use tabs.
- Determined by checking `doc.filename` for these extensions/names.
- Same logic used for both Tab key insertion and indent selection.
