NAME       := e
RUSTYBENCH  ?= cargo run --quiet --manifest-path ../rustybench/Cargo.toml --
TEST_BINARY_ENV := E_TEST_BINARY
TEST_TARGETS := --tests
HOST       := $(shell rustc -vV | awk '/^host:/ {print $$2}')
TARGET     ?= $(subst -unknown-linux-gnu,-unknown-linux-musl,$(HOST))
MUSL_LOADER := $(if $(findstring x86_64,$(TARGET)),/lib/ld-musl-x86_64.so.1,/lib/ld-musl-aarch64.so.1)
MUSL_NATIVE_RUSTFLAGS := $(if $(findstring -linux-musl,$(TARGET)),-L native=/usr/lib)
TARGET_ENV := $(shell echo $(TARGET) | tr '[:lower:]-' '[:upper:]_')
# Fedora's musl cross packages use /usr/<arch>-linux-musl/lib64, while the
# e-crt layout is used by other toolchains. Keep this overridable for hosts
# with a different musl sysroot layout.
MUSL_CRT_DIR ?= $(shell for dir in \
	/usr/lib/e-crt/$(TARGET) \
	/usr/$(subst -unknown,,$(TARGET))/lib64 \
	/usr/$(subst -unknown,,$(TARGET))/lib; do \
	if test -f "$$dir/crt1.o"; then printf '%s' "$$dir"; break; fi; \
done)
# LLVM helper binaries run on the build host; they are not installed in the
# target triple's rustlib directory when cross-compiling to musl.
LLVM_BIN   := $(shell rustc --print sysroot)/lib/rustlib/$(HOST)/bin
MUSL_LINKER := $(LLVM_BIN)/rust-lld
MUSL_TARGET_LIBDIR := $(shell rustc --print target-libdir --target $(TARGET))
PGO_BUILD_DIR := $(CURDIR)/target/pgo-build
PGO_DRIVER_DIR := $(CURDIR)/target/pgo-driver/$(TARGET)
PGO_DIR    := $(CURDIR)/target/pgo-profiles/$(TARGET)
PGO_MERGED := $(PGO_DIR)/merged.profdata
PGO_BINARY := $(PGO_BUILD_DIR)/$(TARGET)/release/$(NAME)
RELEASE_RUSTFLAGS := $(MUSL_NATIVE_RUSTFLAGS) -Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort
LINUX_DYNAMIC_RUSTFLAGS := $(RELEASE_RUSTFLAGS) -Ctarget-feature=-crt-static -Clink-arg=-B$(MUSL_CRT_DIR) -Clink-arg=-dynamic-linker=$(MUSL_LOADER)
PGO_USE_FLAGS := -Cprofile-use=$(PGO_MERGED) -Cllvm-args=-pgo-warn-missing-function

.PHONY: build test test-ci release verify-release verify-release-dynamic bench bench-syscalls release-pgo release-pgo-linux release-pgo-linux-static pgo-instrument pgo-instrument-linux pgo-profile pgo-profile-linux pgo-merge bench-pgo install record gifs ensure-musl-target

build:
	cargo build

test:
	cargo test --quiet

test-ci:
	@test -x "target/$(TARGET)/release/$(NAME)"
	$(TEST_BINARY_ENV)="$(CURDIR)/target/$(TARGET)/release/$(NAME)" \
	RUSTFLAGS="$(MUSL_NATIVE_RUSTFLAGS)" cargo test --quiet --release $(TEST_TARGETS)

release: ensure-musl-target
	cargo clean -p $(NAME) --release --target $(TARGET)
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	RUSTFLAGS="$(MUSL_NATIVE_RUSTFLAGS) -Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort" \
	cargo build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)

