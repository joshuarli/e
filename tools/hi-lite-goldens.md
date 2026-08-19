# hi-lite syntax goldens

`crates/hi-lite/tests/fixtures/*.snippet` contains small, syntax-complete
examples for every built-in language except XSH. Their `.golden` files are
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
