# copland

A ratatui-based UI framework for building terminal UIs consistently and fast.

Inspired by Go's lipgloss and bubbletea, copland provides reusable components
for terminal user interfaces built on top of [ratatui](https://crates.io/crates/ratatui).

## Components

- **TextInput** — Single-line text input with CamelHumps editing, cursor tracking, and placeholder support.
- **ModalStack** — Generic stack-based modal manager with open/replace/close semantics.
- **SelectListState** — Selection and scroll state for navigable lists.
- **InteractionRegistry** — Mouse hit-testing registry with z-order priority.
- **Layout** — Root layout, viewport margins, centered rects, and modal specifications.
- **KeyOutcome** — Key handling result enum (Handled, Ignored, Quit).
- **UiEffect** — Generic modal transition effects (OpenModal, ReplaceModal, CloseModal, CloseAllModals).
