# Bash Redundancy in Code Agents: Analysis and NAVI Solutions

## Summary

Analysis of code agent session data (OpenClaude, Codex, and similar tools) reveals that **Bash tool calls account for 33.2% of all tool invocations** — the single largest category. A significant portion of these calls are redundant, predictable, and could be replaced by purpose-built tools with structured output.

This document defines the problem space and outlines how NAVI's tool architecture should address it.

---

## Data Source

Observations from real agent sessions across multiple projects (navi, psyche-subtitle-toolkit, claudelog, animedb, and others):

| Metric | Value |
|--------|-------|
| Total tool calls analyzed | 5,433 |
| Bash calls | 1,804 (33.2%) |
| Edit + Write calls | 1,536 (28.3%) |
| Other tools (Read, Grep, Glob, etc.) | 2,093 (38.5%) |

Bash dominates over file editing — the agent spends more shell commands than actual code changes.

---

## Bash Usage Breakdown

Estimated distribution of the 1,804 Bash calls:

| Category | Est. % | Est. Calls | Examples |
|----------|--------|------------|----------|
| Test execution | ~28% | ~505 | `pytest tests/ -v`, `npm test`, `cargo test` |
| Build / compilation | ~22% | ~397 | `npm run build`, `cargo build --release`, `make` |
| Package management | ~18% | ~325 | `npm install X`, `pip install Y`, `cargo add Z` |
| Git operations | ~13% | ~235 | `git status`, `git diff --stat`, `git log --oneline` |
| Filesystem navigation | ~10% | ~180 | `ls -la`, `find . -name "*.py"`, `mkdir -p` |
| Other / ad-hoc | ~9% | ~162 | `which`, `env`, `ps`, `curl`, one-off scripts |

### Why This Is a Problem

1. **Token waste**: Each Bash call includes the full command string, stdout, stderr, and exit code in the conversation. A single `cargo test` run can produce 200+ lines of output that the model must process.

2. **Redundant calls**: Agents frequently re-run the same command within minutes — `git status` before and after every edit, `cargo check` after every file save, `ls` to confirm what `Read` already showed.

3. **Unstructured output**: Shell output is plain text. The model must parse it every time. A failed test produces a wall of text that the agent re-reads on every retry.

4. **Latency**: Each Bash call is a round-trip. 1,804 calls × ~1s average = ~30 minutes of pure tool latency per session.

5. **Context pollution**: Verbose build output, test runners, and git logs fill the context window with low-signal text, crowding out actual code context.

---

## Proposed Tools

### 1. TestRunner Tool

**Problem**: Agents run tests hundreds of times per session. The output is verbose, mostly unchanged between runs, and the agent only cares about pass/fail and which tests failed.

**Proposal**:

```
Tool: test_runner
Input:
  - project_path: string (auto-detected if omitted)
  - test_path: string (optional — specific test file or filter)
  - flags: string[] (optional extra args)
  - watch: bool (optional — re-run on change)

Output (structured):
  - status: "pass" | "fail" | "error"
  - total: number
  - passed: number
  - failed: number
  - skipped: number
  - duration_ms: number
  - failures: [{ test_name, file, line, message, diff }]
  - summary: string (one-line human-readable)
```

**Impact**: ~500 Bash calls replaced. Test output reduced from 200+ lines to a structured JSON object. The agent sees exactly which tests failed and why, without parsing walls of text.

**Detection heuristics**: If the command matches `pytest`, `cargo test`, `npm test`, `jest`, `vitest`, `go test`, `bun test`, or similar, route through TestRunner instead of raw Bash.

---

### 2. BuildRunner Tool

**Problem**: Build commands are repeated constantly. The agent runs `cargo build` after every file change, even when the change doesn't affect compilation. Output is mostly noise.

**Proposal**:

```
Tool: build_runner
Input:
  - project_path: string (auto-detected)
  - profile: "debug" | "release" (default: debug)
  - incremental: bool (default: true — skip if no source changes)
  - features: string[] (optional)

Output (structured):
  - status: "success" | "error" | "cached"
  - duration_ms: number
  - warnings: [{ file, line, message, code }]
  - errors: [{ file, line, message, code }]
  - artifact_path: string (optional)
  - cached: bool
```

**Impact**: ~400 Bash calls replaced. The `cached` flag means the tool can skip redundant rebuilds. Structured warnings/errors are easier for the agent to act on than raw compiler output.

**Caching strategy**: Hash the source files (or check mtimes). If nothing changed since the last build, return `cached: true` with the previous result. This alone could eliminate 30-50% of build calls.

---

### 3. PackageManager Tool

**Problem**: Package install commands are predictable and follow patterns. The agent runs `npm install react` without checking if it's already installed, or runs `cargo add` without verifying the crate exists.

**Proposal**:

```
Tool: package_manager
Input:
  - action: "install" | "add" | "remove" | "update" | "check"
  - packages: string[]
  - dev: bool (optional — dev dependency)
  - manager: "auto" | "npm" | "bun" | "cargo" | "pip" | "go" (auto-detected)

Output (structured):
  - status: "success" | "already_installed" | "not_found" | "error"
  - installed: [{ name, version }]
  - conflicts: [{ name, reason }]
  - lockfile_changed: bool
```

**Impact**: ~325 Bash calls replaced. The `check` action lets the agent verify before installing. `already_installed` prevents redundant installs.

