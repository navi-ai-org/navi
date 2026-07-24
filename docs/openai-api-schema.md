# OpenAI API Schema Reference (Chat Completions / Responses / Models)

> Research artifact generated for the NAVI `navi-openai` provider audit.  
> Covers the public REST surface that NAVI consumes or may pass through.

## 1. Chat Completions — `POST /v1/chat/completions`

### Request body

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `model` | string | yes | — | Model ID, e.g. `gpt-5`, `gpt-4.1`, `o4-mini`. |
| `messages` | array | yes | — | Conversation messages (see below). |
| `max_completion_tokens` | integer | no | null | Hard limit on generated tokens. Preferred over `max_tokens`. |
| `max_tokens` | integer | no | null | Deprecated alias. Use `max_completion_tokens` when possible. |
| `temperature` | number | no | 1.0 | Sampling temperature. 0–2. |
| `top_p` | number | no | 1.0 | Nucleus sampling. |
| `n` | integer | no | 1 | Number of completions to generate. NAVI only needs `n=1`. |
| `stream` | boolean | no | false | Whether to stream SSE `chat.completion.chunk` events. |
| `stream_options` | object | no | — | `{ "include_usage": boolean }` when streaming. |
| `tools` | array | no | — | Function tool definitions. |
| `tool_choice` | string / object | no | "none" if no tools, else "auto" | `"none"`, `"auto"`, `"required"`, or `{"type":"function","function":{"name":"..."}}`. |
| `parallel_tool_calls` | boolean | no | provider-specific | Set to `false` to force a single tool call. |
| `response_format` | object | no | `{ "type": "text" }` | `text` or `json_schema` / `json_object`. |
| `reasoning_effort` | string | no | "medium" | For reasoning models (`o4`, `o3`, `o1`). `"low"`, `"medium"`, `"high"`. |
| `prediction` | object | no | — | `{"type":"content","content":"..."}` for prompt caching via prediction. |
| `audio` | object | no | — | Output audio configuration when `modalities` includes `audio`. |
| `modalities` | array | no | `["text"]` | Allowed: `text`, `audio`, `image`. |
| `metadata` | object | no | — | Up to 16 key-value pairs of metadata. |
| `store` | boolean | no | false | Persist the request for later retrieval. |
| `service_tier` | string | no | "auto" | `"auto"`, `"default"`, `"flex"`. |
| `seed` | integer | no | — | Deterministic sampling seed. |
| `user` | string | no | — | End-user identifier. |
| `presence_penalty` | number | no | 0 | -2.0 to 2.0. |
| `frequency_penalty` | number | no | 0 | -2.0 to 2.0. |
| `logit_bias` | object | no | — | Token bias map. |
| `logprobs` | boolean | no | false | Return logprobs. NAVI does not consume. |
| `top_logprobs` | integer | no | null | 0–20, requires `logprobs=true`. |
| `function_call` | string / object | no | — | Deprecated. Use `tool_choice`. |
| `functions` | array | no | — | Deprecated. Use `tools`. |

### Message object

| Field | Type | Roles | Description |
|-------|------|-------|-------------|
| `role` | string | all | `system`, `developer`, `user`, `assistant`, `tool`. |
| `content` | string / array | all | Text or content-parts array. |
| `name` | string | `system`, `developer`, `user` | Optional participant name. |
| `tool_calls` | array | `assistant` | Tool invocations the model wants to make. |
| `tool_call_id` | string | `tool` | Required. ID from the assistant tool call. |
| `refusal` | string | `assistant` | Model refusal message, if any. |

### Content part objects

| Type | Fields |
|------|--------|
| `input_text` | `{ "type": "input_text", "text": "..." }` |
| `input_image` | `{ "type": "input_image", "image_url": "..." \| { "url": "...", "detail": "auto" } }` |
| `input_file` | `{ "type": "input_file", "file_url": "..." \| { "url": "..." }, "filename": "..." }` |
| `input_audio` | `{ "type": "input_audio", "audio_url": "data:..." \| { "url": "..." }, "format": "wav" }` |

For assistant messages in Chat Completions the array uses `text` and `image` parts for output images.

### Tool definition

```json
{
  "type": "function",
  "function": {
    "name": "...",
    "description": "...",
    "parameters": { "type": "object", "properties": {}, "required": [] },
    "strict": false
  }
}
```

### Response object (non-streaming)

