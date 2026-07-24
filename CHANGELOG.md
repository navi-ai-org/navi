# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.8] - 2026-07-23

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.3.7...v0.3.8

OpenAI API meta/rate-limit header support, workspace clippy cleanup, and TUI snapshot fixes.

### Added

- **`ApiMeta`/`RateLimits`** — new `ModelStreamEvent::ApiMeta` variant in `navi-core` surfaces request ID, organization, processing time, API version, and all `x-ratelimit-*` / `x-ratelimit-*-project-tokens` headers.
- **`ProviderBehavior::parse_response_headers`** — OpenAI-compatible parser wired into openai/anthropic/gemini streaming paths and emitted right after `ensure_success`.
- **Test coverage** for `x-request-id`, `openai-organization`, `openai-processing-ms`, `openai-version`, and every `x-ratelimit-*` header (including project token variants).
- **`navi-sdk` re-export** of `ApiMeta`/`RateLimits` to keep engine surfaces in sync.

### Fixed

- **TUI snapshot `modal_thinking_80x24`** updated to include the new `xhigh` and `off` effort levels.
- **Flaky `tool_approval_pending_in_chat`** test by resetting request usage in the test harness so elapsed time is deterministic (`0ms`).
- **Clippy deny error** `overly_complex_bool_expr` in `navi-openai/src/providers/openai.rs`.

### Changed

- **Boxed `ApiMeta`** inside `ModelStreamEvent` to resolve the `large_enum_variant` clippy warning and shrink stream events.
- **Applied `cargo clippy --fix --workspace`** for machine-applicable cleanups (93 files, 983 insertions / 1.140 deletions).
- **Workspace crates + npm packages** bumped to **0.3.8**.

## [0.3.7] - 2026-07-23

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.3.6...v0.3.7

OpenCode Zen free model routing fixes, comprehensive free model test coverage, and TUI test regression fix.

### Bug Fixes

- **Wrong free model id** — `opencode_zen_model_id` mapped `nemotron-3-super-free` (nonexistent) instead of `nemotron-3-ultra-free` (the actual registry ref)
- **Missing free models** — `hy3-free`, `mimo-v2.5-free`, and `north-mini-code-free` were absent from the canonical id mapping; users typing these aliases got the raw string sent to the API instead of the canonical model id
- **Dead code in model mapping** — paid model aliases (qwen3.6-plus, glm-5.1, kimi-k2.6, etc.) were unreachable match arms since the function is only called when `is_free_model_name` returns true; removed
- **`is_free_model_name` underscore blindness** — models named with `_free` suffix (e.g. `DeepSeek_V4_Flash_Free`) were not detected as free because only `-free` and ` free` were checked; now normalizes `_` → `-` before matching
- **TUI model picker test regression** — `model_picker_clips_long_rows_inside_modal_border` failed after the perf commit because `open_model_picker` calls `refresh_authenticated_providers` which overwrites the synthetic provider set; test now registers the provider in config so it survives the refresh

### New Features

- **20 new OpenCode Zen free model tests** covering: free model detection (suffix, substring, case-insensitivity, underscore), canonical id mapping for all 6 registered free models, `opencode/` prefix stripping, separator normalization, empty segment collapsing, whitespace trimming, unrecognized model fallback, public access eligibility, and request model name pass-through for paid/non-opencode providers
- **3 new `canonical_provider_id` tests** covering alias mapping, known provider pass-through, and unknown provider pass-through

### Bindings / SDK

- Workspace crates and npm packages (`@navi-agent/navi`, `@navi-agent/napi`) bumped to **0.3.7**

## [0.3.6] - 2026-07-23

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.3.5...v0.3.6

Harness soft-lock fix for root sessions, essential builtin skills, and SDK skill DTO completeness.

### Bug Fixes

- **Root session tool allowlist** — soft harness apply only for **session-active skills with `harness: true`** (on-disk packs no longer lock authoring builtins like `navi-create-skill`)
- **Deny wording** — root/harness denials say “for the active harness”, not “for this subagent”
- **Private storage errors** — steer agents to `skill_list` / `skill_get` / `load_skill` / `skill_save`

### New Features

- **Builtin essentials** (pool `navi`): `navi-create-skill` hardened, plus `navi-harness-author` and `navi-skill-pools`
- **Dual harness activation** documented (CLI/install vs chat)

### Bindings / SDK

