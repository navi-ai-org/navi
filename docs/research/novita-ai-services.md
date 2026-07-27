# Novita AI Services Research

## Overview

Novita AI is an AI API platform offering OpenAI-compatible inference, image
generation, and embeddings. The key services relevant to NAVI are:

- **LLM Chat Completions** (OpenAI-compatible) — core text generation
- **Embeddings** — for memory/RAG use cases
- **Image Generation** (SDXL, Flux, etc.) — not core to NAVI (coding agent)

> **Full model catalog:** see [`novita-models-full.md`](./novita-models-full.md) for an
> exhaustive inventory of all 142 LLM/chat models plus embedding, reranker, image,
> video, audio, and AI-search services on the Novita platform. The catalog is compiled
> from the live `GET https://api.novita.ai/v3/openai/models` endpoint (authoritative,
> machine-readable) cross-referenced with the public `https://novita.ai/pricing` page.

## API Details

### LLM Chat Completions (Primary)

- **Base URL:** `https://api.novita.ai/v3/openai`
- **Endpoint:** `POST /chat/completions`
- **Auth:** `Authorization: Bearer <NOVITA_API_KEY>` (standard Bearer header)
- **Format:** OpenAI-compatible — same request/response shape, streaming via SSE
- **Models:** 142 LLM/chat models across DeepSeek, Qwen, GLM, Kimi, MiniMax, Llama, Gemma, ERNIE, and more — see the [full model catalog](./novita-models-full.md) for the complete list with context windows, max output, capabilities, and pricing
- **Features:** Streaming, tool/function calling, JSON mode
- **Pricing:** Per-token (input + output), competitive pricing on open models

### Embeddings

- **Base URL:** `https://api.novita.ai/v3/openai`
- **Endpoint:** `POST /embeddings`
- **Auth:** Same Bearer header
- **Format:** OpenAI-compatible embeddings API
- **Use case:** Memory embedding, semantic search (NAVI auto-memory)

### Image Generation (not core to NAVI)

- Separate REST API at `https://api.novita.ai/v3/`
- SDXL, Flux, and other diffusion models
- Not relevant to NAVI's coding agent use case

## Authentication

- API key obtained from Novita dashboard: `https://novita.ai/key`
- Env var convention: `NOVITA_API_KEY`
- Standard `Authorization: Bearer` header

## OpenAI Compatibility

Novita's chat completions API is fully OpenAI-compatible:
- Same request body schema (model, messages, temperature, max_tokens, tools, etc.)
- Same streaming format (SSE `data:` lines)
- Same tool_call response format
- Same usage statistics

This means NAVI's existing `OpenAiProvider` + `OpenAiChatCompletions` kind works
out-of-the-box — no custom behavior needed beyond standard Bearer auth.

## Registry Integration (Aggregator Mode)

Novita is registered in `crates/navi-core/registry/providers/novita.json` with
`"aggregator": true`. This tells NAVI to treat the provider as an **aggregator**:
the static model list seeded in the registry JSON (109 active LLM/chat models from
the full catalog) is a fallback/seed, and at sync time NAVI fetches the live model
list from the provider's OpenAI-compatible `/models` endpoint
(`https://api.novita.ai/v3/openai/models`) via the shared
`sync_aggregator_models` path. This keeps the available-model set current as
Novita adds or retires models without requiring a registry re-release.

The seeded JSON includes only **active** LLM/chat models — archived models, test/dev
models (`ai_infer_test_*`, `dev/*`, `gt-4p`), and non-LLM services (embeddings,
rerankers, image, video, audio, AI search) are excluded since they use separate
endpoints and are not part of the chat-completions surface.

## Sources

- https://novita.ai/docs/guides/introduction
- https://novita.ai/docs/api-reference
- https://novita.ai/key (API key management)
