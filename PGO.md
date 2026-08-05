# Profile-guided release builds

The Divan suite in `benches/bench.rs` remains a measurement suite. It covers file I/O,
document mutations, syntax rendering, find, viewport movement, command traces, and large-file
rendering with benchmark-owned fixtures and an allocation profiler. `make bench` and the
baseline scripts continue to measure those operations independently.

Previously, `make pgo-profile` put an unscoped `-Cprofile-generate` in `RUSTFLAGS` and ran the
Divan binary with `--sample-size 1`. That could profile benchmark setup, Divan, the benchmark
allocator, and equally weighted synthetic fixtures alongside editor code. It also did not cross
the application process boundary.

## Workload and assumptions

The initial proof of concept is the ignored test
`pgo::profile_editor_startup_find_edit` in `tests/e2e/pgo.rs`. It launches the release-shaped
application through the existing Unix PTY harness, waits for non-empty status-bar content,
opens a deterministic 3,000-line Rust fixture, performs incremental live find and match
navigation, scrolls, edits, and undo/redo, then exits cleanly.

The fixture size, 120×40 terminal, search distribution, and interaction order are engineering
assumptions chosen from the existing render/find benchmarks. They are not user-validated weights.
Cold versus warm state, multiple corpus sizes, terminal matrices, remote-like environments, and
user-curated scenario weighting are deliberately deferred follow-up work.

## Commands

For a host-shaped target such as macOS:

```text
make pgo-instrument TARGET=<target>
make pgo-profile TARGET=<target>
make pgo-merge TARGET=<target>
make release-pgo TARGET=<target>
```

`pgo-profile` already invokes `pgo-merge`; the separate merge target is useful when inspecting or
re-merging raw application profiles. Linux uses the matching container and architecture-aware
LLVM toolchain from `Dockerfile`:

```text
make pgo-profile-linux TARGET=<target>
make release-pgo-linux TARGET=<target>
make release-pgo-linux-static TARGET=<target>
```

The instrumented binary is written under `target/pgo-build/<target>/release/e`, raw profiles and
the merged profile under `target/pgo-profiles/<target>`, and the uninstrumented PTY driver under
`target/pgo-driver/<target>`. The profile driver requires `E_PGO_BINARY` and
`E_PGO_PROFILE_DIR`; it cannot silently fall back to a debug or normal binary.

The application library and binary receive instrumentation through separate `cargo rustc`
invocations. Target-scoped Cargo flags keep host build scripts, proc macros, tests, and benchmark
targets outside that boundary. Profile use is likewise passed only to the application library
and binary, so the final release does not retain the profile runtime. `pgo-merge` prints the
merged profile's first functions and rejects obvious `divan` or benchmark symbols.

## Validation status

The same ignored PTY scenario is the intended baseline-versus-PGO comparison driver. This
refactor does not claim a performance win: measurements still need repeated baseline and PGO
runs on the release target, with startup/readiness and interaction latency reported separately
from Divan results. Profile provenance, missing-function warnings from LLVM, binary section
checks, and the scenario's coverage gaps should be reviewed after each target build.