- `NaviSkillInfo` includes `harness` and `pool`
- Re-export `CREATE_SKILL_ID`, `HARNESS_AUTHOR_ID`, `SKILL_POOLS_ID`, `apply_harness_for_skills`, `materialize_from_skill`, `materialize_after_save`
- Workspace crates and npm packages (`@navi-agent/navi`, `@navi-agent/napi`) bumped to **0.3.6**

## [0.3.5] - 2026-07-23

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.3.4...v0.3.5

Skill pools as folders, host goal framing for SDK/bindings, production workflow/subagent bridge hardening, and TUI chat/tool polish.

### New Features

- **Skill pools** — filesystem skill folders with catalog/prompt surface (root skills + pools); load by `pool/id`
- **Host goal framing** — `build_host_set_goal_user_prompt` / `set_goal_for_host_turn` so the model sees the objective (TUI Set Goal modal + SDK/NAPI/Dart)
- **CI** — navi-core critical-path coverage gate; auto npm publish after GitHub Release

### Bug Fixes

- **Workflow / subagent** — write-scope schema fields, `description` null, `create_files` inherit; production bridge tests against real ToolExecutor schema
- **Self-update** — installer stdio isolated so ANSI/output does not paint over the TUI
- **TUI** — plan write shows markdown body; bash failure cards sanitized; true tokens/s over active generation; hover rail/viewport stability
- **Registry** — prune providers removed from catalog on load/sync; transcription kind for `wispr-flow` in embedded snapshot

### Changes

- Durable mid-turn session saves; multi-skill harness pack materialize flag
- TUI tests aligned with Ctrl+M (empty Ctrl+Enter) and header-only pure tool-error envelopes

### Bindings

- Workspace crate versions and npm packages (`@navi-agent/navi`, `@navi-agent/napi`) bumped to 0.3.5
- Homebrew tap formula updated for 0.3.5 binaries

## [0.3.4] - 2026-07-22

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.3.3...v0.3.4

Harness packs (loop/graph materialize + soft enforcement), skill CLI install/activate, aligned thread goals, and docs for the harness vision.

### New Features

- **Harness packs** — store under `{data_dir}/harnesses/<id>/` with `loop.toml` and optional `graph.toml`; deterministic materialize from skills with capability inventory filtering
- **Harness runtime soft-apply** — on skill activation: capability card in developer context, entry-node `allow_tools` merged with skill allowlists, pack `max_turns` / optional token budget for goals auto-continue
- **CLI** — `navi harness list|show|materialize`; materialize hook after `navi skill install`
- **Skills CLI** — `navi skill install` / `list`; activate skills via `--skill`
- **Thread goals** — idle auto-continue model aligned with host/SDK/NAPI goal APIs

### Bug Fixes

- **Goals** — auto-continue when idle (status-based tools), not model checklist dependency

### Changes

- Remove dead `build_runner` / `test_runner` code paths
- Docs: harness system vision + stakeholder HTML deck; mark MVP status for pack materialize/run path
- CI: npm trusted publishing workflow (OIDC)

### Bindings

- Workspace crate versions and npm packages (`@navi-agent/navi`, `@navi-agent/napi`) bumped to 0.3.4

## [0.3.3] - 2026-07-21

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.3.2...v0.3.3

Plan mode as markdown design docs, Lua multi-agent workflow tool, host-customizable SDK profiles, ACP peer client, and TUI activity/wait polish.

### New Features

- **Markdown plan mode** — session plan file under `{data_dir}/plans/`; `plan(write)` / `plan(submit)` with markdown design-doc review (context, approach, files, verification)
- **Lua workflow tool** — multi-agent orchestration under hard caps, non-widening write policy, cancel/timeout, journals under `{data_dir}/workflows/`
- **SDK host profiles** — tool/prompt/security profiles, data_dir/config builder injection, full startSession seed, snapshot reopen, compactSession, OpenAI-compat provider upsert, ACP list/delegate bindings
- **ACP client** — generic ACP client for external agent peer turns
- **TUI activity line** — token usage and average tokens/s while streaming

### Bug Fixes

- **Auto-compact** — runs at 80% context before long-horizon rebuild; keeps chat summary visible
- **TUI** — plain-text user prompts; viewport wheel scroll; sticky shell failures; clearer wait status; avg t/s measured from stream start
- **Registry** — recreate corrupt `registry.db` on open; prefer canonical model context and effort levels; embedded snapshot sync (v34)
- **Tests / Dart** — stop PTY smoke hangs and races; harden Dart WebSocket/URL and stop duplicate snake/camel keys

