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
import urllib.request
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
    """Token accounting source: provider | estimated_stream | unknown."""
    token_source: str | None = None
    cache_read_tokens: int | None = None
    cache_write_tokens: int | None = None
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
            token_source="provider" if m.get("total_tokens") or m.get("input_tokens") else "unknown",
            cache_read_tokens=m.get("cache_read_tokens") or None,
            cache_write_tokens=m.get("cache_write_tokens") or None,
            turn_count=m.get("turn_count"),
            assistant_preview=(cr.get("assistant_text") or "")[:500],
            error=cr.get("error"),
            workspace=cr.get("workspace"),
            raw={
                "navi_run": payload.get("run_id"),
                "exit": r.returncode,
                "cache_hit_rate": (
                    round(m["cache_read_tokens"] / m["input_tokens"], 4)
                    if m.get("input_tokens") and m.get("cache_read_tokens") is not None
                    else None
                ),
            },
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


def _chars_to_tokens(n: int) -> int:
    """Rough token estimate when providers omit usage (≈4 chars/token)."""
    return max(0, (n + 3) // 4)


def parse_opencode_json_events(
    stdout: str,
) -> tuple[int, int, list[str], str, dict[str, Any]]:
    """Parse OpenCode --format json events.

    Token usage is reported on ``step_finish`` / ``step-finish`` parts as
    ``part.tokens.{input,output,total,reasoning,cache}``. Multi-step runs
    sum every finished step (billable input accumulates across steps).
    """
    tools = 0
    failed = 0
    names: list[str] = []
    texts: list[str] = []
    inp = out = total = reasoning = cache_read = cache_write = 0
    steps = 0
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        et = (ev.get("type") or ev.get("event") or "").lower()
        part = ev.get("part") if isinstance(ev.get("part"), dict) else {}
        part_type = str(part.get("type") or "").lower()

        if "tool" in et or part_type in ("tool", "tool-call", "tool_use"):
            tools += 1
            name = (
                ev.get("name")
                or ev.get("tool")
                or part.get("tool")
                or part.get("name")
            )
            if name:
                names.append(str(name))
            if ev.get("error") or part.get("error") or part.get("state") == "error":
                failed += 1
        if et in ("text", "message", "assistant") or "text" in et or part_type == "text":
            t = ev.get("text") or part.get("text") or ev.get("content") or ""
            if isinstance(t, str) and t:
                texts.append(t)

        # step_finish carries per-step token totals
        if et in ("step_finish", "step-finish") or part_type in (
            "step-finish",
            "step_finish",
        ):
            tok = part.get("tokens") or ev.get("tokens") or {}
            if isinstance(tok, dict) and tok:
                steps += 1
                inp += int(tok.get("input") or 0)
                out += int(tok.get("output") or 0)
                total += int(tok.get("total") or 0)
                reasoning += int(tok.get("reasoning") or 0)
                cache = tok.get("cache") or {}
                if isinstance(cache, dict):
                    cache_read += int(cache.get("read") or 0)
                    cache_write += int(cache.get("write") or 0)
            # also count tool parts nested in other events
        if part_type == "tool" and part.get("tool"):
            # may double-count if also in type; only if not already tool event
            if "tool" not in et:
                tools += 1
                names.append(str(part["tool"]))

    if total == 0 and (inp or out):
        total = inp + out + reasoning
    usage = {
        "input_tokens": inp or None,
        "output_tokens": out or None,
        "total_tokens": total or None,
        "reasoning_tokens": reasoning or None,
        "cache_read_tokens": cache_read or None,
        "cache_write_tokens": cache_write or None,
        "token_source": "provider" if (inp or out or total) else "unknown",
        "steps_with_usage": steps,
    }
    return tools, failed, names, "".join(texts)[-2000:], usage


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
        tools, failed, names, text, usage = parse_opencode_json_events(
            r.stdout + "\n" + r.stderr
        )
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
            input_tokens=usage.get("input_tokens"),
            output_tokens=usage.get("output_tokens"),
            total_tokens=usage.get("total_tokens"),
            token_source=usage.get("token_source"),
            cache_read_tokens=usage.get("cache_read_tokens"),
            cache_write_tokens=usage.get("cache_write_tokens"),
            assistant_preview=text[:500],
            error=err,
            workspace=str(workspace),
            raw={"exit": r.returncode, "usage": usage},
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


def parse_claude_stream_json(
    stdout: str, task: str = ""
) -> tuple[int, int, list[str], str, dict[str, Any]]:
    """Parse Claude Code ``stream-json`` events.

    Prefer provider ``usage`` on assistant/result messages. When the gateway
    (e.g. cc-proxy) reports zeros, fall back to a multi-turn stream estimate:
    cumulative context re-sent each API turn (chars/4), which is lower than
    real Anthropic-style usage because system/tool schemas are not in the stream.
    """
    tools = 0
    failed = 0
    names: list[str] = []
    texts: list[str] = []
    reported_in = reported_out = 0
    cache_read = cache_write = 0
    # Stream estimate accumulators
    context_chars = len(task)
    est_in = 0
    est_out = 0
    api_turns = 0

    def content_chars(blocks: Any) -> int:
        n = 0
        if isinstance(blocks, str):
            return len(blocks)
        if not isinstance(blocks, list):
            return 0
        for b in blocks:
            if not isinstance(b, dict):
                continue
            if b.get("type") == "text":
                n += len(str(b.get("text") or ""))
            elif b.get("type") == "tool_use":
                n += len(json.dumps(b.get("input") or {}, ensure_ascii=False))
                n += len(str(b.get("name") or ""))
            elif b.get("type") == "tool_result":
                c = b.get("content")
                if isinstance(c, str):
                    n += len(c)
                else:
                    n += len(json.dumps(c or "", ensure_ascii=False))
            else:
                n += len(json.dumps(b, ensure_ascii=False))
        return n

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
            blocks = msg.get("content") or []
            for block in blocks if isinstance(blocks, list) else []:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use":
                    tools += 1
                    if block.get("name"):
                        names.append(str(block["name"]))
                if block.get("type") == "text" and block.get("text"):
                    texts.append(str(block["text"]))
            u = msg.get("usage") or {}
            if isinstance(u, dict):
                reported_in += int(u.get("input_tokens") or 0)
                reported_out += int(u.get("output_tokens") or 0)
                cache_read += int(u.get("cache_read_input_tokens") or 0)
                cache_write += int(u.get("cache_creation_input_tokens") or 0)
            # multi-turn estimate: bill full context as input, this message as output
            out_c = content_chars(blocks)
            est_in += _chars_to_tokens(context_chars)
            est_out += _chars_to_tokens(out_c)
            context_chars += out_c
            api_turns += 1
        if et == "user":
            msg = ev.get("message") or {}
            blocks = msg.get("content") or []
            context_chars += content_chars(blocks)
        if et == "result":
            if ev.get("is_error"):
                failed += 1
            if ev.get("result"):
                texts.append(str(ev["result"]))
            u = ev.get("usage") or {}
            if isinstance(u, dict):
                # result usage is often cumulative; take max with sum of assistants
                ri = int(u.get("input_tokens") or 0)
                ro = int(u.get("output_tokens") or 0)
                if ri or ro:
                    reported_in = max(reported_in, ri)
                    reported_out = max(reported_out, ro)
                cache_read = max(
                    cache_read, int(u.get("cache_read_input_tokens") or 0)
                )
                cache_write = max(
                    cache_write, int(u.get("cache_creation_input_tokens") or 0)
                )
            mu = ev.get("modelUsage") or {}
            if isinstance(mu, dict):
                for _model, stats in mu.items():
                    if not isinstance(stats, dict):
                        continue
                    ri = int(stats.get("inputTokens") or stats.get("input_tokens") or 0)
                    ro = int(stats.get("outputTokens") or stats.get("output_tokens") or 0)
                    if ri or ro:
                        reported_in = max(reported_in, ri)
                        reported_out = max(reported_out, ro)
        if et == "content_block_start":
            cb = ev.get("content_block") or {}
            if cb.get("type") == "tool_use":
                tools += 1
                if cb.get("name"):
                    names.append(str(cb["name"]))

    if reported_in or reported_out:
        usage = {
            "input_tokens": reported_in,
            "output_tokens": reported_out,
            "total_tokens": reported_in + reported_out,
            "cache_read_tokens": cache_read or None,
            "cache_write_tokens": cache_write or None,
            "token_source": "provider",
            "api_turns": api_turns,
        }
    elif est_in or est_out:
        usage = {
            "input_tokens": est_in,
            "output_tokens": est_out,
            "total_tokens": est_in + est_out,
            "token_source": "estimated_stream",
            "api_turns": api_turns,
            "note": (
                "cc-proxy/Claude reported 0 usage; estimated from stream "
                "chars/4 with multi-turn context growth (excludes system/tool schemas)"
            ),
        }
    else:
        usage = {
            "input_tokens": None,
            "output_tokens": None,
            "total_tokens": None,
            "token_source": "unknown",
            "api_turns": api_turns,
        }
    return tools, failed, names, "".join(texts)[-2000:], usage


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
        tools, failed, names, text, usage = parse_claude_stream_json(
            r.stdout + "\n" + r.stderr, task=case.task
        )
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
            input_tokens=usage.get("input_tokens"),
            output_tokens=usage.get("output_tokens"),
            total_tokens=usage.get("total_tokens"),
            token_source=usage.get("token_source"),
            cache_read_tokens=usage.get("cache_read_tokens"),
            cache_write_tokens=usage.get("cache_write_tokens"),
            assistant_preview=text[:500],
            error=err,
            workspace=str(workspace),
            raw={
                "exit": r.returncode,
                "proxy": env["ANTHROPIC_BASE_URL"],
                "usage": usage,
            },
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
    tin = sum(r.input_tokens or 0 for r in results)
    tout = sum(r.output_tokens or 0 for r in results)
    ttot = sum(
        (r.total_tokens if r.total_tokens is not None else ((r.input_tokens or 0) + (r.output_tokens or 0)))
        for r in results
        if r.total_tokens is not None or r.input_tokens is not None or r.output_tokens is not None
    )
    cache_read = sum(r.cache_read_tokens or 0 for r in results)
    cache_write = sum(r.cache_write_tokens or 0 for r in results)
    token_cases = sum(
        1
        for r in results
        if r.total_tokens is not None or r.input_tokens is not None or r.output_tokens is not None
    )
    sources = sorted({r.token_source for r in results if r.token_source})
    cache_hit_rate = round(cache_read / tin, 4) if tin > 0 else None
    billable_input = max(0, tin - cache_read) if tin else None
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
        "input_tokens": tin or None,
        "output_tokens": tout or None,
        "total_tokens": ttot or None,
        "cache_read_tokens": cache_read or None,
        "cache_write_tokens": cache_write or None,
        "billable_input_tokens": billable_input,
        "cache_hit_rate": cache_hit_rate,
        "tokens_per_success": (round(ttot / passed, 1) if passed and ttot else None),
        "token_cases": token_cases,
        "token_sources": sources,
    }


