.PHONY: all check test lint cov tlc verus conformance integration functional run clean spec-sha manifest

all: check test

check:
	cargo check --workspace --all-targets

test:
	cargo test --workspace

lint:
	cargo clippy --all-targets -- -D warnings

cov:
	cargo llvm-cov --workspace --fail-under-lines 100 \
	  -p clock -p ids -p domain -p store -p service

tlc:
	bash scripts/run_tlc.sh

verus:
	@echo "Verus is best-effort; see README. Install verus and run 'verus crates/<crate>/src/lib.rs'"

integration:
	cargo test --test integration

functional:
	cargo test --test functional

conformance:
	cargo test --test conformance

run:
	cargo run -p server --release

spec-sha:
	bash scripts/check_spec_sha.sh

manifest:
	bash scripts/manifestcheck.sh

clean:
	cargo clean