### Changes

- **AGENTS.md** — short agent constitution (≤200 lines); domain detail in `docs/`
- Prefer `Result`/context over production panics across crates
- Strip third-party product attribution from internal comments

### Bindings

- Workspace crate versions bumped to 0.3.3

## [0.3.2] - 2026-07-17

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.3.1...v0.3.2

Session goal reliability and session rewind (history + project files), with TUI entry points and neutral product copy.

### New Features

- **Session rewind** — per-user-turn file snapshots under session data; **Revert to here** and command-palette **Rewind…** restore chat history and project files to a past prompt
- **TUI goal controls** — command palette **Set Goal** / **Pause Goal** / **Resume Goal** / **Clear Goal** for multi-turn objectives with auto-continuation

### Bug Fixes

- **Goal tools and live UI** — `create_goal` / `update_goal` / checklist tools publish `GoalUpdated`; goal tools are Direct; SDK `set_goal` / `update_goal_status` notify clients so the goal chip and auto-continue stay in sync
- **Goal complete gate** — checklist must be fully verified/skipped before `update_goal(complete)`; pause/resume toggle auto-continuation correctly

### Changes

- Neutralize product-style marketing comments in TUI/core (no user-facing API change)

### Bindings

- Workspace crate versions bumped to 0.3.2

## [0.3.1] - 2026-07-17

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.3.0...v0.3.1

### Bug Fixes

- **TUI mouse text selection** — full mouse capture (incl. free-motion) so drag-select works; selection survives wheel; Down+Up without Drag still selects; no auto-scroll on block focus during drag start
- **TUI composer focus restore** — click anywhere on the input panel (draft box + meta strip) restores cursor after chat block selection; keyboard Up/Down scroll by lines instead of hopping blocks
- **Message Actions last choice** — remembers the last action (e.g. Copy session) across opens and restarts via `tui.last_message_action`
- **Install script GitHub rate limit** — resolve latest release via HTML redirect instead of unauthenticated API (403)

## [0.3.0] - 2026-07-17

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.7...v0.3.0

Bug-fix and cleanup release: provider cache isolation for multi-instance, TUI mouse/scroll/focus regression fix, usage tracking overhaul, and Wispr Flow provider removal.

### Bug Fixes

- **Charm Hyper cache isolation** — each NAVI instance now gets a unique session-affinity identity for provider cache, preventing cache overlap when running multiple instances with the same Charm Hyper provider
- **Fallback provider identity isolation** — fallback/secondary requests also receive isolated cache identities
- **TUI mouse scroll regression** — restored `REPORT_EVENT_TYPES` Kitty keyboard flag; without it, some terminals stop emitting mouse wheel events as `Event::Mouse` and instead emit arrow-key sequences, causing the scroll wheel to select chat blocks instead of scrolling the viewport line-by-line
- **TUI input focus restore** — composer hit region elevated to z=100 so clicks on the input box reliably restore cursor focus even when chat line hit regions (z=5) or "jump to latest" (z=80) overlap; `FocusComposer` action clears block selection and text drag state
- **TUI wheel scroll cleanup** — `clear_chat_selection_for_wheel` ensures scroll wheel never leaves a scrollback block focused, preventing the "selection hopping" visual glitch
- **Usage tracking double-count** — provider usage snapshots are now treated as cumulative (not incremental); `UsageUiState::observe_request_usage` computes deltas so session totals and cost are not inflated when a provider emits partial usage updates
- **Usage tracking stale during long turns** — account-backed providers (charm-hyper, openrouter, xai, openai, commandcode) now refresh every 30s while a turn is active or the Usage modal is open
- **Anthropic prompt usage at stream start** — `message_start` SSE now emits a `ModelStreamEvent::Usage` so the context meter updates immediately instead of waiting for `message_delta`
- **In-progress usage estimate** — `ModelDelta` and `ModelThinkingDelta` update a conservative output token estimate shown separately from billed totals

### Removed

- **Wispr Flow voice dictation provider** — removed `wispr-flow` transcription provider from registry, `WisprFlow` variant from `TranscriptionProviderKind` / `RemoteTranscriptionKind`, `wispr_flow.rs` client, and `base64` dependency from `navi-voice`

## [0.2.7] - 2026-07-16

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.6...v0.2.7

Reliability release focused on TUI terminal solidity (Kitty / multi-window), modal list navigation, and durable vision-tool session restore.