---

### 4. GitOps Tool

**Problem**: Git commands are highly repetitive. `git status` before every edit, `git diff` after, `git log` to check history. The output format is consistent and parseable.

**Proposal**:

```
Tool: git_ops
Input:
  - command: "status" | "diff" | "log" | "branch" | "stash" | "remote"
  - args: string[] (optional extra flags)
  - format: "json" | "text" (default: json)

Output (structured — for status):
  - branch: string
  - ahead: number
  - behind: number
  - staged: [{ file, status }]
  - modified: [{ file, status }]
  - untracked: [string]
  - conflicts: [string]

Output (structured — for diff):
  - files: [{ file, additions, deletions, hunks: [{ header, lines }] }]
  - stats: { additions, deletions, files_changed }

Output (structured — for log):
  - commits: [{ hash, author, date, message, files_changed }]
```

**Impact**: ~235 Bash calls replaced. JSON output eliminates parsing overhead. The agent can reason about git state directly instead of interpreting text.

---

### 5. FSBrowser Tool

**Problem**: The agent uses Bash for `ls`, `find`, `mkdir`, and `wc` even though it has `Read` and `Grep`. These calls are for directory listing and file discovery — operations that should be part of the file tooling, not shell commands.

**Proposal**:

Extend the existing `Read` / `Glob` tools or add a new `fs_browser`:

```
Tool: fs_browser
Input:
  - action: "list" | "find" | "stat" | "mkdir" | "tree"
  - path: string
  - pattern: string (optional — glob pattern for find)
  - depth: number (optional — for tree)
  - hidden: bool (optional — include dotfiles)

Output (structured):
  - entries: [{ name, type, size, modified, permissions }]
  - total: number
  - path: string
```

**Impact**: ~180 Bash calls replaced. Consolidates filesystem operations into the existing tool surface.

---

## Impact Projection

| Scenario | Remaining Bash | Reduction |
|----------|---------------|-----------|
| Current (no changes) | 1,804 | 0% |
| TestRunner + BuildRunner only | ~900 | -50% |
| All 5 tools implemented | ~170 | -90% |

The remaining ~170 Bash calls would be genuinely unique operations: running ad-hoc scripts, checking process status, making HTTP requests, and other one-off tasks that don't fit a pattern.

---

## Implementation Priority

| Priority | Tool | Calls Saved | Complexity | Notes |
|----------|------|-------------|------------|-------|
| 🔴 HIGH | TestRunner | ~500 | Medium | Highest call count, most verbose output |
| 🔴 HIGH | BuildRunner | ~400 | Medium | Caching logic adds complexity but huge payoff |
| 🟡 MED | PackageManager | ~325 | Low | Mostly wrapping existing package managers |
| 🟡 MED | GitOps | ~235 | Low | Git porcelain is already structured |
| 🟢 LOW | FSBrowser | ~180 | Low | Extends existing tools, lowest impact |

---

## NAVI Integration

These tools should be implemented as **native NAVI tools** in `navi-core`, not as external plugins. Reasons:

1. **Security policy integration**: TestRunner and BuildRunner execute arbitrary code. They must go through `SecurityPolicy` and approval flow like `bash` does.

2. **Harness awareness**: The harness should know when a test run is cached vs. fresh, when a build was skipped, etc. This affects loop detection and observation budgets.

3. **Structured events**: Each tool should emit structured events (`tool.test_runner.completed`, `tool.build_runner.cached`) so the TUI and Tutor can render results appropriately.

4. **Context optimization**: The harness can inject only the failure summary into context instead of the full stdout, reducing token consumption.

### Detection vs. Routing

Two approaches:

**A. Detection (recommended for v1)**: Keep `bash` as the tool. Add a pre-execution hook that detects known commands (regex matching on `cargo test`, `npm test`, etc.) and routes them through the specialized tool transparently. The agent doesn't need to change.

**B. Explicit routing**: Add the tools as separate tool definitions. The agent learns to use `test_runner` instead of `bash` for tests. Requires model fine-tuning or prompt engineering.

Approach A is safer — existing agents work unchanged, and the optimization is invisible. Approach B is cleaner long-term but requires agent behavior change.

---

## Metrics to Track

After implementation, measure:

- **Bash call count**: Target <200 per session (down from ~1,800)
- **Context token usage**: Target 30-40% reduction from less verbose tool output
- **Build skip rate**: Percentage of BuildRunner calls returning `cached`
- **Test rerun rate**: Percentage of TestRunner calls with unchanged results
- **Agent latency**: Total tool round-trip time per session

---

## Appendix: Real Session Examples

### Example 1: Redundant cargo check

In a typical navi development session, the agent runs `cargo check` 15-20 times. Between many of these, no source files changed. With BuildRunner caching, 10+ of these could return instantly.

### Example 2: Test wall-of-text

A `cargo test` run produces ~80 lines of output for 7 passing tests. The agent only needs to know "7/7 passed." The full output adds ~2,000 tokens to context for no value.

### Example 3: Git status spam

The agent runs `git status` before writing a file and again after. The only difference is the file it just wrote. A structured diff between the two calls would be 3 lines instead of 20.

### Example 4: Package install without check

The agent runs `bun install react` without first checking if react is already in package.json. A `check` action would return `already_installed` in 10ms instead of running a full install.
