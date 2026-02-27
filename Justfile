target := `rustc -vV | grep host | awk '{print $2}'`

build-dev:
  cargo build

# Optimized release build (~313KB). Requires: rustup toolchain install nightly && rustup component add rust-src --toolchain nightly
# Rebuilds std with LTO so the linker can tree-shake within it, and uses panic=immediate-abort
# to strip all panic/backtrace/formatting machinery.
release:
  RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort" \
  cargo +nightly build --release \
    -Z build-std=std \
    -Z build-std-features= \
    --target {{target}}
  @ls -lh target/{{target}}/release/e

install: release
  cp target/{{target}}/release/e /usr/local/bin/e

test:
  cargo test --test e2e -- --test-threads=4

# Record e2e tests as asciicast .cast files (single-threaded for clean capture)
record:
  rm -rf tests/e2e/recordings/*.cast tests/e2e/recordings/*.gif
  E2E_RECORD=1 cargo test --test e2e -- --test-threads=1

# Convert recorded .cast files to animated GIFs (requires: cargo install --git https://github.com/asciinema/agg)
gifs:
  @for f in tests/e2e/recordings/*.cast; do \
    agg "$f" "${f%.cast}.gif" 2>/dev/null; \
  done
  @echo "$(ls tests/e2e/recordings/*.gif | wc -l | tr -d ' ') GIFs → tests/e2e/recordings/"

setup:
  prek install --install-hooks

pc:
  prek run --all-files
