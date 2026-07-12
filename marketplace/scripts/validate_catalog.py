#!/usr/bin/env python3
"""Validate marketplace catalog.json structure (CI-friendly, no network)."""

from __future__ import annotations

import json
import sys
from pathlib import Path

KINDS = {"plugin", "skill", "mcp", "integration"}
REQUIRED = {"id", "name", "version", "publisher", "artifact_dir"}


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    catalog_path = root / "catalog.json"
    data = json.loads(catalog_path.read_text(encoding="utf-8"))
    if data.get("version") != 1:
        print("catalog.version must be 1", file=sys.stderr)
        return 1
    plugins = data.get("plugins")
    if not isinstance(plugins, list):
        print("catalog.plugins must be a list", file=sys.stderr)
        return 1
    seen: set[str] = set()
    for i, entry in enumerate(plugins):
        if not isinstance(entry, dict):
            print(f"plugins[{i}] must be an object", file=sys.stderr)
            return 1
        missing = REQUIRED - entry.keys()
        if missing:
            print(f"plugins[{i}] missing {sorted(missing)}", file=sys.stderr)
            return 1
        pid = entry["id"]
        if pid in seen:
            print(f"duplicate plugin id: {pid}", file=sys.stderr)
            return 1
        seen.add(pid)
        kind = entry.get("kind", "plugin")
        if kind not in KINDS:
            print(f"{pid}: invalid kind {kind!r}", file=sys.stderr)
            return 1
        if entry.get("wasm_hash") and not str(entry["wasm_hash"]).startswith("sha256:"):
            print(f"{pid}: wasm_hash must start with sha256:", file=sys.stderr)
            return 1
    print(f"ok: {len(plugins)} catalog entries")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
