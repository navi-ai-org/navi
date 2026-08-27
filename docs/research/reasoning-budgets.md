# Reasoning / Thinking Token Budgets by Provider

This summary collects the actual token-budget semantics for reasoning/thinking
controls across OpenAI, Anthropic, Google Gemini, and OpenRouter.  Values are
only included when they appear in first-party documentation, API schemas, or
official SDK source files.  Gaps are explicitly marked.

## Summary of the `max` vs `xhigh` question

| Provider | Are `max` and `xhigh` the same budget? | Evidence |
| --- | --- | --- |
| **OpenRouter** | **Yes** | OpenRouter states `xhigh` has the *"Same allocation as max"* (~95% of `max_tokens`). |
| **Anthropic** | **No** | `max` is *"absolute maximum capability with no constraints on token spending"*; `xhigh` is a separate *"extended capability for long-horizon work"* level. |
| **OpenAI** | Unknown / not published | Both are distinct enum values in the SDK and docs, but no fixed token budget is disclosed. |
| **Google Gemini** | N/A | Gemini 3 uses `minimal/low/medium/high` and Gemini 2.5 uses an integer `thinkingBudget`; there are no `xhigh` or `max` levels. |

---

## 1. OpenAI — `reasoning.effort` / `reasoning_effort`

OpenAI exposes `reasoning.effort` (or the older `reasoning_effort` on Chat
Completions).  The supported set is model-dependent.

### Supported values (from the official Python SDK)

