# Novita AI Provider Integration

## Context

Novita AI is a new official NAVI partner. Their API is OpenAI-compatible
(chat completions + embeddings), so the integration follows the same pattern
as Groq, Nvidia, and other OpenAI-compatible providers. The key deliverables:

1. Add `novita` as a known `ProviderId` constant
2. Add a `NovitaBehavior` (standard Bearer auth, no custom headers needed)
3. Register it in `behavior_for_provider`
4. Add a registry provider JSON (`providers/novita.json`) with curated models
5. Add `novita` to the `CHARM_HYPER`-style known list test
6. Update `lib.rs` re-exports if needed

## Approach

Novita's API is standard OpenAI Chat Completions with Bearer auth. No OAuth,
no custom headers, no special rate-limit handling. The integration is:

- `ProviderKind::OpenAiChatCompletions` (existing enum variant)
- `NovitaBehavior` struct implementing `ProviderBehavior` (like `GroqBehavior`)
- Standard `standard_bearer_headers` for auth
- Registry JSON with curated models (DeepSeek V3, DeepSeek R1, Llama 3.3, Qwen)

## Files

### 1. `crates/navi-core/src/provider_id.rs`
- Add `pub const NOVITA: &'static str = "novita";`
- Add to `known()` debug_assert list
- Add to `known_accepts_all_predefined_constants` test array

### 2. `crates/navi-openai/src/providers/behavior.rs`
- Add `pub(crate) struct NovitaBehavior;`
- Implement `ProviderBehavior` (standard Bearer, no base URL default, ChatCompletions stream route)
- Add `ProviderId::NOVITA => Box::new(NovitaBehavior)` in `behavior_for_provider`

### 3. Registry provider JSON
- Create `providers/novita.json` in the navi-registry repo format
- Curated models: DeepSeek V3, DeepSeek R1, Llama-3.3-70B, Qwen series
- `kind: "openai-chat-completions"`, `api_key_env: "NOVITA_API_KEY"`,
  `base_url: "https://api.novita.ai/v3/openai"`

### 4. Tests
- `crates/navi-core/src/provider_id.rs` — add NOVITA to known constants test
- `crates/navi-openai/src/providers/behavior.rs` — add Novita to behavior tests
- Provider config roundtrip test

## Verification

```bash
cargo check -p navi-core -p navi-openai
cargo test -p navi-core --lib provider_id -- --test-threads=4
cargo test -p navi-openai --lib -- --test-threads=4
```
