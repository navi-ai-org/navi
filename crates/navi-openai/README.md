# navi-openai

[![Crates.io](https://img.shields.io/crates/v/navi-openai)](https://crates.io/crates/navi-openai)
[![License](https://img.shields.io/crates/l/navi-openai)](../LICENSE)

`ModelProvider` implementation for OpenAI-compatible APIs — the provider engine behind [NAVI](https://github.com/navi-ai-org/navi).

All NAVI providers (OpenAI, Anthropic, Google Gemini, OpenRouter, GitHub Copilot, xAI, and custom endpoints) are routed through this crate via protocol adapters.

## Supported protocols

| Protocol | Providers |
|----------|-----------|
| `openai-responses` | OpenAI, xAI (Responses-style reasoning) |
| `openai-chat-completions` | Anthropic, Google Gemini, OpenRouter, custom OpenAI-compatible |

## Provider adapters

Some provider IDs trigger special behavior in the stream layer:

- **`anthropic`** — Anthropic Messages streaming format
- **`google-gemini`** — Gemini Generate Content streaming
- **`openrouter`** — required headers and reasoning config
- **`github-copilot`** — device OAuth bearer tokens and Copilot request headers
- **`openai` / `xai`** — Responses-style reasoning effort

## What's inside

| Module | Purpose |
|--------|---------|
| `provider` | `OpenAiProvider` implementing `navi_core::ModelProvider` |
| `transport` | HTTP streaming, SSE parsing, and request dispatch |
| `sse` | Server-Sent Events parser for streaming responses |
| `mapping` | Wire-format translation between NAVI model types and provider APIs |
| `oauth` | Device OAuth flows for OpenAI and GitHub Copilot |
| `errors` | `ProviderError` types with retry classification |
| `types` | `OpenAiApiKind` and provider-specific configuration |

## Usage

You normally don't depend on `navi-openai` directly — use [`navi-providers`](https://crates.io/crates/navi-providers) instead, which re-exports this crate's public API and can be swapped without downstream churn.

```rust
use navi_providers::{OpenAiProvider, OpenAiApiKind};
```

## Part of the NAVI workspace

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
