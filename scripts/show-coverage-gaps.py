#!/usr/bin/env python3
"""Show uncovered lines for specific files in an lcov.info."""
import sys
from pathlib import Path


def parse_lcov_missing(path, target_suffix):
    current = None
    missing = []
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if line.startswith("SF:"):
                p = line[3:].replace("\\", "/")
                current = p if p.endswith(target_suffix) else None
            elif current and line.startswith("DA:"):
                parts = line[3:].split(",")
                if len(parts) == 2:
                    lineno = int(parts[0])
                    count = int(parts[1])
                    if count == 0:
                        missing.append(lineno)
            elif line == "end_of_record":
                if current and missing:
                    return current, missing
                current = None
                missing = []
    return None, []


def group_lines(lines):
    groups = []
    start = None
    prev = None
    for ln in lines:
        if start is None:
            start = ln
        elif ln != prev + 1:
            groups.append((start, prev))
            start = ln
        prev = ln
    if start is not None:
        groups.append((start, prev))
    return groups


def main():
    lcov = sys.argv[1] if len(sys.argv) > 1 else "coverage/lcov-core-cu.info"
    targets = sys.argv[2:] if len(sys.argv) > 2 else [
        "computer_use.rs",
        "inspect.rs",
        "open_app.rs",
        "input.rs",
        "capture.rs",
    ]

    for target in targets:
        path, missing = parse_lcov_missing(lcov, target)
        if path is None:
            print(f"\n{target}: not found in {lcov}")
            continue
        short = path.split("crates/")[-1] if "crates/" in path else path
        print(f"\n{short}: {len(missing)} uncovered lines")
        groups = group_lines(missing)
        for s, e in groups:
            if s == e:
                print(f"  L{s}")
            else:
                print(f"  L{s}-{e}")


if __name__ == "__main__":
    main()
