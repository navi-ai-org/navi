#!/usr/bin/env bash
# Run NAPI + Dart binding tests inside Docker (or on host if DOCKER=0).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_TERM_COLOR=always
export NAVI_NO_REGISTRY_UPDATE=1
export RUST_BACKTRACE=1
export CARGO_INCREMENTAL=0

# navi-napi defaults to voice-onnx (downloads ORT libs). Those prebuilts need a
# newer glibc than Debian bookworm, so Docker / portable CI disables the feature.
NAPI_FEATURES_ARGS=()
if [[ "${NAVI_NAPI_NO_ONNX:-0}" == "1" ]]; then
  NAPI_FEATURES_ARGS=(--no-default-features)
fi

run_tests() {
  echo "== cargo test navi-sdk engine_api =="
  cargo test -p navi-sdk --lib engine_api -- --test-threads=2

  echo "== cargo test navi-dart (serial) =="
  cargo test -p navi-dart -- --test-threads=1

  echo "== cargo test navi-napi ${NAPI_FEATURES_ARGS[*]:-} =="
  cargo test -p navi-napi "${NAPI_FEATURES_ARGS[@]}" -- --test-threads=2

  echo "== build navi-napi cdylib ${NAPI_FEATURES_ARGS[*]:-} =="
  cargo build -p navi-napi "${NAPI_FEATURES_ARGS[@]}"

  if command -v node >/dev/null 2>&1; then
    echo "== node binding tests =="
    (
      cd crates/navi-napi
      # Stage the cargo-built cdylib as the platform .node artifact.
      if [[ -f ../../target/debug/libnavi_napi.so ]]; then
        cp -f ../../target/debug/libnavi_napi.so "navi.linux-$(uname -m | sed 's/x86_64/x64/;s/aarch64/arm64/').node"
      elif [[ -f ../../target/debug/libnavi_napi.dylib ]]; then
        cp -f ../../target/debug/libnavi_napi.dylib "navi.darwin-$(uname -m | sed 's/x86_64/x64/;s/aarch64/arm64/').node"
      elif [ ! -f navi.linux-x64.node ] && [ ! -f navi.linux-arm64.node ] && [ -f package.json ]; then
        npm run build || true
      fi
      # Force-exit: the native engine keeps a Tokio runtime alive after tests.
      node --test --test-force-exit test/*.test.mjs
    )
  else
    echo "node not found; skipping JS binding tests"
  fi

  echo "ALL BINDING CHECKS PASSED"
}

if [[ "${DOCKER:-1}" == "1" ]]; then
  echo "Running binding tests in Docker (rust:bookworm)..."
  # navi-browser optionally depends on a local CloakBrowser checkout:
  # crates/navi-browser -> ../../../../lab/CloakBrowser-rust/...
  # With the repo mounted at /work, that resolves to /lab/...
  HOST_LAB="$(cd "$ROOT/../../lab" 2>/dev/null && pwd || true)"
  DOCKER_LAB_MOUNT=()
  if [[ -n "${HOST_LAB}" && -d "${HOST_LAB}" ]]; then
    DOCKER_LAB_MOUNT=(-v "${HOST_LAB}:/lab:ro")
  fi

  docker run --rm \
    -v "$ROOT":/work \
    -v navi-cargo-registry:/usr/local/cargo/registry \
    -v navi-cargo-git:/usr/local/cargo/git \
    -v navi-target-cache:/work/target \
    "${DOCKER_LAB_MOUNT[@]}" \
    -e CARGO_HOME=/usr/local/cargo \
    -e PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    -e NAVI_NO_REGISTRY_UPDATE=1 \
    -e NAVI_NAPI_NO_ONNX=1 \
    -w /work \
    rust:bookworm \
    bash -lc '
      set -euo pipefail
      export PATH="/usr/local/cargo/bin:${PATH:-/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin}"
      export NAVI_NAPI_NO_ONNX=1
      apt-get update -qq
      DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
        pkg-config libssl-dev ca-certificates curl build-essential python3 >/dev/null
      # Node 20 for NAPI JS tests
      curl -fsSL https://deb.nodesource.com/setup_20.x | bash - >/dev/null
      DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nodejs >/dev/null
      rustc --version
      cargo --version
      node --version
      DOCKER=0 bash scripts/test-bindings-docker.sh
    '
else
  run_tests
fi
