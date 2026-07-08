#!/usr/bin/env bash
#
# NAVI installer — downloads a prebuilt binary from GitHub Releases.
#
# Primary install method (recommended):
#   curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh
#
# Options:
#   curl -fsSL ... | sh -s -- --version 0.1.2
#   curl -fsSL ... | sh -s -- --to /usr/local/bin
#   curl -fsSL ... | sh -s -- --version v0.1.2 --verify
#
# Environment:
#   NAVI_VERSION   — version or tag (default: latest release)
#   NAVI_INSTALL   — install directory (default: ~/.local/bin)
#   NAVI_REPO      — GitHub repo (default: navi-ai-org/navi)
#
set -euo pipefail

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

http_get() {
  local url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$url"
  else
    error "Neither curl nor wget found."
    exit 1
  fi
}

download_file() {
  local url="$1"
  local dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --progress-bar -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q --show-progress -O "$dest" "$url"
  else
    error "Neither curl nor wget found."
    exit 1
  fi
}

# Strip leading v so "v0.1.2" and "0.1.2" both work.
normalize_version() {
  local v="$1"
  v="${v#v}"
  echo "$v"
}

get_latest_version() {
  local url="${GITHUB_API}/releases/latest"
  local body version
  body=$(http_get "$url") || {
    error "Failed to query ${url}"
    exit 1
  }
  version=$(printf '%s' "$body" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
  version=$(normalize_version "$version")
  if [ -z "$version" ]; then
    error "Could not determine latest version from GitHub."
    error "Set NAVI_VERSION or pass --version."
    exit 1
  fi
  echo "$version"
}

verify_sha256() {
  local archive_path="$1"
  local sums_url="$2"
  local archive_name
  archive_name=$(basename "$archive_path")

  local sums_file
  sums_file=$(mktemp)
  if ! download_file "$sums_url" "$sums_file" 2>/dev/null; then
    warn "SHA256SUMS.txt not available for this release; skipping verify."
    rm -f "$sums_file"
    return 0
  fi

  local expected
  expected=$(grep -E "[[:space:]]${archive_name}$" "$sums_file" | awk '{print $1}' | head -1)
  rm -f "$sums_file"

  if [ -z "$expected" ]; then
    warn "No checksum entry for ${archive_name}; skipping verify."
    return 0
  fi

  local actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$archive_path" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$archive_path" | awk '{print $1}')
  else
    warn "No sha256sum/shasum available; skipping verify."
    return 0
  fi

  if [ "$actual" != "$expected" ]; then
    error "Checksum mismatch for ${archive_name}"
    error "  expected: ${expected}"
    error "  actual:   ${actual}"
    exit 1
  fi
  info "Checksum OK"
}

main() {
  local version="${NAVI_VERSION:-}"
  local install_dir="${NAVI_INSTALL:-$HOME/.local/bin}"
  local do_verify=1

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
        do_verify=0
        shift
        ;;
      --help|-h)
        cat <<'EOF'
NAVI installer — install prebuilt binaries from GitHub Releases

Usage:
  curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh
  curl -fsSL ... | sh -s -- [OPTIONS]

Options:
  --version, -v <VERSION>  Version or tag (e.g. 0.1.2 or v0.1.2). Default: latest
  --to, -t <DIR>           Install directory (default: ~/.local/bin)
  --verify                 Verify archive against SHA256SUMS.txt (default)
  --no-verify              Skip checksum verification
  --help, -h               Show this help

Environment:
  NAVI_VERSION   Same as --version
  NAVI_INSTALL   Same as --to
  NAVI_REPO      GitHub repo (default: navi-ai-org/navi)
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
    info "Fetching latest version..."
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

  local tmp_dir
  tmp_dir=$(mktemp -d)
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp_dir'" EXIT

  info "Downloading ${download_url}..."
  local archive_path="${tmp_dir}/${archive_name}"
  if ! download_file "$download_url" "$archive_path"; then
    error "Download failed. Check that v${version} exists:"
    error "  https://github.com/${REPO}/releases"
    exit 1
  fi

  if [ "$do_verify" -eq 1 ]; then
    verify_sha256 "$archive_path" "$sums_url"
  fi

  local extract_dir="${tmp_dir}/extracted"
  mkdir -p "$extract_dir"
  info "Extracting..."
  if [ "$ext" = "zip" ]; then
    unzip -qo "$archive_path" -d "$extract_dir"
  else
    tar -xzf "$archive_path" -C "$extract_dir"
  fi

  local binary_path
  binary_path=$(find "$extract_dir" -name "$binary_name" -type f | head -1)
  if [ -z "$binary_path" ]; then
    error "Could not find ${binary_name} in ${archive_name}."
    exit 1
  fi

  mkdir -p "$install_dir"
  local dest="${install_dir}/${binary_name}"

  if [ ! -w "$install_dir" ] && [ "$install_dir" = "$HOME/.local/bin" ]; then
    install_dir="$HOME/.navi/bin"
    mkdir -p "$install_dir"
    dest="${install_dir}/${binary_name}"
    warn "~/.local/bin not writable, using ~/.navi/bin instead"
  fi

  # Atomic-ish replace
  cp "$binary_path" "${dest}.tmp"
  chmod +x "${dest}.tmp"
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