```json
{
  "id": "chatcmpl-...",
  "object": "chat.completion",
  "created": 1234567890,
  "model": "gpt-5",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "...",
        "tool_calls": [...],
        "refusal": null,
        "audio": null
      },
      "finish_reason": "stop",
      "logprobs": null
    }
  ],
  "usage": {
    "prompt_tokens": 10,
    "completion_tokens": 20,
    "total_tokens": 30,
    "prompt_tokens_details": { "audio_tokens": 0, "cached_tokens": 0 },
    "completion_tokens_details": { "audio_tokens": 0, "reasoning_tokens": 0, "accepted_prediction_tokens": 0, "rejected_prediction_tokens": 0 }
  },
  "service_tier": "default",
  "system_fingerprint": "fp_..."
}
```

### Streaming chunk (`chat.completion.chunk`)

```json
{
  "id": "chatcmpl-...",
  "object": "chat.completion.chunk",
  "created": 1234567890,
  "model": "gpt-5",
  "choices": [
    {
      "index": 0,
      "delta": { "role": "assistant", "content": "...", "tool_calls": [...] },
      "finish_reason": null,
      "logprobs": null
    }
  ],
  "usage": { "prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30, ... }
}
```

Important SSE events:
- `data: {...}` one or more times.
- `data: [DONE]` ends the stream.
- For tool-call streaming, `delta.tool_calls` contains partial objects with `index`, `id`, `function.name`, `function.arguments`.

---

## 2. Responses API — `POST /v1/responses`

### Request body

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `model` | string | yes | — | Model ID. |
| `input` | array / string | yes | — | Input items or single text. |
| `instructions` | string | no | — | System/developer instructions, separate from input. |
| `max_tokens` | integer | no | — | Max output tokens. |
| `temperature` | number | no | 1.0 | Sampling temperature. |
| `top_p` | number | no | 1.0 | Nucleus sampling. |
| `stream` | boolean | no | false | Stream SSE events. |
| `stream_options` | object | no | — | `{ "include_usage": boolean }`. |
| `tools` | array | no | — | Function tool definitions (sibling shape to Chat Completions, but `parameters` at top level for Responses). |
| `tool_choice` | string / object | no | "auto" | `"none"`, `"auto"`, `"required"`, or `{"type":"function","name":"..."}`. |
| `parallel_tool_calls` | boolean | no | provider-specific | Set to `false` to disable. |
| `reasoning` | object | no | — | `{ "effort": "low" \| "medium" \| "high" }` for reasoning models. |
| `text` | object | no | `{ "format": { "type": "text" } }` | Response text format, may include `json_schema`. |
| `previous_response_id` | string | no | — | Continue a previous response. |
| `truncation` | string | no | "disabled" | `"disabled"` or `"auto"`. |
| `store` | boolean | no | true | Persist the response for later retrieval. |
| `metadata` | object | no | — | Key-value metadata. |
| `user` | string | no | — | End-user identifier. |

### Input item types

| Type | Fields |
|------|--------|
| `message` | `{ "type": "message", "role": "user" \| "system" \| "developer", "content": "..." \| [...] }` |
| `function_call` | `{ "type": "function_call", "call_id": "...", "name": "...", "arguments": "..." }` |
| `function_call_output` | `{ "type": "function_call_output", "call_id": "...", "output": "..." }` |
| `reasoning` | `{ "type": "reasoning", "summary": [...] }` |
| `file_search_call` | `{ "type": "file_search_call", ... }` |
| `computer_call` | `{ "type": "computer_call", ... }` |
| `computer_call_output` | `{ "type": "computer_call_output", ... }` |

Content parts inside `message.content` are similar to Chat Completions but use Responses-style type names (`input_text`, `input_image`, `input_file`, `input_audio`).

### Tool definition (Responses)

```json
{
  "type": "function",
  "name": "...",
  "description": "...",
  "parameters": { "type": "object", ... },
  "strict": false
}
```

Note: In Responses API the `parameters` object is at the top level, not nested under `function`.

### Response object