The `Reasoning` shared param defines the `effort` field with possible values
`none`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`.  Not every model
accepts every value:

- `gpt-5.1` defaults to `none` and supports `none`, `low`, `medium`, `high`.
- Models before `gpt-5.1` default to `medium` and do not support `none`.
- `gpt-5-pro` defaults to and only supports `high`.
- `xhigh` is supported for models after `gpt-5.1-codex-max`.

**Sources:**
- OpenAI Python SDK `src/openai/types/shared_params/reasoning.py` — `effort` docstring.
- https://github.com/openai/openai-python/blob/main/src/openai/types/shared_params/reasoning.py
- https://developers.openai.com/api/docs/guides/reasoning

### Token budget per effort level

OpenAI does **not** publish a fixed token budget or percentage-of-`max_tokens`
formula for `reasoning.effort`.  The docs describe `effort` as an adaptive,
qualitative control:

> "Lower effort favors speed and lower token usage, while at higher effort the
> model thinks more completely to provide higher quality responses. The models
> also reason adaptively across reasoning efforts, using fewer tokens for
> simpler tasks and thinking harder for complex tasks."

The total output is bounded by `max_output_tokens` (or `max_completion_tokens`
on Chat Completions), which counts **both** reasoning tokens and visible output
tokens.  OpenAI recommends reserving at least 25,000 tokens of headroom when
experimenting with reasoning models.

| Effort | Token budget | Notes |
| --- | --- | --- |
| `none` | 0 | Disables reasoning entirely where supported. |
| `minimal` | **Not published** | Adaptive; fewer tokens than `low`. |
| `low` | **Not published** | Adaptive; faster/cheaper. |
| `medium` | **Not published** | Default for most reasoning models (model-dependent). |
| `high` | **Not published** | Adaptive; more thorough reasoning. |
| `xhigh` | **Not published** | Distinct value from `max`; no numeric budget disclosed. |
| `max` | **Not published** | Distinct value from `xhigh`; no numeric budget disclosed. |

**Sources:**
- https://developers.openai.com/api/docs/guides/reasoning
- https://platform.openai.com/docs/api-reference/responses — `max_output_tokens` includes reasoning tokens.

### Important caps

- **Minimum:** none documented for reasoning itself; `max_output_tokens` must be
  large enough to leave room for the final answer.
- **Maximum:** bounded by `max_output_tokens` / `max_completion_tokens` and the
  model's context window.

---

## 2. Anthropic — `output_config.effort` and `thinking.budget_tokens`

Anthropic has two related mechanisms:

1. **Adaptive thinking + `output_config.effort`** on newer models (Opus 4.7/4.8,
   Sonnet 5, Fable 5, Mythos 5, etc.).
2. **Legacy manual extended thinking** with `thinking: { type: "enabled",
   budget_tokens: N }` on older models (Sonnet 4.6, Opus 4.6, etc.).

### `output_config.effort` (adaptive thinking)

The Python SDK defines the enum as `low`, `medium`, `high`, `xhigh`, `max`.
There is **no** `none` or `minimal` value for `output_config.effort`.

**Source:**
- Anthropic Python SDK `src/anthropic/types/output_config_param.py`
- https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/output_config_param.py

Anthropic is explicit that **effort is a behavioral signal, not a strict token
budget**:

> "Effort is a behavioral signal, not a strict token budget. At lower effort
> levels, Claude will still think on sufficiently difficult problems, but it
> will think less than it would at higher effort levels for the same problem."

The `max_tokens` parameter is the hard cap on total output (thinking + text +
tool calls).  `effort` only shapes how much of that budget Claude allocates to
thinking.

| Effort | Token budget | Notes |
| --- | --- | --- |
| `low` | **Not a fixed budget** | Minimizes thinking; may skip thinking on simple tasks. |
| `medium` | **Not a fixed budget** | Moderate thinking. |
| `high` | **Not a fixed budget** | Default; almost always thinks. |
| `xhigh` | **Not a fixed budget** | "Always thinks deeply with extended exploration." |
| `max` | **Not a fixed budget** | "Always thinks with no constraints on thinking depth." |
| `none` / `minimal` | N/A | Not supported by `output_config.effort`. |

**Sources:**
- https://platform.claude.com/docs/en/build-with-claude/effort
- https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking

### Legacy `thinking.budget_tokens` (manual extended thinking)

For models that still accept manual extended thinking, `budget_tokens` is an
absolute integer cap on thinking tokens.

| Rule | Value | Source |
| --- | --- | --- |
| Minimum | `1,024` tokens | https://platform.claude.com/docs/en/build-with-claude/extended-thinking |
| Maximum | No documented absolute max; must be `< max_tokens` (except under interleaved thinking, where it can exceed `max_tokens` and is bounded by the context window) | https://platform.claude.com/docs/en/build-with-claude/extended-thinking |
| Valid range | `1024 <= budget_tokens < max_tokens` (legacy non-interleaved) | Ruby SDK docs, https://www.rubydoc.info/github/anthropics/anthropic-sdk-ruby/main/Anthropic%2FModels%2FThinkingConfigEnabled:budget_tokens |

There is **no official effort-to-budget_tokens mapping** for Anthropic's effort
levels.  The migration docs explicitly state that `budget_tokens` has no direct
replacement; use `thinking: { type: "adaptive" }` and `output_config.effort`
instead.

**Source:**
- https://platform.claude.com/docs/en/build-with-claude/extended-thinking

---

## 3. Google Gemini — `thinkingLevel` and `thinkingBudget`

Google has two different controls depending on the model generation:

- **Gemini 3.x:** `thinkingLevel` (`minimal`, `low`, `medium`, `high`).
- **Gemini 2.5:** `thinkingBudget` (absolute integer token budget).

### Gemini 3.x `thinkingLevel`

These are dynamic/behavioral levels, not fixed token budgets.  Supported levels
and defaults are model-specific:

| `thinkingLevel` | Gemini 3.6 & 3.5 Flash | Gemini 3.1 Pro | Gemini 3.5 & 3.1 Flash-Lite | Gemini 3.1 Flash-Lite Image | Gemini 3 Flash | Description |
| --- | --- | --- | --- | --- | --- | --- |
| `minimal` | Supported | Not supported | Supported (Default) | Supported (Default) | Supported | As close to "no thinking" as possible; may still reason minimally on complex tasks. |
| `low` | Supported | Supported | Supported | Not supported | Supported | Minimizes latency and cost. |
| `medium` | Supported (Default) | Supported | Supported | Not supported | Supported | Balanced thinking for most tasks. |
| `high` | Supported (Dynamic) | Supported (Default, Dynamic) | Supported (Dynamic) | Supported (Dynamic) | Supported (Default, Dynamic) | Maximizes reasoning depth. |

There are **no `none`, `xhigh`, or `max`** levels.  Thinking cannot be disabled
for Gemini 3.1 Pro, Gemini 3 Flash, or Flash-Lite.

**Source:**
- https://ai.google.dev/gemini-api/docs/generate-content/thinking

### Gemini 2.5 `thinkingBudget`

`thinkingBudget` is an absolute integer token budget.  `-1` enables dynamic
thinking, `0` disables thinking where allowed.

| Model | Default | Valid range | Disable thinking | Dynamic thinking |
| --- | --- | --- | --- | --- |
| 2.5 Pro | Dynamic thinking | `128` to `32,768` | Cannot be disabled | `thinkingBudget = -1` (default) |
| 2.5 Flash | Dynamic thinking | `0` to `24,576` | `thinkingBudget = 0` | `thinkingBudget = -1` (default) |
| 2.5 Flash Preview | Dynamic thinking | `0` to `24,576` | `thinkingBudget = 0` | `thinkingBudget = -1` (default) |
| 2.5 Flash Lite | Thinking off | `512` to `24,576` | `thinkingBudget = 0` | `thinkingBudget = -1` |
| 2.5 Flash Lite Preview | Thinking off | `512` to `24,576` | `thinkingBudget = 0` | `thinkingBudget = -1` |
| Robotics-ER 1.6 Preview | Dynamic thinking | `0` to `24,576` | `thinkingBudget = 0` | `thinkingBudget = -1` (default) |
| 2.5 Flash Live Native Audio Preview (09-2025) | Dynamic thinking | `0` to `24,576` | `thinkingBudget = 0` | `thinkingBudget = -1` (default) |

**Source:**
- https://ai.google.dev/gemini-api/docs/generate-content/thinking

### OpenAI-compatible `reasoning_effort` → Gemini `thinking_budget` mapping

Google's OpenAI-compatible endpoint maps `reasoning_effort` to a
`thinking_budget` for Gemini 2.5 models:

| `reasoning_effort` (OpenAI) | `thinking_level` (Gemini 3.1 Pro) | `thinking_level` (Gemini 3.1 Flash-Lite) | `thinking_level` (Gemini 3 Flash) | `thinking_budget` (Gemini 2.5) |
| --- | --- | --- | --- | --- |
| `minimal` | `low` | `minimal` | `minimal` | `1,024` |
| `low` | `low` | `low` | `low` | `1,024` |
| `medium` | `medium` | `medium` | `medium` | `8,192` |
| `high` | `high` | `high` | `high` | `24,576` |

This table does **not** include `xhigh` or `max`, so those effort values are not
explicitly mapped.  `none` can disable thinking for 2.5 Flash models but not for
2.5 Pro or any Gemini 3 model.

**Source:**
- https://ai.google.dev/gemini-api/docs/openai

---

## 4. OpenRouter — normalized `reasoning.effort` and `reasoning.max_tokens`

OpenRouter normalizes provider-specific reasoning controls through a single
`reasoning` object.  It supports either `effort` or `max_tokens`, but not both in
one request.

**Source:**
- https://openrouter.ai/docs/guides/best-practices/reasoning-tokens

### `reasoning.effort` formula

For models that OpenRouter maps to a token budget, the conversion is:

```
budget_tokens = max(min(max_tokens * {effort_ratio}, 128000), 1024)
```

| `effort` | Ratio | Approximate budget (% of `max_tokens`) | Notes |
| --- | --- | --- | --- |
| `max` | `0.95` | ~95% | Largest portion. |
| `xhigh` | `0.95` | ~95% | **Same allocation as `max`.** |
| `high` | `0.80` | ~80% | Large portion. |
| `medium` | `0.50` | ~50% | Moderate portion; default when `reasoning.enabled: true`. |
| `low` | `0.20` | ~20% | Smaller portion. |
| `minimal` | `0.10` | ~10% | Even smaller portion. |
| `none` | — | 0 | Disables reasoning entirely. |

**Caps:**
- **Minimum:** 1,024 tokens.
- **Maximum:** 128,000 tokens.
- `max_tokens` must be strictly higher than the resulting reasoning budget so
  the final response has room.

**Source:**
- https://openrouter.ai/docs/guides/best-practices/reasoning-tokens

### Provider behavior notes

- **Effort-only models** (OpenAI o-series / GPT-5, Grok, minimax, stepfun):
  OpenRouter sends the native `effort` value.  If `reasoning.max_tokens` is also
  supplied, it is used to determine which effort level to send.
- **`max_tokens`-budget models** (Gemini, Anthropic, some Qwen):
  `reasoning.max_tokens` is forwarded directly (minimum 1,024).  If only
  `reasoning.effort` is supplied, OpenRouter converts it using the percentage
  table above.

**Source:**
- https://openrouter.ai/docs/guides/best-practices/reasoning-tokens

---

## Key takeaways

1. **Only OpenRouter publishes a numeric effort-to-token-budget formula** with
   explicit percentages and clamp caps (1,024–128,000).
2. **OpenAI** documents `reasoning.effort` levels but does **not** disclose fixed
   token budgets; reasoning is adaptive and bounded by `max_output_tokens`.
3. **Anthropic** `output_config.effort` is explicitly a behavioral signal, not a
   strict budget; the only absolute cap available is the legacy
   `thinking.budget_tokens` (≥1,024 and < `max_tokens`).
4. **Google Gemini 2.5** uses an absolute `thinkingBudget` with model-specific
   ranges; **Gemini 3.x** uses qualitative `thinkingLevel` values with no fixed
   token amounts.
5. **`max` and `xhigh` are the same budget on OpenRouter** (~95%), but are
   **different effort levels on Anthropic** and **distinct but undocumented
   values on OpenAI**.
