# Vision-Based Desktop Control Pipeline

This document describes a proposed Linux desktop-control pipeline for NAVI clients that use vision-capable models. It intentionally does not depend on OCR, OmniParser, or a separate UI-element parser. The model receives screenshots or video frames directly and returns structured actions for a trusted local executor.

The design goal is to let NAVI drive graphical applications on Linux while respecting Wayland's security model. Wayland is not a desktop automation API; observation and control must be composed from portals, compositor APIs, accessibility APIs, and local input backends.

## High-Level Pipeline

```text
Observe:
  PipeWire frames from the ScreenCast portal
  optional compositor state from Hyprland IPC or similar APIs
  optional AT-SPI accessibility tree

Think:
  vision-capable model receives the current frame and structured context
  model emits one validated action from a fixed action schema

Act:
  RemoteDesktop portal + libei/EIS for consented synthetic input
  compositor IPC for window/workspace operations when available
  AT-SPI actions when semantically reliable
  privileged uinput fallback only when explicitly configured

Verify:
  capture a new frame
  compare visible state and structured context
  continue, wait, retry, or ask the user
```

Vision replaces the UI parser. It does not replace capture, input, permission handling, coordinate mapping, action validation, or post-action verification.

## Observation Layer

Use the most stable observation source available for the environment.

| Source | Use | Notes |
|---|---|---|
| `xdg-desktop-portal` ScreenCast | General Wayland frame capture | Produces PipeWire streams after user consent. Prefer this for portable Linux support. |
| PipeWire stream | Continuous frame source | Better than repeatedly shelling out for screenshots in a long-running agent loop. |
| Hyprland IPC / `hyprctl` | Window, monitor, workspace, cursor context | Hyprland-specific but very useful for grounding and coordinate mapping. |
| `grim` / `slurp` | Hyprland/wlroots screenshot fallback | Good for prototypes and debugging. Less portable than portals. |
| AT-SPI2 | Semantic UI context when available | Exposes roles, names, states, text, values, focus, and actions for accessible apps. |

The model should receive both the image and non-visual context when available. Example context packet:

```json
{
  "active_window": {
    "id": "0xabc",
    "class": "firefox",
    "title": "GitHub"
  },
  "monitors": [
    {
      "name": "DP-1",
      "x": 0,
      "y": 0,
      "width": 2560,
      "height": 1440,
      "scale": 1.0
    }
  ],
  "cursor": {
    "x": 1200,
    "y": 700
  }
}
```

## Model Contract

Do not allow free-form desktop actions. The model should emit exactly one structured action from a fixed schema, plus confidence and a short rationale for audit/debug UI.

Example click action:

```json
{
  "type": "click",
  "target": {
    "description": "Save button in the lower-right corner",
    "x": 1420,
    "y": 922
  },
  "button": "left",
  "confidence": 0.84,
  "requires_confirmation": false
}
```

Example typing action:

```json
{
  "type": "type_text",
  "target": {
    "description": "Search input at the top of the window",
    "x": 310,
    "y": 85
  },
  "text": "hello world",
  "confidence": 0.78,
  "requires_confirmation": false
}
```

Recommended action set:

| Action | Purpose |
|---|---|
| `click` | Single pointer click at a coordinate. |
| `double_click` | Double-click at a coordinate. |
| `right_click` | Context-menu click. |
| `move_pointer` | Move pointer without clicking. |
| `drag` | Drag from one coordinate to another. |
| `scroll` | Scroll at a coordinate with signed amount. |
| `type_text` | Insert text into the focused or targeted field. |
| `press_key` | Press a single key. |
| `hotkey` | Press a key combination. |
| `wait` | Wait for UI state to change. |
| `focus_window` | Focus a known window by compositor id when available. |
| `switch_workspace` | Switch workspace via compositor IPC when available. |
| `ask_user` | Request clarification or confirmation. |

The executor must validate action type, coordinates, monitor bounds, text size, target window, and safety policy before acting.

## Action Layer

Prefer consented and compositor-aware action paths before privileged fallbacks.

