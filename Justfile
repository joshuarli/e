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

setup:
  prek install --install-hooks

pc:
  prek run --all-files