def print_table(report: dict[str, Any]) -> None:
    print("\n=== Tool-quality agent comparison ===")
    print(f"suite: {report['suite']}  model baseline: {report['baseline_model_note']}")
    print(f"started: {report['started_at']}")
    if report.get("proxy_metrics"):
        pm = report["proxy_metrics"]
        print(
            f"proxy: reqs={pm.get('requests')} cache_hit={pm.get('cache_hit_rate')} "
            f"cache_read={pm.get('cache_read_tokens')} prefix_breaks={pm.get('prefix_breaks')} "
            f"billable_in={pm.get('billable_input_tokens')}"
        )
    print()
    header = (
        f"{'agent':<14} {'score':>7} {'pass':>8} {'tools':>7} {'failT':>6} "
        f"{'avgT':>7} {'avgMs':>8} {'tokIn':>10} {'cache%':>7} {'tokΣ':>10} {'src':>10}"
    )
    print(header)
    print("-" * len(header))
    for agent, block in report["agents"].items():
        s = block["summary"]
        pass_s = f"{s['passed']}/{s['total']}"
        avg_t = s.get("avg_tool_calls_on_success")
        avg_ms = s.get("avg_wall_ms_on_success")
        src = ",".join(s.get("token_sources") or []) or "-"
        if len(src) > 10:
            src = src[:9] + "…"
        hit = s.get("cache_hit_rate")
        hit_s = f"{hit*100:.1f}%" if isinstance(hit, (int, float)) else "-"
        print(
            f"{agent:<14} {s['score']:>7.1f} {pass_s:>8} "
            f"{s.get('total_tool_calls') or 0:>7} {s.get('total_failed_tool_calls') or 0:>6} "
            f"{(avg_t if avg_t is not None else '-'):>7} "
            f"{(avg_ms if avg_ms is not None else '-'):>8} "
            f"{(s.get('input_tokens') if s.get('input_tokens') is not None else '-'):>10} "
            f"{hit_s:>7} "
            f"{(s.get('total_tokens') if s.get('total_tokens') is not None else '-'):>10} "
            f"{src:>10}"
        )
    print()
    # Per-case matrix
    case_ids = report["case_ids"]
    agents = list(report["agents"].keys())
    print("per-case pass (✓/✗) and tokens:")
    print(f"{'case':<28} " + " ".join(f"{a[:12]:>12}" for a in agents))
    for cid in case_ids:
        cells = [f"{cid[:28]:<28}"]
        for a in agents:
            found = next(
                (r for r in report["agents"][a]["results"] if r["case_id"] == cid),
                None,
            )
            if not found:
                cells.append(f"{'?':>12}")
            else:
                mark = "✓" if found["passed"] else "✗"
                tok = found.get("total_tokens")
                if tok is None and (
                    found.get("input_tokens") is not None or found.get("output_tokens") is not None
                ):
                    tok = (found.get("input_tokens") or 0) + (found.get("output_tokens") or 0)
                tok_s = f"{tok}" if tok is not None else "-"
                cells.append(f"{mark} {tok_s:>9}"[:12].rjust(12))
        print(" ".join(cells))
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
    ap.add_argument(
        "--metrics-proxy",
        action="store_true",
        help=(
            "Start llm_metrics_proxy and route navi LLM traffic through it "
            "(sets NAVI_BENCH_BASE_URL). Measures cache hit rate + prefix breaks."
        ),
    )
    ap.add_argument(
        "--proxy-listen",
        default="127.0.0.1:18765",
        help="host:port for metrics proxy (default 127.0.0.1:18765)",
    )
    ap.add_argument(
        "--proxy-upstream",
        default=os.environ.get("NAVI_BENCH_PROXY_UPSTREAM", "https://opencode.ai/zen/v1"),
        help="Upstream OpenAI-compatible base URL for the metrics proxy",
    )
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

    proxy_proc: subprocess.Popen | None = None
    proxy_base: str | None = None
    proxy_log = args.out.parent / f"{args.out.stem}-proxy-events.jsonl"

    print(f"suite={args.suite} cases={len(cases)} agents={agents}", flush=True)
    print(f"work_root={work_root}", flush=True)

    try:
        if args.metrics_proxy:
            proxy_proc, proxy_base = start_metrics_proxy(
                listen=args.proxy_listen,
                upstream=args.proxy_upstream,
                log_path=proxy_log,
            )
            os.environ["NAVI_BENCH_BASE_URL"] = proxy_base
            report["metrics_proxy"] = {
                "listen": args.proxy_listen,
                "upstream": args.proxy_upstream,
                "base_url": proxy_base,
                "events_log": str(proxy_log),
            }
            print(f"metrics proxy: {proxy_base} → {args.proxy_upstream}", flush=True)
            # Reset counters so this run is clean
            fetch_proxy_metrics(proxy_base, reset=True)

        for agent in agents:
            print(f"\n--- agent: {agent} ---", flush=True)
            results: list[CaseResult] = []
            for case in cases:
                print(f"  case {case.id} …", flush=True)
                # Snapshot proxy metrics per-case when enabled
                proxy_before = (
                    fetch_proxy_metrics(proxy_base) if proxy_base and agent == "navi" else None
                )
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
                if proxy_base and agent == "navi" and proxy_before is not None:
                    proxy_after = fetch_proxy_metrics(proxy_base) or {}
                    delta = proxy_delta(proxy_before, proxy_after)
                    res.raw = {**(res.raw or {}), "proxy_delta": delta}
                    # Prefer proxy cache numbers when navi metrics are zero/missing
                    if not res.cache_read_tokens and delta.get("cache_read_tokens"):
                        res.cache_read_tokens = delta["cache_read_tokens"]
                    if not res.cache_write_tokens and delta.get("cache_write_tokens"):
                        res.cache_write_tokens = delta["cache_write_tokens"]
                    if not res.input_tokens and delta.get("input_tokens"):
                        res.input_tokens = delta["input_tokens"]
                        res.token_source = "proxy"
                    if not res.output_tokens and delta.get("output_tokens"):
                        res.output_tokens = delta["output_tokens"]
                    if res.input_tokens and res.output_tokens and not res.total_tokens:
                        res.total_tokens = res.input_tokens + res.output_tokens
                status = "PASS" if res.passed else "FAIL"
                hit = ""
                if res.input_tokens and res.cache_read_tokens is not None:
                    hit = f" cache%={100.0 * (res.cache_read_tokens or 0) / res.input_tokens:.1f}"
                print(
                    f"    → {status} wall={res.wall_time_ms}ms tools={res.tool_calls} "
                    f"tokIn={res.input_tokens}{hit} "
                    f"err={(res.error or '')[:80]!r}",
                    flush=True,
                )
                results.append(res)

            report["agents"][agent] = {
                "summary": score_agent(results),
                "results": [asdict(r) for r in results],
            }
    finally:
        if proxy_base:
            final_proxy = fetch_proxy_metrics(proxy_base)
            if final_proxy:
                report["proxy_metrics"] = final_proxy
                print(
                    f"\nproxy totals: reqs={final_proxy.get('requests')} "
                    f"cache_hit={final_proxy.get('cache_hit_rate')} "
                    f"prefix_breaks={final_proxy.get('prefix_breaks')} "
                    f"cache_read={final_proxy.get('cache_read_tokens')} "
                    f"input={final_proxy.get('input_tokens')}",
                    flush=True,
                )
                print(
                    f"lane_prefix_breaks={final_proxy.get('lane_prefix_breaks')} "
                    f"(global_noise={final_proxy.get('prefix_breaks')})",
                    flush=True,
                )
                by_lane = final_proxy.get("by_lane") or {}
                if by_lane:
                    print("by lane:", flush=True)
                    for lane, b in sorted(by_lane.items()):
                        print(
                            f"  {lane:<18} reqs={b.get('requests')} hit={b.get('cache_hit_rate')} "
                            f"breaks={b.get('lane_prefix_breaks')}",
                            flush=True,
                        )
                blame = final_proxy.get("component_break_counts") or {}
                solo = final_proxy.get("component_solo_breaks") or {}
                if any(blame.values()) or any(solo.values()):
                    print("within-lane break blame (component | solo):", flush=True)
                    for comp, n in sorted(blame.items(), key=lambda kv: -kv[1]):
                        if n or solo.get(comp):
                            print(
                                f"  {comp:<14} in_break={n}  solo={solo.get(comp, 0)}",
                                flush=True,
                            )
        if proxy_proc is not None:
            proxy_proc.terminate()
            try:
                proxy_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proxy_proc.kill()
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
        "| Agent | Score | Pass | Tools | Avg tools | Avg ms | Tok in | Cache hit | Cache read | Tok Σ | Source |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|",
    ]
    for agent, block in report["agents"].items():
        s = block["summary"]
        src = ",".join(s.get("token_sources") or []) or "-"
        hit = s.get("cache_hit_rate")
        hit_s = f"{hit*100:.1f}%" if isinstance(hit, (int, float)) else "-"
        lines.append(
            f"| {agent} | {s['score']} | {s['passed']}/{s['total']} | "
            f"{s.get('total_tool_calls') or 0} | "
            f"{s.get('avg_tool_calls_on_success') or '-'} | "
            f"{s.get('avg_wall_ms_on_success') or '-'} | "
            f"{s.get('input_tokens') or '-'} | "
            f"{hit_s} | "
            f"{s.get('cache_read_tokens') or '-'} | "
            f"{s.get('total_tokens') or '-'} | {src} |"
        )
    lines.append("")
    if report.get("proxy_metrics"):
        pm = report["proxy_metrics"]
        lines.append("## Proxy metrics (all LLM traffic)")
        lines.append("")
        lines.append(f"- Requests: `{pm.get('requests')}`")
        lines.append(f"- Cache hit rate: `{pm.get('cache_hit_rate')}`")
        lines.append(f"- Cache read tokens: `{pm.get('cache_read_tokens')}`")
        lines.append(f"- Cache write tokens: `{pm.get('cache_write_tokens')}`")
        lines.append(f"- Billable input (input − cache_read): `{pm.get('billable_input_tokens')}`")
        lines.append(
            f"- Lane prefix breaks (real mid-session): `{pm.get('lane_prefix_breaks')}`"
        )
        lines.append(
            f"- Global prefix breaks (includes main↔subagent noise): `{pm.get('prefix_breaks')}`"
        )
        lines.append(f"- Last prefix hash: `{pm.get('last_prefix_hash')}`")
        by_lane = pm.get("by_lane") or {}
        if by_lane:
            lines.append("")
            lines.append("### By request lane")
            lines.append("")
            lines.append("| Lane | Reqs | Cache hit | Lane breaks |")
            lines.append("|---|---:|---:|---:|")
            for lane, b in sorted(by_lane.items()):
                lines.append(
                    f"| `{lane}` | {b.get('requests')} | {b.get('cache_hit_rate')} | "
                    f"{b.get('lane_prefix_breaks')} |"
                )
        blame = pm.get("component_break_counts") or {}
        solo = pm.get("component_solo_breaks") or {}
        if any(blame.values()) or any(solo.values()):
            lines.append("")
            lines.append("### Within-lane break blame by component")
            lines.append("")
            lines.append("| Component | In break | Solo break |")
            lines.append("|---|---:|---:|")
            for comp, n in sorted(blame.items(), key=lambda kv: -kv[1]):
                lines.append(f"| `{comp}` | {n} | {solo.get(comp, 0)} |")
            lines.append("")
            lines.append(
                "Components: `instructions`, `system`, `developer` (AGENTS/memory/skills), "
                "`tools`, `tools_names`, `first_user`. Lanes separate main agent from "
                "repo_explore / memory_extract / subagent traffic."
            )
        if report.get("metrics_proxy", {}).get("events_log"):
            lines.append(f"- Events log: `{report['metrics_proxy']['events_log']}`")
            lines.append(
                f"- Analyze: `python3 benchmarks/scripts/llm_metrics_proxy.py "
                f"--analyze {report['metrics_proxy']['events_log']}`"
            )
        lines.append("")
    lines.append("## Tokens")
    lines.append("")
    lines.append(
        "- **navi**: provider usage from runtime `TokensUpdated` / bench metrics "
        "(includes `cache_read_tokens` / `cache_write_tokens`)."
    )
    lines.append(
        "- **metrics proxy** (optional): reverse-proxies the provider and "
        "independently sums usage + detects prompt-prefix breaks."
    )
    lines.append(
        "- **opencode**: sum of `step_finish.part.tokens` (input/output/total per step)."
    )
    lines.append(
        "- **claude-code**: provider usage when non-zero; if the gateway "
        "(cc-proxy) reports zeros, **estimated_stream** = multi-turn "
        "context growth at ~4 chars/token (excludes system/tool schemas)."
    )
    lines.append("")
    lines.append("## Scoring")
    lines.append("")
    lines.append(
        "Score = 50% success rate + 20% (1 − failed-tool rate) + "
        "15% tool efficiency (fewer tools on successes) + 15% speed."
    )
    lines.append("")
    lines.append(
        "Primary axis: **quality of tool use** under a shared model tier "
        "(DeepSeek V4 Flash free / flash via cc-proxy)."
    )
    md_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"Wrote {md_path}", flush=True)
    return 0


