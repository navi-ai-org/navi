# Providers

Provider configuration is defined in `navi-core/src/config.rs`. Runtime provider calls are implemented in `navi-openai`.

## Protocol Kinds

`ProviderKind` has two configured values:

- `openai-responses`
- `openai-chat-completions`

`navi-openai` maps these to concrete request paths. Some providers need special handling even when they are configured as chat-completions.

## Current Provider Implementation

`OpenAiProvider` supports:

- OpenAI Responses API: `/responses`
- OpenAI-compatible Chat Completions: `/chat/completions`
- Anthropic Messages adapter for provider id `anthropic`
- Gemini Generate Content adapter for provider id `google-gemini`
- OpenRouter headers for provider id `openrouter`
- GitHub Copilot OAuth bearer tokens and request headers for provider id `github-copilot`

`list_models()` calls the provider's `/models` endpoint when supported. The model picker can sync one provider or all providers.

## Credentials

Provider keys are resolved in this order:

1. Environment variable declared by `ProviderConfig.api_key_env`
2. Provider-specific external auth sources, for example OpenCode Zen's `auth.json`
3. Credential store under NAVI's data directory

The TUI should not ask for API keys on startup. It asks only when the user selects a model whose provider has no resolved key. Settings includes `Provider Accounts`, a provider configuration modal that shows configured/unconfigured status, opens API key setup, syncs provider models, and starts OAuth for compatible providers.

## OAuth

OAuth is provider-specific. Do not add a generic OAuth toggle unless the provider documents a compatible authorization flow and the request adapter knows how to use the resulting token.

Currently implemented:

| Provider | Flow | Storage | Runtime usage |
|---|---|---|---|
| GitHub Copilot | GitHub device authorization | NAVI credential store as bearer token | `Authorization: Bearer`, `Openai-Intent: conversation-edits`, `x-initiator` |

Most API providers still use API keys. In the provider modal, `OAuth` is shown only for providers with a real implementation; otherwise setup uses the provider's API key.

## Thinking Adapter

The UI exposes a provider-neutral thinking scale:

- `max`
- `high`
- `medium`
- `low`
- `off`

`ThinkingConfig::adapter_for_provider` maps those values to provider-specific request fields:

| Provider | Mapping |
|---|---|
| OpenAI / xAI Responses | `reasoning.effort` when not `off` |
| Anthropic | `thinking` object with budget tokens |
| Gemini | `thinkingConfig.thinkingBudget` |
| OpenRouter | `reasoning.effort` plus `exclude: true` |
| Groq | `reasoning_effort` style values |
| Other chat-completions providers | OpenAI-like effort where supported, otherwise unsupported |

When adding providers, update the adapter only when the provider has documented thinking/reasoning parameters. Do not blindly send unsupported fields.

## Tool Calling

Tool definitions are attached to `ModelRequest.tools`. The provider implementation serializes them into the appropriate schema:

- Responses API tools
- Chat Completions tools

Tool transcripts must preserve provider protocol shape:

- Chat Completions: assistant messages include `tool_calls`, then role `tool` messages include `tool_call_id`.
- Responses: assistant calls are sent as `function_call` input items, and results are sent as `function_call_output`.

Provider adapters can reject tools if native tool calling is not implemented for that protocol. Keep this explicit so unsupported combinations fail clearly instead of producing malformed provider requests.

## Model Catalog

Built-in providers and model lists are in `built_in_providers()`. Project/global config can override or add providers through `[providers]`.

Use `cargo run -p navi-cli -- --print-providers` to inspect the resolved catalog.

## Adding A Provider

1. Add or override `ProviderConfig` with id, label, kind, env var, base URL, and models.
2. Confirm whether it truly accepts OpenAI-compatible request bodies.
3. Add special request/stream parsing in `navi-openai` only if the provider differs from the selected protocol.
4. Add thinking adapter behavior if supported.
5. Add tests for request body generation and SSE/event parsing.
6. Verify with `cargo test -p navi-openai` and `cargo check`.