```json
{
  "id": "resp_...",
  "object": "response",
  "created_at": 1234567890,
  "status": "completed",
  "model": "gpt-5",
  "output": [
    { "type": "message", "role": "assistant", "content": [...] },
    { "type": "function_call", "call_id": "...", "name": "...", "arguments": "..." },
    { "type": "reasoning", "summary": [...] }
  ],
  "output_text": "shortcut text",
  "reasoning": "...",
  "tool_calls": [...],
  "usage": {
    "input_tokens": 10,
    "output_tokens": 20,
    "total_tokens": 30,
    "input_tokens_details": { "cached_tokens": 0 },
    "output_tokens_details": { "reasoning_tokens": 0 }
  },
  "error": null,
  "incomplete_details": null,
  "instructions": null,
  "input": [...]
}
```

### Streaming events (Responses SSE)

Common event `type` values:
- `response.created`
- `response.output_text.delta` → `{ "delta": "..." }`
- `response.output_text.done`
- `response.reasoning_summary_text.delta` → reasoning text
- `response.reasoning_summary_text.done`
- `response.output_item.added` → `{ "item": { "type": "function_call", ... } }`
- `response.function_call_arguments.delta` → `{ "item_id" \| "call_id", "delta": "..." }`
- `response.output_item.done` → final item
- `response.completed` → `{ "response": { ..., "usage": {...} } }`
- `response.failed` → `{ "response": { "error": {...} } }`
- `response.incomplete` → `{ "response": { "incomplete_details": {...} } }`

---

## 3. Models — `GET /v1/models`

### Response

```json
{
  "object": "list",
  "data": [
    {
      "id": "gpt-5",
      "object": "model",
      "created": 1234567890,
      "owned_by": "openai"
    }
  ]
}
```

Only `id` is strictly required for NAVI’s model listing; the rest may be ignored.

---

## 4. Gaps vs. current `navi-openai` (audit checklist)

The following are not yet fully represented in `navi-openai` and should be reviewed for pass-through support:

### Chat Completions request gaps

- [ ] `response_format` (text/json/json_schema) is not wired.
- [ ] `max_completion_tokens` is not emitted (only `max_tokens` is missing too).
- [ ] `temperature`, `top_p`, `presence_penalty`, `frequency_penalty`, `seed`, `user`, `logit_bias` are not forwarded.
- [ ] `service_tier`, `store`, `metadata` are not forwarded.
- [ ] `audio`, `modalities`, `prediction` are not forwarded.
- [ ] `tool_choice` values `"required"` and explicit function objects are only partially handled.
- [ ] `n` is not configurable; NAVI always behaves like `n=1`.

### Chat Completions response parsing gaps

- [ ] `refusal` field on assistant message is ignored.
- [ ] `audio` output field is ignored.
- [ ] `logprobs` / `top_logprobs` are ignored (acceptable for NAVI, but should be tolerated in JSON).
- [ ] `service_tier` and `system_fingerprint` are not surfaced.
- [ ] `usage.completion_tokens_details` is not parsed.

### Responses API request gaps

- [ ] `text.format` (json_schema / text) is not wired.
- [ ] `truncation`, `previous_response_id`, `store`, `metadata`, `user` are not forwarded.
- [ ] `temperature`, `top_p`, `max_tokens` are not forwarded.
- [ ] `input` string shortcut is not supported (NAVI always emits items array).

### Responses API response parsing gaps

- [ ] `response.incomplete` event is not handled.
- [ ] `response.created` / other status events are ignored (safe, but should be noted).
- [ ] `error`, `incomplete_details`, `output_text` are not parsed (shortcut text exists as helper but not from actual response).
- [ ] `reasoning` top-level field and `reasoning.summary` items are not parsed.
- [ ] `tool_calls` top-level shortcut is not parsed.
- [ ] `usage.output_tokens_details` is not parsed.

### Models

- [x] `GET /v1/models` is implemented and parses `data[].id`.

---

## 5. Implementation strategy for NAVI

`navi-openai` is an engine *provider*, not a full OpenAI SDK. 100% API coverage does not mean every field must be a first-class engine concept; it means:

1. **Known fields** that NAVI can produce (`ModelRequest`/`ProviderConfig`/`ToolDefinition`) are serialized with the correct OpenAI names and shapes.
2. **Unknown / optional fields** exposed through `ProviderRequestOptions.extra_body` or `extra_headers` are forwarded transparently.
3. **Response fields** not used by the engine are *tolerated* in JSON parsing and ignored gracefully, so a 1:1 response object never causes deserialization failures.
4. **Tests** assert exact wire format for every supported field using `wiremock`.

This document should be treated as the source-of-truth spec for the audit and implementation work.
