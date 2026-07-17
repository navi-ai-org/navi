#!/bin/sh
#
# NAVI installer — downloads a prebuilt binary from GitHub Releases.
# POSIX sh compatible (dash on Ubuntu, ash on Alpine, bash, …).
#
# Primary install method:
#   curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh
#
# Pin version (skips "latest" lookup; useful offline / under rate limits):
#   curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh \
#     | sh -s -- --version 0.3.0
#
# Safer: pin the install script by commit and pin the release version:
#   curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/<commit>/scripts/install.sh \
#     | sh -s -- --version 0.3.0
#
# Security (what this script enforces by default):
#   1. HTTPS only (curl/wget fail on TLS errors; no --insecure)
#   2. SHA-256 of the archive MUST match release SHA256SUMS.txt (hard fail)
#   3. Optional Sigstore/cosign verification of SHA256SUMS when `cosign` is installed
#   4. Archives may only contain a single root file: navi / navi.exe (no path traversal)
#   5. Install only under a user-writable directory (default ~/.local/bin)
#
# Package format: .tar.gz (Unix) / .zip (Windows) containing the bare binary at the
# archive root. tar.gz is the industry default (rustup, deno, go, …) — integrity
# comes from checksums/signatures, not from the archive format.
#
set -eu
# Enable pipefail when the shell supports it (bash/ksh); dash/ash ignore this.
( set -o pipefail ) 2>/dev/null && set -o pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

info()  { printf "${CYAN}[navi]${RESET} %s\n" "$*"; }
warn()  { printf "${YELLOW}[navi]${RESET} %s\n" "$*"; }
error() { printf "${RED}[navi]${RESET} %s\n" "$*" >&2; }
success() { printf "${GREEN}[navi]${RESET} %s\n" "$*"; }

REPO="${NAVI_REPO:-navi-ai-org/navi}"
GITHUB_API="https://api.github.com/repos/${REPO}"
GITHUB_DL="https://github.com/${REPO}/releases/download"

# Sigstore identity for keyless cosign signatures produced by Release CI.
COSIGN_CERT_IDENTITY_REGEXP="${NAVI_COSIGN_IDENTITY_REGEXP:-https://github.com/${REPO}/\\.github/workflows/release\\.yml@refs/tags/v.*}"
COSIGN_CERT_OIDC_ISSUER="${NAVI_COSIGN_OIDC_ISSUER:-https://token.actions.githubusercontent.com}"

detect_os() {
  case "$(uname -s)" in
    Linux*)  echo "linux" ;;
    Darwin*) echo "darwin" ;;
    MINGW*|MSYS*|CYGWIN*) echo "win32" ;;
    *) error "Unsupported OS: $(uname -s)"; exit 1 ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64)  echo "x64" ;;
    aarch64|arm64) echo "arm64" ;;
    *) error "Unsupported architecture: $(uname -m)"; exit 1 ;;
  esac
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    error "Required command not found: $1"
    exit 1
  fi
}

# Optional GitHub token for API fallback (higher rate limits).
# Accept GH_TOKEN / GITHUB_TOKEN when set in the environment.
github_auth_header() {
  local token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  if [ -n "$token" ]; then
    printf 'Authorization: Bearer %s' "$token"
  fi
}

# Always use TLS-verifying clients. Never pass --insecure / --no-check-certificate.
http_get() {
  local url="$1"
  local auth=""
  case "$url" in
    https://*) ;;
    *) error "Refusing non-HTTPS URL: $url"; exit 1 ;;
  esac
  auth=$(github_auth_header)
  if command -v curl >/dev/null 2>&1; then
    if [ -n "$auth" ] && printf '%s' "$url" | grep -q 'api\.github\.com'; then
      curl -fsSL --proto '=https' --tlsv1.2 \
        -H "$auth" \
        -H "X-GitHub-Api-Version: 2022-11-28" \
        "$url"
    else
      curl -fsSL --proto '=https' --tlsv1.2 "$url"
    fi
  elif command -v wget >/dev/null 2>&1; then
    if [ -n "$auth" ] && printf '%s' "$url" | grep -q 'api\.github\.com'; then
      wget -qO- --https-only --header="$auth" "$url"
    else
      wget -qO- --https-only "$url"
    fi
  else
    error "Neither curl nor wget found."
    exit 1
  fi
}

# Final URL after redirects (used for /releases/latest → /releases/tag/vX.Y.Z).
http_effective_url() {
  local url="$1"
  case "$url" in
    https://*) ;;
    *) error "Refusing non-HTTPS URL: $url"; exit 1 ;;
  esac
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --proto '=https' --tlsv1.2 -o /dev/null -w '%{url_effective}' "$url"
  elif command -v wget >/dev/null 2>&1; then
    # wget --server-response prints headers to stderr; take the last Location.
    wget -q --https-only --max-redirect=10 --spider -S "$url" 2>&1 \
      | sed -n 's/^  Location: //p' \
      | tail -1 \
      | tr -d '\r'
  else
    error "Neither curl nor wget found."
    exit 1
  fi
}

