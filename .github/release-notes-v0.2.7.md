## Highlights

**0.2.7** hardens the TUI terminal stack (Kitty progressive enhancement, multi-window safety), fixes model/effort modal navigation, and makes session restore rehydrate `view_image` attachments from durable storage so vision tool results survive after the source file is gone.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.2.6...v0.2.7

### Sessions & vision tools

- Durable content-addressed **attachment store** under `{data_dir}/attachments/`
- **Session replay** rebuilds provider history with tool turns + image rehydrate (path first, then attachment store)
- Providers attach tool images as follow-up multimodal user content where the wire format requires it (OpenAI Chat/Responses, Gemini, CommandCode; Anthropic keeps tool_result images)

### TUI terminal solid path

- **Kitty progressive enhancement** negotiated (`DISAMBIGUATE` + `REPORT_EVENT_TYPES`) instead of the broken push-0 + immediate-pop disable
- **FocusGained** reasserts keyboard / paste / focus / mouse modes
- Free mouse motion (`?1003`) only while image hover can fire
- Global shortcuts work with text in the composer; bare ASCII control-byte fallback; **Ctrl+X** help when Ctrl+. needs Kitty
- Leak filter kept as residual CSI/OSC safety net

### Modal navigation

- Agent routes / attachment **model pickers**: open on first available model (Recent-safe), Down recovers from stale selection, PageUp/PageDown, mouse wheel on BgModelPicker
- **Effort** picker: cursor follows selection independently of the active level (arrow keys actually move)

### Bindings

- `@navi-agent/napi` **0.2.7** and platform packages
- `@navi-agent/navi` **0.2.7** CLI packages
- Workspace crate versions bumped to **0.2.7**

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.2.7
```

```bash
npm install -g @navi-agent/navi@0.2.7
npm install @navi-agent/napi@0.2.7
```

## Changelog

- Tag range: https://github.com/navi-ai-org/navi/compare/v0.2.6...v0.2.7
- See [CHANGELOG.md](https://github.com/navi-ai-org/navi/blob/v0.2.7/CHANGELOG.md)
