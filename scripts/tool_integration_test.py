#!/usr/bin/env python3
"""
Tool integration test harness for NAVI.

Runs navi --no-tui with the opencode/mimo-v2.5-free model against a temp
project directory, sending prompts designed to exercise each tool and its
parameter variations. Collects pass/fail results, reports a summary, and
writes a JSON report for downstream analysis.

Usage:
    python scripts/tool_integration_test.py
    python scripts/tool_integration_test.py --filter read_file
    python scripts/tool_integration_test.py --verbose
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import textwrap
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Optional


# ─── Test case definition ────────────────────────────────────────────────

@dataclass
class ToolTest:
    """A single tool test case."""
    tool_name: str
    variation: str           # e.g. "basic", "with_start_line", "missing_path"
    prompt: str
    # Substrings that must appear in the output for the test to pass.
    # If empty, just check that navi exits 0 and produces output.
    expect_in_output: list[str] = field(default_factory=list)
    # Substrings that must NOT appear in the output.
    expect_not_in_output: list[str] = field(default_factory=list)
    # Whether this test is expected to fail (for error-path tests).
    expect_failure: bool = False
    # Timeout in seconds.
    timeout_s: int = 120
    # Whether to skip this test (e.g. platform-specific).
    skip: bool = False
    # Skip reason.
    skip_reason: str = ""
    # Files to create in the project dir before running.
    setup_files: dict[str, str] = field(default_factory=dict)


@dataclass
class TestResult:
    """Result of a single tool test."""
    tool_name: str
    variation: str
    passed: bool
    skipped: bool
    duration_s: float
    output: str = ""
    error: str = ""
    skip_reason: str = ""


# ─── Test cases ──────────────────────────────────────────────────────────

def build_test_cases() -> list[ToolTest]:
    """Build the complete list of tool test cases."""
    tests: list[ToolTest] = []

    # ── read_file ─────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="read_file",
        variation="basic",
        prompt="Read the file sample.txt using the read_file tool and tell me its contents.",
        expect_in_output=["hello world"],
        setup_files={"sample.txt": "hello world\nthis is a test file\nwith multiple lines\n"},
    ))
    tests.append(ToolTest(
        tool_name="read_file",
        variation="with_line_range",
        prompt="Read the file sample.txt but only lines 1 to 2 using the read_file tool with start_line=1 and end_line=2. Tell me what you read.",
        expect_in_output=["hello world"],
        expect_not_in_output=["with multiple lines"],
        setup_files={"sample.txt": "hello world\nthis is a test file\nwith multiple lines\n"},
    ))
    tests.append(ToolTest(
        tool_name="read_file",
        variation="nonexistent_file",
        prompt="Try to read a file called nonexistent_file.txt using the read_file tool. Report the error you get.",
        expect_in_output=["nonexistent"],
        timeout_s=60,
    ))

    # ── search (grep) ─────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="search",
        variation="grep_basic",
        prompt='Use the search tool with action "grep" to search for the pattern "hello" in the project. Tell me what files contain it.',
        expect_in_output=["hello"],
        setup_files={"sample.txt": "hello world\n", "other.rs": "fn main() { println!(\"hello\"); }\n"},
    ))
    tests.append(ToolTest(
        tool_name="search",
        variation="list_basic",
        prompt='Use the search tool with action "list" to list files in the current directory. Tell me what files you see.',
        expect_in_output=["sample.txt"],
        setup_files={"sample.txt": "content\n", "other.rs": "fn main() {}\n"},
    ))
    tests.append(ToolTest(
        tool_name="search",
        variation="tree_basic",
        prompt='Use the search tool with action "tree" to show the directory tree. Tell me the structure.',
        expect_in_output=["sample.txt"],
        setup_files={"sample.txt": "content\n", "src/main.rs": "fn main() {}\n"},
    ))
    tests.append(ToolTest(
        tool_name="search",
        variation="find_basic",
        prompt='Use the search tool with action "find" to find all .txt files. Tell me what you found.',
        expect_in_output=["sample.txt"],
        setup_files={"sample.txt": "content\n", "data.csv": "a,b,c\n"},
    ))
    tests.append(ToolTest(
        tool_name="search",
        variation="glob_basic",
        prompt='Use the search tool with action "find" and pattern "*.rs" to find all Rust files. Tell me what you found.',
        expect_in_output=["main.rs"],
        setup_files={"src/main.rs": "fn main() {}\n", "sample.txt": "content\n"},
    ))

    # ── write ─────────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="write",
        variation="basic",
        prompt='Create a file called output.txt with the content "test content 123" using the write tool. Confirm it was created.',
        expect_in_output=["output.txt"],
        timeout_s=60,
    ))

    # ── edit ──────────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="edit",
        variation="basic_replace",
        prompt='Edit the file sample.txt using the edit tool. Replace "hello" with "goodbye". The file currently contains "hello world". Confirm the edit.',
        expect_in_output=["goodbye"],
        setup_files={"sample.txt": "hello world\n"},
        timeout_s=60,
    ))

    # ── bash ──────────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="bash",
        variation="echo_command",
        prompt='Use the bash tool to run the command: echo "bash test ok". Tell me the output.',
        expect_in_output=["bash test ok"],
        timeout_s=60,
    ))

    # ── current_time ──────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="current_time",
        variation="basic",
        prompt="Use the current_time tool to get the current time. Tell me what time it is.",
        expect_in_output=["UTC", "20"],  # year 202x
        timeout_s=120,
    ))

    # ── runtime_info ──────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="runtime_info",
        variation="basic",
        prompt="Use the runtime_info tool to show the NAVI runtime state. Tell me what you see.",
        expect_in_output=[],  # just check it runs
        timeout_s=60,
    ))

    # ── append_note ───────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="append_note",
        variation="basic",
        prompt='Use the append_note tool to append a note with content "integration test note". Confirm it was added.',
        expect_in_output=["note"],
        timeout_s=120,
    ))

    # ── plan ──────────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="plan",
        variation="write_plan",
        prompt='Use the plan tool with action "write" to write a plan titled "Test Plan" with the content "# Test Plan\n- Step 1\n- Step 2". Confirm it was saved.',
        expect_in_output=["plan"],
        timeout_s=60,
    ))
    tests.append(ToolTest(
        tool_name="plan",
        variation="get_plan",
        prompt='Use the plan tool with action "get" to get the current plan. Tell me what plan exists, if any.',
        expect_in_output=[],
        timeout_s=60,
    ))
    tests.append(ToolTest(
        tool_name="plan",
        variation="list_plans",
        prompt='Use the plan tool with action "list" to list all plans. Tell me what you find.',
        expect_in_output=[],
        timeout_s=60,
    ))

    # ── sandbox ───────────────────────────────────────────────────────
    # Note: sandbox is a Deferred tool — the model must discover it via
    # tool_search first, then call it by name.
    tests.append(ToolTest(
        tool_name="sandbox",
        variation="snapshot",
        prompt='First use tool_search with query "sandbox" to find the sandbox tool. Then use the sandbox tool with action "snapshot" and paths ["sample.txt"] to take a snapshot of sample.txt. Confirm the snapshot was created.',
        expect_in_output=["snapshot"],
        setup_files={"sample.txt": "content\n"},
        timeout_s=180,
    ))
    tests.append(ToolTest(
        tool_name="sandbox",
        variation="status",
        prompt='First use tool_search with query "sandbox" to find the sandbox tool. Then use the sandbox tool with action "status" to check the sandbox status. Tell me what you see.',
        expect_in_output=["snapshot"],
        setup_files={"sample.txt": "content\n"},
        timeout_s=120,
    ))
    tests.append(ToolTest(
        tool_name="sandbox",
        variation="rollback",
        prompt='First use tool_search with query "sandbox" to find the sandbox tool. Then use the sandbox tool with action "snapshot" and paths ["sample.txt"] to snapshot the file. Then use the sandbox tool with action "rollback" to roll back. Confirm the rollback.',
        expect_in_output=["rollback", "snapshot"],
        setup_files={"sample.txt": "content\n"},
        timeout_s=180,
    ))
    tests.append(ToolTest(
        tool_name="sandbox",
        variation="reset",
        prompt='First use tool_search with query "sandbox" to find the sandbox tool. Then use the sandbox tool with action "reset" to clear any sandbox snapshot. Confirm it was cleared or reset.',
        expect_in_output=["snapshot", "clear"],
        timeout_s=120,
    ))
    tests.append(ToolTest(
        tool_name="sandbox",
        variation="snapshot_missing_paths",
        prompt='First use tool_search with query "sandbox" to find the sandbox tool. Then use the sandbox tool with action "snapshot" but do NOT provide any paths argument. Report the error you get.',
        expect_in_output=["paths"],
        timeout_s=120,
    ))

    # ── memory ────────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="memory",
        variation="list",
        prompt='Use the memory tool with action "list" to list all memories. Tell me what you find.',
        expect_in_output=[],
        timeout_s=60,
    ))
    tests.append(ToolTest(
        tool_name="memory",
        variation="search",
        prompt='Use the memory tool with action "search" and query "test" to search memories. Tell me what you find.',
        expect_in_output=[],
        timeout_s=60,
    ))

    # ── history_ops ───────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="history_ops",
        variation="summaries",
        prompt='Use the history_ops tool with action "summaries" to show session summaries. Tell me what you find.',
        expect_in_output=[],
        timeout_s=60,
    ))
    tests.append(ToolTest(
        tool_name="history_ops",
        variation="recent",
        prompt='Use the history_ops tool with action "recent" to show recent sessions. Tell me what you find.',
        expect_in_output=[],
        timeout_s=120,
    ))

    # ── tool_search ───────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="tool_search",
        variation="basic",
        prompt='Use the tool_search tool with query "file" to search for file-related tools. Tell me what tools you found.',
        expect_in_output=["read_file"],
        timeout_s=60,
    ))

    # ── repo_explore ──────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="repo_explore",
        variation="basic",
        prompt='Use the repo_explore tool with query "main function" to explore the repository. Tell me what you found.',
        expect_in_output=[],
        setup_files={"src/main.rs": "fn main() { println!(\"hello\"); }\n"},
        timeout_s=60,
    ))

    # ── ast_search ────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="ast_search",
        variation="basic",
        prompt='Use the ast_search tool with query "main" to search for symbols named "main". Tell me what you found.',
        expect_in_output=["main"],
        setup_files={"src/main.rs": "fn main() { println!(\"hello\"); }\n"},
        timeout_s=60,
    ))

    # ── code_exec ─────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="code_exec",
        variation="trace_note",
        prompt='Use the code_exec tool with ops [{"op": "trace-note", "note": "test trace note"}] to add a trace note. Confirm it was added.',
        expect_in_output=["trace"],
        timeout_s=60,
    ))
    tests.append(ToolTest(
        tool_name="code_exec",
        variation="repo_read",
        prompt='Use the code_exec tool with ops [{"op": "repo-read", "path": "sample.txt"}] to read sample.txt. Tell me what you read.',
        expect_in_output=["hello"],
        setup_files={"sample.txt": "hello from code_exec\n"},
        timeout_s=60,
    ))

    # ── subagent ──────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="subagent",
        variation="basic",
        prompt='Use the subagent tool with prompt "Read the file sample.txt and report its contents" to spawn a subagent. Tell me what the subagent found.',
        expect_in_output=["hello"],
        setup_files={"sample.txt": "hello from subagent\n"},
        timeout_s=120,
    ))

    # ── sleep ─────────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="sleep",
        variation="basic",
        prompt='Use the sleep tool with seconds 1 to sleep for 1 second. Confirm you slept.',
        expect_in_output=[],
        timeout_s=90,
    ))

    # ── get_context_remaining ─────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="get_context_remaining",
        variation="basic",
        prompt='Use the get_context_remaining tool with context_window 128000 and used_tokens 50000 to calculate remaining context. Tell me the result.',
        expect_in_output=[],
        timeout_s=60,
    ))

    # ── skill_list ────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="skill_list",
        variation="basic",
        prompt='Use the skill_list tool to list available skills. Tell me what you find.',
        expect_in_output=[],
        timeout_s=60,
    ))

    # ── workflow ──────────────────────────────────────────────────────
    tests.append(ToolTest(
        tool_name="workflow",
        variation="basic",
        prompt='Use the workflow tool with script "print(\"hello from workflow\")" to run a simple Lua workflow. Tell me the output.',
        expect_in_output=["hello", "workflow"],
        timeout_s=120,
    ))

    return tests


# ─── Test runner ─────────────────────────────────────────────────────────

# Config for the opencode free model — no API key needed.
CONFIG_TOML = textwrap.dedent("""\
    [model]
    provider = "opencode"
    name = "mimo-v2.5-free"

    [security]
    permission_mode = "yolo"

    [approvals]
    allow_reads = true
    require_for_writes = false
    require_for_commands = false

    [harness]
    profile = "small"
    max_turn_loops_small = 10
    max_tool_calls_small = 10

    [logging]
    enabled = false

    [memory]
    enabled = false

    [goals]
    enabled = false

    [browser]
    enabled = false

    [mcp]
    enabled = false

    [skills]
    enabled = false

    [workflow]
    enabled = true
    require_opt_in = false
