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

# Install the navi binary to ~/.cargo/bin (or wherever cargo install puts it).
install:
    cargo install --path crates/navi-cli

# Install the navi binary in release mode.
install-release:
    cargo install --path crates/navi-cli --release

check:
    cargo check --workspace --all-targets

# Verify SDK ↔ N-API binding parity (no drift between surfaces).
parity-check:
    cargo test -p navi-sdk --lib engine_api::tests::napi_binding_covers_all -- --test-threads={{test_threads}}

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

# Run the full agentic benchmark corpus with the baseline benchmark model.
bench suite="benchmarks/suites" output="benchmarks/runs/benchmark-latest.json" provider="opencode" model="deepseek-v4-flash-free":
    mkdir -p "$(dirname "{{output}}")"
    @status=0; cargo run -p navi-cli -- bench run "{{suite}}" --auto-approve --provider "{{provider}}" --model "{{model}}" --output "{{output}}" || status=$?; node benchmarks/site/generate-runs-index.mjs; echo "Wrote {{output}}"; exit $status

# Run only the smoke benchmark suite.
bench-smoke output="benchmarks/runs/smoke-latest.json" provider="opencode" model="deepseek-v4-flash-free":
    @just bench benchmarks/suites/smoke "{{output}}" "{{provider}}" "{{model}}"

# Print the local benchmark report path.
bench-index:
    node benchmarks/site/generate-runs-index.mjs

# Generate the local benchmark index and print the report path.
bench-report: bench-index
    @echo "Open benchmarks/site/index.html"

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

# ─── Registry ────────────────────────────────────────────────────────────────

# Build the navi-dart native library (debug).
dart-build:
    cargo build -p navi-dart

# Build the navi-dart native library (release).
dart-build-release:
    cargo build -p navi-dart --release

# Run navi-dart Rust tests.
dart-test:
    cargo test -p navi-dart -- --test-threads=1

# ─── Registry ────────────────────────────────────────────────────────────────

# Sync the embedded registry snapshot from the remote navi-registry database repo.
# After running this, rebuild navi to embed the updated snapshot.
sync-registry-snapshot:
    @echo "Fetching latest registry from navi-ai-org/navi-registry..."
    git archive --remote=https://github.com/navi-ai-org/navi-registry.git main manifest.json providers | tar -x -C crates/navi-core/registry-snapshot/
    @echo "Registry snapshot updated. Rebuild navi to embed it."
    @echo "  cargo check -p navi-core"

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
