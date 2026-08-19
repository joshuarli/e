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

`Language::from_name` accepts the programming-language names from syntect's
default syntax set (including C++, Java, Ruby, SQL, Haskell, Lua, and the
embedded/template forms) plus common aliases such as `rs`, `py`, `js`, `ts`,
`sh`, `yml`, and `md`. These languages reuse a small set of static scanner rule
families; only the languages that need multiline or structural behavior have a
specialized lexer. Rule tables and lexer modes are private implementation
details; the crate intentionally does not expose a grammar DSL, themes, ANSI
colors, or editor abstractions.

`Language::from_filename`, `Language::from_shebang`, and `Language::detect` own
filename, shebang, and comment-delimiter policy for clients. Callers do not
need a second supported-language registry.

## Benchmarks

The checked-in golden corpus has warm (reused highlighter and scratch) and cold
(setup included) rustybench workloads:

```sh
cargo bench --bench hi_lite
```

The crate benchmark is intentionally weighted toward small, syntax-complete
snippets. The warm aggregate is the steady-state target; cold runs expose
allocation costs for callers that do not retain their scratch buffer.