verify-release:
	@test -f "target/$(TARGET)/release/$(NAME)"
	@if command -v otool >/dev/null 2>&1 && otool -l "target/$(TARGET)/release/$(NAME)" 2>/dev/null | grep -q '__llvm_prf'; then \
		echo 'release still contains PGO profile sections; rebuild with make release-pgo' >&2; \
		exit 1; \
	fi
	@if strings "target/$(TARGET)/release/$(NAME)" 2>/dev/null | grep -q 'LLVM Profile'; then \
		echo 'release still contains the LLVM profile runtime; profile use must be limited to the application crate' >&2; \
		exit 1; \
	fi
	@if echo "$(TARGET)" | grep -q -- '-linux-musl$$'; then \
		command -v readelf >/dev/null || { echo 'readelf is required for release verification'; exit 1; }; \
		file "target/$(TARGET)/release/$(NAME)" | grep -Eq 'static-pie linked|statically linked' || { echo 'release is not statically linked'; exit 1; }; \
		file "target/$(TARGET)/release/$(NAME)" | grep -q 'stripped' || { echo 'release is not stripped'; exit 1; }; \
		! readelf -l "target/$(TARGET)/release/$(NAME)" | grep -q INTERP || { echo 'release has a dynamic ELF interpreter'; exit 1; }; \
		! readelf -d "target/$(TARGET)/release/$(NAME)" | grep -q NEEDED || { echo 'release has dynamic dependencies'; exit 1; }; \
		! readelf -S "target/$(TARGET)/release/$(NAME)" | grep -q llvm_prf || { echo 'release still contains PGO profile sections'; exit 1; }; \
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
		! readelf -S "target/$(TARGET)/release/$(NAME)" | grep -q llvm_prf || { echo 'release still contains PGO profile sections'; exit 1; }; \
	else \
		echo "Skipping ELF checks for $(TARGET)"; \
	fi

lint:
	cargo fmt --all
	cargo clippy --fix --allow-dirty --all-targets --all-features -- --deny warnings

bench:
	@$(RUSTYBENCH) baseline --root "$(CURDIR)" --baseline "$(CURDIR)/benches/baseline.json" -- cargo bench --bench bench

bench-syscalls:
	@$(RUSTYBENCH) syscalls --root "$(CURDIR)"

# Build the release-shaped application used by the PTY profile driver. The
# target-scoped flags keep host build scripts, proc macros, and the profile
# driver itself out of the instrumented set. The library and binary are built
# separately because the application's hot paths live in the library crate.
pgo-instrument: ensure-musl-target
	rm -rf "$(PGO_BUILD_DIR)/$(TARGET)" "$(PGO_DIR)"
	mkdir -p "$(PGO_DIR)"
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(RELEASE_RUSTFLAGS)" \
	CARGO_TARGET_DIR="$(PGO_BUILD_DIR)" \
	cargo rustc --release --target $(TARGET) --lib \
	  -Z build-std=std \
	  -Z build-std-features= \
	  -- -Cprofile-generate="$(PGO_DIR)"
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(RELEASE_RUSTFLAGS)" \
	CARGO_TARGET_DIR="$(PGO_BUILD_DIR)" \
	cargo rustc --release --target $(TARGET) --bin $(NAME) \
	  -Z build-std=std \
	  -Z build-std-features= \
	  -- -Cprofile-generate="$(PGO_DIR)"
	@test -x "$(PGO_BINARY)" || { echo "instrumented binary was not built: $(PGO_BINARY)" >&2; exit 1; }
	@if command -v otool >/dev/null 2>&1 && ! otool -l "$(PGO_BINARY)" 2>/dev/null | grep -q '__llvm_prf'; then \
		echo 'instrumented binary has no LLVM profile sections' >&2; exit 1; \
	fi
	@if echo "$(TARGET)" | grep -q -- '-linux-musl$$'; then \
		readelf -S "$(PGO_BINARY)" | grep -q 'llvm_prf' || { echo 'instrumented binary has no LLVM profile sections' >&2; exit 1; }; \
	fi

