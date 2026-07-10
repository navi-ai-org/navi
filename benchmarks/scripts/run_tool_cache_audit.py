#!/usr/bin/env python3
"""Full NAVI tool-coverage + cache-break audit.

Runs through the LLM metrics proxy:

  1. Agent turn that must exercise every registered builtin tool
  2. Memory CLI: list / search / dream / distill / checkpoint
  3. Offline analysis of prefix breaks by lane + component

Usage:
  export OPENCODE_API_KEY=...
  export NAVI_BIN=target/release/navi
  python3 benchmarks/scripts/run_tool_cache_audit.py \\
    --out benchmarks/runs/agent-compare/tool-cache-audit.json

Env:
  NAVI_BENCH_BASE_URL   set by this script to the metrics proxy
  NAVI_BENCH_ALLOW_ALL_TOOLS=1  allows sleep / request_user_input in bench
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
PROXY_SCRIPT = ROOT / "benchmarks/scripts/llm_metrics_proxy.py"


def resolve_cmd_api_key() -> str | None:
    """Prefer real Command Code key; never use the local-proxy placeholder `cc-proxy`."""
    env = os.environ.get("CMD_API_KEY") or os.environ.get("COMMANDCODE_API_KEY")
    if env and env.strip() and env.strip() != "cc-proxy":
        return env.strip()
    auth_path = Path.home() / ".commandcode" / "auth.json"
    if auth_path.exists():
        try:
            data = json.loads(auth_path.read_text(encoding="utf-8"))
            key = data.get("apiKey") or data.get("api_key")
            if isinstance(key, str) and key.strip():
                return key.strip()
        except (OSError, json.JSONDecodeError):
            pass
    return env.strip() if env and env.strip() else None


def make_temp_config_for_model(provider: str, model: str) -> Path:
    """Clone user config with [model] overridden so memory dream uses commandcode."""
    src = Path.home() / ".config" / "navi" / "config.toml"
    tmp = Path(tempfile.mkdtemp(prefix="navi-audit-cfg-")) / "navi"
    tmp.mkdir(parents=True, exist_ok=True)
    lines = src.read_text(encoding="utf-8").splitlines(keepends=True) if src.exists() else []
    out: list[str] = []
    i = 0
    replaced = False
    while i < len(lines):
        stripped = lines[i].strip()
        if stripped == "[model]":
            out.append("[model]\n")
            out.append(f'provider = "{provider}"\n')
            out.append(f'name = "{model}"\n')
            out.append("\n")
            replaced = True
            i += 1
            # Skip existing keys under [model] until the next section header.
            while i < len(lines):
                s = lines[i].strip()
                if s.startswith("[") and s.endswith("]") and not s.startswith("[["):
                    break
                if s.startswith("[["):
                    break
                i += 1
            continue
        out.append(lines[i])
        i += 1
    if not replaced:
        out.insert(0, f'[model]\nprovider = "{provider}"\nname = "{model}"\n\n')
    (tmp / "config.toml").write_text("".join(out), encoding="utf-8")
    return tmp

# Canonical builtin + runtime-registered tools we expect the agent can see.
# Aliases counted separately if observed.
EXPECTED_BUILTIN = [
    # core IO
    "read",
    "read_file",
    "search",
    "grep",
    "fs_browser",
    "list_dir",
    "glob",
    "write",
    "write_file",
    "apply_patch",
    "bash",
    "process",
    # code / index
    "code",
    "code_edit",
    "code_exec",
    "ast_search",
    "symbol_goto",
    "symbol_references",
    "dependency_graph_query",
    "test_discovery",
    "ownership_churn_query",
    # orchestration
    "plan",
    "question",
    "set_goal",
    "create_goal",
    "get_goal",
    "update_goal",
    "update_goal_checklist",
    "subagent",
    "repo_explore",
    "branch_race_start",
    # memory
    "memory",
    "append_note",
    "history_ops",
    # system
    "runtime_info",
    "current_time",
    "sleep",
    "get_context_remaining",
    "request_user_input",
    "new_context_window",
    "tool_search",
    "sandbox",
    "package_manager",
    "verifier",
    "init_session",
    "mark_feature_done",
    "view_image",
    "inspect_image",
    "load_skill",
]

# Goal tools may be create_goal OR set_goal depending on registration.
GOAL_EQUIV = {"set_goal", "create_goal"}


def start_proxy(listen: str, upstream: str, log_path: Path) -> tuple[subprocess.Popen, str]:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.Popen(
        [
            sys.executable,
            str(PROXY_SCRIPT),
            "--listen",
            listen,
            "--upstream",
            upstream,
            "--log",
            str(log_path),
            "-v",
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    host, _, port = listen.partition(":")
    base = f"http://{host}:{port or '18765'}"
    deadline = time.time() + 20
    while time.time() < deadline:
        if proc.poll() is not None:
            out = proc.stdout.read() if proc.stdout else ""
            raise SystemExit(f"proxy exited: {out[-2000:]}")
        try:
            with urllib.request.urlopen(base + "/health", timeout=1) as r:
                if r.status == 200:
                    return proc, base
        except Exception:
            time.sleep(0.1)
    proc.terminate()
    raise SystemExit("proxy not healthy")


def proxy_metrics(base: str, reset: bool = False) -> dict[str, Any]:
    url = base.rstrip("/") + "/_metrics" + ("?reset=1" if reset else "")
    with urllib.request.urlopen(url, timeout=5) as r:
        return json.loads(r.read().decode())


def run(cmd: list[str], env: dict[str, str], timeout: int) -> subprocess.CompletedProcess:
    print(f"$ {' '.join(cmd)}", flush=True)
    return subprocess.run(
        cmd,
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        timeout=timeout,
    )


def tools_from_navi_result(payload: dict[str, Any]) -> list[str]:
    names: list[str] = []
    for case in payload.get("results") or []:
        for ev in case.get("events") or []:
            kind = ev.get("kind") or ev
            if not isinstance(kind, dict):
                continue
            for key in ("ToolRequested", "ToolStarted", "ToolCompleted"):
                if key in kind and isinstance(kind[key], dict):
                    n = kind[key].get("tool_name") or kind[key].get("name")
                    if n:
                        names.append(str(n))
            # flattened
            if kind.get("type") in ("ToolRequested", "ToolStarted") or "tool_name" in kind:
                n = kind.get("tool_name") or kind.get("name")
                if n:
                    names.append(str(n))
    return names


def coverage_report(observed: list[str]) -> dict[str, Any]:
    counts = Counter(observed)
    seen = set(counts)
    # goal equivalence
    missing = []
    for t in EXPECTED_BUILTIN:
        if t in seen:
            continue
        if t in GOAL_EQUIV and (seen & GOAL_EQUIV):
            continue
        # aliases
        if t == "read" and "read_file" in seen:
            continue
        if t == "write" and "write_file" in seen:
            continue
        if t == "search" and "grep" in seen:
            continue
        if t == "inspect_image" and "view_image" in seen:
            continue
        missing.append(t)
    return {
        "expected": EXPECTED_BUILTIN,
        "observed_counts": dict(sorted(counts.items())),
        "unique_observed": sorted(seen),
        "missing": missing,
        "coverage_rate": round((len(EXPECTED_BUILTIN) - len(missing)) / len(EXPECTED_BUILTIN), 4),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--out",
        type=Path,
        default=ROOT / "benchmarks/runs/agent-compare/tool-cache-audit.json",
    )
    ap.add_argument("--navi-bin", default=os.environ.get("NAVI_BIN", "navi"))
    ap.add_argument("--provider", default="opencode")
    ap.add_argument("--model", default="deepseek-v4-flash-free")
    ap.add_argument("--proxy-listen", default="127.0.0.1:18768")
    ap.add_argument(
        "--proxy-upstream",
        default=os.environ.get("NAVI_BENCH_PROXY_UPSTREAM", "https://opencode.ai/zen/v1"),
    )
    ap.add_argument(
        "--suite",
        type=Path,
        default=ROOT / "benchmarks/suites/tool-cache-audit",
    )
    ap.add_argument("--skip-agent", action="store_true")
    ap.add_argument("--skip-memory", action="store_true")
    ap.add_argument(
        "--lean-memory",
        action="store_true",
        default=True,
        help="Skip distill by default; only status/list/search/dream (cheaper).",
    )
    ap.add_argument(
        "--full-memory",
        action="store_true",
        help="Also run distill + checkpoint (more LLM calls / RAM).",
    )
    args = ap.parse_args()

    args.out.parent.mkdir(parents=True, exist_ok=True)
    events_log = args.out.with_name(args.out.stem + "-proxy-events.jsonl")
    if events_log.exists():
        events_log.unlink()

    report: dict[str, Any] = {
        "version": 1,
        "kind": "tool_cache_audit",
        "started_at": datetime.now(timezone.utc).isoformat(),
        "provider": args.provider,
        "model": args.model,
        "phases": {},
        "notes": [
            "Lean suite: capped turns/tools; no MCP/chrome; single repo_explore/subagent.",
            "Prefer prebuilt NAVI_BIN — avoid cargo release on low-RAM hosts.",
        ],
    }

    # Command Code: real key (not cc-proxy placeholder) + correct upstream default.
    if args.provider in ("commandcode", "command-code", "cc"):
        if "commandcode.ai" not in args.proxy_upstream and "opencode.ai" in args.proxy_upstream:
            args.proxy_upstream = "https://api.commandcode.ai"
        key = resolve_cmd_api_key()
        if key:
            os.environ["CMD_API_KEY"] = key
            print(f"CMD_API_KEY loaded (len={len(key)})", flush=True)
        else:
            print("WARNING: no CMD_API_KEY / ~/.commandcode/auth.json — auth may fail", flush=True)

    proxy_proc, proxy_base = start_proxy(args.proxy_listen, args.proxy_upstream, events_log)
    env = {
        **os.environ,
        "NAVI_BENCH_BASE_URL": proxy_base,
        "NAVI_BENCH_ALLOW_ALL_TOOLS": "1",
        "NAVI_NO_REGISTRY_UPDATE": "1",
        # Cap any accidental nested builds the agent might trigger.
        "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "1"),
        "CARGO_INCREMENTAL": os.environ.get("CARGO_INCREMENTAL", "0"),
    }
    if os.environ.get("CMD_API_KEY"):
        env["CMD_API_KEY"] = os.environ["CMD_API_KEY"]
    print(f"proxy={proxy_base} → {args.proxy_upstream}", flush=True)
    print(f"provider={args.provider} model={args.model}", flush=True)
    proxy_metrics(proxy_base, reset=True)
    temp_cfg: Path | None = None

    try:
        # ── Phase 1: agent full tool checklist ─────────────────────────
        if not args.skip_agent:
            print("\n=== Phase 1: agent tool checklist ===", flush=True)
            agent_json = args.out.with_name(args.out.stem + "-agent.json")
            t0 = time.monotonic()
            r = run(
                [
                    args.navi_bin,
                    "bench",
                    "run",
                    str(args.suite),
                    "--project",
                    str(ROOT),
                    "--provider",
                    args.provider,
                    "--model",
                    args.model,
                    "--auto-approve",
                    "--json",
                    "--output",
                    str(agent_json),
                ],
                env,
                timeout=1900,
            )
            wall = int((time.monotonic() - t0) * 1000)
            payload: dict[str, Any] = {}
            if agent_json.exists():
                payload = json.loads(agent_json.read_text())
            tools = tools_from_navi_result(payload)
            # also scan stdout events if empty
            cov = coverage_report(tools)
            phase_metrics = proxy_metrics(proxy_base)
            # Prefer navi-native cache metrics when proxy cannot parse provider SSE.
            navi_cache = {}
            if payload.get("results"):
                m = (payload["results"][0].get("metrics") or {})
                tin = int(m.get("input_tokens") or 0)
                cr = int(m.get("cache_read_tokens") or 0)
                navi_cache = {
                    "input_tokens": tin,
                    "output_tokens": int(m.get("output_tokens") or 0),
                    "cache_read_tokens": cr,
                    "cache_write_tokens": int(m.get("cache_write_tokens") or 0),
                    "cache_hit_rate": round(cr / tin, 4) if tin else None,
                    "tool_calls": m.get("tool_calls"),
                    "failed_tool_calls": m.get("failed_tool_calls"),
                    "turn_count": m.get("turn_count"),
                }
            report["phases"]["agent"] = {
                "wall_ms": wall,
                "exit": r.returncode,
                "stderr_tail": (r.stderr or "")[-2000:],
                "stdout_tail": (r.stdout or "")[-2000:],
                "tools_called": tools,
                "coverage": cov,
                "proxy_delta_snapshot": phase_metrics,
                "navi_metrics": navi_cache,
                "case_passed": bool((payload.get("results") or [{}])[0].get("passed"))
                if payload.get("results")
                else False,
                "case_error": (payload.get("results") or [{}])[0].get("error")
                if payload.get("results")
                else None,
            }
            print(
                f"  tools unique={len(set(tools))} coverage={cov['coverage_rate']} "
                f"missing={cov['missing']}",
                flush=True,
            )
            print(
                f"  proxy hit={phase_metrics.get('cache_hit_rate')} "
                f"lane_breaks={phase_metrics.get('lane_prefix_breaks')} "
                f"global_breaks={phase_metrics.get('prefix_breaks')}",
                flush=True,
            )

        # ── Phase 2: memory CLI (dream / distill / checkpoint) ─────────
        if not args.skip_memory:
            print("\n=== Phase 2: memory CLI (dream/distill/checkpoint) ===", flush=True)
            before = proxy_metrics(proxy_base)
            # Dream/distill use the active config model unless we override via temp XDG config.
            mem_env = dict(env)
            if args.provider not in ("opencode",):
                temp_cfg = make_temp_config_for_model(args.provider, args.model)
                # ProjectDirs uses XDG_CONFIG_HOME/navi or ~/.config/navi
                mem_env["XDG_CONFIG_HOME"] = str(temp_cfg.parent)
                print(
                    f"  memory LLM via temp config: {args.provider}:{args.model} "
                    f"(XDG_CONFIG_HOME={temp_cfg.parent})",
                    flush=True,
                )
            mem_cmds = [
                ([args.navi_bin, "memory", "status"], 60, False),
                ([args.navi_bin, "memory", "list"], 60, False),
                ([args.navi_bin, "memory", "search", "audit"], 60, False),
                (
                    [
                        args.navi_bin,
                        "memory",
                        "dream",
                        "--sessions",
                        "3",
                        "--instructions",
                        "Focus on tool-audit session facts; drop noise. Keep output small.",
                    ],
                    300,
                    True,  # needs LLM → use temp config
                ),
            ]
            if args.full_memory:
                mem_cmds.extend(
                    [
                        ([args.navi_bin, "memory", "checkpoint"], 180, True),
                        ([args.navi_bin, "memory", "distill"], 240, True),
                    ]
                )
            mem_results = []
            for cmd, to, use_llm_cfg in mem_cmds:
                t0 = time.monotonic()
                run_env = mem_env if use_llm_cfg else env
                try:
                    r = run(cmd, run_env, timeout=to)
                    mem_results.append(
                        {
                            "cmd": cmd,
                            "exit": r.returncode,
                            "wall_ms": int((time.monotonic() - t0) * 1000),
                            "stdout_tail": (r.stdout or "")[-1500:],
                            "stderr_tail": (r.stderr or "")[-1500:],
                        }
                    )
                    label = "dream" if "dream" in cmd else cmd[-1]
                    print(f"  {label} exit={r.returncode}", flush=True)
                    if r.returncode != 0:
                        print(f"    stderr: {(r.stderr or '')[:300]}", flush=True)
                except subprocess.TimeoutExpired:
                    mem_results.append({"cmd": cmd, "exit": -1, "error": "timeout"})
                    print(f"  TIMEOUT {cmd}", flush=True)
            after = proxy_metrics(proxy_base)
            report["phases"]["memory_cli"] = {
                "commands": mem_results,
                "proxy_before": before,
                "proxy_after": after,
            }

        # ── Phase 3: offline proxy analysis ────────────────────────────
        print("\n=== Phase 3: proxy analyze ===", flush=True)
        ar = run(
            [sys.executable, str(PROXY_SCRIPT), "--analyze", str(events_log)],
            env,
            timeout=60,
        )
        analysis: dict[str, Any] = {}
        # analyze prints JSON first then text; parse first JSON object
        try:
            text = ar.stdout or ""
            # find outermost JSON by braces
            start = text.find("{")
            if start >= 0:
                depth = 0
                end = None
                for i, ch in enumerate(text[start:], start):
                    if ch == "{":
                        depth += 1
                    elif ch == "}":
                        depth -= 1
                        if depth == 0:
                            end = i + 1
                            break
                if end:
                    analysis = json.loads(text[start:end])
        except json.JSONDecodeError:
            analysis = {"raw": (ar.stdout or "")[:5000]}
        report["phases"]["proxy_analysis"] = analysis
        report["proxy_final"] = proxy_metrics(proxy_base)
        report["proxy_events_log"] = str(events_log)
        report["ended_at"] = datetime.now(timezone.utc).isoformat()

        args.out.write_text(json.dumps(report, indent=2), encoding="utf-8")
        md = args.out.with_suffix(".md")
        write_markdown(md, report)
        print(f"\nWrote {args.out}", flush=True)
        print(f"Wrote {md}", flush=True)
        print_summary(report)
        return 0
    finally:
        proxy_proc.terminate()
        try:
            proxy_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proxy_proc.kill()
        if temp_cfg is not None:
            try:
                shutil.rmtree(temp_cfg.parent, ignore_errors=True)
            except Exception:
                pass


def print_summary(report: dict[str, Any]) -> None:
    print("\n========== TOOL + CACHE AUDIT SUMMARY ==========", flush=True)
    agent = report.get("phases", {}).get("agent") or {}
    cov = agent.get("coverage") or {}
    print(
        f"coverage={cov.get('coverage_rate')} missing={cov.get('missing')}",
        flush=True,
    )
    pf = report.get("proxy_final") or {}
    print(
        f"proxy hit={pf.get('cache_hit_rate')} lane_breaks={pf.get('lane_prefix_breaks')} "
        f"global_breaks={pf.get('prefix_breaks')} reqs={pf.get('requests')}",
        flush=True,
    )
    by_lane = pf.get("by_lane") or (report.get("phases", {}).get("proxy_analysis") or {}).get(
        "by_lane"
    ) or {}
    if by_lane:
        print("by lane:", flush=True)
        for lane, b in sorted(by_lane.items()):
            print(
                f"  {lane:<18} reqs={b.get('requests')} hit={b.get('cache_hit_rate')} "
                f"breaks={b.get('lane_prefix_breaks')}",
                flush=True,
            )
    blame = pf.get("component_break_counts") or {}
    solo = pf.get("component_solo_breaks") or {}
    if any(blame.values()):
        print("within-lane component blame:", flush=True)
        for c, n in sorted(blame.items(), key=lambda kv: -kv[1]):
            if n:
                print(f"  {c:<14} in_break={n} solo={solo.get(c, 0)}", flush=True)
    mem = report.get("phases", {}).get("memory_cli") or {}
    for c in mem.get("commands") or []:
        cmd = c.get("cmd") or []
        label = " ".join(cmd[1:3]) if len(cmd) >= 3 else str(cmd)
        print(f"memory: {label} exit={c.get('exit')}", flush=True)


def write_markdown(path: Path, report: dict[str, Any]) -> None:
    agent = report.get("phases", {}).get("agent") or {}
    cov = agent.get("coverage") or {}
    pf = report.get("proxy_final") or {}
    lines = [
        "# Tool + cache audit",
        "",
        f"- Started: `{report.get('started_at')}`",
        f"- Model: `{report.get('provider')}:{report.get('model')}`",
        f"- Coverage rate: `{cov.get('coverage_rate')}`",
        f"- Missing tools: `{', '.join(cov.get('missing') or []) or 'none'}`",
        f"- Proxy cache hit: `{pf.get('cache_hit_rate')}`",
        f"- Lane prefix breaks: `{pf.get('lane_prefix_breaks')}`",
        f"- Global prefix breaks (noise): `{pf.get('prefix_breaks')}`",
        f"- Events: `{report.get('proxy_events_log')}`",
        "",
        "## Observed tools",
        "",
    ]
    for name, n in (cov.get("observed_counts") or {}).items():
        lines.append(f"- `{name}` × {n}")
    lines.append("")
    lines.append("## By lane")
    lines.append("")
    by_lane = pf.get("by_lane") or {}
    lines.append("| Lane | Reqs | Hit | Breaks |")
    lines.append("|---|---:|---:|---:|")
    for lane, b in sorted(by_lane.items()):
        lines.append(
            f"| `{lane}` | {b.get('requests')} | {b.get('cache_hit_rate')} | "
            f"{b.get('lane_prefix_breaks')} |"
        )
    lines.append("")
    lines.append("## Component blame (within-lane)")
    lines.append("")
    lines.append("| Component | In break | Solo |")
    lines.append("|---|---:|---:|")
    blame = pf.get("component_break_counts") or {}
    solo = pf.get("component_solo_breaks") or {}
    for c, n in sorted(blame.items(), key=lambda kv: -kv[1]):
        lines.append(f"| `{c}` | {n} | {solo.get(c, 0)} |")
    lines.append("")
    lines.append("## Memory CLI")
    lines.append("")
    for c in (report.get("phases", {}).get("memory_cli") or {}).get("commands") or []:
        cmd = " ".join(c.get("cmd") or [])
        lines.append(f"- `{cmd}` exit={c.get('exit')} wall_ms={c.get('wall_ms')}")
        if c.get("stderr_tail"):
            lines.append(f"  - stderr: `{c['stderr_tail'][:200].replace(chr(10), ' ')}`")
    lines.append("")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    sys.exit(main())
