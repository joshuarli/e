# `e` — Design Document

A performant, minimalist, intuitive terminal text editor for single-file editing. No tabs, no file browser, no split panes. macOS and Linux only.

## 1. Startup

### Command line

```
e [file]
```

- No arguments: opens an empty unnamed buffer.
- One argument, file exists: opens the file.
- One argument, file doesn't exist: opens an empty buffer with the filename set (for saving later).
- More than one file argument: print `Usage: e [file]` to stderr and exit with code 1.

### Safety checks on open

- If the file is larger than 5MB, prompt `e: {filename} is {size}MB. Open anyway? (y/n)` on stderr. Declining exits cleanly.
- After reading, if the first 8KB contains any null byte, prompt `e: {filename} appears to be binary. Open anyway? (y/n)` on stderr. Declining exits cleanly.

### Piped stdin

`e` supports piped input (e.g. `git log | e`). When stdin is not a TTY:
- Read all stdin data before entering the editor.
- The buffer is unnamed (no filename).
- Keyboard input must come from `/dev/tty` instead of stdin.

### File locking

When opening a named file, acquire a lock file at `~/.config/e/buffers/<encoded_path>.elock`. If the lock already exists, print an error and exit. The path is encoded by replacing `/` with `%2F` and `%` with `%25`. On exit, the lock is released.

### Restored state

On open, restore the last known cursor position and undo history for the file (see Persistence). The cursor is clamped to valid buffer bounds. If the cursor was not at the origin, center the viewport on it.

## 2. Line ending and whitespace handling

- On read: CRLF (`\r\n`) is normalized to LF (`\n`). Lone `\r` is preserved.
- On write: trailing whitespace (spaces and tabs) is stripped from every line, and a trailing newline is ensured.
- Tabs display as 2 columns: a dim grey pipe character `|` followed by a space.

## 3. Display

### Layout

The terminal is divided into:
1. **Text area** — fills all rows except the last two. Shows file content with optional line numbers.
2. **Completions area** — zero or more rows above the status bar showing tab-completion suggestions (dim text).
3. **Status bar** — second-to-last row, reverse video. Left side: `" {filename}{*} [{language}]"` where `*` appears if dirty and language is auto-detected (or "Text"). Right side: `" e v{version} "`.
4. **Command line** — last row. When a command buffer is active: yellow background with black text. When a status message is showing: plain text. Otherwise blank.

### Line numbers (ruler)

On by default. Togglable. When on:
- Line numbers are right-aligned in a gutter whose width accommodates the largest line number plus one trailing space.
- The current line's number has a white background with black text.
- Other line numbers are dim.
- Continuation rows from soft-wrapping get a blank gutter.

### Soft-wrap

Long lines wrap at the right edge of the viewport. There is no horizontal scrolling. The viewport tracks both which logical line and which wrapped sub-row is at the top of the screen.

### Trailing whitespace highlighting

On lines that contain non-whitespace content, any trailing whitespace (spaces/tabs) is highlighted with a red background.

### Cursor visibility

The cursor is hidden during find-browse mode and when a selection is active. When the command buffer is active, the cursor appears in the command line.

### Rendering approach

All output for a frame is buffered and written in a single write call. Frames are wrapped in the synchronized output protocol (DEC private mode 2026) so supporting terminals hold rendering until the frame is complete. Lines are overwritten in-place with erase-to-end-of-line after content (not clear-then-draw).

## 4. Syntax highlighting

### Language detection

Language is detected from the filename extension (~45 languages supported). Extensions starting with `.` match as suffix. Others (like `Makefile`, `Dockerfile`) match as exact basename or prefix followed by `.` (e.g. `Dockerfile.release`).

### Highlight types and colors

| Type | Color | Used for |
|---|---|---|
| Comment | grey | Line/block comments |
| Keyword | yellow | Language keywords |
| Type | cyan | Type names, booleans, semver versions |
| String | green | String literals |
| Number | red | Numeric literals |
| Bracket | magenta | `()[]{}` |
| Operator | yellow | Multi-char operators like `&&`, `||`, `!=`, `=>` |

### Highlighting rules

16 languages have syntax highlighting rules: Rust, Python, Go, TypeScript, JavaScript, Shell, C, TOML, JSON, YAML, Makefile, HTML, CSS, Dockerfile, Markdown, INI/Config.

Each language defines: line comment prefix, block comment delimiters, string delimiters (some multiline), keywords, types, operators, and whether to highlight numbers.

