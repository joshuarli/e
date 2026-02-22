build-dev:
  cargo build

build:
  cargo build --release

setup:
  prek install --install-hooks

pc:
  prek run --all-files
