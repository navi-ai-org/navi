//! navi-compose — spatiotemporal component composition for NAVI.
//!
//! NAVI is composed of components with declared dependencies and generations.
//! This crate is the **host** of the [Matrix](https://github.com/enrell/matrix)
//! kernel: the kernel owns composition (registration, dependency resolution,
//! generations, admission, resources, lifecycle, journal) and never executes
//! component code; this crate executes.
//!
//! ```text
//!   Compose (host)                          Kernel (matrix-core)
//!   ├── frontend threads (navi.ui.*)   ◄──  Activated / Withdrawn lifecycle
//!   ├── native handlers (navi.*@1)     ◄──  invoke() admission + generation check
//!   ├── terminal lease + resources     ◄──  acquire_external / release (I01/I05)
//!   └── async effect drain             ──►  load_manifest / reload / dispose
//! ```
//!
//! Frontends are components: `navi.ui.tui` (ratatui) and `navi.ui.plain`
//! (line-based) both bind to `navi.engine.driver@1` and can be swapped at
//! runtime with [`Compose::reload`] — the old generation is quiesced, releases
//! the exclusive terminal lease, and the successor is activated with fresh
//! bindings. Session state lives in the engine, so it survives the swap.

pub mod host;
pub mod manifest;
pub mod native;
pub mod parts;
pub mod process_frontend;
pub mod slots;
pub mod tool_proxy;
pub mod ui;

pub use host::{
    ActivationOutcome, ComponentStatus, Compose, ComposeOptions, QueuedCommand, RestartOutcome,
    RestartPolicy, default_home, describe_restart,
};
pub use manifest::{
    CallPolicySpec, ComponentKind, ComponentManifest, Execution, RequirementSpec, SandboxSpec,
};
pub use native::{
    Handler, HandlerError, HandlerResult, HandlerTable, Invocation, NativeForwarder,
};
pub use ui::{
    PlainFrontend, TuiFrontend, UiFrontend, command_capability, default_manifests,
    engine_manifest, ui_manifest,
};
pub use tool_proxy::{PluginToolProxy, PluginToolSpec};
pub use parts::{BrandedPrompt, ComponentValues, TerseCompaction};
pub use process_frontend::ProcessFrontend;
pub use slots::{ComposeCompaction, ComposePrompt, Slots};

/// Frontend host vocabulary (re-exported from `navi-tui`).
pub use navi_tui::host::{
    ControlChannel, ControlHandle, ControlMsg, TerminalLease, UiCommandSink, UiExit, UiHost,
};

/// Versioned interfaces this prototype defines.
pub mod interfaces {
    /// Frontend contract (`UiFrontend`).
    pub const UI_FRONTEND: &str = "navi.ui.frontend@1";
    /// Engine facade consumed by frontends (`EngineDriver`).
    pub const ENGINE_DRIVER: &str = "navi.engine.driver@1";
    /// Engine event stream (`RuntimeEvent` broadcast).
    pub const ENGINE_EVENTS: &str = "navi.engine.events@1";
    /// Engine command admission.
    pub const ENGINE_COMMAND: &str = "navi.engine.command@1";
    /// Compaction strategy (runtime-swappable example).
    pub const COMPACTION: &str = "navi.compaction@1";
}
