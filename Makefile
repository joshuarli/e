NAME   := e
UNAME  := $(shell uname -m)
TARGET := $(UNAME)-apple-darwin

.PHONY: setup build-dev release install test test-ci record gifs pc

setup:
	rustup show active-toolchain
	prek install --install-hooks

build-dev:
	cargo build

release:
	cargo clean -p $(NAME) --release --target $(TARGET)
	RUSTFLAGS="-Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort" \
	cargo build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)

install: release
	sudo cp target/$(TARGET)/release/$(NAME) /usr/local/bin/$(NAME)

test:
	cargo test -- --test-threads=4

# So we don't do duplicate work (building both debug and release) in CI.
test-ci:
	cargo test --release -- --test-threads=4

# Record e2e tests as asciicast .cast files (single-threaded for clean capture)
record:
	rm -rf tests/e2e/recordings/*.cast tests/e2e/recordings/*.gif
	E2E_RECORD=1 cargo test --test e2e -- --test-threads=1

# Convert recorded .cast files to animated GIFs (requires: cargo install --git https://github.com/asciinema/agg)
gifs:
	@for f in tests/e2e/recordings/*.cast; do \
	  agg "$$f" "$${f%.cast}.gif" 2>/dev/null; \
	done
	@echo "$$(ls tests/e2e/recordings/*.gif | wc -l | tr -d ' ') GIFs → tests/e2e/recordings/"

pc:
	prek run --all-files
