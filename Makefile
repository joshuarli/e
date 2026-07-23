NAME       := e
TARGET     := $(shell rustc -vV | awk '/^host:/ {print $$2}')
LLVM_BIN   := $(shell rustc --print sysroot)/lib/rustlib/$(TARGET)/bin
PGO_DIR    := $(CURDIR)/target/pgo-profiles
PGO_MERGED := $(PGO_DIR)/merged.profdata

.PHONY: build release release-pgo pgo-profile bench-pgo install test test-ci record gifs

build:
	cargo build

test:
	cargo test --quiet

test-ci:
	cargo test --quiet --release

release:
	cargo clean -p $(NAME) --release --target $(TARGET)
	RUSTFLAGS="-Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort" \
	cargo build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)

lint:
	cargo fmt --all
	cargo clippy --fix --allow-dirty --all-targets --all-features -- --deny warnings

# Collect PGO profiles from benchmarks — only re-run when hot paths change.
# No build-std or -Cpanic=immediate-abort here: the profiler runtime needs unwinding.
pgo-profile:
	rm -rf $(PGO_DIR) && mkdir -p $(PGO_DIR)
	RUSTFLAGS="-Cprofile-generate=$(PGO_DIR)" \
	cargo bench --bench bench -- --profile-time 1 "highlight|search|document|render|viewport"
	$(LLVM_BIN)/llvm-profdata merge -o $(PGO_MERGED) $(PGO_DIR)

# PGO-optimized release: uses gathered profiles + all aggressive flags.
release-pgo: $(PGO_MERGED)
	cargo clean -p $(NAME) --release --target $(TARGET)
	RUSTFLAGS="-Cprofile-use=$(PGO_MERGED) -Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort" \
	cargo build --release \
	  -Z build-std=std \
	  -Z build-std-features= \
	  --target $(TARGET)

# Benchmark regular release vs PGO. Requires: critcmp (cargo install critcmp)
bench-pgo: $(PGO_MERGED)
	cargo bench --bench bench -- --save-baseline regular 2>/dev/null
	RUSTFLAGS="-Cprofile-use=$(PGO_MERGED)" \
	cargo bench --bench bench -- --save-baseline pgo 2>/dev/null
	critcmp regular pgo

$(PGO_MERGED):
	$(MAKE) pgo-profile

install: release-pgo
	cp target/$(TARGET)/release/$(NAME) ~/usr/bin/$(NAME)
	codesign -fs - ~/usr/bin/$(NAME)

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