### New Features

- **Durable attachment store** — content-addressed blobs under `{data_dir}/attachments/` so `view_image` (and similar) bytes survive after the project file is deleted
- **Session replay** — `model_messages_from_agent_events` rebuilds full provider history from events, rehydrating tool images from path or attachment store
- **SDK `session_request_from_snapshot`** — reopen a saved session with rehydrated multimodal history

### Bug Fixes

- **TUI Kitty keyboard protocol** — negotiate progressive enhancement (`DISAMBIGUATE` + `REPORT_EVENT_TYPES`) instead of the no-op push-0/pop disable; `FocusGained` reasserts modes
- **Multi-window garbage input** — free mouse motion (`?1003`) only while image hover can fire; leak filter retained as safety net
- **Global shortcuts while typing** — Ctrl+letter works with composer draft; bare ASCII control-byte fallback; **Ctrl+X** opens Help when Ctrl+. needs Kitty
- **Agent / attachment model pickers** — open on first available model (Recent-safe); Down recovers from stale index; PageUp/PageDown; mouse wheel on BgModelPicker
- **Effort picker** — arrow keys move the cursor independently of the active level (highlight no longer stuck on current effort)
- **Provider vision tool wiring** — tool-result images sent as follow-up multimodal user content for OpenAI Chat/Responses, Gemini, and CommandCode (Anthropic keeps images in tool_result blocks)

### Bindings

- `@navi-agent/napi` 0.2.7 and platform packages
- `@navi-agent/navi` 0.2.7 CLI packages
- Workspace crate versions bumped to 0.2.7

## [0.2.6] - 2026-07-15

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.5...v0.2.6

