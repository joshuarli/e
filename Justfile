name    := "e"
target  := arch() + "-apple-darwin"
nightly := "nightly-2026-02-23"

setup:
  rustup toolchain install {{ nightly }}
  rustup component add rust-src --toolchain {{ nightly }}
  prek install --install-hooks

build-dev:
  cargo build

release:
    cargo clean -p {{ name }} --release --target {{ target }}
    RUSTFLAGS="-Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort" \
    cargo +{{ nightly }} build --release \
      -Z build-std=std \
      -Z build-std-features= \
      --target {{ target }}

install: release
  sudo cp target/{{target}}/release/{{ name }} /usr/local/bin/{{ name }}

test *args:
  cargo test -- --test-threads=4

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

pc:
  prek run --all-files