download_file() {
  local url="$1"
  local dest="$2"
  case "$url" in
    https://*) ;;
    *) error "Refusing non-HTTPS URL: $url"; exit 1 ;;
  esac
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --proto '=https' --tlsv1.2 --progress-bar -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q --https-only --show-progress -O "$dest" "$url"
  else
    error "Neither curl nor wget found."
    exit 1
  fi
}

normalize_version() {
  local v="$1"
  v="${v#v}"
  # Reject path traversal / weird tags early.
  case "$v" in
    ''|*/*|*..*|*[!A-Za-z0-9._-]*)
      error "Invalid version string: $1"
      exit 1
      ;;
  esac
  echo "$v"
}

get_latest_version() {
  local version final body url

  # 1) Prefer HTML redirect (no api.github.com rate limit — unauthenticated
  #    API is only 60 req/hour/IP and commonly 403s on shared networks).
  #    https://github.com/OWNER/REPO/releases/latest → .../tag/vX.Y.Z
  url="https://github.com/${REPO}/releases/latest"
  if final=$(http_effective_url "$url" 2>/dev/null); then
    version=$(printf '%s' "$final" | sed -n 's|.*/releases/tag/\([^/?#]*\)$|\1|p')
    if [ -n "$version" ]; then
      version=$(normalize_version "$version")
      if [ -n "$version" ]; then
        echo "$version"
        return 0
      fi
    fi
  fi

  # 2) API fallback (uses GH_TOKEN / GITHUB_TOKEN when set).
  url="${GITHUB_API}/releases/latest"
  if body=$(http_get "$url" 2>/dev/null); then
    version=$(printf '%s' "$body" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
    version=$(normalize_version "${version:-}")
    if [ -n "$version" ]; then
      echo "$version"
      return 0
    fi
  fi

  error "Could not determine latest version from GitHub."
  error "The GitHub API often returns 403 when rate-limited (no auth)."
  error "Workarounds:"
  error "  curl -fsSL …/install.sh | sh -s -- --version 0.3.0"
  error "  GH_TOKEN=… curl -fsSL …/install.sh | sh   # authenticated API"
  error "  export NAVI_VERSION=0.3.0"
  exit 1
}

sha256_file() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$path" | awk '{print $1}'
  else
    error "Need sha256sum or shasum to verify the download."
    exit 1
  fi
}

# Hard-fail integrity check. Checksums alone do not prove authenticity if the
# entire release is replaced; use cosign for that when available.
verify_sha256() {
  local archive_path="$1"
  local sums_path="$2"
  local archive_name
  archive_name=$(basename "$archive_path")

  if [ ! -s "$sums_path" ]; then
    error "SHA256SUMS.txt is missing or empty."
    error "Refusing to install without integrity data."
    exit 1
  fi

  local expected
  expected=$(
    # Accept "HASH  name" or "HASH *name" (text/binary modes).
    awk -v name="$archive_name" '
      $2 == name || $2 == ("*" name) || $2 == ("./" name) { print $1; exit }
    ' "$sums_path"
  )

  if [ -z "$expected" ]; then
    error "No SHA-256 entry for ${archive_name} in SHA256SUMS.txt"
    error "Refusing to install."
    exit 1
  fi

  # Hex-only 64-char checksum.
  if ! printf '%s' "$expected" | grep -Eq '^[0-9a-fA-F]{64}$'; then
    error "Malformed checksum for ${archive_name}: ${expected}"
    exit 1
  fi

  local actual
  actual=$(sha256_file "$archive_path")
  # Case-insensitive compare
  if [ "$(printf '%s' "$actual" | tr 'A-F' 'a-f')" != "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" ]; then
    error "Checksum mismatch for ${archive_name}"
    error "  expected: ${expected}"
    error "  actual:   ${actual}"
    error "The download may be corrupt or tampered with. Aborting."
    exit 1
  fi
  info "SHA-256 OK ($(printf '%s' "$actual" | cut -c1-12)…)"
}

# Authenticate SHA256SUMS via Sigstore (proves it was produced by our Release
# workflow). Optional: skipped with a warning if cosign is not installed.
verify_cosign_bundle() {
  local sums_path="$1"
  local bundle_path="$2"

  if [ ! -f "$bundle_path" ]; then
    warn "No Sigstore bundle (SHA256SUMS.txt.sigstore.json) for this release."
    warn "SHA-256 still verified; install cosign for signature authentication:"
    warn "  https://docs.sigstore.dev/cosign/system_config/installation/"
    return 0
  fi

  if ! command -v cosign >/dev/null 2>&1; then
    warn "cosign not found — skipping signature authentication of SHA256SUMS."
    warn "Install cosign to verify releases were signed by GitHub Actions CI."
    return 0
  fi

  info "Verifying Sigstore signature of SHA256SUMS.txt…"
  if cosign verify-blob \
      --bundle "$bundle_path" \
      --certificate-identity-regexp "$COSIGN_CERT_IDENTITY_REGEXP" \
      --certificate-oidc-issuer "$COSIGN_CERT_OIDC_ISSUER" \
      "$sums_path" >/dev/null; then
    success "Sigstore signature OK (Release workflow identity)"
  else
    error "Sigstore verification failed for SHA256SUMS.txt"
    error "Refusing to install."
    exit 1
  fi
}

# Reject path traversal / multi-file payloads. Only a single root binary is allowed.
assert_safe_archive_members() {
  archive_path="$1"
  expected_name="$2"
  ext="$3"
  count=0
  list_file=$(mktemp)

  if [ "$ext" = "zip" ]; then
    unzip -Z1 "$archive_path" >"$list_file"
  else
    tar -tzf "$archive_path" >"$list_file"
  fi

  while IFS= read -r member || [ -n "$member" ]; do
    [ -z "$member" ] && continue
    # Normalize trailing slash directories (reject them).
    case "$member" in
      */) error "Archive must not contain directories: $member"; rm -f "$list_file"; exit 1 ;;
    esac
    # Strip leading ./
    member="${member#./}"
    case "$member" in
      ""|*"/"*|*".."*|*"\\"*)
        error "Unsafe archive member rejected: $member"
        error "Expected a single root file named '${expected_name}'."
        rm -f "$list_file"
        exit 1
        ;;
      "$expected_name")
        count=$((count + 1))
        ;;
      *)
        error "Unexpected archive member: $member"
        error "Expected only '${expected_name}'."
        rm -f "$list_file"
        exit 1
        ;;
    esac
  done <"$list_file"
  rm -f "$list_file"

  if [ "$count" -ne 1 ]; then
    error "Archive must contain exactly one file named '${expected_name}' (found $count)."
    exit 1
  fi
}

