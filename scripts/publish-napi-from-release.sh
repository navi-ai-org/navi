#!/usr/bin/env bash
# Stage NAPI .node binaries from a GitHub Release and publish @navi-agent/napi packages.
#
# Usage:
#   ./scripts/publish-napi-from-release.sh v0.4.9
#
# Expects the GitHub Release to carry assets named:
#   navi-napi-linux-x64.node
#   navi-napi-linux-arm64.node
#   navi-napi-darwin-x64.node
#   navi-napi-darwin-arm64.node
#   navi-napi-win32-x64.dll
# (uploaded by the build-napi job in release.yml).
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
  echo "example: $0 v0.4.9" >&2
  exit 2
fi

tag="$version"
version="${version#v}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
napi_root="$root/crates/navi-napi"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

platforms=(linux-x64 linux-arm64 darwin-x64 darwin-arm64 win32-x64)

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

asset_name() {
  local platform="$1"
  if [[ "$platform" == win32-* ]]; then
    echo "navi-napi-$platform.dll"
  else
    echo "navi-napi-$platform.node"
  fi
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
  if [[ "$(node -p "require('$pkg_json').name")" == "@navi-agent/napi" ]]; then
    node -e '
      const fs = require("fs");
      const path = process.argv[1];
      const want = process.argv[2];
      const pkg = JSON.parse(fs.readFileSync(path, "utf8"));
      let changed = false;
      if (pkg.optionalDependencies) {
        for (const k of Object.keys(pkg.optionalDependencies)) {
          if (k.startsWith("@navi-agent/napi-") && pkg.optionalDependencies[k] !== want) {
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

if oidc_mode; then
  echo "auth: OIDC / trusted publishing (skipping npm whoami)"
  echo "node $(node -v)  npm $(npm -v)"
else
  npm whoami --registry https://registry.npmjs.org/ >/dev/null
  echo "auth: npm login as $(npm whoami --registry https://registry.npmjs.org/)"
fi

echo "downloading NAPI release assets for $repo@$tag"
patterns=()
for plat in "${platforms[@]}"; do
  patterns+=(--pattern "$(asset_name "$plat")")
done
gh release download "$tag" \
  --repo "$repo" \
  --dir "$tmp_dir" \
  "${patterns[@]}"

# Stage binaries into the platform package directories.
staged=0
for plat in "${platforms[@]}"; do
  asset="$(asset_name "$plat")"
  src="$tmp_dir/$asset"
  dst_dir="$napi_root/npm/$plat"
  if [[ ! -f "$src" ]]; then
    echo "warning: asset $asset not found in release, skipping $plat" >&2
    continue
  fi
  # The loader always resolves a `.node` filename, even on Windows where the
  # toolchain produces a .dll — napi-rs addons are still loaded as .node.
  cp "$src" "$dst_dir/navi.$plat.node"
  staged=$((staged + 1))
done

if [[ $staged -eq 0 ]]; then
  echo "error: no NAPI binaries found in release $tag" >&2
  exit 1
fi
echo "staged binaries for $staged platforms"

# Platform packages first, then the meta package (optionalDependencies).
for plat in "${platforms[@]}"; do
  dst_dir="$napi_root/npm/$plat"
  if ls "$dst_dir"/navi.*.node >/dev/null 2>&1; then
    publish_dir "$dst_dir"
  else
    echo "skipping @navi-agent/napi-$plat (no staged binary)"
  fi
done
publish_dir "$napi_root"

echo "published @navi-agent/napi packages for $tag"
