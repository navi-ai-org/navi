# NAVI development tasks. Install: https://github.com/casey/just
# Qlty (code quality): https://qlty.sh — `curl https://qlty.sh | sh`
# Coverage (optional): `cargo install cargo-llvm-cov`

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

export CARGO_TEST_THREADS := "4"
test_threads := "4"
coverage_lcov := "coverage/lcov.info"

# ─── Build ───────────────────────────────────────────────────────────────────

build:
    cargo build --workspace

build-release:
    cargo build --workspace --release

check:
    cargo check --workspace --all-targets

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# ─── Tests ───────────────────────────────────────────────────────────────────

test *args:
    cargo test --workspace --test-threads={{test_threads}} {{args}}

test-crate crate *args:
    cargo test -p {{crate}} --test-threads={{test_threads}} {{args}}

# ─── Coverage (llvm-cov → LCOV for Qlty) ─────────────────────────────────────

_require-llvm-cov:
    @command -v cargo-llvm-cov >/dev/null || { \
      echo "Missing cargo-llvm-cov. Install: cargo install cargo-llvm-cov"; \
      exit 1; \
    }

coverage: _require-llvm-cov
    mkdir -p coverage
    cargo llvm-cov --workspace --lcov --output-path {{coverage_lcov}} -- --test-threads={{test_threads}}
    @echo "Wrote {{coverage_lcov}}"

coverage-summary: _require-llvm-cov
    cargo llvm-cov --workspace --summary-only -- --test-threads={{test_threads}}

coverage-html: _require-llvm-cov
    cargo llvm-cov --workspace --html -- --test-threads={{test_threads}}
    @echo "Open target/llvm-cov/html/index.html"

# Upload LCOV to Qlty Cloud (optional; requires `qlty` auth / OIDC in CI)
coverage-upload: coverage _require-qlty
    qlty coverage publish {{coverage_lcov}}

# ─── Qlty (lint, format, smells, metrics) ────────────────────────────────────

_require-qlty:
    @command -v qlty >/dev/null || { \
      echo "Missing qlty CLI. Install: curl https://qlty.sh | sh"; \
      exit 1; \
    }

qlty-check: _require-qlty
    qlty check --all

qlty-fmt: _require-qlty
    qlty fmt

qlty-fmt-check: _require-qlty
    qlty fmt --dry-run

qlty-smells: _require-qlty
    qlty smells --all

qlty-metrics: _require-qlty
    qlty metrics --all --max-depth=2 --sort complexity --limit 15

# Full static analysis via Qlty (clippy, rustfmt, security scanners, etc.)
analyze: qlty-check qlty-smells

# ─── Rust tooling (without Qlty) ─────────────────────────────────────────────

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# ─── Aggregates ────────────────────────────────────────────────────────────────

# Fast local gate: format, compile, unit tests
verify: fmt-check check test

# Pre-PR: verify + clippy + Qlty
ci: verify clippy analyze

# Install optional dev tools (qlty, llvm-cov)
setup-tools:
    @echo "Installing cargo-llvm-cov (coverage)..."
    cargo install cargo-llvm-cov --locked
    @command -v qlty >/dev/null || { \
      echo "Installing qlty..."; \
      curl -fsSL https://qlty.sh | sh; \
    }
    @echo "Done. Run: just --list"