#!/usr/bin/env python3
"""LLM metrics reverse proxy for NAVI / agent benchmarks.

All OpenAI-compatible (and Anthropic-style) traffic can be routed through this
proxy so the bench observes:

  * request/response counts and latency
  * provider usage: input / output / cache_read / cache_write tokens
  * **cache hit rate** = cache_read / input (when input > 0)
  * **prefix breaks** with per-component blame:
      instructions | system | developer | tools | tools_names | first_user
    so mid-case cache invalidation can be attributed precisely

Endpoints:
  GET  /health          → {"ok": true}
  GET  /_metrics        → JSON aggregate (+ optional ?reset=1)
  POST /_metrics/reset  → clear counters
  GET  /_metrics/events → last N request events
  *any other path*      → reverse-proxied to --upstream

Example:
  python3 benchmarks/scripts/llm_metrics_proxy.py \\
    --listen 127.0.0.1:18765 \\
    --upstream https://opencode.ai/zen/v1 \\
    --log benchmarks/runs/agent-compare/proxy-events.jsonl

  NAVI_BENCH_BASE_URL=http://127.0.0.1:18765 navi bench run ...

Analyze a JSONL log after a run:
  python3 benchmarks/scripts/llm_metrics_proxy.py --analyze path/to/events.jsonl
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import threading
import time
import urllib.error
import urllib.request
from collections import deque
from dataclasses import asdict, dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import parse_qs, urlparse


# ── metrics store ────────────────────────────────────────────────────────────


# Component names tracked for cache-prefix attribution.
PREFIX_COMPONENTS = (
    "instructions",  # OpenAI Responses `instructions` field
    "system",  # role=system messages
    "developer",  # role=developer messages (AGENTS, memory, skills, plan-mode)
    "tools",  # full tool JSON schemas
    "tools_names",  # tool name list only (schema-detail changes vs add/remove)
    "first_user",  # first user message prefix (bench preamble / task)
)


@dataclass
class RequestEvent:
    ts: float
    method: str
    path: str
    status: int
    latency_ms: int
    model: str | None = None
    stream: bool = False
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_write_tokens: int = 0
    request_bytes: int = 0
    response_bytes: int = 0
    prefix_hash: str | None = None
    prefix_break: bool = False
    # True only when this request continues the same lane as the previous
    # request in that lane and the prefix changed (not cross-lane noise).
    lane_prefix_break: bool = False
    lane: str = "main"
    # Per-component short hashes (16 hex chars) and which ones changed.
    component_hashes: dict[str, str | None] = field(default_factory=dict)
    components_changed: list[str] = field(default_factory=list)
    component_meta: dict[str, Any] = field(default_factory=dict)
    error: str | None = None


@dataclass
class MetricsStore:
    lock: threading.Lock = field(default_factory=threading.Lock)
    started_at: float = field(default_factory=time.time)
    requests: int = 0
    errors: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_write_tokens: int = 0
    total_latency_ms: int = 0
    prefix_breaks: int = 0
    # Breaks computed within each lane (ignores main↔subagent interleaving).
    lane_prefix_breaks: int = 0
    last_prefix_hash: str | None = None
    last_component_hashes: dict[str, str | None] = field(default_factory=dict)
    # Per-lane last state for accurate mid-session break detection.
    last_by_lane: dict[str, dict[str, Any]] = field(default_factory=dict)
    # How often each component was among the changed set on a *lane* break.
    component_break_counts: dict[str, int] = field(
        default_factory=lambda: {c: 0 for c in PREFIX_COMPONENTS}
    )
    # Breaks where only this component changed (solo blame) within a lane.
    component_solo_breaks: dict[str, int] = field(
        default_factory=lambda: {c: 0 for c in PREFIX_COMPONENTS}
    )
    by_lane: dict[str, dict[str, int]] = field(default_factory=dict)
    events: deque = field(default_factory=lambda: deque(maxlen=2000))
    by_model: dict[str, dict[str, int]] = field(default_factory=dict)

    def record(self, ev: RequestEvent) -> None:
        with self.lock:
            self.requests += 1
            if ev.error or ev.status >= 400:
                self.errors += 1
            self.input_tokens += ev.input_tokens
            self.output_tokens += ev.output_tokens
            self.cache_read_tokens += ev.cache_read_tokens
            self.cache_write_tokens += ev.cache_write_tokens
            self.total_latency_ms += ev.latency_ms
            if ev.prefix_break:
                self.prefix_breaks += 1
            if ev.lane_prefix_break:
                self.lane_prefix_breaks += 1
                for c in ev.components_changed:
                    if c in self.component_break_counts:
                        self.component_break_counts[c] += 1
                if len(ev.components_changed) == 1:
                    solo = ev.components_changed[0]
                    if solo in self.component_solo_breaks:
                        self.component_solo_breaks[solo] += 1
            if ev.prefix_hash:
                self.last_prefix_hash = ev.prefix_hash
            if ev.component_hashes:
                self.last_component_hashes = dict(ev.component_hashes)
            self.last_by_lane[ev.lane] = {
                "prefix_hash": ev.prefix_hash,
                "component_hashes": dict(ev.component_hashes),
            }
            lane_b = self.by_lane.setdefault(
                ev.lane,
                {
                    "requests": 0,
                    "input_tokens": 0,
                    "cache_read_tokens": 0,
                    "prefix_breaks": 0,
                    "lane_prefix_breaks": 0,
                },
            )
            lane_b["requests"] += 1
            lane_b["input_tokens"] += ev.input_tokens
            lane_b["cache_read_tokens"] += ev.cache_read_tokens
            if ev.prefix_break:
                lane_b["prefix_breaks"] += 1
            if ev.lane_prefix_break:
                lane_b["lane_prefix_breaks"] += 1
            self.events.append(ev)
            key = ev.model or "?"
            bucket = self.by_model.setdefault(
                key,
                {
                    "requests": 0,
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "cache_read_tokens": 0,
                    "cache_write_tokens": 0,
                    "prefix_breaks": 0,
                },
            )
            bucket["requests"] += 1
            bucket["input_tokens"] += ev.input_tokens
            bucket["output_tokens"] += ev.output_tokens
            bucket["cache_read_tokens"] += ev.cache_read_tokens
            bucket["cache_write_tokens"] += ev.cache_write_tokens
            if ev.prefix_break:
                bucket["prefix_breaks"] += 1

    def snapshot(self, reset: bool = False) -> dict[str, Any]:
        with self.lock:
            inp = self.input_tokens
            cache_hit_rate = (self.cache_read_tokens / inp) if inp > 0 else None
            billable_input = max(0, inp - self.cache_read_tokens)
            by_lane_out: dict[str, Any] = {}
            for lane, b in self.by_lane.items():
                li = b.get("input_tokens") or 0
                cr = b.get("cache_read_tokens") or 0
                by_lane_out[lane] = {
                    **b,
                    "cache_hit_rate": round(cr / li, 4) if li else None,
                }
            out = {
                "started_at": self.started_at,
                "uptime_s": round(time.time() - self.started_at, 2),
                "requests": self.requests,
                "errors": self.errors,
                "input_tokens": self.input_tokens,
                "output_tokens": self.output_tokens,
                "cache_read_tokens": self.cache_read_tokens,
                "cache_write_tokens": self.cache_write_tokens,
                "billable_input_tokens": billable_input,
                "cache_hit_rate": round(cache_hit_rate, 4) if cache_hit_rate is not None else None,
                "prefix_breaks": self.prefix_breaks,
                "lane_prefix_breaks": self.lane_prefix_breaks,
                "last_prefix_hash": self.last_prefix_hash,
                "last_component_hashes": dict(self.last_component_hashes),
                "component_break_counts": dict(self.component_break_counts),
                "component_solo_breaks": dict(self.component_solo_breaks),
                "by_lane": by_lane_out,
                "avg_latency_ms": (
                    round(self.total_latency_ms / self.requests, 1) if self.requests else None
                ),
                "by_model": dict(self.by_model),
            }
            if reset:
                self.requests = 0
                self.errors = 0
                self.input_tokens = 0
                self.output_tokens = 0
                self.cache_read_tokens = 0
                self.cache_write_tokens = 0
                self.total_latency_ms = 0
                self.prefix_breaks = 0
                self.lane_prefix_breaks = 0
                self.component_break_counts = {c: 0 for c in PREFIX_COMPONENTS}
                self.component_solo_breaks = {c: 0 for c in PREFIX_COMPONENTS}
                self.by_lane.clear()
                self.by_model.clear()
                # keep last hashes + events for continuity unless hard reset
            return out

    def events_list(self, limit: int = 50) -> list[dict[str, Any]]:
        with self.lock:
            items = list(self.events)[-limit:]
            return [asdict(e) for e in items]


STORE = MetricsStore()
UPSTREAM = "https://opencode.ai/zen/v1"
EVENT_LOG: str | None = None
VERBOSE = False


# ── usage / prefix parsing ───────────────────────────────────────────────────


def _json_u64(v: Any) -> int:
    if v is None:
        return 0
    try:
        return int(v)
    except (TypeError, ValueError):
        return 0


def extract_usage(obj: dict[str, Any]) -> dict[str, int]:
    """Normalize OpenAI / Anthropic / OpenCode usage blobs."""
    usage = obj.get("usage") if isinstance(obj.get("usage"), dict) else {}
    # nested prompt_tokens_details.cached_tokens (OpenAI)
    details = usage.get("prompt_tokens_details") if isinstance(usage, dict) else None
    if not isinstance(details, dict):
        details = {}
    # OpenCode step tokens.cache
    tokens = obj.get("tokens") if isinstance(obj.get("tokens"), dict) else {}
    cache = tokens.get("cache") if isinstance(tokens.get("cache"), dict) else {}

    input_tokens = (
        _json_u64(usage.get("input_tokens"))
        or _json_u64(usage.get("inputTokens"))  # Command Code
        or _json_u64(usage.get("prompt_tokens"))
        or _json_u64(tokens.get("input"))
    )
    output_tokens = (
        _json_u64(usage.get("output_tokens"))
        or _json_u64(usage.get("outputTokens"))  # Command Code
        or _json_u64(usage.get("completion_tokens"))
        or _json_u64(tokens.get("output"))
    )
    # Command Code: usage.inputTokenDetails.cacheReadTokens
    cc_details = usage.get("inputTokenDetails") if isinstance(usage.get("inputTokenDetails"), dict) else {}
    cache_read = (
        _json_u64(usage.get("cache_read_input_tokens"))
        or _json_u64(usage.get("cache_read_tokens"))
        or _json_u64(usage.get("prompt_cache_hit_tokens"))  # OpenCode Zen
        or _json_u64(cc_details.get("cacheReadTokens"))
        or _json_u64(details.get("cached_tokens"))
        or _json_u64(usage.get("cached_tokens"))
        or _json_u64(cache.get("read"))
    )
    cache_write = (
        _json_u64(usage.get("cache_creation_input_tokens"))
        or _json_u64(usage.get("cache_write_tokens"))
        or _json_u64(usage.get("prompt_cache_miss_tokens"))  # OpenCode: first-write/miss
        or _json_u64(cc_details.get("cacheWriteTokens"))
        or _json_u64(cache.get("write"))
    )
    return {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_read_tokens": cache_read,
        "cache_write_tokens": cache_write,
    }


def parse_response_usage(body: bytes, content_type: str) -> dict[str, int]:
    text = body.decode("utf-8", errors="replace")
    totals = {
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_tokens": 0,
        "cache_write_tokens": 0,
    }

    def merge(u: dict[str, int]) -> None:
        for k, v in u.items():
            # Prefer last non-zero for streaming cumulative, else max
            if v:
                totals[k] = max(totals[k], v)

    if "text/event-stream" in content_type or text.lstrip().startswith("data:"):
        for line in text.splitlines():
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if not data or data == "[DONE]":
                continue
            try:
                obj = json.loads(data)
            except json.JSONDecodeError:
                continue
            if not isinstance(obj, dict):
                continue
            # OpenAI stream: usage on final chunk; Anthropic: message_delta / message_start
            if "usage" in obj:
                merge(extract_usage(obj))
            if "totalUsage" in obj:
                merge(extract_usage({"usage": obj["totalUsage"]}))
            # nested response.usage
            resp = obj.get("response")
            if isinstance(resp, dict) and "usage" in resp:
                merge(extract_usage(resp))
            # Anthropic content
            if obj.get("type") in ("message_delta", "message_start", "message", "finish"):
                merge(extract_usage(obj.get("message") if isinstance(obj.get("message"), dict) else obj))
                if obj.get("type") == "finish":
                    merge(extract_usage(obj))
        return totals

    try:
        obj = json.loads(text)
        if isinstance(obj, dict):
            merge(extract_usage(obj))
    except json.JSONDecodeError:
        # last-ditch: find usage JSON fragments
        for m in re.finditer(r'"usage"\s*:\s*(\{[^}]{0,800}\})', text):
            try:
                merge(extract_usage({"usage": json.loads(m.group(1))}))
            except json.JSONDecodeError:
                pass
    return totals


def _short_hash(text: str | bytes | None) -> str | None:
    if text is None:
        return None
    if isinstance(text, str):
        if not text.strip():
            return None
        data = text.encode("utf-8", errors="replace")
    else:
        if not text:
            return None
        data = text
    return hashlib.sha256(data).hexdigest()[:16]


def _message_text(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts: list[str] = []
        for part in content:
            if isinstance(part, dict):
                t = part.get("text") or part.get("content") or ""
                if isinstance(t, str) and t:
                    parts.append(t)
            elif isinstance(part, str):
                parts.append(part)
        return "\n".join(parts)
    return ""


def analyze_request_prefix(request_body: bytes) -> dict[str, Any]:
    """Split request into cache-relevant components and hash each separately.

    Returns:
      {
        "prefix_hash": combined hash or None,
        "component_hashes": {name: hash|None},
        "component_meta": {counts, sizes, tool_names, ...},
      }
    """
    empty = {
        "prefix_hash": None,
        "component_hashes": {c: None for c in PREFIX_COMPONENTS},
        "component_meta": {},
        "lane": "main",
    }
    try:
        obj = json.loads(request_body.decode("utf-8", errors="replace"))
    except (json.JSONDecodeError, UnicodeDecodeError):
        return empty
    if not isinstance(obj, dict):
        return empty

    # Command Code wraps the OpenAI-ish payload under params{...}.
    if isinstance(obj.get("params"), dict):
        params = obj["params"]
        merged = dict(obj)
        for k, v in params.items():
            if k not in merged or merged.get(k) in (None, "", [], {}):
                merged[k] = v
        obj = merged

    instructions = obj.get("instructions") if isinstance(obj.get("instructions"), str) else ""
    system_parts: list[str] = []
    developer_parts: list[str] = []
    first_user = ""
    # Command Code / some providers put system as a top-level string.
    if isinstance(obj.get("system"), str) and obj["system"].strip():
        system_parts.append(obj["system"])
    elif isinstance(obj.get("system"), list):
        for block in obj["system"]:
            if isinstance(block, dict):
                t = _message_text(block.get("text") or block.get("content") or block)
                if t:
                    system_parts.append(t)
            elif isinstance(block, str) and block.strip():
                system_parts.append(block)
    messages = obj.get("messages") or obj.get("input") or []
    if isinstance(messages, list):
        for msg in messages:
            if not isinstance(msg, dict):
                continue
            role = (msg.get("role") or "").lower()
            text = _message_text(msg.get("content"))
            if role == "system":
                if text:
                    system_parts.append(text)
            elif role == "developer":
                if text:
                    developer_parts.append(text)
            elif role == "user" and not first_user:
                first_user = text[:4000]
            # Keep scanning system/developer even after user (multi-turn history may
            # only have system at the front; developer blocks are still prefix).

    tools = obj.get("tools") if isinstance(obj.get("tools"), list) else []
    tool_names: list[str] = []
    tools_blob = ""
    if tools:
        for t in tools:
            if isinstance(t, dict):
                # OpenAI function tools nest name under function
                name = t.get("name")
                if not name and isinstance(t.get("function"), dict):
                    name = t["function"].get("name")
                if name:
                    tool_names.append(str(name))
        try:
            tools_blob = json.dumps(tools, sort_keys=True, ensure_ascii=False)
        except (TypeError, ValueError):
            tools_blob = str(tools)

    system_text = "\n\n".join(system_parts)
    developer_text = "\n\n".join(developer_parts)
    tools_names_text = "\n".join(tool_names)

    component_hashes = {
        "instructions": _short_hash(instructions[:20000] if instructions else None),
        "system": _short_hash(system_text[:40000] if system_text else None),
        "developer": _short_hash(developer_text[:40000] if developer_text else None),
        "tools": _short_hash(tools_blob[:80000] if tools_blob else None),
        "tools_names": _short_hash(tools_names_text if tools_names_text else None),
        "first_user": _short_hash(first_user if first_user else None),
    }
    # Combined: only components that exist, in stable order
    combined_parts = [
        f"{name}:{component_hashes[name]}"
        for name in PREFIX_COMPONENTS
        if component_hashes[name]
    ]
    prefix_hash = _short_hash("|".join(combined_parts)) if combined_parts else None

    lane = classify_request_lane(system_text, developer_text, first_user, len(tools))
    meta = {
        "lane": lane,
        "system_msgs": len(system_parts),
        "developer_msgs": len(developer_parts),
        "system_chars": len(system_text),
        "developer_chars": len(developer_text),
        "instructions_chars": len(instructions or ""),
        "tools_count": len(tools),
        "tool_names": tool_names[:80],
        "first_user_chars": len(first_user),
        "first_user_preview": first_user[:120].replace("\n", " ") if first_user else "",
    }
    return {
        "prefix_hash": prefix_hash,
        "component_hashes": component_hashes,
        "component_meta": meta,
        "lane": lane,
    }


def classify_request_lane(
    system_text: str,
    developer_text: str,
    first_user: str,
    tools_count: int,
) -> str:
    """Heuristic lane so concurrent main/subagent traffic is not mixed for breaks."""
    s = (system_text or "").lower()
    u = (first_user or "").lstrip()
    if "memory extraction" in s or u.startswith("Conversation turn to analyze"):
        return "memory_extract"
    if "checkpoint-writer" in s or "checkpoint_markdown" in s:
        return "checkpoint"
    if "repository exploration" in s or u.startswith("Query:"):
        return "repo_explore"
    if "subagent worker" in s:
        return "subagent"
    if "memory maintenance" in s or "navi dream" in s or "navi dream" in u.lower():
        return "memory_dream"
    if "process distillation" in s:
        return "memory_distill"
    if "memory consolidation" in s:
        return "memory_consolidate"
    if "title generator" in s:
        return "session_title"
    if "recap" in s and tools_count == 0:
        return "recap"
    # Command Code puts a large system string + tools; treat as main when tools present.
    if tools_count > 0 or len(s) >= 800 or u.startswith("[navi-bench"):
        return "main"
    if tools_count == 0 and len(s) < 800 and not u:
        return "utility_no_tools"
    return "main"


def stable_prefix_hash(request_body: bytes) -> str | None:
    """Backward-compatible combined prefix hash."""
    return analyze_request_prefix(request_body)["prefix_hash"]


def diff_component_hashes(
    prev: dict[str, str | None] | None,
    cur: dict[str, str | None],
) -> list[str]:
    """Return component names whose hash changed (or appeared/disappeared)."""
    if not prev:
        return []
    changed: list[str] = []
    for name in PREFIX_COMPONENTS:
        a = prev.get(name)
        b = cur.get(name)
        if a != b and (a is not None or b is not None):
            changed.append(name)
    return changed


def analyze_events_log(path: str) -> dict[str, Any]:
    """Offline analysis of a proxy JSONL event log (component blame)."""
    events: list[dict[str, Any]] = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                continue

    total = len(events)
    # Reconstruct per-lane breaks if needed (also works for older logs).
    last_by_lane: dict[str, dict[str, Any]] = {}
    counts = {c: 0 for c in PREFIX_COMPONENTS}
    solo = {c: 0 for c in PREFIX_COMPONENTS}
    lane_breaks: list[dict[str, Any]] = []
    by_lane: dict[str, dict[str, int]] = {}
    for e in events:
        meta = e.get("component_meta") or {}
        lane = e.get("lane") or meta.get("lane")
        if not lane:
            # Re-classify older logs that lack lane tags.
            lane = classify_request_lane(
                system_text="" if meta.get("system_chars") is None else ("x" * min(int(meta.get("system_chars") or 0), 2000)),
                developer_text="",
                first_user=str(meta.get("first_user_preview") or ""),
                tools_count=int(meta.get("tools_count") or 0),
            )
            # Prefer previews / tool counts over fake system text
            fu = str(meta.get("first_user_preview") or "")
            tools_n = int(meta.get("tools_count") or 0)
            sys_chars = int(meta.get("system_chars") or 0)
            if fu.startswith("Conversation turn to analyze"):
                lane = "memory_extract"
            elif fu.startswith("Query:"):
                lane = "repo_explore"
            elif tools_n == 0 and sys_chars and sys_chars < 800:
                lane = "utility_no_tools"
            elif fu.startswith("[navi-bench") or "navi-bench scope" in fu:
                lane = "main"
            else:
                lane = lane or "main"
        ch = e.get("component_hashes") or {}
        lb = by_lane.setdefault(
            lane, {"requests": 0, "input_tokens": 0, "cache_read_tokens": 0, "lane_prefix_breaks": 0}
        )
        lb["requests"] += 1
        lb["input_tokens"] += int(e.get("input_tokens") or 0)
        lb["cache_read_tokens"] += int(e.get("cache_read_tokens") or 0)
        prev = last_by_lane.get(lane)
        changed: list[str] = []
        is_break = False
        if prev and ch:
            changed = diff_component_hashes(prev.get("component_hashes") or {}, ch)
            if changed or (
                e.get("prefix_hash")
                and prev.get("prefix_hash")
                and e.get("prefix_hash") != prev.get("prefix_hash")
            ):
                is_break = True
        if is_break:
            lb["lane_prefix_breaks"] += 1
            for c in changed:
                if c in counts:
                    counts[c] += 1
            if len(changed) == 1:
                solo[changed[0]] = solo.get(changed[0], 0) + 1
            lane_breaks.append(
                {
                    "ts": e.get("ts"),
                    "lane": lane,
                    "model": e.get("model"),
                    "components_changed": changed,
                    "input_tokens": e.get("input_tokens"),
                    "cache_read_tokens": e.get("cache_read_tokens"),
                    "component_meta": meta,
                }
            )
        if ch:
            last_by_lane[lane] = {
                "prefix_hash": e.get("prefix_hash"),
                "component_hashes": ch,
            }

    tin = sum(e.get("input_tokens") or 0 for e in events)
    cr = sum(e.get("cache_read_tokens") or 0 for e in events)
    by_lane_out: dict[str, Any] = {}
    for lane, b in by_lane.items():
        li = b["input_tokens"]
        by_lane_out[lane] = {
            **b,
            "cache_hit_rate": round(b["cache_read_tokens"] / li, 4) if li else None,
        }
    return {
        "events": total,
        "prefix_breaks_global_noise": sum(1 for e in events if e.get("prefix_break")),
        "lane_prefix_breaks": len(lane_breaks),
        "cache_hit_rate": round(cr / tin, 4) if tin else None,
        "input_tokens": tin,
        "cache_read_tokens": cr,
        "component_break_counts": counts,
        "component_solo_breaks": solo,
        "by_lane": by_lane_out,
        "break_events": lane_breaks[:50],
    }


def extract_model(request_body: bytes) -> str | None:
    try:
        obj = json.loads(request_body.decode("utf-8", errors="replace"))
        if isinstance(obj, dict):
            m = obj.get("model")
            return str(m) if m else None
    except (json.JSONDecodeError, UnicodeDecodeError):
        return None
    return None


def is_stream_request(request_body: bytes) -> bool:
    try:
        obj = json.loads(request_body.decode("utf-8", errors="replace"))
        return bool(isinstance(obj, dict) and obj.get("stream"))
    except (json.JSONDecodeError, UnicodeDecodeError):
        return False


# ── HTTP handler ─────────────────────────────────────────────────────────────


class ProxyHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt: str, *args: Any) -> None:
        if VERBOSE:
            sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))

    def do_GET(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path in ("/health", "/_health"):
            self._json(200, {"ok": True, "upstream": UPSTREAM})
            return
        if parsed.path in ("/_metrics", "/metrics"):
            qs = parse_qs(parsed.query)
            reset = qs.get("reset", ["0"])[0] in ("1", "true", "yes")
            self._json(200, STORE.snapshot(reset=reset))
            return
        if parsed.path in ("/_metrics/events", "/metrics/events"):
            qs = parse_qs(parsed.query)
            limit = int(qs.get("limit", ["50"])[0])
            self._json(200, {"events": STORE.events_list(limit)})
            return
        self._proxy()

    def do_POST(self) -> None:  # noqa: N802
        parsed = urlparse(self.path)
        if parsed.path in ("/_metrics/reset", "/metrics/reset"):
            snap = STORE.snapshot(reset=True)
            self._json(200, {"reset": True, "previous": snap})
            return
        self._proxy()

    def do_OPTIONS(self) -> None:  # noqa: N802
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.end_headers()

    def do_PUT(self) -> None:  # noqa: N802
        self._proxy()

    def do_DELETE(self) -> None:  # noqa: N802
        self._proxy()

    def _json(self, code: int, payload: dict[str, Any]) -> None:
        data = json.dumps(payload, indent=2).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length") or 0)
        if length <= 0:
            return b""
        return self.rfile.read(length)

    def _proxy(self) -> None:
        t0 = time.monotonic()
        body = self._read_body()
        method = self.command
        # Strip leading /v1 if upstream already ends with /v1 and path also starts with /v1
        path = self.path
        upstream_base = UPSTREAM.rstrip("/")
        # Client may call http://proxy/v1/chat/completions while upstream is .../zen/v1
        # → join carefully.
        if path.startswith("/"):
            # If path is /v1/... and upstream ends with /v1, drop duplicate /v1
            if upstream_base.endswith("/v1") and (path == "/v1" or path.startswith("/v1/")):
                suffix = path[3:] or "/"
                url = upstream_base + suffix
            elif upstream_base.endswith("/v1") and not path.startswith("/v1"):
                url = upstream_base + path
            else:
                url = upstream_base + path
        else:
            url = upstream_base + "/" + path

        headers = {}
        for k, v in self.headers.items():
            lk = k.lower()
            if lk in ("host", "content-length", "transfer-encoding", "connection"):
                continue
            headers[k] = v
        # Cloudflare / many CDNs block Python-urllib's default User-Agent.
        ua = headers.get("User-Agent") or headers.get("user-agent") or ""
        if not ua or "python-urllib" in ua.lower() or ua.startswith("Python-urllib"):
            headers["User-Agent"] = "navi-llm-metrics-proxy/1.0"

        model = extract_model(body) if body else None
        stream = is_stream_request(body) if body else False
        prefix_info = analyze_request_prefix(body) if body else {
            "prefix_hash": None,
            "component_hashes": {c: None for c in PREFIX_COMPONENTS},
            "component_meta": {},
            "lane": "main",
        }
        prefix = prefix_info["prefix_hash"]
        component_hashes = prefix_info["component_hashes"]
        component_meta = prefix_info["component_meta"]
        lane = prefix_info.get("lane") or "main"
        components_changed: list[str] = []
        prefix_break = False
        lane_prefix_break = False
        with STORE.lock:
            # Global interleaving break (noisy — includes main↔subagent).
            if prefix and STORE.last_prefix_hash and STORE.last_prefix_hash != prefix:
                prefix_break = True
            if prefix and STORE.last_component_hashes:
                global_changed = diff_component_hashes(
                    STORE.last_component_hashes, component_hashes
                )
                if global_changed:
                    prefix_break = True
            # Per-lane break: only compare to previous request of the same lane.
            prev_lane = STORE.last_by_lane.get(lane)
            if prev_lane and component_hashes:
                components_changed = diff_component_hashes(
                    prev_lane.get("component_hashes") or {},
                    component_hashes,
                )
                if components_changed or (
                    prefix
                    and prev_lane.get("prefix_hash")
                    and prev_lane.get("prefix_hash") != prefix
                ):
                    lane_prefix_break = True
                    if not components_changed and prev_lane.get("component_hashes"):
                        components_changed = diff_component_hashes(
                            prev_lane.get("component_hashes") or {},
                            component_hashes,
                        )

        req = urllib.request.Request(url, data=body if body else None, method=method, headers=headers)
        status = 502
        resp_body = b""
        resp_headers: list[tuple[str, str]] = []
        err_msg: str | None = None
        try:
            with urllib.request.urlopen(req, timeout=600) as resp:
                status = resp.getcode() or 200
                resp_body = resp.read()
                resp_headers = [(k, v) for k, v in resp.headers.items()]
        except urllib.error.HTTPError as e:
            status = e.code
            resp_body = e.read() if e.fp else b""
            resp_headers = [(k, v) for k, v in (e.headers.items() if e.headers else [])]
            err_msg = f"HTTP {e.code}"
        except Exception as e:
            status = 502
            err_msg = str(e)
            resp_body = json.dumps({"error": str(e), "proxy": "llm_metrics_proxy"}).encode()
            resp_headers = [("Content-Type", "application/json")]

        # Relay response
        self.send_response(status)
        content_type = "application/json"
        for k, v in resp_headers:
            lk = k.lower()
            if lk in ("transfer-encoding", "content-length", "connection"):
                continue
            if lk == "content-type":
                content_type = v
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(resp_body)))
        self.end_headers()
        try:
            self.wfile.write(resp_body)
        except BrokenPipeError:
            pass

        usage = parse_response_usage(resp_body, content_type) if resp_body else {
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_read_tokens": 0,
            "cache_write_tokens": 0,
        }
        latency_ms = int((time.monotonic() - t0) * 1000)
        ev = RequestEvent(
            ts=time.time(),
            method=method,
            path=path,
            status=status,
            latency_ms=latency_ms,
            model=model,
            stream=stream,
            input_tokens=usage["input_tokens"],
            output_tokens=usage["output_tokens"],
            cache_read_tokens=usage["cache_read_tokens"],
            cache_write_tokens=usage["cache_write_tokens"],
            request_bytes=len(body),
            response_bytes=len(resp_body),
            prefix_hash=prefix,
            prefix_break=prefix_break,
            lane_prefix_break=lane_prefix_break,
            lane=lane,
            component_hashes=component_hashes,
            components_changed=components_changed,
            component_meta=component_meta,
            error=err_msg,
        )
        STORE.record(ev)
        if EVENT_LOG:
            try:
                with open(EVENT_LOG, "a", encoding="utf-8") as f:
                    f.write(json.dumps(asdict(ev)) + "\n")
            except OSError:
                pass
        if VERBOSE or lane_prefix_break or prefix_break or (status >= 400):
            hit = (
                f" cache_hit={usage['cache_read_tokens']}/{usage['input_tokens']}"
                if usage["input_tokens"]
                else ""
            )
            if lane_prefix_break:
                blame = ",".join(components_changed) if components_changed else "?"
                br = f" LANE_BREAK[{lane}:{blame}]"
            elif prefix_break:
                br = f" CROSS_LANE_NOISE[{lane}]"
            else:
                br = ""
            sys.stderr.write(
                f"[proxy] {status} {method} {path} {latency_ms}ms lane={lane} "
                f"in={usage['input_tokens']} out={usage['output_tokens']}"
                f" cr={usage['cache_read_tokens']}{hit}{br}\n"
            )


def main() -> int:
    global UPSTREAM, EVENT_LOG, VERBOSE
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--listen", default="127.0.0.1:18765", help="host:port")
    ap.add_argument(
        "--upstream",
        default="https://opencode.ai/zen/v1",
        help="Upstream OpenAI-compatible base URL",
    )
    ap.add_argument("--log", default=None, help="Append JSONL request events to this path")
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument(
        "--analyze",
        metavar="JSONL",
        default=None,
        help="Offline: attribute prefix breaks in a proxy events JSONL and exit",
    )
    args = ap.parse_args()

    if args.analyze:
        report = analyze_events_log(args.analyze)
        print(json.dumps(report, indent=2))
        print("\n=== By lane ===", flush=True)
        for lane, b in sorted((report.get("by_lane") or {}).items()):
            print(
                f"  {lane:<18} reqs={b.get('requests')} hit={b.get('cache_hit_rate')} "
                f"lane_breaks={b.get('lane_prefix_breaks')}",
                flush=True,
            )
        print("\n=== Component blame (within-lane breaks) ===", flush=True)
        for c, n in sorted(
            (report.get("component_break_counts") or {}).items(),
            key=lambda kv: -kv[1],
        ):
            solo = (report.get("component_solo_breaks") or {}).get(c, 0)
            if n or solo:
                print(f"  {c:<14} in_break={n:3d}  solo={solo:3d}", flush=True)
        print(
            f"\nlane_breaks={report.get('lane_prefix_breaks')}  "
            f"global_noise={report.get('prefix_breaks_global_noise')}  "
            f"hit={report.get('cache_hit_rate')}  events={report.get('events')}",
            flush=True,
        )
        return 0

    UPSTREAM = args.upstream.rstrip("/")
    EVENT_LOG = args.log
    VERBOSE = args.verbose

    host, _, port_s = args.listen.partition(":")
    port = int(port_s or "18765")
    server = ThreadingHTTPServer((host, port), ProxyHandler)
    print(
        f"llm_metrics_proxy listening on http://{host}:{port} → {UPSTREAM}",
        flush=True,
    )
    print(f"  metrics: http://{host}:{port}/_metrics", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nshutting down", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