# The Linux container supplies the matching target architecture, libc, linker,
# and LLVM tools. Keep its dynamic profile shape aligned with release-pgo-linux.
pgo-instrument-linux: ensure-musl-target
	rm -rf "$(PGO_BUILD_DIR)/$(TARGET)" "$(PGO_DIR)"
	mkdir -p "$(PGO_DIR)"
	CARGO_TARGET_$(TARGET_ENV)_LINKER=clang \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(LINUX_DYNAMIC_RUSTFLAGS)" \
	CARGO_TARGET_DIR="$(PGO_BUILD_DIR)" \
	cargo rustc --release --target $(TARGET) --lib \
	  -Z build-std=std \
	  -Z build-std-features= \
	  -- -Cprofile-generate="$(PGO_DIR)"
	CARGO_TARGET_$(TARGET_ENV)_LINKER=clang \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(LINUX_DYNAMIC_RUSTFLAGS)" \
	CARGO_TARGET_DIR="$(PGO_BUILD_DIR)" \
	cargo rustc --release --target $(TARGET) --bin $(NAME) \
	  -Z build-std=std \
	  -Z build-std-features= \
	  -- -Cprofile-generate="$(PGO_DIR)"
	@test -x "$(PGO_BINARY)" || { echo "instrumented binary was not built: $(PGO_BINARY)" >&2; exit 1; }
	@if command -v otool >/dev/null 2>&1 && ! otool -l "$(PGO_BINARY)" 2>/dev/null | grep -q '__llvm_prf'; then \
		echo 'instrumented binary has no LLVM profile sections' >&2; exit 1; \
	fi
	@if echo "$(TARGET)" | grep -q -- '-linux-musl$$'; then \
		readelf -S "$(PGO_BINARY)" | grep -q 'llvm_prf' || { echo 'instrumented binary has no LLVM profile sections' >&2; exit 1; }; \
	fi