Patch on top of the 0.2.4–0.2.5 agent/tooling work. For the full arc since **0.2.3** (plugins marketplace, browser, remote voice, skills rewrite, bindings parity, registry, TUI hubs), see the [0.2.4](#024---2026-07-15) and [0.2.5](#025---2026-07-15) sections below, or the GitHub compare: https://github.com/navi-ai-org/navi/compare/v0.2.3...v0.2.6

### Bug Fixes

- **Edit/write path agency** — accept absolute paths (and stop hard-rejecting after path normalization); project path jail is forced only in Restricted mode (optional elsewhere via `restrict_paths_to_project`)
- **Tool error TUI** — structured errors (`error` / `error_code` / `hint`) render as plain Code/Hint text instead of raw ` ```json ` dumps
- **Global shortcuts** — Ctrl+M and other Ctrl+letter chords work across terminal encodings (ASCII control bytes / empty Ctrl+Enter → model picker)
- **Registry on Windows** — model catalog filenames use `__` instead of `:` (illegal on Windows); ids restored at build time
- **navi-lite tests** — multi-thread Tokio runtime for health mission tests that use `block_in_place`

### Performance

- **Faster unit/CI graphs** — `navi-core` defaults no longer pull candle embeddings; product binaries (`navi-cli`, `navi-napi`) opt in explicitly
- **`navi-voice` default off onnx** — avoids ort-sys download/compile on ordinary test builds
- **Release/CI test gate** — drop full `navi-cli` bin link and `navi-voice` from unit gate; no step timeout on release tests; lean gate without the full sdk/tui/wasmtime graph
- **Package manager tests** — real cargo/npm/go/bun dispatch tests ignored on gate; `run_pkg` hard-caps at 30s

### Bindings

- `@navi-agent/napi` 0.2.6 and platform packages
- Workspace crate versions bumped to 0.2.6

## [0.2.5] - 2026-07-15

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.4...v0.2.5

### New Features

- **String-replace `edit` tool** (with multi-edit via `edits[]`) as the preferred coding edit path
- **Lean Direct tool schema** — small core surface listed to the model; power tools discovered via `tool_search` (Deferred/Hidden aliases)
- **Message queue UX** — remove items, clickable/hoverable `N queued` chip, preserve input draft on drain
- **Session recap** hard-capped to 3 lines (no full-file dumps in recap)

### Changes

- Remove `process`, `verifier`, and `branch_race_start` agent tools
- Redirect common bash file dumps (`sed` / `cat` / `rg` / `ls` / …) to native tools (`read_file` / `search`)
- Drop rustquty quality-metrics tooling from the repo

### Bindings

- `@navi-agent/napi` 0.2.5 and platform packages
- Workspace crate versions bumped to 0.2.5

## [0.2.4] - 2026-07-15

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.3...v0.2.4

Large feature release after 0.2.3: WASM plugins + marketplace, browser automation, remote voice, modular skills, full bindings parity, registry catalog, and substantial TUI/reliability work (~87 commits / ~55k lines across the 0.2.3→0.2.6 window, most of it landing here).

### Plugins & marketplace

- **WASM-only plugins** (ADR path) — drop native `navi-plugin-host` / libloading path; load via `wasmtime` + host brokers (FS/HTTP/git)
- **Marketplace** — catalog, example packages, validator, signed `hello-echo` artifact, Discord MCP package, package sidecars
- Install side effects: apply **skill** and **MCP** config merges (with confirmation); stage marketplace kinds (`plugin` | `skill` | `mcp` | `integration`)
- Host-mediated **TUI extensions** via `tui.json` (palette commands, phase 5)
- WASM runtime **enabled by default** with finished E2E install path

### Browser & server

- **`browser` tool** — headless automation (Cloak/CDP backends; `navi browser status|doctor|install`)
- Server routes and TUI hubs for browser/plugin status
- Vendor cloakbrowser stub so CI workspace resolves without external checkout

### Voice

- **Remote dictation** — OpenAI, Groq, Wispr Flow transcription clients
- Config: `[voice]` provider/model from `config.toml`; registry catalog for remote transcription providers
- SDK/CLI wire-up, remote doctor, cache/sync of transcription provider rows

### Skills

- **Modular skill store** (SQLite) with manage tools and skill CRUD APIs
- Drop deprecated `skills.dirs` and filesystem `SKILL.md` discovery
- NAPI/Dart/SDK surface for skill list/activate/CRUD

### Registry & models

- Remote **canonical model catalog** sync and provider base resolution
- **Model-specific effort levels** (registry `reasoning_levels` → picker; remove adaptive thinking / learning tutor mode)
- Effort wire mapping for providers; stabilize prefix cache (stop isolating Charm Hyper cache per session)
- xAI / Grok: Grok Build OAuth routing headers, device-code UX, weekly usage bars

### Tools & agent runtime

- **`repo_explore`** — BM25 + symbol search as a real tool (not a subagent)
- Kill timed-out **bash/process trees**; background timeouts return `ok=false` (no hung “Waiting for model”)
- Harden subagents; live subagent progress after background spawn
- Refined git guards; English-only harness/docs strings
- Lean tool protocol / plan–goal guidance in the system prompt

### Sessions & reliability

- **Rewind history** when editing a past user message
- Persist **partial model output** on turn error
- Resume mid-stream prefill; project credentials for memory CLI
- Cut Hyper credit burn via cache, titles, and dedicated memory model

### TUI

- **Desktop notifications** for finished unfocused turns/jobs (`tui.desktop_notifications`)
- **Self-update** + About modal
- **Setup wizard** — visual list for permission mode and marketplace tip
- **Plan** as modal + live progress strip / topbar (not chat JSON dumps)
- Jump to latest with Ctrl+Down; usage meter / command palette / settings / MCP status polish
- Text and image paste while the model is streaming; image hover lightbox
- Modal scroll fixes; reorganized modals; cleaner apply_patch diffs; numbered diff gutters
- Yield Ctrl+O/V to Providers and OAuth modals; remove double-click cancel

### SDK / NAPI / Dart

- Full engine surface for voice, memory, MCP, skills, plugins, accounts, routing models, session rewind, updates
- Close remaining `NaviEngine` gaps; Docker binding verifier (`scripts/test-bindings-docker.sh`)
- `navi-dart` C ABI gap-fill to match SDK capabilities
- `@navi-agent/napi` 0.2.4 + platform packages

### Performance & CI

- Cut session bloat, SQLite thrash, and streaming TUI cost
- Faster PR matrix and lighter TUI harness
- CI no longer runs on bare pushes to `main` (tags, PRs, manual)
- Multi-agent tool-quality **benchmark suite** + token extractors (navi / OpenCode / Claude Code)

### Bindings

- `@navi-agent/napi` 0.2.4 and platform packages
- `navi-dart` 0.2.4 C ABI gap-fill
- `scripts/test-bindings-docker.sh` Docker binding verifier

### Chores

- Version bump to 0.2.4

## [0.2.3] - 2026-07-09

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.2...v0.2.3

### New Features

- TUI: colored write/edit diffs, plain process/patch tool streams, boxed markdown tables
- TUI: jump-to-latest control, hover context % chip, session USD/credit spend tracking
- Session recap + per-turn context token meter; plan tools polish
- Local voice ASR surface for desktop clients
- Image hover previews (Kitty/Sixel/iTerm2) and `[Image N]` chips

### Bug Fixes

- **Prompt cache / quota**: stop double system prompt on Chat Completions; stable tool order for prefix caching; cache-aware Hyper credit estimates
- **Multimodal Grok/xAI**: treat unknown SKUs via provider defaults; fix Ctrl+R sync inheritance so new models (e.g. `grok-4.5`) get vision/context from defaults + family siblings instead of bare `NULL` rows
- Registry snapshot: add `grok-4.5` / `grok-4.20` with `supports_images`
- Context meter: include cached tokens from aggregator usage reports
- Charm Hyper credits reporting + embedded pricing fallback when SQLite pricing is empty

### Chores

- Version bump to 0.2.3
- CI runs on push/PR/tags; Release gates on tests before multi-platform publish
- ONNX voice is optional (`--features voice-onnx`) for portable musl/Windows builds

## [0.2.2] - 2026-07-09

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.0...v0.2.2

### New Features

- Add **`navi-lite`**: sealed, mission-scoped headless runtime for edge/embedded Linux prototypes (feature-gated `navi-core` without embeddings, TUI, MCP, or plugins)
- Ship **`navi-lite`** prebuilt binaries alongside full `navi` for all platforms
- Ship **portable musl Linux binaries** (Alpine toolchain) for containers and enterprise images
- Add **xAI Composer 2.5** models (`composer-2.5`, `grok-composer-2.5-fast`)
- Harden installers: strict SHA-256, single-file archives, optional Sigstore verification
- Sign `SHA256SUMS.txt` keyless with Sigstore (GitHub Actions OIDC)

### Bug Fixes

- Make `install.sh` POSIX/dash-safe (`curl | sh` on Ubuntu/Alpine)
- Reject unsafe multi-member release archives during install
- Fix Linux arm64 musl release builds (Docker Alpine on arm runners)
- Fix macOS package validation without bash `mapfile`

### Documentation

- Document `navi-lite` sealed edge runtime and mission allowlist model
- Install security controls and container/Linux portability notes
- Sample Alpine Dockerfile for agent sidecars

### Chores

- Drop OpenSSL/`native-tls` (`hf-hub` on rustls) to enable musl builds
- CI builds `navi-lite` and checks the lite binary
- Stricter multi-asset release packaging and checksum validation

## [0.2.0] - 2026-07-08

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.1.2...v0.2.0

### New Features

- First public multi-platform prebuilt binaries and one-line installer
- Plan Mode, goals, multi-provider registry, OAuth, session cost estimates

### Bug Fixes

- Registry merge, concurrent SQLite, deferred MCP tools, TUI layout

### Documentation

- Public install path and first-release notes

### Chores

- Dependency and registry snapshot updates for the binary release

## [0.1.2] - 2026-07-04

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.1.0...v0.1.2

### New Features

- Multimodal attachments and `analyze_attachment`

### Bug Fixes

- Compact image indicators; registry background tasks without Tokio runtime

### Documentation

- Multimodal release notes

### Chores

- Provider media request mapping updates

## [0.1.0] - 2026-06-29

Full changelog: https://github.com/navi-ai-org/navi/releases/tag/v0.1.0

### New Features

- Initial open-source scaffold of the NAVI agent engine and TUI

[Unreleased]: https://github.com/navi-ai-org/navi/compare/v0.3.6...HEAD
[0.3.6]: https://github.com/navi-ai-org/navi/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/navi-ai-org/navi/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/navi-ai-org/navi/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/navi-ai-org/navi/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/navi-ai-org/navi/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/navi-ai-org/navi/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/navi-ai-org/navi/compare/v0.2.7...v0.3.0
[0.2.7]: https://github.com/navi-ai-org/navi/compare/v0.2.6...v0.2.7
[0.2.6]: https://github.com/navi-ai-org/navi/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/navi-ai-org/navi/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/navi-ai-org/navi/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/navi-ai-org/navi/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/navi-ai-org/navi/compare/v0.2.0...v0.2.2
[0.2.0]: https://github.com/navi-ai-org/navi/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/navi-ai-org/navi/compare/v0.1.0...v0.1.2
[0.1.0]: https://github.com/navi-ai-org/navi/releases/tag/v0.1.0
