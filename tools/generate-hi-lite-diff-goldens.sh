#!/bin/sh
set -eu

# Diff goldens are generated from hi-lite's dependency-free port of fx's
# semantic `compute` and `formatUnified` rules. The terminal renderer is not
# involved; checked-in files remain renderer-neutral semantic rows.
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cargo run --quiet --manifest-path "$root/crates/hi-lite/Cargo.toml" \
  --example generate_diff_goldens