The highlighter processes each line byte-by-byte, producing a highlight type per character. Multi-line state (block comments, multiline strings, fenced code blocks) carries across lines.

### Specialized highlighting

- **JSON**: keys (followed by `:`) are yellow; string values are green. `true`/`false`/`null` are cyan.
- **YAML**: keys (before unquoted `:`) are yellow; quoted values green; anchors `&name` and aliases `*name` cyan; booleans cyan. Comments respect quote nesting.
- **INI/Config**: section headers `[section]` as keywords; key=value with key highlighting; boolean values (true/false/yes/no/on/off) as types; inline comments after whitespace.
- **Markdown**: headers as keywords; fenced code block content as strings; inline code, bold (`**`), italic (`*`); blockquote prefix `>` as comments; list markers as numbers; HTML comments.

### Semver highlighting

A post-pass on every line detects version patterns like `v1.2.3` or `0.3.5-beta.1+build.42` and highlights them as cyan. Must have word boundaries on both sides. Skips text already marked as comments.

### Highlight caching

Per-line highlight state is cached and keyed by buffer version. The cache is fully recomputed when the buffer changes (version increments on every edit). During pure scrolling, the cache is reused with zero recomputation.

## 5. Bracket and quote matching

When the cursor is on a bracket `()[]{}`, the matching bracket is highlighted with a magenta background. Scanning stops after 1000 lines.

When the cursor is on a `"` or `'`, the matching quote on the same line is highlighted with a magenta background. Quotes are paired sequentially (first with second, third with fourth, etc.). Escaped quotes (preceded by odd number of backslashes) are skipped.

## 6. Selection

Selection is defined by an anchor and a cursor position. When they are equal, there is no selection. The anchor stays fixed while the cursor moves during selection extension.

- **Shift+Arrow keys**: extend selection in the corresponding direction.
- **Shift+Left/Right**: snap to 2-space indent stops in leading whitespace (same as regular left/right).
- **Ctrl+Shift+Left/Right**: extend selection by word.
- **Ctrl+Shift+Up**: select from cursor to start of file.
- **Ctrl+Shift+Down**: select from cursor to end of file.
- **Mouse drag**: extend selection to drag position.

Selection is rendered with reverse video. It overrides all other highlighting.

Pressing any non-extending movement key (arrows, Home, End, word movement) collapses the selection. Left collapses to the start, Right to the end. Esc clears the selection.

## 7. Movement

### Basic movement

- **Up/Down**: move cursor, preserving a "desired column" (sticky column) that persists across consecutive up/down presses. The desired column resets on any non-vertical key.
- **Left/Right**: move one character. In leading spaces, snap to 2-space indent stops (even columns). Wrap across line boundaries.
- **Home**: column 0.
- **End**: end of line.
- **PageUp/PageDown**: move by the number of visible text rows. Preserves desired column.
- **Ctrl+T**: go to start of file.
- **Ctrl+G**: go to end of file.

### Word movement

- **Ctrl+Left**: move to previous word boundary. Collapses selection. Wraps to end of previous line.
- **Ctrl+Right**: move to next word boundary. Collapses selection. Wraps to start of next line.

Word characters are ASCII alphanumeric plus underscore. Word boundary logic: backward skips non-word chars then word chars; forward skips word chars then non-word chars.

### Scroll

Mouse wheel scrolls by 3 screen rows (respecting soft-wrap). After scrolling, the cursor is clamped into the visible viewport.

## 8. Editing

### Character insertion

1. If selection active and the character has an auto-close pair: wrap the selection with the pair. E.g., selecting `foo` and typing `(` produces `(foo)` with `foo` still selected.
2. If selection active otherwise: delete the selection first.
3. **Skip-over**: if the character is a closing character (`)]}"'`) and the next character in the buffer matches, just advance the cursor without inserting.
4. **Auto-close**: if the character has an auto-close pair and the next character is a boundary (space, tab, closing char, or end of line), insert both characters and place the cursor between them.
5. Otherwise: insert normally.

Auto-close pairs: `(→)`, `[→]`, `{→}`, `"→"`, `'→'`.

### Tab

- If selection active: indent all selected lines (add tab or 2 spaces at start of non-blank lines).
- Otherwise: insert a tab for `.c`, `.h`, `.go`, `Makefile` files; 2 spaces for everything else.

### Enter

Delete selection if active. Copy leading whitespace (spaces and tabs) from the current line. Insert newline followed by the copied indent.

### Backspace