def start_metrics_proxy(
    listen: str,
    upstream: str,
    log_path: Path,
) -> tuple[subprocess.Popen, str]:
    script = ROOT / "benchmarks/scripts/llm_metrics_proxy.py"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.Popen(
        [
            sys.executable,
            str(script),
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
    host, _, port_s = listen.partition(":")
    base = f"http://{host}:{port_s or '18765'}"
    # Wait for health
    deadline = time.time() + 15
    last_err = ""
    while time.time() < deadline:
        if proc.poll() is not None:
            out = ""
            if proc.stdout:
                out = proc.stdout.read() or ""
            raise SystemExit(f"metrics proxy exited early: {out[-2000:]}")
        try:
            with urllib.request.urlopen(base + "/health", timeout=1) as r:
                if r.status == 200:
                    return proc, base
        except Exception as e:
            last_err = str(e)
            time.sleep(0.15)
    proc.terminate()
    raise SystemExit(f"metrics proxy failed to become healthy: {last_err}")


def fetch_proxy_metrics(base: str | None, reset: bool = False) -> dict[str, Any] | None:
    if not base:
        return None
    url = base.rstrip("/") + "/_metrics" + ("?reset=1" if reset else "")
    try:
        with urllib.request.urlopen(url, timeout=5) as r:
            return json.loads(r.read().decode("utf-8"))
    except Exception:
        return None


def proxy_delta(before: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    keys = (
        "requests",
        "errors",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "prefix_breaks",
        "billable_input_tokens",
    )
    out: dict[str, Any] = {}
    for k in keys:
        try:
            out[k] = int(after.get(k) or 0) - int(before.get(k) or 0)
        except (TypeError, ValueError):
            out[k] = after.get(k)
    inp = out.get("input_tokens") or 0
    cr = out.get("cache_read_tokens") or 0
    out["cache_hit_rate"] = round(cr / inp, 4) if inp else None
    # Per-component break deltas
    before_c = before.get("component_break_counts") or {}
    after_c = after.get("component_break_counts") or {}
    before_s = before.get("component_solo_breaks") or {}
    after_s = after.get("component_solo_breaks") or {}
    out["component_break_counts"] = {
        k: int(after_c.get(k) or 0) - int(before_c.get(k) or 0)
        for k in set(before_c) | set(after_c)
    }
    out["component_solo_breaks"] = {
        k: int(after_s.get(k) or 0) - int(before_s.get(k) or 0)
        for k in set(before_s) | set(after_s)
    }
    try:
        out["lane_prefix_breaks"] = int(after.get("lane_prefix_breaks") or 0) - int(
            before.get("lane_prefix_breaks") or 0
        )
    except (TypeError, ValueError):
        out["lane_prefix_breaks"] = after.get("lane_prefix_breaks")
    return out


if __name__ == "__main__":
    sys.exit(main())
