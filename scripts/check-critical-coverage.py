#!/usr/bin/env python3
"""Fail if critical navi-core production paths have insufficient line coverage.

Parses an lcov.info produced by `cargo llvm-cov` and checks per-file line
floors (and optional function hit counts). Designed for CI: small surface,
no global 80% gate that mocks can game.

Usage:
  scripts/check-critical-coverage.py coverage/lcov-core.info
  scripts/check-critical-coverage.py coverage/lcov-core.info --list
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field
from pathlib import Path


# Relative path suffixes (matched against SF: records) → minimum line % floor.
# Floors sit below measured baseline (2026-07 dry-run) with headroom so CI is not
# flaky, but still fail hard if production-path tests are removed (→ ~0%).
# backends/plan are well unit-tested; subagent/update mix large runtime/OS paths.
CRITICAL_FILES: dict[str, float] = {
    "crates/navi-core/src/tool/builtin/workflow/backends.rs": 50.0,  # ~92% baseline
    "crates/navi-core/src/tool/builtin/subagent.rs": 30.0,  # ~38% baseline (large runtime)
    "crates/navi-core/src/update.rs": 30.0,  # ~40% baseline (installer OS branches)
    "crates/navi-core/src/tool/builtin/plan.rs": 50.0,  # ~87% baseline
}

# Function name substrings that must have been executed at least once.
# Names are demangled-ish llvm-cov FN labels (Rust may mangle); we match
# case-sensitive substrings against FNDA records.
CRITICAL_FUNCTIONS: list[str] = [
    "build_subagent_bridge_input",
    "run_silent",
]


@dataclass
class FileCov:
    path: str
    lines_found: int = 0
    lines_hit: int = 0
    # function_name → hit count (from FNDA)
    functions: dict[str, int] = field(default_factory=dict)

    @property
    def line_pct(self) -> float:
        if self.lines_found <= 0:
            return 0.0
        return 100.0 * self.lines_hit / self.lines_found


def parse_lcov(path: Path) -> list[FileCov]:
    files: list[FileCov] = []
    current: FileCov | None = None
    with path.open(encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line = raw.rstrip("\n")
            if line.startswith("SF:"):
                current = FileCov(path=line[3:])
            elif current is None:
                continue
            elif line.startswith("LF:"):
                current.lines_found = int(line[3:] or "0")
            elif line.startswith("LH:"):
                current.lines_hit = int(line[3:] or "0")
            elif line.startswith("FNDA:"):
                # FNDA:<hits>,<name>
                body = line[5:]
                comma = body.find(",")
                if comma < 0:
                    continue
                hits_s, name = body[:comma], body[comma + 1 :]
                try:
                    hits = int(hits_s)
                except ValueError:
                    hits = 0
                current.functions[name] = current.functions.get(name, 0) + hits
            elif line == "end_of_record":
                files.append(current)
                current = None
    return files


def normalize_match(sf: str, suffix: str) -> bool:
    """Match lcov SF path against a repo-relative suffix (handles abs paths)."""
    sf_n = sf.replace("\\", "/")
    suf_n = suffix.replace("\\", "/")
    return sf_n.endswith(suf_n) or sf_n.endswith("/" + suf_n) or suf_n in sf_n


def find_file(files: list[FileCov], suffix: str) -> FileCov | None:
    matches = [f for f in files if normalize_match(f.path, suffix)]
    if not matches:
        return None
    # Prefer the longest path match (most specific).
    matches.sort(key=lambda f: len(f.path), reverse=True)
    return matches[0]


def function_hits(files: list[FileCov], needle: str) -> tuple[int, list[str]]:
    total = 0
    names: list[str] = []
    for f in files:
        for name, hits in f.functions.items():
            if needle in name:
                total += hits
                names.append(f"{name} ({hits} hits in {f.path})")
    return total, names


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("lcov", type=Path, help="Path to lcov.info from cargo llvm-cov")
    ap.add_argument(
        "--list",
        action="store_true",
        help="List coverage for critical files and exit 0 (no gate)",
    )
    ap.add_argument(
        "--floor",
        type=float,
        default=None,
        help="Override all per-file floors with this percent (debug)",
    )
    args = ap.parse_args()

    if not args.lcov.is_file():
        print(f"error: lcov file not found: {args.lcov}", file=sys.stderr)
        return 2

    files = parse_lcov(args.lcov)
    if not files:
        print(f"error: no SF records in {args.lcov}", file=sys.stderr)
        return 2

    print(f"Parsed {len(files)} file records from {args.lcov}")
    print()
    print("Critical path line coverage:")
    print(f"{'file':<70} {'hit':>6} {'found':>6} {'pct':>7}  floor")
    print("-" * 100)

    failures: list[str] = []

    for suffix, floor in CRITICAL_FILES.items():
        if args.floor is not None:
            floor = args.floor
        cov = find_file(files, suffix)
        if cov is None:
            pct_s = "MISSING"
            hit = found = 0
            ok = False
            failures.append(f"{suffix}: missing from coverage report (0%)")
        else:
            hit, found = cov.lines_hit, cov.lines_found
            pct = cov.line_pct
            pct_s = f"{pct:5.1f}%"
            ok = found > 0 and pct + 1e-9 >= floor
            if found == 0:
                failures.append(f"{suffix}: LF=0 (no instrumented lines)")
            elif not ok:
                failures.append(
                    f"{suffix}: {pct:.1f}% < {floor:.0f}% floor "
                    f"({hit}/{found} lines)"
                )
        status = "ok" if ok else "FAIL"
        print(f"{suffix:<70} {hit:6d} {found:6d} {pct_s:>7}  {floor:4.0f}%  [{status}]")

    print()
    print("Critical functions (must be hit at least once):")
    for needle in CRITICAL_FUNCTIONS:
        hits, names = function_hits(files, needle)
        if hits <= 0:
            print(f"  FAIL  {needle}: 0 hits")
            failures.append(f"function {needle!r}: 0 hits (production path untested)")
        else:
            print(f"  ok    {needle}: {hits} hit(s)")
            if args.list:
                for n in names[:5]:
                    print(f"          {n}")

    print()
    if args.list:
        return 0

    if failures:
        print("Coverage gate FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        print(
            "\nRaise coverage on the listed production paths, or temporarily "
            "adjust floors in scripts/check-critical-coverage.py if intentional.",
            file=sys.stderr,
        )
        return 1

    print("Coverage gate passed (critical paths).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
