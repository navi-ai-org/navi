# Novita AI Services Research

## Overview

Novita AI is an AI API platform offering OpenAI-compatible inference, image
generation, and embeddings. The key services relevant to NAVI are:

- **LLM Chat Completions** (OpenAI-compatible) — core text generation
- **Embeddings** — for memory/RAG use cases
- **Image Generation** (SDXL, Flux, etc.) — not core to NAVI (coding agent)

## API Details

### LLM Chat Completions (Primary)

- **Base URL:** `https://api.novita.ai/v3/openai`
- **Endpoint:** `POST /chat/completions`
- **Auth:** `Authorization: Bearer <NOVITA_API_KEY>` (standard Bearer header)
- **Format:** OpenAI-compatible — same request/response shape, streaming via SSE
- **Models:** DeepSeek V3, DeepSeek R1, Llama 3.3, Qwen, and other open models
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

## Sources

- https://novita.ai/docs/guides/introduction
- https://novita.ai/docs/api-reference
- https://novita.ai/key (API key management)