1. If selection active: delete selection.
2. If in leading whitespace, at an even column >= 2, and all preceding characters on the line are spaces: delete 2 spaces (smart dedent to previous indent stop).
3. If cursor is between an auto-close pair (the character before cursor closes to the character after cursor): delete both characters.
4. Otherwise: delete one character backward.
5. At column 0: join with previous line.

### Ctrl+Backspace

Delete one word backward. At column 0, join with previous line. Uses the same word boundary logic as Ctrl+Left.

### Forward delete

Delete one character forward. At end of line, join with next line.

### Kill line (Ctrl+K)

Delete the entire current line including its trailing newline.

### Duplicate line (Ctrl+J)

Insert a copy of the current line below it. Cursor moves to the same column on the new line.

### Shift+Tab (dedent)

Remove one level of indentation from the current line or all selected lines: removes a leading tab or 2 leading spaces.

### Indent/Dedent on selections

When indenting or dedenting a selection, if the selection ends at column 0 of a line, that line is excluded. Operations on multiple lines are grouped into a single undo step. Lines are processed in reverse order to keep byte offsets stable.

## 9. Comment toggle (Ctrl+D)

Detects language from filename. If no language detected, shows error in status bar.

Line range: the selection (excluding the last line if selection ends at column 0), or just the current line.

Toggle logic:
1. Check if all non-blank lines in the range start with `"{comment_prefix} "` (after their indent).
2. If all commented → uncomment (remove the prefix). If not all commented → comment.
3. When commenting: find the minimum indent across non-blank lines, insert `"{comment_prefix} "` at that indent position. Skip blank lines. Skip lines already commented (to avoid double-commenting).
4. When uncommenting: remove the first occurrence of `"{comment_prefix} "` from each line.

Also available as `comment on` / `comment off` commands to force one direction.

The entire operation is a single undo step.

## 10. Clipboard

### Platform detection

Priority order:
1. macOS: `pbcopy`/`pbpaste`
2. Linux/Wayland: `wl-copy`/`wl-paste` (if `WAYLAND_DISPLAY` is set)
3. Linux/X11: `xclip` (with `-selection clipboard`)
4. Linux/X11 fallback: `xsel` (with `--clipboard`)
5. Internal fallback: in-memory string

Copy always stores internally as well as to system clipboard. Paste reads from the system clipboard.

### Copy/Cut/Paste

- **Ctrl+C**: copy selection to clipboard (no-op if no selection).
- **Ctrl+X**: copy then delete selection (no-op if no selection).
- **Ctrl+V**: paste from clipboard. Deletes selection first if active.

### Smart paste (re-indentation)

For multi-line pastes (2+ lines):
1. Find the minimum indentation of non-empty lines in lines 2+ of the pasted text.
2. Target indent = cursor column if the first line has content, else the current line's indent.
3. Re-indent lines 2+ by stripping the minimum indent and prepending the target indent as spaces.

### Bracketed paste

The editor enables bracketed paste mode. When the terminal sends paste markers, all text between them is accumulated into a single paste event (rather than being processed as individual keystrokes). This ensures the entire paste is a single undo step and avoids auto-close/auto-indent on each character.

Newlines inside bracketed paste are preserved as literal characters. Backspace in paste is preserved as `\x7f`.

## 11. Undo/Redo

### Grouping heuristics

