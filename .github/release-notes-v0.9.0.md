## Highlights

**0.9.0 is a breaking release** — speech-to-text is removed from the product, and
it is not coming back. Everything else is additive or a fix, and there are no
config or storage migrations beyond deleting `[voice]`.

- **Speech-to-text removed.** The `navi-voice` crate, the `[voice]` config table,
  `navi voice`, the `/voice/*` HTTP routes and the `voice*` bindings in
  `navi-napi` / `navi-dart` are gone, along with the registry's
  transcription-provider catalog. Keeping per-vendor dictation APIs current is
  not something a solo maintainer can carry, so dictation is the user's own
  integration now. Existing `config.toml` files with a `[voice]` table still
  load — serde ignores the unknown key.
- **Every provider catalog re-verified against the vendor's current lineup.**
  32 provider files re-checked against primary sources (vendor docs, pricing
  pages, live `/models` endpoints): **191 models added** (gpt-6 family,
  `claude-opus-5-5`, `claude-fable-5/-5-1`, `grok-4.7`, the GLM-5.3 family,
  `kimi-k3`, the qwen3.8 family, the MiMo-V2.6 family, Gemini 3.5–3.8,
  `step-5-preview`, `gpt-transcribe`, …) and **251 retired entries dropped**
  (DeepSeek v3.x/R1/coder, Claude 3.x, o1/o3/o4, legacy GPT-4o/4.1, Gemini
  1.5/2.0, grok-2/3, moonshot-v1, abab6.5, `whisper-1`, …). New canonical model
  files carry `sources` provenance for context window, output limits and pricing.
- **`navi registry sync` survives a flaky network.** It used to abort the whole
  sync on the first DNS/TLS hiccup; fetches now retry transient failures only.
- **Tool calling on aggregator gateways works again.** Duplicate empty
  tool-call ids and a forced `tool_choice` in thinking mode both caused hard
  400s on b.ai-backed models — both fixed.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.8.2...v0.9.0

### Removed

- **The speech-to-text stack.** `crates/navi-voice` (recorders, WAV handling,
  remote OpenAI-compatible transcription client, doctor), `[voice]` in
  `NaviConfig`, `navi voice {status,providers,doctor,transcribe}`,
  `navi-sdk`'s `voice` module and `VoiceConfigUpdate`, the `navi-server`
  `/voice/*` routes (and the dev proxy entry), the `navi-napi` `voice*` methods,
  the `navi-dart` `navi_engine_voice_*` FFI, and `navi-core`'s transcription
  catalog: `transcription_catalog.rs`, `TranscriptionProviderKind`,
  `RegistryTranscriptionProvider/Model/Pricing`,
  `RegistryManifest.transcription_providers`, the `transcription_providers`
  table with its CRUD/seed/prune helpers, `fetch_transcription_provider`,
  `download_transcription_updates` and
  `apply_registry_update_atomically_with_transcription`.

### Added

- **b.ai:** `mimo-v2.6-pro` ($0.435 in / $0.0036 cache-read / $0.87 out per 1M)
  and `mimo-v2.6-flash` ($0.14 / $0.0028 / $0.28), with pricing and context
  provenance from their B.AI model pages. The b.ai catalog is now exactly the
  live `/v1/models` list: 51 rows, none missing, none stale.

### Fixed

- **`navi registry sync` retries transient failures.** The fetcher aborted on
  the first network error because the CLI path used the bare fetcher while the
  retry helpers only covered the background update path. Registry fetches
  (manifest, providers, canonical models) now retry up to 3 times with
  backoff, and only for transient failures — transport errors plus
  408/425/429/5xx. 4xx fail fast, and the final error names the attempt count
  and a recovery hint.

- **Model ids stored under a Windows-safe filename resolve again.**
  `embedded_model_catalog()` keyed the catalog by the on-disk label, so ids like
  `gemma3:12b`, `qwen2.5-coder:32b` and `nemotron-3-ultra-550b-a55b:free` never
  matched. Their canonical metadata (max output, thinking, reasoning levels,
  attachments) was silently dropped and the model fell back to its `api_name`.
  The catalog is now keyed by the JSON `id`, matching the SQLite store and the
  local-directory loader.

- **Tool calls no longer carry duplicate empty ids into the next request.**
  OpenAI-compatible gateways stream continuation chunks with `"id": ""`
  (observed on b.ai + `qwen3.8-flash`); the accumulator overwrote the captured
  id with that empty string, so a two-call batch produced
  `Duplicate value for 'tool_call_id' of  in message[N]`. Empty ids are ignored
  while streaming, a call that still finishes without one gets a unique
  `navi_call_N`, and `repair_tool_call_pairing` renames empty ids in rebuilt
  history while re-pointing their results at the new id.

- **Aggregator providers are no longer sent a forced `tool_choice`.** Several
  upstreams behind a gateway reject a forced object `tool_choice` while the
  model runs in thinking mode (b.ai + `qwen3.8-flash` answers HTTP 400, surfaced
  as `openai_error / bad_response_status_code`), which broke the first turn of
  every new session on those models. The session-title nudge still lives in the
  system prompt and the runtime still derives a fallback title.

- **A prune-only registry merge reloads the cache.**
  `merge_embedded_provider_updates` reported "nothing changed" when all it did
  was delete providers that vanished upstream, so `load_registry` kept serving a
  snapshot that still listed them.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.9.0
```
