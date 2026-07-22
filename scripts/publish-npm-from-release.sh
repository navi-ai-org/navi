#!/usr/bin/env bash
# Stage platform binaries from a GitHub Release and publish @navi-agent/navi packages.
#
# Usage:
#   ./scripts/publish-npm-from-release.sh v0.3.3
#
# Local: requires `npm login` (or equivalent token).
# CI (Trusted Publishing / OIDC): set NAVI_NPM_OIDC=1 and workflow permissions:
#   id-token: write
#   contents: read
# Do not set NODE_AUTH_TOKEN / NPM_TOKEN for publish when using OIDC.
set -euo pipefail

version="${1:-}"
repo="${NAVI_GITHUB_REPO:-navi-ai-org/navi}"

if [ -z "$version" ]; then
  echo "usage: $0 <version-or-tag>" >&2
  echo "example: $0 v0.1.0" >&2
  exit 2
fi

tag="$version"
version="${version#v}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
npm_root="$root/npm/navi"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

# True when publishing via GitHub Actions OIDC trusted publishing.
oidc_mode() {
  [[ "${NAVI_NPM_OIDC:-}" == "1" || "${NAVI_NPM_OIDC:-}" == "true" ]] \
    || [[ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" && -n "${GITHUB_ACTIONS:-}" ]]
}

npm_exists() {
  local pkg="$1"
  npm view "$pkg@$version" version --registry https://registry.npmjs.org/ >/dev/null 2>&1
}

# Ensure package.json version matches the release tag before publish.
sync_package_version() {
  local dir="$1"
  local pkg_json="$dir/package.json"
  node -e '
    const fs = require("fs");
    const path = process.argv[1];
    const want = process.argv[2];
    const pkg = JSON.parse(fs.readFileSync(path, "utf8"));
    if (pkg.version !== want) {
      console.log(`bump ${pkg.name}: ${pkg.version} -> ${want}`);
      pkg.version = want;
      fs.writeFileSync(path, JSON.stringify(pkg, null, 2) + "\n");
    }
  ' "$pkg_json" "$version"

  # Meta package: keep optionalDependencies on the same version.
  if [[ "$(node -p "require('$pkg_json').name")" == "@navi-agent/navi" ]]; then
    node -e '
      const fs = require("fs");
      const path = process.argv[1];
      const want = process.argv[2];
      const pkg = JSON.parse(fs.readFileSync(path, "utf8"));
      let changed = false;
      if (pkg.optionalDependencies) {
        for (const k of Object.keys(pkg.optionalDependencies)) {
          if (k.startsWith("@navi-agent/navi-") && pkg.optionalDependencies[k] !== want) {
            pkg.optionalDependencies[k] = want;
            changed = true;
          }
        }
      }
      if (changed) {
        fs.writeFileSync(path, JSON.stringify(pkg, null, 2) + "\n");
        console.log(`synced optionalDependencies to ${want}`);
      }
    ' "$pkg_json" "$version"
  fi
}

publish_dir() {
  local dir="$1"
  local pkg
  pkg="$(node -p "require('$dir/package.json').name")"

  sync_package_version "$dir"

  if npm_exists "$pkg"; then
    echo "$pkg@$version already exists, skipping"
    return
  fi

  echo "packing $pkg@$version"
  (cd "$dir" && npm pack --dry-run --json >/dev/null)
  echo "publishing $pkg@$version"
  # Trusted publishing (OIDC) authenticates automatically in GHA when configured.
  # --provenance is automatic for trusted publishing from public repos; do not force tokens.
  (cd "$dir" && npm publish --access public --registry https://registry.npmjs.org/)
}

require_cmd gh
require_cmd npm
require_cmd node
require_cmd tar
require_cmd unzip

if oidc_mode; then
  echo "auth: OIDC / trusted publishing (skipping npm whoami)"
  echo "node $(node -v)  npm $(npm -v)"
else
  npm whoami --registry https://registry.npmjs.org/ >/dev/null
  echo "auth: npm login as $(npm whoami --registry https://registry.npmjs.org/)"
fi

echo "downloading release assets for $repo@$tag"
gh release download "$tag" \
  --repo "$repo" \
  --dir "$tmp_dir" \
  --pattern 'navi-linux-x64.tar.gz' \
  --pattern 'navi-linux-arm64.tar.gz' \
  --pattern 'navi-darwin-x64.tar.gz' \
  --pattern 'navi-darwin-arm64.tar.gz' \
  --pattern 'navi-win32-x64.zip'

rm -f "$npm_root"/npm/*/navi "$npm_root"/npm/*/navi.exe

stage_unix() {
  local platform="$1"
  local archive="$tmp_dir/navi-$platform.tar.gz"
  local extract_dir="$tmp_dir/extract-$platform"
  mkdir -p "$extract_dir"
  tar -xzf "$archive" -C "$extract_dir"
  local binary
  binary="$(find "$extract_dir" -type f -name navi | head -1)"
  if [ -z "$binary" ]; then
    echo "missing navi binary in $archive" >&2
    exit 1
  fi
  cp "$binary" "$npm_root/npm/$platform/navi"
  chmod +x "$npm_root/npm/$platform/navi"
}

stage_windows() {
  local platform="win32-x64"
  local archive="$tmp_dir/navi-$platform.zip"
  local extract_dir="$tmp_dir/extract-$platform"
  mkdir -p "$extract_dir"
  unzip -q "$archive" -d "$extract_dir"
  local binary
  binary="$(find "$extract_dir" -type f -name navi.exe | head -1)"
  if [ -z "$binary" ]; then
    echo "missing navi.exe binary in $archive" >&2
    exit 1
  fi
  cp "$binary" "$npm_root/npm/$platform/navi.exe"
}

stage_unix linux-x64
stage_unix linux-arm64
stage_unix darwin-x64
stage_unix darwin-arm64
stage_windows

# Platform packages first, then the meta package (optionalDependencies).
publish_dir "$npm_root/npm/linux-x64"
publish_dir "$npm_root/npm/linux-arm64"
publish_dir "$npm_root/npm/darwin-x64"
publish_dir "$npm_root/npm/darwin-arm64"
publish_dir "$npm_root/npm/win32-x64"
publish_dir "$npm_root"

echo "published npm packages for $tag"