extract_binary() {
  local archive_path="$1"
  local extract_dir="$2"
  local binary_name="$3"
  local ext="$4"

  assert_safe_archive_members "$archive_path" "$binary_name" "$ext"
  mkdir -p "$extract_dir"

  if [ "$ext" = "zip" ]; then
    # Extract only the expected file to a known path.
    unzip -qo "$archive_path" "$binary_name" -d "$extract_dir"
  else
    # Extract only the named member; refuse absolute names (already checked).
    tar -xzf "$archive_path" -C "$extract_dir" "$binary_name" 2>/dev/null \
      || tar -xzf "$archive_path" -C "$extract_dir" "./$binary_name"
  fi

  local binary_path="${extract_dir}/${binary_name}"
  if [ ! -f "$binary_path" ]; then
    error "Extraction failed: ${binary_path} missing."
    exit 1
  fi
  # Must be a regular file (not symlink out of the extract dir).
  if [ -L "$binary_path" ]; then
    error "Refusing to install a symlink from the archive."
    exit 1
  fi
  echo "$binary_path"
}

main() {
  local version="${NAVI_VERSION:-}"
  local install_dir="${NAVI_INSTALL:-$HOME/.local/bin}"
  local do_verify=1
  local require_cosign=0

  while [ $# -gt 0 ]; do
    case "$1" in
      --version|-v)
        version="$2"
        shift 2
        ;;
      --to|-t)
        install_dir="$2"
        shift 2
        ;;
      --verify)
        do_verify=1
        shift
        ;;
      --no-verify)
        error "--no-verify is disabled for safety. Pin --version instead if needed."
        exit 1
        ;;
      --require-cosign)
        require_cosign=1
        shift
        ;;
      --help|-h)
        cat <<'EOF'
NAVI installer — secure install of prebuilt binaries from GitHub Releases

Usage:
  curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh
  curl -fsSL ... | sh -s -- [OPTIONS]

Options:
  --version, -v <VERSION>  Version or tag (e.g. 0.3.0 or v0.3.0). Default: latest
  --to, -t <DIR>           Install directory (default: ~/.local/bin)
  --verify                 Require SHA-256 match (default, always on)
  --require-cosign         Fail if Sigstore/cosign verification is unavailable
  --help, -h               Show this help

Environment:
  NAVI_VERSION   Same as --version
  NAVI_INSTALL   Same as --to
  GH_TOKEN / GITHUB_TOKEN  Optional; used only if HTML latest-resolve fails
  NAVI_REPO      GitHub repo (default: navi-ai-org/navi)

