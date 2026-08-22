NAME       := e
RUSTYBENCH  ?= cargo run --quiet --manifest-path ../rustybench/Cargo.toml --
TEST_BINARY_ENV := E_TEST_BINARY
TEST_TARGETS := --tests
HOST       := $(shell rustc -vV | awk '/^host:/ {print $$2}')
TARGET     ?= $(subst -unknown-linux-gnu,-unknown-linux-musl,$(HOST))
TARGET_ENV := $(shell printf '%s' "$(TARGET)" | tr '[:lower:]-' '[:upper:]_')
CARGO_CMD  := $(if $(findstring -linux-musl,$(TARGET)),$(shell command -v musl-cargo 2>/dev/null || printf cargo),cargo)
# Docker's musl-cargo wrapper owns linker, CRT, and loader flags. The Makefile
# selects only the build mode and keeps Cargo invocations readable on macOS as
# well as in the Linux release image.
cargo = $(if $(findstring -linux-musl,$(TARGET)),,$(if $(filter release-static release-dynamic test,$(1)),CARGO_TARGET_$(TARGET_ENV)_LINKER=clang ,)$(if $(filter release-static release-dynamic,$(1)),RUSTFLAGS="-Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort")) MUSL_TARGET="$(TARGET)" MUSL_BUILD_MODE="$(1)" $(CARGO_CMD)

.PHONY: build test test-ci release release-dynamic verify-release verify-release-dynamic bench bench-hi-lite bench-syscalls install record gifs gen-xsh

# Regenerate the XSH vocabulary region of ../hi-lite/src/languages/xsh.rs from the
# registry (XSH_REPO overrides the path)
gen-xsh:
	cargo run --quiet --example gen_xsh

build:
	$(call cargo,dev) build

test:
	$(call cargo,test) test --quiet

test-ci:
	@test -x "target/$(TARGET)/release/$(NAME)"
	$(TEST_BINARY_ENV)="$(CURDIR)/target/$(TARGET)/release/$(NAME)" \
	$(call cargo,test) test --quiet --release $(TEST_TARGETS)

release:
	$(call cargo,release-static) clean -p $(NAME) --release --target $(TARGET)
	$(call cargo,release-static) build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)

verify-release:
	@test -f "target/$(TARGET)/release/$(NAME)"
	@if echo "$(TARGET)" | grep -q -- '-linux-musl$$'; then \
		command -v readelf >/dev/null || { echo 'readelf is required for release verification'; exit 1; }; \
		file "target/$(TARGET)/release/$(NAME)" | grep -Eq 'static-pie linked|statically linked' || { echo 'release is not statically linked'; exit 1; }; \
		file "target/$(TARGET)/release/$(NAME)" | grep -q 'stripped' || { echo 'release is not stripped'; exit 1; }; \
		! readelf -l "target/$(TARGET)/release/$(NAME)" | grep -q INTERP || { echo 'release has a dynamic ELF interpreter'; exit 1; }; \
		! readelf -d "target/$(TARGET)/release/$(NAME)" | grep -q NEEDED || { echo 'release has dynamic dependencies'; exit 1; }; \
	else \
		echo "Skipping ELF checks for $(TARGET)"; \
	fi

verify-release-dynamic:
	@test -f "target/$(TARGET)/release/$(NAME)"
	@if echo "$(TARGET)" | grep -q -- '-linux-musl$$'; then \
		command -v readelf >/dev/null || { echo 'readelf is required for release verification'; exit 1; }; \
		file "target/$(TARGET)/release/$(NAME)" | grep -q 'dynamically linked' || { echo 'release is not dynamically linked'; exit 1; }; \
		file "target/$(TARGET)/release/$(NAME)" | grep -q 'stripped' || { echo 'release is not stripped'; exit 1; }; \
		readelf -l "target/$(TARGET)/release/$(NAME)" | grep -q '/lib/ld-musl-' || { echo 'release does not use the musl loader'; exit 1; }; \
		readelf -d "target/$(TARGET)/release/$(NAME)" | grep -q NEEDED || { echo 'release has no dynamic dependencies'; exit 1; }; \
	else \
		echo "Skipping ELF checks for $(TARGET)"; \
	fi

lint:
	cargo fmt --all
	cargo clippy --fix --allow-dirty --all-targets --all-features -- --deny warnings

bench:
	@$(RUSTYBENCH) baseline --root "$(CURDIR)" --baseline "$(CURDIR)/benches/baseline.json" -- cargo bench --bench bench

# Benchmark hi-lite independently across the complete checked-in golden corpus.
bench-hi-lite:
	@$(RUSTYBENCH) baseline --root "$(CURDIR)" --baseline "$(CURDIR)/../hi-lite/benches/hi-lite-baseline.json" -- cargo bench --manifest-path "$(CURDIR)/../hi-lite/Cargo.toml" --bench hi_lite

bench-syscalls:
	@$(RUSTYBENCH) syscalls --root "$(CURDIR)"

release-dynamic:
	$(call cargo,release-dynamic) clean -p $(NAME) --release --target $(TARGET)
	$(call cargo,release-dynamic) build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)

install: release
	cp target/$(TARGET)/release/$(NAME) ~/usr/bin/$(NAME)
	@if test "$$(uname -s)" = Darwin; then \
		codesign -fs - ~/usr/bin/$(NAME); \
	fi

# Record e2e tests as asciicast .cast files (single-threaded for clean capture)
record:
	rm -rf tests/e2e/recordings/*.cast tests/e2e/recordings/*.gif
	E2E_RECORD=1 cargo test --test e2e -- --test-threads=1

# Convert recorded .cast files to animated GIFs (requires: cargo install --git https://github.com/asciinema/agg)
gifs:
	@for f in tests/e2e/recordings/*.cast; do \
	  agg "$$f" "$${f%.cast}.gif" 2>/dev/null; \
	done
	@echo "$$(ls tests/e2e/recordings/*.gif | wc -l | tr -d ' ') GIFs ??? tests/e2e/recordings/"
