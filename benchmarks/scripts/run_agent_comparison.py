#!/usr/bin/env python3
"""Multi-agent tool-quality benchmark for NAVI.

Compares code agents on the same fixtures + verifier commands:

  * navi          — headless `navi bench` (native metrics)
  * opencode      — `opencode run` (OpenCode Zen / free flash or commandcode)
  * claude-code   — `claude -p` via cc-proxy (DeepSeek V4 flash as Anthropic model)
  * grok          — optional manual slot (records a pre-filled or skipped run)

Baseline model (default): DeepSeek V4 Flash free via OpenCode Zen for navi +
opencode; for Claude Code, `deepseek/deepseek-v4-flash` through cc-proxy
(http://localhost:19429).

Usage:
  python3 benchmarks/scripts/run_agent_comparison.py \\
    --suite benchmarks/suites/tool-quality \\
    --agents navi,opencode,claude-code \\
    --out benchmarks/runs/agent-compare/latest.json

  # Navi-only (reuses native bench runner, fastest for iteration):
  python3 benchmarks/scripts/run_agent_comparison.py --agents navi --cases tq-smoke-fix

  # Smoke:
  just bench-tool-quality-smoke
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]

# Default model matrix — same capability tier across agents.
DEFAULT_NAVI_PROVIDER = "opencode"
DEFAULT_NAVI_MODEL = "deepseek-v4-flash-free"
DEFAULT_OPENCODE_MODEL = "opencode/deepseek-v4-flash-free"
# Claude Code + cc-proxy (Command Code): use DeepSeek flash through Anthropic-compatible API.
DEFAULT_CC_PROXY_URL = "http://localhost:19429"
DEFAULT_CC_MODEL = "deepseek/deepseek-v4-flash"
DEFAULT_CC_API_KEY = "cc-proxy"


@dataclass
class CaseDef:
    id: str
    title: str
    fixture: Path
    task: str
    timeout_ms: int
    max_tool_calls: int | None
    max_turns: int | None
    verifier_cmd: str
    verifier_timeout_ms: int
    tags: list[str] = field(default_factory=list)
    metadata: dict[str, str] = field(default_factory=dict)
    source: Path | None = None


@dataclass
class CaseResult:
    case_id: str
    agent: str
    model: str
    passed: bool
    wall_time_ms: int
    tool_calls: int | None = None
    failed_tool_calls: int | None = None
    tool_call_names: list[str] = field(default_factory=list)
    files_changed: int = 0
    diff_lines_added: int = 0
    diff_lines_removed: int = 0
    input_tokens: int | None = None
    output_tokens: int | None = None
    total_tokens: int | None = None
    turn_count: int | None = None
    assistant_preview: str = ""
    error: str | None = None
    workspace: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)


def load_cases(suite_dir: Path, only: set[str] | None) -> list[CaseDef]:
    files = sorted(suite_dir.glob("*.toml")) + sorted(suite_dir.glob("*.json"))
    cases: list[CaseDef] = []
    for path in files:
        if path.suffix == ".toml":
            data = tomllib.loads(path.read_text(encoding="utf-8"))
        else:
            data = json.loads(path.read_text(encoding="utf-8"))
        case_id = data["id"]
        if only and case_id not in only:
            continue
        fixture = Path(data["fixture"])
        if not fixture.is_absolute():
            fixture = (ROOT / fixture).resolve()
        verifiers = data.get("verifiers") or []
        if not verifiers:
            raise SystemExit(f"{path}: missing verifiers")
        v0 = verifiers[0]
        cases.append(
            CaseDef(
                id=case_id,
                title=data.get("title", case_id),
                fixture=fixture,
                task=data["task"],
                timeout_ms=int(data.get("timeout_ms") or 600_000),
                max_tool_calls=data.get("max_tool_calls"),
                max_turns=data.get("max_turns"),
                verifier_cmd=v0["command"],
                verifier_timeout_ms=int(v0.get("timeout_ms") or 120_000),
                tags=list(data.get("tags") or []),
                metadata=dict(data.get("metadata") or {}),
                source=path,
            )
        )
    if not cases:
        raise SystemExit(f"no cases in {suite_dir}")
    return cases


def copy_fixture(fixture: Path, dest: Path) -> None:
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(
        fixture,
        dest,
        ignore=shutil.ignore_patterns("target", ".git", "__pycache__", "*.pyc"),
    )


def git_diff_stats(workspace: Path) -> tuple[int, int, int]:
    """Return (files_changed, lines_added, lines_removed) vs clean fixture via git if possible."""
    try:
        subprocess.run(
            ["git", "init", "-q"],
            cwd=workspace,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "add", "-A"],
            cwd=workspace,
            check=True,
            capture_output=True,
        )
        # Baseline commit of original fixture state is already the working tree
        # after agent edits; instead compare to initial commit made before agent.
        # Caller should call git_init_baseline first.
        r = subprocess.run(
            ["git", "diff", "--numstat", "HEAD"],
            cwd=workspace,
            capture_output=True,
            text=True,
            check=False,
        )
        added = removed = 0
        files = 0
        for line in r.stdout.splitlines():
            parts = line.split("\t")
            if len(parts) < 3:
                continue
            a, b = parts[0], parts[1]
            if a.isdigit() and b.isdigit():
                files += 1
                added += int(a)
                removed += int(b)
        return files, added, removed
    except Exception:
        return 0, 0, 0


def git_init_baseline(workspace: Path) -> None:
    subprocess.run(["git", "init", "-q"], cwd=workspace, check=True, capture_output=True)
    subprocess.run(
        ["git", "config", "user.email", "bench@navi.local"],
        cwd=workspace,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "navi-bench"],
        cwd=workspace,
        check=True,
        capture_output=True,
    )
    subprocess.run(["git", "add", "-A"], cwd=workspace, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-q", "-m", "baseline", "--allow-empty"],
        cwd=workspace,
        check=True,
        capture_output=True,
    )


def run_verifier(workspace: Path, cmd: str, timeout_ms: int) -> tuple[bool, str]:
    try:
        r = subprocess.run(
            cmd,
            shell=True,
            cwd=workspace,
            capture_output=True,
            text=True,
            timeout=timeout_ms / 1000.0,
        )
        ok = r.returncode == 0
        out = (r.stdout or "") + (r.stderr or "")
        return ok, out[-4000:]
    except subprocess.TimeoutExpired:
        return False, "verifier timeout"
    except Exception as e:
        return False, str(e)


def count_tools_from_navi_result(case_result: dict[str, Any]) -> tuple[int, int, list[str]]:
    metrics = case_result.get("metrics") or {}
    tools = int(metrics.get("tool_calls") or 0)
    failed = int(metrics.get("failed_tool_calls") or 0)
    names: list[str] = []
    for ev in case_result.get("events") or []:
        kind = ev.get("kind") or ev
        if isinstance(kind, dict):
            if "ToolRequested" in kind or kind.get("type") == "ToolRequested":
                inv = kind.get("ToolRequested") or kind
                if isinstance(inv, dict):
                    name = inv.get("tool_name") or inv.get("name")
                    if name:
                        names.append(str(name))
            # nested RuntimeEventKind enum serialization
            for k, v in kind.items():
                if k in ("ToolRequested", "ToolStarted") and isinstance(v, dict):
                    name = v.get("tool_name") or v.get("name")
                    if name:
                        names.append(str(name))
    return tools, failed, names


def run_navi(
    case: CaseDef,
    work_root: Path,
    provider: str,
    model: str,
    navi_bin: str,
) -> CaseResult:
    """Run a single case via `navi bench` on a one-case temp suite."""
    suite_dir = work_root / f"navi-suite-{case.id}"
    suite_dir.mkdir(parents=True, exist_ok=True)
    # Write a one-case suite that points at the original fixture path (navi copies it).
    src = case.source
    assert src is not None
    shutil.copy2(src, suite_dir / src.name)
    out_json = work_root / f"navi-{case.id}.json"
    cmd = [
        navi_bin,
        "bench",
        "run",
        str(suite_dir),
        "--project",
        str(ROOT),
        "--provider",
        provider,
        "--model",
        model,
        "--auto-approve",
        "--output",
        str(out_json),
        "--json",
    ]
    t0 = time.monotonic()
    try:
        r = subprocess.run(
            cmd,
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=case.timeout_ms / 1000.0 + 30,
            env={**os.environ, "NAVI_NO_REGISTRY_UPDATE": "1"},
        )
        wall = int((time.monotonic() - t0) * 1000)
        payload: dict[str, Any] = {}
        if out_json.exists():
            payload = json.loads(out_json.read_text(encoding="utf-8"))
        elif r.stdout.strip().startswith("{"):
            payload = json.loads(r.stdout)
        results = payload.get("results") or []
        if not results:
            return CaseResult(
                case_id=case.id,
                agent="navi",
                model=f"{provider}:{model}",
                passed=False,
                wall_time_ms=wall,
                error=(r.stderr or r.stdout or "navi produced no results")[-2000:],
            )
        cr = results[0]
        tools, failed, names = count_tools_from_navi_result(cr)
        m = cr.get("metrics") or {}
        return CaseResult(
            case_id=case.id,
            agent="navi",
            model=f"{provider}:{model}",
            passed=bool(cr.get("passed")),
            wall_time_ms=int(m.get("wall_time_ms") or wall),
            tool_calls=tools if tools else int(m.get("tool_calls") or 0),
            failed_tool_calls=failed if failed else int(m.get("failed_tool_calls") or 0),
            tool_call_names=names,
            files_changed=int(m.get("files_changed") or 0),
            diff_lines_added=int(m.get("diff_lines_added") or 0),
            diff_lines_removed=int(m.get("diff_lines_removed") or 0),
            input_tokens=m.get("input_tokens"),
            output_tokens=m.get("output_tokens"),
            total_tokens=m.get("total_tokens"),
            turn_count=m.get("turn_count"),
            assistant_preview=(cr.get("assistant_text") or "")[:500],
            error=cr.get("error"),
            workspace=cr.get("workspace"),
            raw={"navi_run": payload.get("run_id"), "exit": r.returncode},
        )
    except subprocess.TimeoutExpired:
        return CaseResult(
            case_id=case.id,
            agent="navi",
            model=f"{provider}:{model}",
            passed=False,
            wall_time_ms=case.timeout_ms,
            error="navi timeout",
        )
    except Exception as e:
        return CaseResult(
            case_id=case.id,
            agent="navi",
            model=f"{provider}:{model}",
            passed=False,
            wall_time_ms=int((time.monotonic() - t0) * 1000),
            error=str(e),
        )


def parse_opencode_json_events(stdout: str) -> tuple[int, int, list[str], str]:
    tools = 0
    failed = 0
    names: list[str] = []
    texts: list[str] = []
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        et = (ev.get("type") or ev.get("event") or "").lower()
        if "tool" in et:
            tools += 1
            name = (
                ev.get("name")
                or ev.get("tool")
                or (ev.get("part") or {}).get("tool")
                or (ev.get("part") or {}).get("name")
            )
            if name:
                names.append(str(name))
            if ev.get("error") or (ev.get("part") or {}).get("error"):
                failed += 1
        if et in ("text", "message", "assistant") or "text" in et:
            t = ev.get("text") or ev.get("content") or ""
            if isinstance(t, str) and t:
                texts.append(t)
        # opencode part events
        part = ev.get("part") or {}
        if isinstance(part, dict) and part.get("type") == "tool":
            tools += 1
            if part.get("tool"):
                names.append(str(part["tool"]))
    return tools, failed, names, "".join(texts)[-2000:]


def run_opencode(case: CaseDef, work_root: Path, model: str) -> CaseResult:
    workspace = work_root / f"opencode-{case.id}"
    copy_fixture(case.fixture, workspace)
    git_init_baseline(workspace)
    cmd = [
        "opencode",
        "run",
        "--pure",
        "--auto",
        "--format",
        "json",
        "-m",
        model,
        "--dir",
        str(workspace),
        case.task,
    ]
    t0 = time.monotonic()
    try:
        r = subprocess.run(
            cmd,
            cwd=workspace,
            capture_output=True,
            text=True,
            timeout=case.timeout_ms / 1000.0,
        )
        wall = int((time.monotonic() - t0) * 1000)
        tools, failed, names, text = parse_opencode_json_events(r.stdout + "\n" + r.stderr)
        files, add, rem = git_diff_stats(workspace)
        ok, vout = run_verifier(workspace, case.verifier_cmd, case.verifier_timeout_ms)
        err = None if ok else (vout or r.stderr or "verifier failed")[-2000:]
        if r.returncode != 0 and not ok:
            err = (err or "") + f"\nopencode exit={r.returncode}"
        return CaseResult(
            case_id=case.id,
            agent="opencode",
            model=model,
            passed=ok,
            wall_time_ms=wall,
            tool_calls=tools or None,
            failed_tool_calls=failed or None,
            tool_call_names=names,
            files_changed=files,
            diff_lines_added=add,
            diff_lines_removed=rem,
            assistant_preview=text[:500],
            error=err,
            workspace=str(workspace),
            raw={"exit": r.returncode},
        )
    except subprocess.TimeoutExpired:
        return CaseResult(
            case_id=case.id,
            agent="opencode",
            model=model,
            passed=False,
            wall_time_ms=case.timeout_ms,
            error="opencode timeout",
            workspace=str(workspace),
        )
    except FileNotFoundError:
        return CaseResult(
            case_id=case.id,
            agent="opencode",
            model=model,
            passed=False,
            wall_time_ms=0,
            error="opencode binary not found in PATH",
        )


def parse_claude_stream_json(stdout: str) -> tuple[int, int, list[str], str]:
    tools = 0
    failed = 0
    names: list[str] = []
    texts: list[str] = []
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        et = ev.get("type") or ""
        if et == "assistant":
            msg = ev.get("message") or {}
            for block in msg.get("content") or []:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use":
                    tools += 1
                    if block.get("name"):
                        names.append(str(block["name"]))
                if block.get("type") == "text" and block.get("text"):
                    texts.append(str(block["text"]))
        if et == "result":
            if ev.get("is_error"):
                failed += 1
            if ev.get("result"):
                texts.append(str(ev["result"]))
        if et == "content_block_start":
            cb = ev.get("content_block") or {}
            if cb.get("type") == "tool_use":
                tools += 1
                if cb.get("name"):
                    names.append(str(cb["name"]))
    return tools, failed, names, "".join(texts)[-2000:]


def run_claude_code(
    case: CaseDef,
    work_root: Path,
    model: str,
    proxy_url: str,
    api_key: str,
) -> CaseResult:
    workspace = work_root / f"claude-{case.id}"
    copy_fixture(case.fixture, workspace)
    git_init_baseline(workspace)
    env = {
        **os.environ,
        "ANTHROPIC_BASE_URL": proxy_url.rstrip("/"),
        "ANTHROPIC_API_KEY": api_key,
        # Some builds honor these for OpenAI-compat gateways:
        "ANTHROPIC_AUTH_TOKEN": api_key,
    }
    # Strip trailing /v1 if user passed full base — Claude SDK often wants host root.
    base = proxy_url.rstrip("/")
    if base.endswith("/v1"):
        env["ANTHROPIC_BASE_URL"] = base[: -len("/v1")]
    else:
        env["ANTHROPIC_BASE_URL"] = base

    cmd = [
        "claude",
        "-p",
        case.task,
        "--model",
        model,
        "--output-format",
        "stream-json",
        "--verbose",
        "--dangerously-skip-permissions",
        "--permission-mode",
        "bypassPermissions",
    ]
    t0 = time.monotonic()
    try:
        r = subprocess.run(
            cmd,
            cwd=workspace,
            capture_output=True,
            text=True,
            timeout=case.timeout_ms / 1000.0,
            env=env,
        )
        wall = int((time.monotonic() - t0) * 1000)
        tools, failed, names, text = parse_claude_stream_json(r.stdout + "\n" + r.stderr)
        files, add, rem = git_diff_stats(workspace)
        ok, vout = run_verifier(workspace, case.verifier_cmd, case.verifier_timeout_ms)
        err = None if ok else (vout or r.stderr or "verifier failed")[-2000:]
        if r.returncode != 0 and not ok:
            err = (err or "") + f"\nclaude exit={r.returncode}"
        return CaseResult(
            case_id=case.id,
            agent="claude-code",
            model=f"cc-proxy:{model}",
            passed=ok,
            wall_time_ms=wall,
            tool_calls=tools or None,
            failed_tool_calls=failed or None,
            tool_call_names=names,
            files_changed=files,
            diff_lines_added=add,
            diff_lines_removed=rem,
            assistant_preview=text[:500],
            error=err,
            workspace=str(workspace),
            raw={"exit": r.returncode, "proxy": env["ANTHROPIC_BASE_URL"]},
        )
    except subprocess.TimeoutExpired:
        return CaseResult(
            case_id=case.id,
            agent="claude-code",
            model=f"cc-proxy:{model}",
            passed=False,
            wall_time_ms=case.timeout_ms,
            error="claude timeout",
            workspace=str(workspace),
        )
    except FileNotFoundError:
        return CaseResult(
            case_id=case.id,
            agent="claude-code",
            model=f"cc-proxy:{model}",
            passed=False,
            wall_time_ms=0,
            error="claude binary not found in PATH",
        )


def score_agent(results: list[CaseResult]) -> dict[str, Any]:
    """Composite tool-quality score in [0, 100]."""
    n = len(results)
    if n == 0:
        return {"score": 0.0, "success_rate": 0.0}
    passed = sum(1 for r in results if r.passed)
    success_rate = passed / n
    tool_calls = [r.tool_calls for r in results if r.passed and r.tool_calls is not None]
    failed_tools = sum(r.failed_tool_calls or 0 for r in results)
    total_tools = sum(r.tool_calls or 0 for r in results)
    fail_rate = (failed_tools / total_tools) if total_tools else 0.0
    # Efficiency: fewer tools on successes is better (normalize loosely).
    avg_tools = (sum(tool_calls) / len(tool_calls)) if tool_calls else 0.0
    # Map avg tools 0..40 into efficiency 1..0
    efficiency = max(0.0, min(1.0, 1.0 - (avg_tools / 40.0))) if tool_calls else 0.5
    walls = [r.wall_time_ms for r in results if r.passed]
    avg_wall = (sum(walls) / len(walls)) if walls else 0.0
    # Map 0..10min into speed score
    speed = max(0.0, min(1.0, 1.0 - (avg_wall / 600_000.0))) if walls else 0.0

    score = 100.0 * (
        0.50 * success_rate
        + 0.20 * (1.0 - fail_rate)
        + 0.15 * efficiency
        + 0.15 * speed
    )
    return {
        "score": round(score, 2),
        "success_rate": round(success_rate, 4),
        "passed": passed,
        "total": n,
        "failed_tool_rate": round(fail_rate, 4),
        "avg_tool_calls_on_success": round(avg_tools, 2) if tool_calls else None,
        "avg_wall_ms_on_success": int(avg_wall) if walls else None,
        "total_tool_calls": total_tools,
        "total_failed_tool_calls": failed_tools,
    }


def print_table(report: dict[str, Any]) -> None:
    print("\n=== Tool-quality agent comparison ===")
    print(f"suite: {report['suite']}  model baseline: {report['baseline_model_note']}")
    print(f"started: {report['started_at']}")
    print()
    header = f"{'agent':<14} {'score':>7} {'pass':>8} {'tools':>7} {'failT':>6} {'avgT':>7} {'avgMs':>8}"
    print(header)
    print("-" * len(header))
    for agent, block in report["agents"].items():
        s = block["summary"]
        pass_s = f"{s['passed']}/{s['total']}"
        avg_t = s.get("avg_tool_calls_on_success")
        avg_ms = s.get("avg_wall_ms_on_success")
        print(
            f"{agent:<14} {s['score']:>7.1f} {pass_s:>8} "
            f"{s.get('total_tool_calls') or 0:>7} {s.get('total_failed_tool_calls') or 0:>6} "
            f"{(avg_t if avg_t is not None else '-'):>7} "
            f"{(avg_ms if avg_ms is not None else '-'):>8}"
        )
    print()
    # Per-case matrix
    case_ids = report["case_ids"]
    agents = list(report["agents"].keys())
    print("per-case pass (✓/✗):")
    print(f"{'case':<28} " + " ".join(f"{a[:10]:>10}" for a in agents))
    for cid in case_ids:
        row = [cid[:28]]
        for a in agents:
            found = next(
                (r for r in report["agents"][a]["results"] if r["case_id"] == cid),
                None,
            )
            if not found:
                row.append(f"{'?':>10}")
            else:
                mark = "✓" if found["passed"] else "✗"
                row.append(f"{mark:>10}")
        print(" ".join(f"{c:>10}" if i else f"{c:<28}" for i, c in enumerate(row)))
    print()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument(
        "--suite",
        type=Path,
        default=ROOT / "benchmarks/suites/tool-quality",
        help="Suite directory of .toml cases",
    )
    ap.add_argument(
        "--agents",
        default="navi,opencode,claude-code",
        help="Comma list: navi,opencode,claude-code,grok (grok is skip/manual)",
    )
    ap.add_argument("--cases", default="", help="Comma list of case ids (default: all)")
    ap.add_argument(
        "--out",
        type=Path,
        default=ROOT / "benchmarks/runs/agent-compare/latest.json",
    )
    ap.add_argument("--navi-bin", default=os.environ.get("NAVI_BIN", "navi"))
    ap.add_argument("--navi-provider", default=DEFAULT_NAVI_PROVIDER)
    ap.add_argument("--navi-model", default=DEFAULT_NAVI_MODEL)
    ap.add_argument("--opencode-model", default=DEFAULT_OPENCODE_MODEL)
    ap.add_argument("--claude-model", default=DEFAULT_CC_MODEL)
    ap.add_argument(
        "--cc-proxy-url",
        default=os.environ.get("CC_PROXY_URL", DEFAULT_CC_PROXY_URL),
        help="cc-proxy base URL (with or without /v1)",
    )
    ap.add_argument(
        "--cc-api-key",
        default=os.environ.get("CC_PROXY_API_KEY", DEFAULT_CC_API_KEY),
    )
    ap.add_argument(
        "--work-root",
        type=Path,
        default=None,
        help="Keep agent workspaces under this dir (default: temp)",
    )
    ap.add_argument("--keep-workspaces", action="store_true")
    args = ap.parse_args()

    only = {c.strip() for c in args.cases.split(",") if c.strip()} or None
    cases = load_cases(args.suite.resolve(), only)
    agents = [a.strip() for a in args.agents.split(",") if a.strip()]

    if args.work_root:
        work_root = args.work_root.resolve()
        work_root.mkdir(parents=True, exist_ok=True)
        cleanup = False
    else:
        td = tempfile.TemporaryDirectory(prefix="navi-agent-compare-")
        work_root = Path(td.name)
        cleanup = not args.keep_workspaces

    started = datetime.now(timezone.utc).isoformat()
    report: dict[str, Any] = {
        "version": 1,
        "kind": "tool_quality_agent_comparison",
        "suite": str(args.suite),
        "started_at": started,
        "baseline_model_note": (
            f"navi={args.navi_provider}:{args.navi_model}; "
            f"opencode={args.opencode_model}; "
            f"claude-code=cc-proxy({args.cc_proxy_url})→{args.claude_model}"
        ),
        "case_ids": [c.id for c in cases],
        "agents": {},
    }

    print(f"suite={args.suite} cases={len(cases)} agents={agents}", flush=True)
    print(f"work_root={work_root}", flush=True)

    try:
        for agent in agents:
            print(f"\n--- agent: {agent} ---", flush=True)
            results: list[CaseResult] = []
            for case in cases:
                print(f"  case {case.id} …", flush=True)
                if agent == "navi":
                    res = run_navi(
                        case, work_root, args.navi_provider, args.navi_model, args.navi_bin
                    )
                elif agent == "opencode":
                    res = run_opencode(case, work_root, args.opencode_model)
                elif agent in ("claude-code", "claude", "cc"):
                    res = run_claude_code(
                        case,
                        work_root,
                        args.claude_model,
                        args.cc_proxy_url,
                        args.cc_api_key,
                    )
                elif agent == "grok":
                    res = CaseResult(
                        case_id=case.id,
                        agent="grok",
                        model="manual/reference",
                        passed=False,
                        wall_time_ms=0,
                        error=(
                            "Manual/reference slot — run the case yourself and merge "
                            "results, or omit --agents grok for automated compare."
                        ),
                    )
                else:
                    res = CaseResult(
                        case_id=case.id,
                        agent=agent,
                        model="?",
                        passed=False,
                        wall_time_ms=0,
                        error=f"unknown agent {agent}",
                    )
                status = "PASS" if res.passed else "FAIL"
                print(
                    f"    → {status} wall={res.wall_time_ms}ms tools={res.tool_calls} "
                    f"err={(res.error or '')[:80]!r}",
                    flush=True,
                )
                results.append(res)

            report["agents"][agent] = {
                "summary": score_agent(results),
                "results": [asdict(r) for r in results],
            }
    finally:
        if cleanup and "td" in locals():
            td.cleanup()

    report["ended_at"] = datetime.now(timezone.utc).isoformat()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"\nWrote {args.out}", flush=True)
    print_table(report)

    # Also write a markdown summary next to JSON
    md_path = args.out.with_suffix(".md")
    lines = [
        "# Tool-quality agent comparison",
        "",
        f"- Suite: `{report['suite']}`",
        f"- Started: {report['started_at']}",
        f"- Baseline: {report['baseline_model_note']}",
        "",
        "| Agent | Score | Pass | Tool calls | Failed tools | Avg tools (ok) | Avg ms (ok) |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for agent, block in report["agents"].items():
        s = block["summary"]
        lines.append(
            f"| {agent} | {s['score']} | {s['passed']}/{s['total']} | "
            f"{s.get('total_tool_calls') or 0} | {s.get('total_failed_tool_calls') or 0} | "
            f"{s.get('avg_tool_calls_on_success') or '-'} | {s.get('avg_wall_ms_on_success') or '-'} |"
        )
    lines.append("")
    lines.append("## Scoring")
    lines.append("")
    lines.append(
        "Score = 50% success rate + 20% (1 − failed-tool rate) + "
        "15% tool efficiency (fewer tools on successes) + 15% speed."
    )
    lines.append("")
    lines.append("Primary axis: **quality of tool use** under a shared model tier "
                 "(DeepSeek V4 Flash free / flash via cc-proxy).")
    md_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"Wrote {md_path}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