Individual operations (inserts and deletes) are automatically grouped for undo. A new group starts when any of these conditions are met:
1. The operation kind changes (insert after delete, or vice versa).
2. More than 1 second has elapsed since the last operation.
3. The cursor jumped (the cursor position before this operation doesn't match where the last operation left it).
4. A space or newline was inserted (word boundary).

### Explicit grouping

Operations can be explicitly forced into a single undo group (used by indent, dedent, comment toggle, and paste). `seal()` forces a boundary.

### Behavior

- **Ctrl+Z**: undo the last group. Cursor returns to position before the group.
- **Ctrl+Y**: redo the last undone group. Cursor moves to position after the group.
- Any new edit clears the redo stack.

### Persistence

Undo history is saved to `~/.config/e/undo.bin` on quit and after saves. All files share one binary database. Each entry is keyed by absolute file path and validated against the file's modification time. If the file was modified externally (mtime mismatch), the stored undo history is silently discarded.

On re-open, the undo and redo stacks are restored.

Concurrency: the database file is locked with `flock` during reads (shared) and writes (exclusive).

## 12. Find

### Opening find

**Ctrl+F** opens the find prompt. If there is a selection of 100 characters or fewer, it is prefilled into the prompt.

### Live search

As the user types, matches are highlighted immediately. The view jumps to the first match. Matches are found using regex with smart-case: if the pattern is all lowercase, the search is case-insensitive; otherwise case-sensitive. Invalid regex is silently ignored (no highlights).

### Find browse mode

After pressing Enter, the editor enters browse mode:
- **Up**: jump to previous match (wraps around).
- **Down**: jump to next match (wraps around).
- The status bar shows `"match X of Y"`.
- The current match is highlighted with a green background; other matches with yellow background.
- The cursor is hidden during browse mode.
- **Esc**: exit browse mode, select the current match (so copy/delete/etc. can act on it), clear all highlights.
- **Any other key**: exit browse mode, process the key normally.

### Canceling

Pressing Esc or Ctrl+Q in the find prompt cancels and clears all highlights.

## 13. Replace all

Available via the command palette: `replaceall <pattern> <replacement>`.

Uses the same smart-case regex as find. If a selection is active, replacement is confined to the selected range. Otherwise it operates on the whole file.

Reports the number of replacements. The operation is a single undo step.

## 14. Command palette

### Opening

**Ctrl+P** opens the command palette with prompt `"> "`.

### Tab completion

- Empty input + Tab: shows all command names.
- Partial input + Tab with one match: autocompletes.
- Partial input + Tab with multiple matches: shows all matching names and completes the common prefix.

### Available commands

| Command | Description |
|---|---|
| `save [filename]` | Save. With filename: save-as. |
| `quit` / `q` | Quit. |
| `goto <line>` | Jump to line number (1-indexed). Centers viewport. |
| `ruler` | Toggle line numbers. |
| `find <pattern>` | Find (same as Ctrl+F submit). |
| `replaceall <pattern> <replacement>` | Replace all matches. |
| `comment [on\|off]` | Toggle/force line comments. |
| `selectall` | Select all text. |
| `trim` | Strip trailing whitespace from all lines. |

### Argument parsing

Supports single-quoted (`'arg with spaces'`), double-quoted (`"arg"`), and unquoted tokens. No backslash escaping.

### Goto line (Ctrl+L)

Opens a prompt `"goto: "`. On submit, jumps to the line number (1-indexed, clamped to valid range) and centers the viewport.

## 15. Command buffer (mini-editor)

The command buffer is a modal single-line text input used for the command palette, find, goto, save-as prompts, and sudo password entry.

### Modes

- **Command**: command palette (Ctrl+P).
- **Find**: regex find (Ctrl+F).
- **Goto**: goto line (Ctrl+L).
- **Prompt**: save-as filename prompt.
- **SudoSave**: password prompt (input displayed as `*`).

### Key handling

- **Enter**: submit input.
- **Esc / Ctrl+Q**: cancel.
- **Tab**: request tab completion (command mode only).
- **Printable char**: insert at cursor.
- **Backspace**: delete before cursor.
- **Left/Right**: move cursor.
- **Up/Down**: navigate history (per-session).

### History

Non-empty inputs are saved to history on close. Up navigates backward through history, Down forward. Past the end clears the input.

### Paste in command buffer

Bracketed paste into the command buffer strips newlines and carriage returns, then inserts the cleaned text.

## 16. Save

### Normal save (Ctrl+S)

- If the file has a name: create parent directories if needed (`mkdir -p`). Write the file (with trailing whitespace stripping and newline ensuring). Mark as clean. Update cached modification time. Save undo history to disk.
- If the file has no name: open a save-as prompt.

### Sudo save

If writing or creating directories fails with permission denied:
1. Write cleaned content to a temp file `/tmp/e_sudo_{pid}`.
2. Open a password prompt (input masked with `*`).
3. On submit: pipe the password to `sudo -S mkdir -p` (if directories needed) and `sudo -S cp {tmp} {target}`.
4. Clean up the temp file.

### External modification detection

When the terminal sends a focus-in event, the editor stats the file and compares the modification time with its cached value. If different, it prompts `"{filename} changed on disk. Reload? (y/n)"`. Accepting re-reads the file, clamps the cursor, clears find state.

## 17. Quit

- **Ctrl+Q**: if the buffer is clean, saves undo history and cursor position, then exits.
- If dirty: shows `"Save changes to {name}? (y/n)"` in the status bar.
  - **y**: saves the file, then exits.
  - **n**: saves undo history and cursor position (but not the file), then exits.
  - **Any other key**: cancels the quit.

## 18. Mouse

### Click detection

Double-click and triple-click are detected by checking if clicks occur within 400ms at the same screen position. The click count cycles 1 → 2 → 3 → 1.

- **Single click**: place cursor, begin drag.
- **Double click**: select word (scan through word characters: alphanumeric + underscore).
- **Triple click**: select entire line (including trailing newline if not the last line).

### Drag

While dragging, the selection extends to the current mouse position.

### Scroll wheel

Scrolls by 3 screen rows per wheel event. Handles soft-wrapped lines correctly. After scrolling, the cursor is clamped into the visible viewport if it would be off-screen.

### Screen-to-buffer mapping

Clicks are mapped from screen coordinates to buffer positions by walking from the scroll position through wrapped lines, counting screen rows. Tab expansion (2 display columns per tab) is accounted for.

## 19. Persistence

All persistent data is stored under `~/.config/e/`.

| File | Content |
|---|---|
| `keybindings.ini` | User keybinding overrides |
| `undo.bin` | Undo history for all files (single binary database) |
| `cursor.bin` | Last cursor position per file |
| `buffers/<path>.elock` | File lock files |

### Undo history format

A single binary file stores entries for all files. Each entry contains the absolute file path, the file's modification time, and serialized undo/redo stacks. On save, entries for other files are preserved as-is (raw bytes, no deserialization). Entries whose files no longer exist or whose mtimes no longer match are pruned.

### Cursor position format

A single binary file stores the last cursor position (line, col) per absolute file path. No mtime validation — cursor is clamped on load. Entries for deleted files are pruned on write.

## 20. Keybindings

### Default bindings

| Key | Action |
|---|---|
| Ctrl+S | Save |
| Ctrl+Q | Quit |
| Ctrl+Z | Undo |
| Ctrl+Y | Redo |
| Ctrl+A | Select all |
| Ctrl+C | Copy |
| Ctrl+X | Cut |
| Ctrl+V | Paste |
| Ctrl+K | Kill line |
| Ctrl+T | Go to top |
| Ctrl+G | Go to end |
| Ctrl+R | Toggle ruler |
| Ctrl+P | Command palette |
| Ctrl+L | Goto line |
| Ctrl+F | Find |
| Ctrl+H | Delete word backward |
| Ctrl+D | Toggle comment |
| Ctrl+J | Duplicate line |
| Ctrl+W | Select word at cursor |

### Non-configurable bindings

Arrow keys, Shift+arrows, Ctrl+arrows, Ctrl+Shift+arrows, Home, End, PageUp, PageDown, Tab, Shift+Tab, Delete, Backspace, Enter, Esc, and printable character insertion.

### Configuration

Keybindings can be overridden via `~/.config/e/keybindings.ini`:
```ini
ctrl+s = save
ctrl+q = quit
```
Lines starting with `#` or `[` are ignored. Only `ctrl+<single_char>` is supported.

## 21. Terminal compatibility

### Escape sequences consumed

The editor recognizes these escape sequences as special keys (beyond what the terminal library provides):

- Ctrl+Shift+Up/Down (CSI 1;6 A/B)
- Ctrl+Left/Right (CSI 1;5 D/C and rxvt-style ESC O d/c for tmux compatibility)
- Ctrl+Shift+Left/Right (CSI 1;6 D/C)
- Ctrl+Backspace in CSI u encoding (for kitty, ghostty, etc.)
- Focus in/out events (CSI I)

### Modes enabled on entry

Mouse tracking (button events, button-motion for drag, SGR mode for large terminals), bracketed paste, and focus events. All disabled on exit in reverse order.

### SIGWINCH

Terminal resize is detected via a signal handler that sets an atomic flag. The flag is polled on the main loop's 500ms timeout. On resize, the viewport dimensions update and a full redraw is triggered.

## 22. Status messages

Status messages display on the command line row when the command buffer is not active. They expire after 3 seconds. Some messages (quit confirmation, reload prompt, find match count) do not auto-expire.

## 23. Select word (Ctrl+W)

Places the cursor at the start of the word and the anchor at the end of the word under the cursor. Word characters are ASCII alphanumeric plus underscore. No-op if the cursor is not on a word character.

## 24. Trim command

Strips trailing whitespace (spaces and tabs) from every line in the buffer. This is an in-buffer edit (undoable), unlike the automatic stripping that happens on file write.

## 25. Indent style

- 2 spaces for all files except `.c`, `.h`, `.go`, `Makefile` which use tabs.
- This applies to both Tab key insertion and selection indentation.
