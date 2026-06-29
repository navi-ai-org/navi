#!/usr/bin/env bash
#
# NAVI installer — downloads the latest prebuilt binary for your platform.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/install.sh | sh -s -- --version 0.2.0
#   curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/install.sh | sh -s -- --to /usr/local/bin
#
# Environment variables:
#   NAVI_VERSION   — install a specific version (default: latest)
#   NAVI_INSTALL   — installation directory (default: ~/.local/bin)
#
set -euo pipefail

# ── Colors & helpers ──────────────────────────────────────────────────────────

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

# ── Platform detection ────────────────────────────────────────────────────────

detect_os() {
  case "$(uname -s)" in
    Linux*)   echo "linux" ;;
    Darwin*)  echo "darwin" ;;
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

# ── Dependency checks ─────────────────────────────────────────────────────────

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    error "Required command not found: $1"
    error "Please install $1 and try again."
    exit 1
  fi
}

# ── Version resolution ────────────────────────────────────────────────────────

get_latest_version() {
  local url="https://api.github.com/repos/navi-ai-org/navi/releases/latest"
  local version

  if command -v curl >/dev/null 2>&1; then
    version=$(curl -fsSL "$url" | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name":\s*"v?([^"]+)".*/\1/')
  elif command -v wget >/dev/null 2>&1; then
    version=$(wget -qO- "$url" | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name":\s*"v?([^"]+)".*/\1/')
  else
    error "Neither curl nor wget found. Please install one and try again."
    exit 1
  fi

  if [ -z "$version" ]; then
    error "Could not determine latest version from GitHub."
    error "Try setting NAVI_VERSION explicitly."
    exit 1
  fi

  echo "$version"
}

# ── Download ──────────────────────────────────────────────────────────────────

download() {
  local url="$1"
  local dest="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --progress-bar -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q --show-progress -O "$dest" "$url"
  fi
}

# ── Main ──────────────────────────────────────────────────────────────────────

main() {
  local version="${NAVI_VERSION:-}"
  local install_dir="${NAVI_INSTALL:-$HOME/.local/bin}"

  # Parse arguments
  while [ $# -gt 0 ]; do
    case "$1" in
      --version|-v)  version="$2"; shift 2 ;;
      --to|-t)       install_dir="$2"; shift 2 ;;
      --help|-h)
        echo "NAVI installer"
        echo ""
        echo "Usage: install.sh [OPTIONS]"
        echo ""
        echo "Options:"
        echo "  --version, -v <VERSION>  Install a specific version (default: latest)"
        echo "  --to, -t <DIR>           Install to a specific directory (default: ~/.local/bin)"
        echo "  --help, -h               Show this help"
        echo ""
        echo "Environment variables:"
        echo "  NAVI_VERSION  — install a specific version"
        echo "  NAVI_INSTALL  — installation directory"
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

  # Resolve version
  if [ -z "$version" ]; then
    info "Fetching latest version..."
    version=$(get_latest_version)
  fi

  info "Installing NAVI ${BOLD}v${version}${RESET}"

  # Build download URL
  local archive_name
  local binary_name="navi"
  local ext="tar.gz"

  if [ "$os" = "win32" ]; then
    archive_name="navi-${os}-${arch}.zip"
    binary_name="navi.exe"
    ext="zip"
  else
    archive_name="navi-${os}-${arch}.tar.gz"
  fi

  local base_url="https://github.com/navi-ai-org/navi/releases/download/v${version}"
  local download_url="${base_url}/${archive_name}"

  # Create temp directory
  local tmp_dir
  tmp_dir=$(mktemp -d)
  trap 'rm -rf "$tmp_dir"' EXIT

  # Download
  info "Downloading ${download_url}..."
  local archive_path="${tmp_dir}/${archive_name}"
  if ! download "$download_url" "$archive_path"; then
    error "Download failed. Check that version v${version} exists:"
    error "  https://github.com/navi-ai-org/navi/releases"
    exit 1
  fi

  # Extract
  info "Extracting..."
  if [ "$ext" = "zip" ]; then
    unzip -qo "$archive_path" -d "$tmp_dir/extracted"
  else
    tar -xzf "$archive_path" -C "$tmp_dir/extracted" 2>/dev/null || {
      mkdir -p "$tmp_dir/extracted"
      tar -xzf "$archive_path" -C "$tmp_dir/extracted"
    }
  fi

  # Find the binary
  local binary_path
  binary_path=$(find "$tmp_dir/extracted" -name "$binary_name" -type f | head -1)

  if [ -z "$binary_path" ]; then
    error "Could not find ${binary_name} in the downloaded archive."
    exit 1
  fi

  # Install
  mkdir -p "$install_dir"
  local dest="${install_dir}/${binary_name}"

  # Fallback if default dir is not writable
  if [ ! -w "$install_dir" ] && [ "$install_dir" = "$HOME/.local/bin" ]; then
    install_dir="$HOME/.navi/bin"
    mkdir -p "$install_dir"
    dest="${install_dir}/${binary_name}"
    warn "~/.local/bin not writable, using ~/.navi/bin instead"
  fi

  cp "$binary_path" "$dest"
  chmod +x "$dest"

  success "NAVI v${version} installed to ${BOLD}${dest}${RESET}"

  # PATH check
  case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *)
      warn ""
      warn "${YELLOW}${install_dir} is not in your PATH.${RESET}"
      warn ""
      warn "Add it to your shell profile:"
      warn ""
      if [ -f "$HOME/.zshrc" ]; then
        warn "  echo 'export PATH=\"${install_dir}:\$PATH\"' >> ~/.zshrc"
      elif [ -f "$HOME/.bashrc" ]; then
        warn "  echo 'export PATH=\"${install_dir}:\$PATH\"' >> ~/.bashrc"
      else
        warn "  export PATH=\"${install_dir}:\$PATH\""
      fi
      warn ""
      ;;
  esac

  # Verify
  if command -v "$dest" >/dev/null 2>&1; then
    info ""
    info "Run ${BOLD}navi${RESET} to get started."
  fi
}

main "$@"
