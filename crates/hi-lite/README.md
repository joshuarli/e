# hi-lite

`hi-lite` is a dependency-free, line-oriented syntax highlighter. Its public
boundary is bytes plus a language and opaque lexical state; presentation,
documents, caches, and terminal rendering remain owned by callers.

```rust
use hi_lite::{Highlighter, Kind, Language};

let mut highlighter = Highlighter::new(Language::Rust);
let mut kinds = Vec::new();
let result = highlighter.highlight_into(b"fn main() {}", &mut kinds);

assert_eq!(result[0], Kind::Keyword);
```

`Highlighter` retains state across lines and reuses the caller's `Vec<Kind>`.
For an editor cache, use `state()` and `set_state()` to resume from an opaque
checkpoint. `highlight_line_into` is the stateless, caller-sized-buffer form.

`runs` exposes contiguous byte ranges without changing the scanner's simple
per-byte representation. `byte_kinds_to_char_kinds_into` is available when a
renderer needs UTF-8 character or two-column tab entries. Bracket and quote
matching accept line callbacks rather than an editor buffer.

`Language::from_name` accepts canonical names and common aliases such as `rs`,
`py`, `js`, `ts`, `sh`, `yml`, and `md`. Rule tables and lexer modes are private
implementation details; the crate intentionally does not expose a grammar DSL,
themes, ANSI colors, or editor abstractions.
