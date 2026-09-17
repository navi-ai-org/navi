## Highlights

**0.7.1** fixes OpenCode Go connectivity. Every request now carries the stable
`x-opencode-session` header the Go gateway requires — including the
Anthropic-messages path, which previously failed with `MissingSessionID`.
Model errors are reported with the real cause instead of a misleading
"Authentication failed", free-tier models selected on the Go endpoint get
actionable guidance, and the OpenCode free-model catalog is synced with the
current registry snapshot.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.7.0...v0.7.1

### Fixed

- **OpenCode Go `MissingSessionID`** — `stream_anthropic_messages` never
  applied per-request session headers, and on this tree the OpenCode
  behaviors did not emit the official client fingerprint at all, so requests
  went out without a stable `x-opencode-session` and the Go gateway rejected
  them with `400 MissingSessionID`. All four stream paths (responses,
  chat-completions, anthropic-messages, gemini) now apply session headers, and
  every OpenCode request carries `User-Agent: opencode` with
  `x-opencode-client` / `x-opencode-project` / rotating `x-opencode-request`
  correlation headers; auxiliary calls without a session get generated
  fallback ids. Header values are sanitized (visible ASCII, capped length) so
  a malformed session id can never break a request.
- **Misleading auth errors** — a `401 ModelError` ("model is not supported")
  from the Zen/Go gateway was wrapped as "Authentication failed". Error
  formatting now maps `MissingSessionID`, `FreeTierError`, and `ModelError` to
  messages that name the real cause (`ctrl+m` to pick a model included in the
  Go plan).
- **Free-tier models on OpenCode Go** — free-tier models (Zen-only) selected on
  the Go endpoint now return actionable guidance instead of a bare rejection.
- **OpenCode free-model catalog** — the free-model allowlist is synced with the
  current registry snapshot: `laguna-s-2.1-free`, `nemotron-3.5-lightning-free`,
  `ox-alpha-free` / `x-preview-f-free`, and `muse-spark-1.2-contributor-free`
  are now recognized. A consistency test guards the allowlist against the
  pinned embedded registry.

### Upgrade

```sh
curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh
```

Or pin the version:

```sh
curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh -s -- --version v0.7.1
```

### Verification

- `navi-openai`: 297 unit tests pass, including a new wiremock test that the
  Claude-messages endpoint carries the stable session header.
- `navi-core`: OpenCode/Zen free-list tests pass; `cargo fmt --check` clean.
- TUI screenshot goldens (23) and CLI PTY smoke pass.