# Run only the ignored PTY workload. Cargo builds this driver in its own target
# directory with empty profile flags; LLVM_PROFILE_FILE is set on the child
# application by tests/e2e/harness.rs, never on the driver process.
pgo-profile: pgo-instrument
	rm -f "$(PGO_DIR)"/*.profraw
	RUSTFLAGS= \
	CARGO_ENCODED_RUSTFLAGS= \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS= \
	CARGO_TARGET_DIR="$(PGO_DRIVER_DIR)" \
	E_PGO_BINARY="$(PGO_BINARY)" \
	E_PGO_PROFILE_DIR="$(PGO_DIR)" \
	cargo test --test e2e -- \
	  pgo::profile_editor_startup_find_edit --ignored --exact --test-threads=1
	$(MAKE) pgo-merge TARGET="$(TARGET)"

pgo-profile-linux: pgo-instrument-linux
	rm -f "$(PGO_DIR)"/*.profraw
	RUSTFLAGS= \
	CARGO_ENCODED_RUSTFLAGS= \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS= \
	CARGO_TARGET_DIR="$(PGO_DRIVER_DIR)" \
	E_PGO_BINARY="$(PGO_BINARY)" \
	E_PGO_PROFILE_DIR="$(PGO_DIR)" \
	cargo test --test e2e -- \
	  pgo::profile_editor_startup_find_edit --ignored --exact --test-threads=1
	$(MAKE) pgo-merge TARGET="$(TARGET)"

# Merge only raw profiles emitted by the instrumented child process and reject
# obvious benchmark/tooling provenance before a profile can reach release use.
pgo-merge:
	@test -n "$$(find "$(PGO_DIR)" -maxdepth 1 -type f -name '*.profraw' -print -quit)" || { echo "no application raw profiles found in $(PGO_DIR)" >&2; exit 1; }
	$(LLVM_BIN)/llvm-profdata merge -o "$(PGO_MERGED)" "$(PGO_DIR)"/*.profraw
	@echo "Merged application profile: $(PGO_MERGED)"
	@$(LLVM_BIN)/llvm-profdata show --counts --all-functions "$(PGO_MERGED)" | sed -n '1,24p'
	@if $(LLVM_BIN)/llvm-profdata show --all-functions "$(PGO_MERGED)" | grep -Eiq 'rustybench|benchmark'; then \
		echo 'profile contains benchmark/tooling symbols; refusing to use it' >&2; \
		exit 1; \
	fi

# PGO-optimized release: build dependencies and build-std without profile
# runtime support, then apply the profile only to the application crates.
release-pgo: ensure-musl-target pgo-profile
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(RELEASE_RUSTFLAGS)" \
	cargo build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(RELEASE_RUSTFLAGS)" \
	cargo rustc --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET) --lib -- \
	  $(PGO_USE_FLAGS)
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(RELEASE_RUSTFLAGS)" \
	cargo rustc --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET) --bin $(NAME) -- \
	  $(PGO_USE_FLAGS)

release-pgo-linux: pgo-profile-linux
	CARGO_TARGET_$(TARGET_ENV)_LINKER=clang \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(LINUX_DYNAMIC_RUSTFLAGS)" \
	cargo build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)
	CARGO_TARGET_$(TARGET_ENV)_LINKER=clang \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(LINUX_DYNAMIC_RUSTFLAGS)" \
	cargo rustc --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET) --lib -- \
	  $(PGO_USE_FLAGS)
	CARGO_TARGET_$(TARGET_ENV)_LINKER=clang \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(LINUX_DYNAMIC_RUSTFLAGS)" \
	cargo rustc --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET) --bin $(NAME) -- \
	  $(PGO_USE_FLAGS)

release-pgo-linux-static: ensure-musl-target pgo-profile
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(RELEASE_RUSTFLAGS)" \
	cargo build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(RELEASE_RUSTFLAGS)" \
	cargo rustc --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET) --lib -- \
	  $(PGO_USE_FLAGS)
	CARGO_TARGET_$(TARGET_ENV)_LINKER="$(MUSL_LINKER)" \
	CARGO_TARGET_$(TARGET_ENV)_RUSTFLAGS="$(RELEASE_RUSTFLAGS)" \
	cargo rustc --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET) --bin $(NAME) -- \
	  $(PGO_USE_FLAGS)

# Benchmark regular release vs PGO and compare persisted baselines.
bench-pgo: pgo-profile
	@BASELINE="$(CURDIR)/benches/baseline.json"; \
	PGO_BASELINE="$(CURDIR)/benches/pgo-baseline.json"; \
	$(RUSTYBENCH) baseline --root "$(CURDIR)" --baseline "$$BASELINE" --quiet -- cargo bench --bench bench; \
	RUSTYBENCH_BENCH_RUSTFLAGS="-Cprofile-use=$(PGO_MERGED)" $(RUSTYBENCH) baseline --root "$(CURDIR)" --baseline "$$PGO_BASELINE" --quiet -- cargo bench --bench bench; \
	$(RUSTYBENCH) diff "$$BASELINE" "$$PGO_BASELINE"

install: release-pgo
	cp target/$(TARGET)/release/$(NAME) ~/usr/bin/$(NAME)
	@if test "$$(uname -s)" = Darwin; then \
		codesign -fs - ~/usr/bin/$(NAME); \
	fi

ensure-musl-target:
	@if ! echo "$(TARGET)" | grep -q -- '-linux-musl$$'; then exit 0; fi; \
	if test -f "$(MUSL_TARGET_LIBDIR)/self-contained/libunwind.a"; then \
		exit 0; \
	fi; \
	if command -v rustup >/dev/null 2>&1; then \
		echo "Installing Rust target $(TARGET)"; \
		rustup target add "$(TARGET)"; \
	else \
		echo "Rust target $(TARGET) is missing and rustup is unavailable" >&2; \
		exit 1; \
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