| Backend | Use | Notes |
|---|---|---|
| RemoteDesktop portal + libei/EIS | General Wayland synthetic input | Modern path for consented input. Avoids pretending to be hardware. |
| RemoteDesktop portal Notify methods | Pointer, keyboard, touch events | Useful where EIS is unavailable but the portal backend supports direct notify calls. |
| Hyprland IPC / `hyprctl dispatch` | Workspaces, focus, move, resize | More reliable than pointer actions for window management. Hyprland-specific. |
| AT-SPI action/value APIs | Accessible controls | Prefer semantic actions over coordinate clicks when the target is reliable. |
| `wtype` | Text entry on supported Wayland environments | Good focused-text fallback. |
| `ydotool` / `/dev/uinput` | Last-resort input injection | Powerful but requires elevated permission or a daemon. Treat as explicitly privileged mode. |

For browsers, editors, terminals, and other tools with native automation APIs, prefer those APIs over desktop pixels. Examples include Chrome DevTools Protocol, Playwright, DBus interfaces, application CLIs, terminal PTYs, and project-local tools.

## Coordinate Mapping

Coordinate handling is a core reliability risk. The executor must know which coordinate space the model used and map it to the input backend's coordinate space.

Track these separately:

| Coordinate Space | Meaning |
|---|---|
| Frame pixels | Pixel dimensions of the captured image passed to the model. |
| Stream logical size | Size reported by the ScreenCast portal/PipeWire stream. |
| Compositor logical coordinates | Window/monitor layout coordinates, often affected by scaling. |
| Device/input coordinates | Coordinates expected by the input injection backend. |

Common failure cases:

- fractional scaling changes the relation between frame pixels and compositor coordinates
- multi-monitor layouts can have negative monitor origins
- the captured source may be a window, not the full desktop
- the UI can move between observation and action
- animations, popovers, or focus changes can invalidate a target

The executor should reject or re-observe when the target coordinate falls outside the selected capture region or no longer matches the expected active window.

## Safety Gates

Vision models can misread UI state and choose destructive actions. NAVI should require explicit confirmation or policy approval for high-risk actions.

Examples of high-risk intents:

- delete, remove, erase, reset, or discard
- send, publish, submit, upload, or share
- buy, pay, transfer, subscribe, or authorize payment
- install software, grant permissions, or change security settings
- run shell commands from a GUI
- expose, copy, paste, or transmit secrets

The confirmation gate should be based on the intended action, visible target, active app, and tool policy, not only on the model's `requires_confirmation` flag.

## Verification Loop

Every action should be followed by observation. The agent should not assume success from an input call returning `Ok`.

Verification signals:

- new frame differs in the expected region
- active window/title changed as expected
- expected modal, toast, or page appeared
- AT-SPI focus/value/state changed as expected
- compositor state changed as expected

If verification fails, the agent should retry only when the failure mode is clear. Otherwise it should wait, re-observe, choose a safer semantic backend, or ask the user.

## Backend Strategy

Recommended Linux backend selection:

```text
Generic Wayland:
  observe: ScreenCast portal + PipeWire
  act: RemoteDesktop portal + libei/EIS
  semantic: AT-SPI when available
  fallback: privileged uinput only when configured

Hyprland:
  observe: PipeWire or grim
  context: Hyprland IPC / hyprctl clients, monitors, activewindow, cursorpos
  act: libei/EIS when available, Hyprland dispatch for window operations
  fallback: wtype or ydotool when explicitly allowed

X11:
  observe: screenshot APIs
  act: xdotool/wmctrl
  semantic: AT-SPI when available
```

## Non-Goals

This pipeline does not attempt to bypass Wayland by reading application buffers directly. A normal client does not receive other clients' buffer file descriptors. Direct buffer access requires being the compositor, running a Wayland proxy for applications launched through it, or using compositor-provided capture APIs.

This pipeline also does not promise macOS Accessibility-level semantic control on Linux. It is a layered desktop-control strategy that combines vision, portals, compositor context, accessibility, and native APIs.

## Practical Rule

Use vision to understand what is visible. Use real APIs whenever reliability matters.

```text
Vision answers: what appears to be on the screen?
Compositor APIs answer: which windows and monitors exist?
Accessibility APIs answer: which semantic controls exist?
Native APIs answer: how do we do this reliably?
Input backends answer: how do we perform the final physical action?
```
