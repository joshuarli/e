# PTY E2E harness

`tests/e2e/harness.rs` is a thin editor-specific adapter over `ptytest`. It
retains editor keys, fixtures, recording, and cell/style assertions; PTY spawn,
nonblocking I/O, terminal parsing, and cleanup belong to the shared crate.

Run the suite with `cargo test --test e2e`. It uses a validated `C.UTF-8`
hermetic environment and the audited `xterm-minimal-v1` output profile. Use
named screen/state barriers for redraws rather than adding a delay.

The adapter captures the initial terminal lifecycle state and verifies its
restoration after each observed normal editor exit. Forced cleanup is only a
safety net and is not treated as a restoration assertion.

Crate-owned failures write bundles under `target/ptytest-failures/`. A future
semantic snapshot belongs next to its E2E scenario and is updated explicitly
with `PTYTEST_UPDATE_SNAPSHOTS=1`.
