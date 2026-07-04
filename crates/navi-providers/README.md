# navi-providers

[![Crates.io](https://img.shields.io/crates/v/navi-providers)](https://crates.io/crates/navi-providers)
[![License](https://img.shields.io/crates/l/navi-providers)](../LICENSE)

Provider facade for [NAVI](https://github.com/navi-ai-org/navi) — a thin re-export layer over the provider implementation crate.

## Why a facade?

`navi-providers` exists so that downstream crates (TUI, SDK, Tutor) depend on a **stable import surface** while the underlying implementation (`navi-openai`) can be swapped, split, or extended without widespread dependency churn.

```rust
// Always import from navi-providers, never from navi-openai directly
use navi_providers::{OpenAiProvider, OpenAiApiKind, ProviderError};
```

## Re-exported surface

| Type | Description |
|------|-------------|
| `OpenAiProvider` | The provider implementation |
| `OpenAiApiKind` | Protocol selector (Responses / Chat Completions) |
| `ProviderError` | Error types with retry classification |
| `ProviderId` | Provider identifier constants |
| OAuth flows | `openai_browser_oauth`, `github_copilot_device_oauth`, `commandcode_*` |
| Usage reporting | `OpenAiUsageReport`, `OpenAiUsageWindow`, `OpenAiUsageLimitSnapshot` |
| Types module | `navi_providers::types::*` |

## Part of the NAVI workspace

This crate depends on [`navi-core`](https://crates.io/crates/navi-core) and [`navi-openai`](https://crates.io/crates/navi-openai).

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
