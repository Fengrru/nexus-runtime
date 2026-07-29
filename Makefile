# Nexus Runtime — Unified Build Commands
#
# Usage:
#   make build        Build CLI binary (release)
#   make test         Run all tests (unit + property + doc + Phoenix)
#   make test-unit    Run unit tests only
#   make test-phoenix Run Phoenix acceptance tests
#   make lint         Format check + clippy + doc check
#   make fmt          Auto-format all code
#   make doc          Build mdbook + rustdoc
#   make doc-open     Build and open docs in browser
#   make audit        Run cargo-deny security audit
#   make clean        Clean all build artifacts
#   make ci           Full CI pipeline locally

.PHONY: build test test-unit test-phoenix lint fmt doc doc-open doc-api audit clean ci

# ─── Build ──────────────────────────────────────────────

build:
	cargo build --bin nexus --release

build-debug:
	cargo build --bin nexus

# ─── Test ───────────────────────────────────────────────

test:
	cargo test --workspace
	cargo test --workspace --doc

test-unit:
	cargo test -p nexus-core

test-phoenix:
	cargo test -p phoenix-tests -- --nocapture

test-property:
	cargo test -p nexus-core -- property_ --nocapture

# ─── Lint & Format ──────────────────────────────────────

fmt:
	cargo fmt --all

lint: fmt-check clippy doc-check

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

doc-check:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --document-private-items

# ─── Documentation ──────────────────────────────────────

doc: doc-book doc-api

doc-book:
	mdbook build

doc-book-serve:
	mdbook serve --open

doc-api:
	cargo doc --no-deps --workspace --document-private-items

doc-open: doc-api
	@echo "Opening target/doc/nexus_core/index.html..."
	@command -v xdg-open >/dev/null 2>&1 && xdg-open target/doc/nexus_core/index.html || \
	 command -v open >/dev/null 2>&1 && open target/doc/nexus_core/index.html || \
	 echo "Open target/doc/nexus_core/index.html in your browser"

# ─── Security ───────────────────────────────────────────

audit:
	cargo deny check licenses
	cargo deny check advisories

# ─── Benchmarks ─────────────────────────────────────────

bench:
	cargo bench --bench benchmarks

# ─── Clean ──────────────────────────────────────────────

clean:
	cargo clean
	rm -rf target/book

# ─── CI Pipeline (local) ────────────────────────────────

ci: lint test audit
	@echo ""
	@echo "=========================================="
	@echo "  CI Pipeline Complete — All Gates Passed"
	@echo "=========================================="
