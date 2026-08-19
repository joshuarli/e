# hi-lite syntax goldens

`crates/hi-lite/tests/fixtures/*.snippet` contains small, syntax-complete
examples for the specialized and representative popular languages (XSH is
intentionally excluded). Their `.golden` files are
normalized semantic runs, not ANSI output. The normalization maps syntect
scope stacks to hi-lite's eleven `Kind` values and intentionally keeps a string
parent dominant over template interpolation.

To regenerate the fixtures covered by syntect's default syntax bundle, run:

```sh
tools/generate-hi-lite-goldens.sh
```

The reusable oracle lives in `tools/hi-lite-syntect/`; it is deliberately a
standalone Cargo project and is not a workspace member. Syntect's default
bundle does not ship Dockerfile, INI, or TOML grammars, so those three checked-in
goldens are maintained as explicit semantic fixtures and are not overwritten by
the generator. Syntect is therefore not added to either application manifest,
and ordinary `cargo test -p hi-lite` uses only the checked-in goldens.

The public `hi_lite::Language` registry covers the programming-language portion
of that same default bundle. Languages with equivalent lexical structure share
static rule families in `crates/hi-lite/src/languages/generic.rs`; embedded
grammars resolve to their host language. XSH remains intentionally separate.

The same corpus is benchmarked independently with rustybench:

```sh
make bench-hi-lite
```

The benchmark target lives in `crates/hi-lite/benches/hi_lite.rs` and can also be
run directly with `cargo bench --manifest-path crates/hi-lite/Cargo.toml --bench hi_lite`.

`hi_lite_highlight_all_goldens_warm` reports steady-state throughput with
reused scratch storage; the `_cold` variant includes per-fixture scratch setup.
The per-language entries (and their `_cold` counterparts) make slower lexers and
allocation-heavy fixtures easy to attribute.
