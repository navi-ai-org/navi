#!/usr/bin/env bash
# Record the real NAVI TUI in a VHS PTY → assets/brand/navi-demo.gif + .mp4
# No mouse overlay — pure terminal capture.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BRAND="$ROOT/assets/brand"
TAPE="$BRAND/navi-demo.tape"
NAVI_BIN="${NAVI_BIN:-/home/enrell/.local/bin/navi}"
DEMO_DIR="${DEMO_DIR:-/tmp/navi-gif-demo}"
WORK="${WORK:-/tmp/navi-demo-record}"

if ! command -v vhs >/dev/null; then
  echo "vhs is required (https://github.com/charmbracelet/vhs)" >&2
  exit 1
fi
if [[ ! -x "$NAVI_BIN" ]]; then
  echo "navi binary not found at $NAVI_BIN (set NAVI_BIN=...)" >&2
  exit 1
fi

mkdir -p "$DEMO_DIR" "$WORK" "$BRAND"
cat >"$DEMO_DIR/README.md" <<'EOF'
# navi-gif-demo

A tiny sample project used to demo NAVI in a terminal recording.
EOF

mkdir -p "$DEMO_DIR/.navi"
# Free model only — never Charm Hyper for the public demo GIF.
cat >"$DEMO_DIR/.navi/config.toml" <<'EOF'
[model]
provider = "opencode"
name = "mimo-v2.5-free"

[security]
permission_mode = "yolo"
EOF

# VHS only accepts simple relative Output names
TMP_TAPE="$WORK/navi-demo.tape"
{
  echo "Output navi-demo.gif"
  echo "Output navi-demo.mp4"
  awk '/^Output / { next } { print }' "$TAPE"
} >"$TMP_TAPE"
sed -i "s|/home/enrell/.local/bin/navi|${NAVI_BIN}|g" "$TMP_TAPE"

echo "==> Recording TUI with VHS (real PTY, no mouse) …"
(
  cd "$WORK"
  vhs navi-demo.tape
)

if [[ ! -f "$WORK/navi-demo.gif" ]]; then
  echo "VHS did not produce $WORK/navi-demo.gif" >&2
  exit 1
fi

cp -f "$WORK/navi-demo.gif" "$BRAND/navi-demo.gif"
[[ -f "$WORK/navi-demo.mp4" ]] && cp -f "$WORK/navi-demo.mp4" "$BRAND/navi-demo.mp4"

echo "==> Done"
ls -lh "$BRAND/navi-demo.gif" "$BRAND/navi-demo.mp4"
echo "Preview: imv $BRAND/navi-demo.gif"