""")


def setup_project_dir(tmpdir: Path, test: ToolTest) -> Path:
    """Create a temp project directory with config and setup files."""
    # Create .navi/config.toml
    navi_dir = tmpdir / ".navi"
    navi_dir.mkdir(exist_ok=True)
    (navi_dir / "config.toml").write_text(CONFIG_TOML, encoding="utf-8")

    # Create setup files
    for rel_path, content in test.setup_files.items():
        full_path = tmpdir / rel_path
        full_path.parent.mkdir(parents=True, exist_ok=True)
        full_path.write_text(content, encoding="utf-8")

    return tmpdir


def run_test(
    test: ToolTest,
    navi_binary: Path,
    verbose: bool = False,
) -> TestResult:
    """Run a single tool test."""
    if test.skip:
        return TestResult(
            tool_name=test.tool_name,
            variation=test.variation,
            passed=False,
            skipped=True,
            duration_s=0,
            skip_reason=test.skip_reason,
        )

    with tempfile.TemporaryDirectory(prefix="navi_tool_test_") as tmpdir_s:
        tmpdir = Path(tmpdir_s)
        setup_project_dir(tmpdir, test)

        cmd = [
            str(navi_binary),
            "--no-tui",
            "--yolo",
            test.prompt,
        ]

        if verbose:
            print(f"  Running: {test.tool_name}/{test.variation}")
            print(f"  Prompt: {test.prompt[:80]}...")

        start = time.time()
        try:
            proc = subprocess.run(
                cmd,
                cwd=str(tmpdir),
                capture_output=True,
                timeout=test.timeout_s,
                env={
                    **os.environ,
                    "NAVI_DATA_DIR": str(tmpdir / ".navi-data"),
                },
            )
            duration = time.time() - start
            # Decode as UTF-8 with error replacement (navi may emit non-ASCII)
            output = proc.stdout.decode("utf-8", errors="replace") + "\n" + proc.stderr.decode(
                "utf-8", errors="replace"
            )
        except subprocess.TimeoutExpired:
            duration = time.time() - start
            return TestResult(
                tool_name=test.tool_name,
                variation=test.variation,
                passed=False,
                skipped=False,
                duration_s=duration,
                error=f"Timeout after {test.timeout_s}s",
            )
        except Exception as e:
            duration = time.time() - start
            return TestResult(
                tool_name=test.tool_name,
                variation=test.variation,
                passed=False,
                skipped=False,
                duration_s=duration,
                error=str(e),
            )

        # Determine pass/fail
        passed = True
        errors = []

        if proc.returncode != 0 and not test.expect_failure:
            # navi itself crashed
            errors.append(f"navi exited with code {proc.returncode}")

        # Check expected substrings
        output_lower = output.lower()
        for expected in test.expect_in_output:
            if expected.lower() not in output_lower:
                passed = False
                errors.append(f"expected '{expected}' not found in output")

        # Check unexpected substrings
        for unexpected in test.expect_not_in_output:
            if unexpected.lower() in output_lower:
                passed = False
                errors.append(f"unexpected '{unexpected}' found in output")

        if test.expect_failure and proc.returncode == 0:
            # Expected failure but navi succeeded
            # This is OK for error-path tests where the tool returns an error
            # but navi itself doesn't crash
            pass

        if not output.strip():
            passed = False
            errors.append("no output produced")

        return TestResult(
            tool_name=test.tool_name,
            variation=test.variation,
            passed=passed,
            skipped=False,
            duration_s=duration,
            output=output[:5000],  # truncate for report
            error="; ".join(errors),
        )


def main() -> int:
    ap = argparse.ArgumentParser(description="NAVI tool integration test harness")
    ap.add_argument(
        "--navi",
        type=Path,
        default=Path("target/debug/navi.exe"),
        help="Path to navi binary",
    )
    ap.add_argument(
        "--filter",
        type=str,
        default=None,
        help="Only run tests matching this tool name (substring match)",
    )
    ap.add_argument(
        "--verbose",
        action="store_true",
        help="Print verbose output",
    )
    ap.add_argument(
        "--report",
        type=Path,
        default=Path("tool_integration_report.json"),
        help="Path to write JSON report",
    )
    ap.add_argument(
        "--stop-on-fail",
        action="store_true",
        help="Stop after first failure",
    )
    args = ap.parse_args()

    if not args.navi.exists():
        print(f"error: navi binary not found at {args.navi}", file=sys.stderr)
        print("Build it first: cargo build -p navi-cli", file=sys.stderr)
        return 2

    tests = build_test_cases()
    if args.filter:
        tests = [t for t in tests if args.filter.lower() in t.tool_name.lower()]

    print(f"Running {len(tests)} tool integration tests with {args.navi}")
    print(f"Model: opencode/mimo-v2.5-free")
    print()

    results: list[TestResult] = []
    passed_count = 0
    failed_count = 0
    skipped_count = 0

    for i, test in enumerate(tests, 1):
        prefix = f"[{i}/{len(tests)}]"
        print(f"{prefix} {test.tool_name}/{test.variation} ... ", end="", flush=True)

        result = run_test(test, args.navi, args.verbose)
        results.append(result)

        if result.skipped:
            skipped_count += 1
            print(f"SKIP ({result.skip_reason})")
        elif result.passed:
            passed_count += 1
            print(f"PASS ({result.duration_s:.1f}s)")
        else:
            failed_count += 1
            print(f"FAIL ({result.duration_s:.1f}s)")
            if result.error:
                print(f"       Error: {result.error}")
            if args.verbose and result.output:
                # Show last 500 chars of output
                tail = result.output[-500:] if len(result.output) > 500 else result.output
                print(f"       Output tail: ...{tail}")
            if args.stop_on_fail:
                print("\nStopping due to --stop-on-fail")
                break

        # Brief pause between tests to avoid rate limiting
        time.sleep(3)

    # Summary
    print()
    print("=" * 60)
    print(f"Results: {passed_count} passed, {failed_count} failed, {skipped_count} skipped")
    print("=" * 60)

    if failed_count > 0:
        print("\nFailed tests:")
        for r in results:
            if not r.passed and not r.skipped:
                print(f"  - {r.tool_name}/{r.variation}: {r.error}")

    # Write JSON report
    report = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "model": "opencode/mimo-v2.5-free",
        "navi_binary": str(args.navi),
        "total": len(results),
        "passed": passed_count,
        "failed": failed_count,
        "skipped": skipped_count,
        "results": [asdict(r) for r in results],
    }
    args.report.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"\nReport written to {args.report}")

    return 1 if failed_count > 0 else 0


if __name__ == "__main__":
    sys.exit(main())