Security:
  • Downloads only over HTTPS
  • Archive SHA-256 must match release SHA256SUMS.txt (hard fail)
  • When cosign is installed, SHA256SUMS is authenticated via Sigstore
    (GitHub Actions OIDC identity for the Release workflow)
  • Archives must contain only a single root binary (navi / navi.exe)
  • Prefer pinning both script commit and --version for high-assurance installs
EOF
        exit 0
        ;;
      *)
        error "Unknown option: $1"
        exit 1
        ;;
    esac
  done

  local os arch
  os=$(detect_os)
  arch=$(detect_arch)
  info "Detected platform: ${BOLD}${os}-${arch}${RESET}"

  if [ -z "$version" ]; then
    info "Fetching latest version…"
    version=$(get_latest_version)
  else
    version=$(normalize_version "$version")
  fi
  info "Installing NAVI ${BOLD}v${version}${RESET}"

  local archive_name binary_name ext
  binary_name="navi"
  ext="tar.gz"
  if [ "$os" = "win32" ]; then
    archive_name="navi-${os}-${arch}.zip"
    binary_name="navi.exe"
    ext="zip"
    require_cmd unzip
  else
    archive_name="navi-${os}-${arch}.tar.gz"
    require_cmd tar
  fi

  local download_url="${GITHUB_DL}/v${version}/${archive_name}"
  local sums_url="${GITHUB_DL}/v${version}/SHA256SUMS.txt"
  local bundle_url="${GITHUB_DL}/v${version}/SHA256SUMS.txt.sigstore.json"

  local tmp_dir
  tmp_dir=$(mktemp -d)
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp_dir'" EXIT

  info "Downloading ${download_url}…"
  local archive_path="${tmp_dir}/${archive_name}"
  local sums_path="${tmp_dir}/SHA256SUMS.txt"
  local bundle_path="${tmp_dir}/SHA256SUMS.txt.sigstore.json"

  if ! download_file "$download_url" "$archive_path"; then
    error "Download failed. Check that v${version} exists:"
    error "  https://github.com/${REPO}/releases"
    exit 1
  fi

  if ! download_file "$sums_url" "$sums_path"; then
    error "Failed to download SHA256SUMS.txt — refusing to install."
    exit 1
  fi

  # Bundle may not exist for older releases.
  download_file "$bundle_url" "$bundle_path" 2>/dev/null || rm -f "$bundle_path"

  if [ "$do_verify" -eq 1 ]; then
    if [ "$require_cosign" -eq 1 ]; then
      if ! command -v cosign >/dev/null 2>&1; then
        error "--require-cosign set but cosign is not installed."
        exit 1
      fi
      if [ ! -f "$bundle_path" ]; then
        error "--require-cosign set but no Sigstore bundle on this release."
        exit 1
      fi
    fi
    verify_cosign_bundle "$sums_path" "$bundle_path"
    verify_sha256 "$archive_path" "$sums_path"
  fi

  local extract_dir="${tmp_dir}/extracted"
  info "Extracting (single-file ${binary_name})…"
  local binary_path
  binary_path=$(extract_binary "$archive_path" "$extract_dir" "$binary_name" "$ext")

  mkdir -p "$install_dir"
  local dest="${install_dir}/${binary_name}"

  if [ ! -w "$install_dir" ] && [ "$install_dir" = "$HOME/.local/bin" ]; then
    install_dir="$HOME/.navi/bin"
    mkdir -p "$install_dir"
    dest="${install_dir}/${binary_name}"
    warn "~/.local/bin not writable, using ~/.navi/bin instead"
  fi

  # Atomic replace within the install directory.
  cp "$binary_path" "${dest}.tmp"
  chmod 755 "${dest}.tmp"
  mv -f "${dest}.tmp" "$dest"

  success "NAVI v${version} installed to ${BOLD}${dest}${RESET}"

  case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *)
      warn ""
      warn "${install_dir} is not in your PATH."
      warn ""
      warn "Add it to your shell profile:"
      warn ""
      if [ -n "${ZSH_VERSION:-}" ] || [ -f "$HOME/.zshrc" ]; then
        warn "  echo 'export PATH=\"${install_dir}:\$PATH\"' >> ~/.zshrc && source ~/.zshrc"
      elif [ -n "${FISH_VERSION:-}" ] || [ -f "$HOME/.config/fish/config.fish" ]; then
        warn "  fish -c \"fish_add_path ${install_dir}\""
      else
        warn "  echo 'export PATH=\"${install_dir}:\$PATH\"' >> ~/.bashrc && source ~/.bashrc"
      fi
      warn ""
      ;;
  esac

  if "$dest" --help >/dev/null 2>&1; then
    info "Run ${BOLD}navi${RESET} to get started."
  else
    info "Installed. Open a new terminal and run ${BOLD}navi${RESET}."
  fi
}

main "$@"
