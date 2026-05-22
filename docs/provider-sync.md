# Provider And Model Sync

NAVI ships with a built-in provider/model catalog, but the TUI can refresh models from providers that expose a `/models` endpoint.

## Where It Lives

- Built-in catalog: `navi-core/src/config.rs`
- Provider list call: `ModelProvider::list_models`
- OpenAI-compatible implementation: `navi-openai/src/lib.rs`
- TUI model picker and sync actions: `navi-tui/src/lib.rs`

## TUI Behavior

In the model picker:

- type to filter providers and models.
- `tab` refreshes models for the currently selected provider.
- `ctrl+r` refreshes all providers.
- selecting a model whose provider has no key opens the API key modal.

Sync runs asynchronously and reports back through `AsyncEvent::SyncCompleted`.

## Catalog Merge Rules

Config loading starts with defaults, then merges global config, then project config.

Provider overrides are merged by `ProviderConfig.id`:

- If an id already exists, the whole provider config is replaced.
- If an id does not exist, it is appended.

This makes project-level provider overrides powerful but also easy to overuse. Prefer narrow custom providers rather than replacing built-ins unless necessary.

## Failure Modes

Not every provider supports `/models`, even if inference works. Sync code should treat model-list failures as provider-specific failures, not global failures.

If a provider requires custom auth headers for listing models, add that to `OpenAiProvider::list_models`.

## Verification

```bash
cargo run -p navi-cli -- --print-providers
cargo test -p navi-tui model_picker
cargo test -p navi-openai
```
