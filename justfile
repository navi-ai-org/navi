# NAVI development tasks — https://github.com/casey/just
# Quality: rustquty — https://github.com/enrell/rustquty
# First time: `just setup-tools` then `just quality-doctor`

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

# Limit parallel rustc compilations and test threads to avoid OOM on
# machines with constrained RAM.  Override per-invocation with
#   CARGO_BUILD_JOBS=N just test
export CARGO_BUILD_JOBS := "2"
export CARGO_TEST_THREADS := "4"
test_threads := "4"
quality_dir := "quality"
quality_profile := "full"
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
    cargo test --workspace -- --test-threads={{test_threads}} {{args}}

test-crate crate *args:
    cargo test -p {{crate}} -- --test-threads={{test_threads}} {{args}}

harness-replay:
    cargo run -p navi-cli -- eval run evals/suites/b0
    cargo run -p navi-cli -- eval run evals/suites/beyond

# Regenerate the TUI golden snapshots in crates/navi-tui/tests/snapshots/.
# Run this after an intentional rendering change is reviewed by hand.
snapshot-update:
    UPDATE_SNAPSHOTS=1 cargo test -p navi-tui --test screenshots
    @echo "Updated TUI goldens in crates/navi-tui/tests/snapshots/."

# ─── Coverage (cargo-llvm-cov; also used by rustquty's coverage collector) ───

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

# ─── rustquty (local quality: collect + gate) ────────────────────────────────

_require-rustquty:
    @command -v rustquty >/dev/null || { \
      echo "Missing rustquty. Install: cargo install rustquty"; \
      exit 1; \
    }

quality-init: _require-rustquty
    rustquty init

quality-doctor: _require-rustquty
    rustquty doctor

# Collect metrics → quality/metricsSummary.json
quality-collect profile=quality_profile *args: _require-rustquty
    rustquty collect --profile {{profile}} {{args}}

# Compare metrics to quality/baseline.json → quality/qualityReport.json
quality-gate profile=quality_profile *args: _require-rustquty
    rustquty gate --profile {{profile}} {{args}}

# Default workflow: collect then gate (profile: full | fast | deep)
quality profile=quality_profile *args: _require-rustquty
    rustquty qa --profile {{profile}} {{args}}

quality-fast *args:
    @just quality fast {{args}}

quality-deep *args:
    @just quality deep {{args}}

# Verbose violations (file:line)
quality-verbose profile=quality_profile *args:
    @just quality {{profile}} -v {{args}}

# Snapshot current metrics as the ratchet baseline
quality-baseline profile=quality_profile: _require-rustquty
    rustquty collect --profile {{profile}}
    rustquty init-baseline

quality-update-baseline: _require-rustquty
    rustquty update-baseline

# Alias for CI / pre-PR
analyze profile=quality_profile *args:
    @just quality {{profile}} {{args}}

# ─── Rust tooling (direct cargo; rustquty wraps fmt/clippy in profiles) ───────

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# ─── Aggregates ────────────────────────────────────────────────────────────────

# Fast local gate: format, compile, unit tests
verify: fmt-check check test

# Pre-PR: verify + clippy + rustquty (full profile)
ci: verify clippy analyze

# Install rustquty + all collectors it can run (see `just quality-doctor`)
setup-tools:
    @echo "Installing cargo quality tools..."
    cargo install cargo-llvm-cov cargo-nextest cargo-audit cargo-deny cargo-hack cargo-mutants --locked
    @if [ -d ../rustquty ]; then \
      echo "Installing rustquty from ../rustquty (local checkout)..."; \
      cargo install --path ../rustquty/rustquty --locked --force; \
    else \
      echo "Installing rustquty from crates.io..."; \
      cargo install rustquty --locked; \
    fi
    @rustup component add rustfmt clippy 2>/dev/null || true
    @just quality-doctor
