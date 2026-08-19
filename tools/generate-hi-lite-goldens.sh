#!/bin/sh
set -eu

# Syntect is intentionally a generator-only dependency. This script creates a
# checked-in tool project, and leaves only golden text files in the repository.
# The normal test suite never invokes Cargo with syntect and therefore remains
# dependency-free.

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
manifest="$root/tools/hi-lite-syntect/Cargo.toml"
fixtures="$root/crates/hi-lite/tests/fixtures"
for entry in \
  "bash:bash" "c:c" "css:css" "go:go" \
  "html:html" "javascript:js" "json:json" "makefile:makefile" \
  "markdown:md" "python:py" "rust:rs" "typescript:js" "yaml:yml"
do
  name=${entry%%:*}
  token=${entry#*:}
  cargo run --quiet --manifest-path "$manifest" -- \
    "$token" "$fixtures/$name.snippet" "$fixtures/$name.golden"
done
