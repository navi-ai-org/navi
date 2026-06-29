#!/usr/bin/env bash
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

npm_exists() {
  local pkg="$1"
  npm view "$pkg@$version" version --registry https://registry.npmjs.org/ >/dev/null 2>&1
}

publish_dir() {
  local dir="$1"
  local pkg
  pkg="$(node -p "require('$dir/package.json').name")"

  if npm_exists "$pkg"; then
    echo "$pkg@$version already exists, skipping"
    return
  fi

  echo "packing $pkg@$version"
  (cd "$dir" && npm pack --dry-run --json >/dev/null)
  echo "publishing $pkg@$version"
  (cd "$dir" && npm publish --access public --registry https://registry.npmjs.org/)
}

require_cmd gh
require_cmd npm
require_cmd node
require_cmd tar
require_cmd unzip

npm whoami --registry https://registry.npmjs.org/ >/dev/null

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

publish_dir "$npm_root/npm/linux-x64"
publish_dir "$npm_root/npm/linux-arm64"
publish_dir "$npm_root/npm/darwin-x64"
publish_dir "$npm_root/npm/darwin-arm64"
publish_dir "$npm_root/npm/win32-x64"
publish_dir "$npm_root"

echo "published npm packages for $tag"
