//! The compose supervisor: NAVI as a *host* of the Matrix kernel.
//!
//! Responsibilities split (Matrix `CHARTER.md`):
//!
//! * **Kernel** — owns composition: registration, dependency resolution,
//!   generations, admission, resources, lifecycle, journal.
//! * **This host** — executes: starts/stops frontends, acquires the real
//!   terminal, drains async effects, and answers capability calls routed by the
//!   kernel through [`NativeForwarder`].
//!
//! The kernel is synchronous (`parking_lot`, no async runtime): all composition
//! mutations happen on the caller thread, and long-running work is either a
//! frontend thread or an effect queued for the async supervisor loop.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use matrix_core::external::{LifecycleEvent, LifecycleHook};
use matrix_core::{InstanceId, InstanceRef, Journal, Kernel, ResourceHandle, ResourceKind};
use navi_sdk::{EngineDriver, LoadedConfig, NaviSessionRequest, NaviTurnRequest};
use navi_tui::host::{
    ControlChannel, ControlHandle, ControlMsg, TerminalLease, UiCommandSink, UiExit, UiHost,
};
use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};

use crate::manifest::{ComponentKind, ComponentManifest, INPROC_PREFIX};
use crate::native::{Handler, HandlerResult, HandlerTable, Invocation, NativeForwarder};
use crate::parts::ComponentValues;
use crate::slots::Slots;
use crate::tool_proxy::PluginToolSpec;
use crate::ui::{PlainFrontend, TuiFrontend, UiFrontend};

/// Where the compose state lives (`{data_dir}/compose`).
pub fn default_home(data_dir: &Path) -> PathBuf {
    data_dir.join("compose")
}

/// Construction options.
pub struct ComposeOptions {
    /// State directory (manifests, journal, reload requests).
    pub home: PathBuf,
    /// Project root handed to frontends.
    pub project_dir: PathBuf,
    /// Loaded NAVI configuration handed to frontends.
    pub loaded_config: LoadedConfig,
    /// Kernel dry-run (journal suppressed).
    pub dry_run: bool,
    /// Perform real terminal effects when acquiring the TTY lease.
    ///
    /// `false` uses [`TerminalLease::acquire_detached`]: the exclusivity
    /// contract is enforced but raw mode is not touched (headless hosts, CI).
    pub terminal_effects: bool,
    /// Attach the process host so third-party components can run as child
    /// processes (`matrix-host` + `matrix.component`). Linux-only.
    pub process_components: bool,
}

/// Outcome of an activation/reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationOutcome {
    /// Component id.
    pub component: String,
    /// Generation before the operation (`None` when it was not active).
    pub previous_generation: Option<u64>,
    /// Generation after the operation.
    pub generation: Option<u64>,
    /// Kernel state after reconcile (`Active`, `Waiting`, …).
    pub state: String,
    /// Visible reason when the component is `Waiting`.
    pub waiting_reason: Option<String>,
    /// Host-side notes (frontends started/stopped, resources).
    pub notes: Vec<String>,
}

/// A component as seen by `navi components`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentStatus {
    /// Logical id.
    pub id: String,
    /// NAVI classification.
    pub kind: ComponentKind,
    /// Kernel state (`Active`, `Waiting`, `CleanupPending`, …).
    pub state: String,
    /// Kernel generation.
    pub generation: Option<u64>,
    /// Kernel instance id.
    pub instance: Option<u64>,
    /// Visible reason while `Waiting`.
    pub waiting_reason: Option<String>,
    /// Whether the host currently runs a frontend for it.
    pub ui_running: bool,
    /// Whether the host holds the terminal lease for it.
    pub tty: bool,
    /// Restart attempts recorded inside the supervision window.
    pub restarts: usize,
    /// Human-readable last supervision decision, when there is one.
    pub last_restart: Option<String>,
    /// Declared capabilities.
    pub capabilities: Vec<String>,
}

/// One-line description of a supervision decision (status/CLI).
pub fn describe_restart(outcome: &RestartOutcome) -> String {
    match outcome {
        RestartOutcome::Restarted { generation } => match generation {
            Some(generation) => format!("restarted (generation {generation})"),
            None => "restarted".to_string(),
        },
        RestartOutcome::PolicyNever => "failed (policy: never)".to_string(),
        RestartOutcome::BudgetExhausted => "failed (restart budget exhausted)".to_string(),
        RestartOutcome::Failed { reason } => format!("restart failed: {reason}"),
    }
}

/// A user command admitted by the kernel, waiting for the async supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedCommand {
    /// Component that submitted the command.
    pub component: String,
    /// Generation that submitted it.
    pub generation: u64,
    /// Raw command text.
    pub text: String,
}

struct RunningUi {
    generation: u64,
    control: ControlHandle,
    lease: Option<Arc<TerminalLease>>,
    resource: Option<ResourceHandle>,
    exit: Arc<Mutex<Option<UiExit>>>,
    join: Option<std::thread::JoinHandle<()>>,
}

struct ComposeInner {
    home: PathBuf,
    project_dir: PathBuf,
    loaded_config: LoadedConfig,
    kernel: Arc<Kernel>,
    handlers: Arc<HandlerTable>,
    lifecycle: Mutex<Vec<LifecycleEvent>>,
    manifests: RwLock<HashMap<String, ComponentManifest>>,
    uis: RwLock<HashMap<String, Arc<dyn UiFrontend>>>,
    running: Mutex<HashMap<String, RunningUi>>,
    commands: Mutex<Vec<QueuedCommand>>,
    driver: RwLock<Option<Arc<dyn EngineDriver>>>,
    session_id: RwLock<Option<String>>,
    task: RwLock<Option<String>>,
    active_skills: RwLock<Vec<String>>,
    runtime: Option<tokio::runtime::Handle>,
    started_ui: AtomicBool,
    signals_installed: AtomicBool,
    last_error: Mutex<Option<String>>,
    last_status_fingerprint: Mutex<String>,
    terminal_effects: bool,
    process_host: RwLock<Option<Arc<ProcessHost>>>,
    /// Typed values installed by activating components (compaction, prompt, …).
    slots: Slots,
    /// Values each component contributes when it activates.
    component_values: RwLock<HashMap<String, ComponentValues>>,
    /// Restart bookkeeping for supervised components (frontends, plugins).
    supervision: Mutex<Supervision>,
    /// Process components this host has seen `Active` (with the first time),
    /// used to supervise children that die later.
    seen_active: Mutex<HashMap<String, Instant>>,
}

/// Restart budget for supervised components: at most `max_attempts` inside a
/// `window`, each spaced by an exponential backoff capped at `backoff_cap`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartPolicy {
    pub max_attempts: usize,
    pub window: Duration,
    pub backoff: Duration,
    pub backoff_cap: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            window: Duration::from_secs(30),
            backoff: Duration::from_millis(250),
            backoff_cap: Duration::from_secs(4),
        }
    }
}

/// A process component is only supervised once it has been `Active` this long:
/// the first spawn (and the handshake) legitimately take a moment.
const PROCESS_GRACE: Duration = Duration::from_secs(2);

/// Why a supervised component was restarted (recorded in the log/status).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartOutcome {
    /// The component is active again in a new generation.
    Restarted { generation: Option<u64> },
    /// The policy is `never`: nothing is restarted.
    PolicyNever,
    /// The attempt budget inside the window is exhausted.
    BudgetExhausted,
    /// The supervisor could not activate it (the reason is in `last_error`).
    Failed { reason: String },
}

#[derive(Debug, Default)]
struct Supervision {
    /// Supervision policy (budget, window, backoff).
    policy: RestartPolicy,
    /// Recent restart attempts per component (within `policy.window`).
    attempts: HashMap<String, Vec<Instant>>,
    /// Components waiting for their backoff to elapse.
    pending: HashMap<String, Instant>,
    /// Components that gave up (budget exhausted) — terminal until reactivated.
    failed: HashSet<String>,
    /// Last outcome per component, for `navi components status`/tests.
    last_outcome: HashMap<String, RestartOutcome>,
}

/// The compose supervisor handle. Cheap to clone (wraps an `Arc`).
#[derive(Clone)]
pub struct Compose {
    inner: Arc<ComposeInner>,
}

/// Host type for process components (Linux only; `()` elsewhere).
#[cfg(target_os = "linux")]
type ProcessHost = matrix_host::Host;
#[cfg(not(target_os = "linux"))]
type ProcessHost = ();

/// Fans lifecycle events out to the process host *and* the supervisor queue.
///
/// In-process components are declared to the kernel as `process` with the
/// `inproc:<id>` sentinel (the kernel's built-in reducer dispatch is a closed
/// set, so every NAVI capability is served through the host forwarder). Those
/// sentinels must never reach the process host, or it would try to spawn them.
struct CompositeHook {
    inner: Weak<ComposeInner>,
    #[cfg(target_os = "linux")]
    host: Option<Arc<matrix_host::Host>>,
}

impl LifecycleHook for CompositeHook {
    fn on_lifecycle(&self, event: LifecycleEvent) {
        let inner = self.inner.upgrade();
        // Frontends are executed by this host (in-process or as a child that
        // owns the terminal); the process host must never spawn them.
        let is_frontend = inner
            .as_ref()
            .map(|inner| inner.uis.read().contains_key(match &event {
                LifecycleEvent::Activated { logical, .. }
                | LifecycleEvent::Withdrawn { logical, .. }
                | LifecycleEvent::Removed { logical } => logical,
            }))
            .unwrap_or(false);
        #[cfg(target_os = "linux")]
        {
            if let Some(host) = self.host.as_ref()
                && !is_frontend
            {
                let forward = match &event {
                    LifecycleEvent::Activated { entrypoint, .. } => {
                        !entrypoint.starts_with(crate::manifest::INPROC_PREFIX)
                    }
                    // Withdrawn/Removed carry no entrypoint: forwarding is
                    // harmless (killing an unknown child is a no-op).
                    LifecycleEvent::Withdrawn { .. } | LifecycleEvent::Removed { .. } => true,
                };
                if forward {
                    host.on_lifecycle(event.clone());
                }
            }
        }
        if let Some(inner) = inner {
            tracing::info!(
                event = ?std::mem::discriminant(&event),
                is_frontend,
                "lifecycle event queued"
            );
            inner.lifecycle.lock().push(event);
        }
    }
}

/// Attaches the process host and re-installs NAVI's chained forwarder + hook.
///
/// The host policy is derived from the components installed in this compose
/// home: a component that declares a `[sandbox]` runs inside `bwrap` + seccomp,
/// everything else is an explicitly trusted process. Once any component asks for
/// a sandbox the policy is **secure**: a component that is not in the policy is
/// denied (so install sandboxed plugins before the session starts, or restart).
#[cfg(target_os = "linux")]
fn attach_process_host(inner: &Arc<ComposeInner>) -> Result<Option<Arc<ProcessHost>>> {
    let sock_dir = inner.home.join("host");
    let policy = host_policy(inner);
    let host = matrix_host::Host::attach_with_policy(inner.kernel.clone(), &sock_dir, policy)
        .map_err(|err| anyhow!("matrix-host attach failed: {err}"))?;
    // `attach` installed the host as forwarder/hook/event-sink. Re-install the
    // chain so native capabilities stay in-process and everything else is
    // delegated to the child sessions; the host keeps the event sink (it
    // delivers events to components that declare `subscriptions`).
    inner.kernel.set_forwarder(Arc::new(
        NativeForwarder::new(inner.handlers.clone())
            .with_process(host.clone() as Arc<dyn matrix_core::external::CallForwarder>),
    ));
    inner.kernel.set_hook(Arc::new(CompositeHook {
        inner: Arc::downgrade(inner),
        host: Some(host.clone()),
    }));
    tracing::info!(socket = %host.sock_path().display(), "process host attached");
    Ok(Some(host))
}

#[cfg(not(target_os = "linux"))]
fn attach_process_host(_inner: &Arc<ComposeInner>) -> Result<Option<Arc<ProcessHost>>> {
    Ok(None)
}

/// Builds the `matrix-host` policy from the manifests installed on disk.
///
/// Reads `{home}/manifests/*.toml` directly: the policy is fixed when the host
/// attaches, which happens while the compose host is being built.
#[cfg(target_os = "linux")]
fn host_policy(inner: &Arc<ComposeInner>) -> matrix_host::HostPolicy {
    let mut components: HashMap<String, Option<matrix_guard::Sandbox>> = HashMap::new();
    let mut secure = false;
    let dir = inner.home.join("manifests");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(manifest) = ComponentManifest::from_toml(&text) else {
                continue;
            };
            match manifest.sandbox.as_ref() {
                Some(spec) => {
                    secure = true;
                    let workspace = spec
                        .workspace
                        .clone()
                        .unwrap_or_else(|| inner.home.join("workspaces").join(&manifest.id));
                    if let Err(err) = std::fs::create_dir_all(&workspace) {
                        tracing::error!(
                            component = %manifest.id,
                            workspace = %workspace.display(),
                            error = %err,
                            "cannot prepare the sandbox workspace"
                        );
                        continue;
                    }
                    let sandbox = matrix_guard::Sandbox {
                        workspace,
                        read_only: spec.read_only.clone(),
                        memory_bytes: spec.memory_bytes,
                        cpu_seconds: spec.cpu_seconds,
                        file_bytes: spec.file_bytes,
                        open_files: spec.open_files,
                    };
                    if let Err(err) = sandbox.validate() {
                        tracing::error!(
                            component = %manifest.id,
                            error = %err,
                            "invalid sandbox declaration; the component will be denied"
                        );
                        continue;
                    }
                    components.insert(manifest.id.clone(), Some(sandbox));
                }
                None => {
                    components.insert(manifest.id.clone(), None);
                }
            }
        }
    }
    if secure {
        tracing::info!(
            sandboxed = components.values().filter(|entry| entry.is_some()).count(),
            trusted = components.values().filter(|entry| entry.is_none()).count(),
            "process host policy is secure: unlisted components are denied"
        );
    }
    matrix_host::HostPolicy {
        components,
        secure,
        ..Default::default()
    }
}

/// Capability handler for a frontend's command interface.
///
/// The kernel revalidates the caller's activation before dispatch (I02/I03), so
/// a command from a withdrawn generation never reaches this handler.
struct CommandHandler {
    inner: Weak<ComposeInner>,
}

impl Handler for CommandHandler {
    fn invoke(&self, invocation: &Invocation, input: &Value) -> HandlerResult {
        let text = input
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(crate::native::HandlerError::new(
                "invalid-message",
                "empty command",
            ));
        }
        let Some(inner) = self.inner.upgrade() else {
            return Err(crate::native::HandlerError::new(
                "internal",
                "compose host dropped",
            ));
        };
        inner.commands.lock().push(QueuedCommand {
            component: invocation.component.clone(),
            generation: invocation.generation,
            text,
        });
        Ok(json!({"queued": true}))
    }
}

/// Handler exposing the engine's *synchronous* view (session id, generation).
struct EngineInfoHandler {
    inner: Weak<ComposeInner>,
}

impl Handler for EngineInfoHandler {
    fn invoke(&self, invocation: &Invocation, _input: &Value) -> HandlerResult {
        let Some(inner) = self.inner.upgrade() else {
            return Err(crate::native::HandlerError::new(
                "internal",
                "compose host dropped",
            ));
        };
        let session = inner.session_id.read().clone();
        Ok(json!({
            "component": invocation.component,
            "instance": invocation.instance,
            "generation": invocation.generation,
            "session_id": session,
            "epoch": inner.kernel.epoch(),
        }))
    }
}

/// Handler for the swappable compaction component: reports the *providing*
/// component and its generation, so a swap is observable without a full turn.
struct CompactionHandler;

impl Handler for CompactionHandler {
    fn invoke(&self, invocation: &Invocation, _input: &Value) -> HandlerResult {
        Ok(json!({
            "strategy": invocation.component,
            "generation": invocation.generation,
            "instance": invocation.instance,
        }))
    }
}

/// Sink handed to frontends: admits commands through the kernel.
struct KernelCommandSink {
    inner: Weak<ComposeInner>,
    capability: String,
    component: String,
}

impl UiCommandSink for KernelCommandSink {
    fn submit(&self, command: &str) -> std::result::Result<(), String> {
        let Some(inner) = self.inner.upgrade() else {
            return Err("internal: compose host dropped".to_string());
        };
        let (value, ok) = inner
            .kernel
            .invoke(&self.capability, &json!({"text": command}));
        if ok {
            return Ok(());
        }
        let code = value
            .get("code")
            .and_then(|code| code.as_str())
            .unwrap_or("rejected");
        let message = value
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("command rejected");
        Err(format!(
            "{code}: {message} [component state={:?}]",
            inner.kernel.context_state_of(&self.component)
        ))
    }
}

impl Compose {
    /// Creates the supervisor, its kernel, and the built-in components.
    pub fn new(options: ComposeOptions) -> Result<Self> {
        std::fs::create_dir_all(&options.home)
            .map_err(|err| anyhow!("failed to create compose home: {err}"))?;
        let journal_path = options.home.join("journal.jsonl");
        let journal = Journal::open(&journal_path, false, false)
            .map_err(|err| anyhow!("failed to open kernel journal: {err}"))?;
        let kernel = Arc::new(Kernel::new(&options.home, journal, options.dry_run));

        let inner = Arc::new(ComposeInner {
            home: options.home,
            project_dir: options.project_dir,
            loaded_config: options.loaded_config,
            kernel: kernel.clone(),
            handlers: Arc::new(HandlerTable::new()),
            lifecycle: Mutex::new(Vec::new()),
            manifests: RwLock::new(HashMap::new()),
            uis: RwLock::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
            commands: Mutex::new(Vec::new()),
            driver: RwLock::new(None),
            session_id: RwLock::new(None),
            task: RwLock::new(None),
            active_skills: RwLock::new(Vec::new()),
            runtime: tokio::runtime::Handle::try_current().ok(),
            started_ui: AtomicBool::new(false),
            signals_installed: AtomicBool::new(false),
            last_error: Mutex::new(None),
            last_status_fingerprint: Mutex::new(String::new()),
            terminal_effects: options.terminal_effects,
            process_host: RwLock::new(None),
            slots: Slots::new(),
            component_values: RwLock::new(HashMap::new()),
            supervision: Mutex::new(Supervision::default()),
            seen_active: Mutex::new(HashMap::new()),
        });

        kernel.set_forwarder(Arc::new(NativeForwarder::new(inner.handlers.clone())));
        kernel.set_hook(Arc::new(CompositeHook {
            inner: Arc::downgrade(&inner),
            #[cfg(target_os = "linux")]
            host: None,
        }));

        // Process tier: third-party components run as child processes under
        // `matrix-host` (spawn + `matrix.component` sessions + supervision).
        // Attaching replaces the forwarder/hook/event-sink, so it happens here
        // and our chained forwarder + composite hook are installed afterwards.
        if options.process_components {
            match attach_process_host(&inner) {
                Ok(Some(host)) => {
                    *inner.process_host.write() = Some(host);
                }
                Ok(None) => {
                    tracing::info!(
                        "process components are not supported on this platform; \
                         only in-process components are available"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "failed to attach the process host; process components are unavailable"
                    );
                }
            }
        }

        let compose = Self { inner };
        compose.register_builtin_frontends();
        compose.register_builtin_values();
        Ok(compose)
    }

    /// Registers the typed values the built-in components provide.
    ///
    /// A component that contributes a Rust-side part (the compaction strategy,
    /// the prompt builder) installs it into the matching slot when it activates;
    /// the engine holds a proxy that resolves the slot per call.
    fn register_builtin_values(&self) {
        use std::sync::Arc;
        self.register_component_values(
            "navi.compaction.default",
            ComponentValues {
                compaction: Some(Arc::new(navi_core::DefaultCompactionStrategy)),
                ..Default::default()
            },
        );
        self.register_component_values(
            "navi.compaction.terse",
            ComponentValues {
                compaction: Some(Arc::new(crate::parts::TerseCompaction)),
                ..Default::default()
            },
        );
        self.register_component_values(
            "navi.prompt.branded",
            ComponentValues {
                prompt: Some(Arc::new(crate::parts::BrandedPrompt::new(
                    Arc::new(navi_core::DefaultPromptBuilder),
                    "[prompt component: navi.prompt.branded]",
                ))),
                ..Default::default()
            },
        );
    }

    /// Registers the typed values a component installs on activation.
    pub fn register_component_values(&self, component: &str, values: ComponentValues) {
        self.inner
            .component_values
            .write()
            .insert(component.to_string(), values);
    }

    /// The typed slots the engine proxies read from.
    pub fn slots(&self) -> Slots {
        self.inner.slots.clone()
    }

    fn install_component_values(&self, component: &str, generation: u64) {
        let values = self.inner.component_values.read().get(component).cloned();
        let Some(values) = values else { return };
        if values.is_empty() {
            return;
        }
        if let Some(compaction) = values.compaction {
            tracing::info!(
                component,
                generation,
                role = self.inner.slots.compaction.role(),
                "installing component into the compaction slot"
            );
            self.inner.slots.compaction.install(component, generation, compaction);
        }
        if let Some(prompt) = values.prompt {
            tracing::info!(
                component,
                generation,
                role = self.inner.slots.prompt.role(),
                "installing component into the prompt slot"
            );
            self.inner.slots.prompt.install(component, generation, prompt);
        }
    }

    fn clear_component_values(&self, component: &str, generation: u64) {
        if self.inner.slots.compaction.clear_if_owner(component, generation) {
            tracing::info!(
                component,
                generation,
                "compaction slot fell back to the previous provider"
            );
        }
        if self.inner.slots.prompt.clear_if_owner(component, generation) {
            tracing::info!(
                component,
                generation,
                "prompt slot fell back to the previous provider"
            );
        }
    }

    fn register_builtin_frontends(&self) {
        self.register_ui(Arc::new(TuiFrontend));
        self.register_ui(Arc::new(PlainFrontend));
    }

    /// Registers a frontend implementation under its component id.
    pub fn register_ui(&self, frontend: Arc<dyn UiFrontend>) {
        self.inner
            .uis
            .write()
            .insert(frontend.id().to_string(), frontend);
    }

    /// The kernel handle (inspection/advanced use).
    pub fn kernel(&self) -> &Arc<Kernel> {
        &self.inner.kernel
    }

    /// Native handler table (tool packs, providers, … register here).
    pub fn handlers(&self) -> &Arc<HandlerTable> {
        &self.inner.handlers
    }

    /// Binds the engine facade and registers the engine capability handlers.
    pub fn set_engine(&self, driver: Arc<dyn EngineDriver>) {
        *self.inner.driver.write() = Some(driver);
        self.inner
            .handlers
            .register("navi.engine.driver@1", Arc::new(EngineInfoHandler {
                inner: Arc::downgrade(&self.inner),
            }));
        self.inner
            .handlers
            .register("navi.compaction@1", Arc::new(CompactionHandler));
    }

    /// Context handed to every frontend generation (CLI task + active skills).
    pub fn set_frontend_context(&self, task: Option<String>, active_skills: Vec<String>) {
        *self.inner.task.write() = task;
        *self.inner.active_skills.write() = active_skills;
    }

    /// Sets the session frontends must attach to (created once, survives swaps).
    pub fn set_session(&self, session_id: impl Into<String>) {
        *self.inner.session_id.write() = Some(session_id.into());
    }

    /// Active session id, if any.
    pub fn session_id(&self) -> Option<String> {
        self.inner.session_id.read().clone()
    }

    /// Starts a session on the bound engine and records it for frontends.
    pub async fn start_session(
        &self,
        project_dir: Option<PathBuf>,
        active_skills: Vec<String>,
    ) -> Result<String> {
        let driver = self
            .inner
            .driver
            .read()
            .clone()
            .ok_or_else(|| anyhow!("no engine bound to the compose supervisor"))?;
        let request = NaviSessionRequest {
            project_dir,
            session_id: self.session_id(),
            active_skills,
            ..NaviSessionRequest::default()
        };
        let info = driver
            .start_session(request)
            .await
            .map_err(|err| anyhow!("failed to start session: {err}"))?;
        self.set_session(info.id.clone());
        Ok(info.id)
    }

    /// Manifests currently installed.
    pub fn manifests(&self) -> Vec<ComponentManifest> {
        let mut manifests: Vec<ComponentManifest> =
            self.inner.manifests.read().values().cloned().collect();
        manifests.sort_by(|a, b| a.id.cmp(&b.id));
        manifests
    }

    /// Records a manifest on disk **without** activating it.
    ///
    /// Two files are written per component: `<id>.json` (the kernel manifest the
    /// kernel loads) and `<id>.toml` (the NAVI manifest, so a later process can
    /// list/activate the component with its full metadata, including `kind`).
    pub fn install_definition(&self, manifest: ComponentManifest) -> Result<PathBuf> {
        // A sandboxed component needs its private workspace to exist before the
        // host policy is built (`matrix-guard` validates it at spawn time).
        if let Some(spec) = manifest.sandbox.as_ref() {
            let workspace = spec
                .workspace
                .clone()
                .unwrap_or_else(|| self.inner.home.join("workspaces").join(&manifest.id));
            std::fs::create_dir_all(&workspace)
                .map_err(|err| anyhow!("failed to create {}: {err}", workspace.display()))?;
            // The host policy is fixed when the process host attaches; a
            // sandbox installed mid-session takes effect on the next start.
            #[cfg(target_os = "linux")]
            if self.inner.process_host.read().is_some() {
                tracing::warn!(
                    component = %manifest.id,
                    "sandbox declared after the process host attached: \
                     it runs trusted until the session restarts"
                );
            }
        }
        self.inner
            .manifests
            .write()
            .insert(manifest.id.clone(), manifest.clone());
        let path = self.manifest_path(&manifest.id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| anyhow!("failed to create {}: {err}", parent.display()))?;
        }
        std::fs::write(&path, manifest.to_kernel_json().to_string())
            .map_err(|err| anyhow!("failed to write manifest {}: {err}", path.display()))?;
        let toml_path = self.navi_manifest_path(&manifest.id);
        std::fs::write(&toml_path, manifest.to_toml())
            .map_err(|err| anyhow!("failed to write manifest {}: {err}", toml_path.display()))?;
        Ok(path)
    }

    /// Loads every component installed in this compose home (without activating).
    ///
    /// Lets a fresh CLI process see third-party components installed earlier:
    /// their NAVI manifests (with `kind`, description and execution) are on disk
    /// next to the kernel manifests.
    pub fn load_installed(&self) -> Result<usize> {
        let dir = self.inner.home.join("manifests");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(0);
        };
        let mut loaded = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match ComponentManifest::from_toml(&text) {
                Ok(manifest) => {
                    self.inner.manifests.write().insert(manifest.id.clone(), manifest);
                    loaded += 1;
                }
                Err(err) => tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "ignoring unreadable installed manifest"
                ),
            }
        }
        Ok(loaded)
    }

    /// Installs (writes + loads) a component manifest and reconciles.
    pub fn install(&self, manifest: ComponentManifest) -> Result<ActivationOutcome> {
        self.install_definition(manifest.clone())?;
        self.apply(&manifest.id)
    }

    /// Installs every manifest in the set (writing files, activating each).
    pub fn install_all(&self, manifests: &[ComponentManifest]) -> Result<Vec<ActivationOutcome>> {
        let mut outcomes = Vec::new();
        for manifest in manifests {
            outcomes.push(self.install(manifest.clone())?);
        }
        Ok(outcomes)
    }

    /// Writes every manifest **without** activating anything.
    pub fn install_definitions(&self, manifests: &[ComponentManifest]) -> Result<()> {
        for manifest in manifests {
            self.install_definition(manifest.clone())?;
        }
        Ok(())
    }

    /// Activates (or re-activates) an installed component.
    ///
    /// Delegates to the kernel: it validates the manifest, resolves
    /// dependencies, and either publishes the new generation or leaves the
    /// component `Waiting` with a visible reason.
    pub fn activate(&self, id: &str) -> Result<ActivationOutcome> {
        self.apply(id)
    }

    /// Withdraws a component generation (Quiescing → cleanup).
    ///
    /// Matrix v0.1 keeps a *registered definition* continuously desired: the
    /// reconciler recreates an instance for any definition that still exists
    /// (that is what makes `reload` possible). "Stop and stay stopped" is
    /// therefore expressed as *removing the definition* (`dispose_plugin`)
    /// while keeping the installed artifact (the manifest file), so
    /// [`Compose::activate`] restores it later. Consumers keep their
    /// definitions and fall back to `Waiting` (cascade).
    pub fn withdraw(&self, id: &str) -> Result<ActivationOutcome> {
        let previous = self.generation_of(id);
        self.inner.kernel.dispose_plugin(id);
        let notes = self.tick()?;
        Ok(ActivationOutcome {
            component: id.to_string(),
            previous_generation: previous,
            generation: self.generation_of(id),
            state: self.state_of(id).unwrap_or_else(|| "Disposed".to_string()),
            waiting_reason: None,
            notes,
        })
    }

    /// Removes a component entirely: definition, instance and installed artifact.
    pub fn uninstall(&self, id: &str) -> Result<ActivationOutcome> {
        let outcome = self.withdraw(id)?;
        self.inner.manifests.write().remove(id);
        let _ = std::fs::remove_file(self.manifest_path(id));
        let _ = std::fs::remove_file(self.navi_manifest_path(id));
        Ok(outcome)
    }

    /// Runtime swap: replaces the live generation with a new one.
    ///
    /// The kernel withdraws the previous generation *before* activating the
    /// candidate (no overlap); the host stops the old frontend thread, releases
    /// the terminal lease, and starts the successor with fresh bindings.
    pub fn reload(&self, id: &str) -> Result<ActivationOutcome> {
        if !self.inner.manifests.read().contains_key(id) {
            bail!("component `{id}` is not installed");
        }
        self.apply(id)
    }

    fn apply(&self, id: &str) -> Result<ActivationOutcome> {
        let previous = self.generation_of(id);
        let path = self.manifest_path(id);
        if !path.exists() {
            bail!("component `{id}` has no manifest on disk ({})", path.display());
        }
        // The frontend role is exclusive: activating a frontend withdraws any
        // other live frontend first (one terminal, one owner). The kernel
        // records the withdrawal; the host reconciles it in `tick`.
        let kind = self
            .inner
            .manifests
            .read()
            .get(id)
            .map(|manifest| manifest.kind);
        let replaced = if kind == Some(ComponentKind::Ui) {
            self.withdraw_other_frontends(id)
        } else {
            Vec::new()
        };
        let loaded = self
            .inner
            .kernel
            .load_manifest(&path)
            .map_err(|err| anyhow!("kernel rejected manifest `{id}`: {err}"))?;
        debug_assert_eq!(loaded, id);
        let mut notes = self.tick()?;
        for replaced in &replaced {
            notes.push(format!("frontend role: withdrew {replaced}"));
        }
        Ok(ActivationOutcome {
            component: id.to_string(),
            previous_generation: previous,
            generation: self.generation_of(id),
            state: self.state_of(id).unwrap_or_else(|| "Unknown".to_string()),
            waiting_reason: self.inner.kernel.waiting_reason_of(id),
            notes,
        })
    }

    /// Withdraws every live frontend except `keep`.
    ///
    /// NAVI has a single frontend role (one terminal, one owner): activating
    /// `navi.ui.plain` while `navi.ui.tui` runs must swap them, not run both.
    fn withdraw_other_frontends(&self, keep: &str) -> Vec<String> {
        let candidates: Vec<String> = self
            .inner
            .manifests
            .read()
            .values()
            .filter(|manifest| manifest.kind == ComponentKind::Ui && manifest.id != keep)
            .map(|manifest| manifest.id.clone())
            .collect();
        let mut withdrawn = Vec::new();
        for id in candidates {
            if self.inner.kernel.instance_of(&id).is_some() {
                tracing::info!(
                    withdrawn = %id,
                    successor = keep,
                    "frontend role is exclusive; withdrawing the previous frontend"
                );
                self.inner.kernel.dispose_plugin(&id);
                withdrawn.push(id);
            }
        }
        withdrawn
    }

    /// Path of an installed manifest (kernel schema).
    pub fn manifest_path(&self, id: &str) -> PathBuf {
        self.inner.home.join("manifests").join(format!("{id}.json"))
    }

    /// Path of an installed manifest (NAVI schema, for inspection).
    pub fn navi_manifest_path(&self, id: &str) -> PathBuf {
        self.inner.home.join("manifests").join(format!("{id}.toml"))
    }

    /// Kernel generation of a component's live activation.
    pub fn generation_of(&self, id: &str) -> Option<u64> {
        self.inner.kernel.instance_ref_of(id).map(|r| r.generation)
    }

    /// Full kernel reference of a component's live activation.
    pub fn instance_ref_of(&self, id: &str) -> Option<InstanceRef> {
        self.inner.kernel.instance_ref_of(id)
    }

    /// Kernel state of a component.
    pub fn state_of(&self, id: &str) -> Option<String> {
        self.inner.kernel.context_state_of(id)
    }

    /// Drains the lifecycle queue and reconciles host-side state.
    ///
    /// Returns human-readable notes (also useful in tests).
    pub fn tick(&self) -> Result<Vec<String>> {
        let events: Vec<LifecycleEvent> = {
            let mut queue = self.inner.lifecycle.lock();
            std::mem::take(&mut *queue)
        };
        if !events.is_empty() {
            tracing::info!(count = events.len(), "reconciling lifecycle events");
        }
        let mut notes = Vec::new();
        for event in events {
            match event {
                LifecycleEvent::Activated {
                    logical,
                    instance,
                    generation,
                    entrypoint,
                    ..
                } => {
                    notes.push(format!(
                        "activated {logical} (instance={instance} generation={generation})"
                    ));
                    // A frontend is started by *this* host — in-process ones and
                    // process-tier ones (which own the terminal themselves).
                    // Everything else with a real entrypoint is a capability
                    // component the process host spawns.
                    let is_frontend = self.inner.uis.read().contains_key(&logical);
                    if is_frontend || entrypoint.starts_with(INPROC_PREFIX) {
                        self.install_component_values(&logical, generation);
                    }
                    if is_frontend {
                        if let Err(err) = self.start_frontend(&logical, instance, generation) {
                            notes.push(format!("frontend start failed for {logical}: {err}"));
                        }
                    } else if !entrypoint.starts_with(INPROC_PREFIX) {
                        // A capability component with a real binary: the process
                        // host spawns it. From here on the child is supervised.
                        self.inner
                            .seen_active
                            .lock()
                            .insert(logical.clone(), Instant::now());
                        notes.push(format!(
                            "process component {logical} activated; the process host spawns it"
                        ));
                    }
                }
                LifecycleEvent::Withdrawn {
                    logical,
                    instance,
                    generation,
                } => {
                    notes.push(format!(
                        "withdrawn {logical} (instance={instance} generation={generation})"
                    ));
                    self.inner.seen_active.lock().remove(&logical);
                    self.clear_component_values(&logical, generation);
                    if let Some(exit) = self.stop_frontend(&logical) {
                        notes.push(format!("frontend {logical} stopped ({exit:?})"));
                    }
                }
                LifecycleEvent::Removed { logical } => {
                    notes.push(format!("removed {logical}"));
                    self.inner.seen_active.lock().remove(&logical);
                    self.stop_frontend(&logical);
                }
            }
        }
        Ok(notes)
    }

    fn start_frontend(&self, logical: &str, instance: u64, generation: u64) -> Result<()> {
        let frontend = self.inner.uis.read().get(logical).cloned();
        tracing::info!(
            component = %logical,
            generation,
            registered = frontend.is_some(),
            "frontend activation requested"
        );
        let Some(frontend) = frontend else {
            return Ok(()); // not a frontend (engine, tool pack, …)
        };
        if self.inner.running.lock().contains_key(logical) {
            // A previous generation is still draining: stop it first (the
            // kernel already revoked its admission).
            self.stop_frontend(logical);
        }

        // I05: record ownership *before* publishing the effect.
        let resource = if frontend.requires_tty() {
            match self.inner.kernel.acquire_external(
                logical,
                ResourceKind::Task {
                    label: "tty".to_string(),
                },
                InstanceId(instance),
                generation,
            ) {
                Ok(handle) => Some(handle),
                Err(err) => {
                    self.fail_activation(logical, &format!("terminal resource denied: {err}"));
                    return Ok(());
                }
            }
        } else {
            None
        };

        let lease = if frontend.requires_tty() {
            // A process-tier frontend performs the terminal OS effect itself;
            // the host keeps the exclusive-ownership bookkeeping only.
            let acquired = if self.inner.terminal_effects && !frontend.child_owns_terminal() {
                TerminalLease::acquire(format!("{logical}#{generation}"))
            } else {
                TerminalLease::acquire_detached(format!("{logical}#{generation}"))
            };
            match acquired {
                Ok(lease) => Some(lease),
                Err(err) => {
                    if let Some(handle) = resource {
                        let _ = self.inner.kernel.release(handle);
                    }
                    self.fail_activation(logical, &format!("terminal lease failed: {err}"));
                    return Ok(());
                }
            }
        } else {
            None
        };

        let Some(driver) = self.inner.driver.read().clone() else {
            if let Some(handle) = resource {
                let _ = self.inner.kernel.release(handle);
            }
            self.fail_activation(logical, "no engine driver bound");
            return Ok(());
        };

        let control = ControlChannel::new();
        let exit_slot: Arc<Mutex<Option<UiExit>>> = Arc::new(Mutex::new(None));
        let command_capability = crate::ui::command_capability(logical);
        // Admission handler for this frontend's commands: the kernel validates
        // the caller's activation before dispatch (I02/I03).
        self.inner.handlers.register(
            command_capability.clone(),
            Arc::new(CommandHandler {
                inner: Arc::downgrade(&self.inner),
            }),
        );
        let host = UiHost {
            driver,
            lease: lease.clone(),
            control: control.clone(),
            component_id: logical.to_string(),
            generation,
            command_capability,
            commands: Arc::new(KernelCommandSink {
                inner: Arc::downgrade(&self.inner),
                capability: crate::ui::command_capability(logical),
                component: logical.to_string(),
            }),
            project_dir: self.inner.project_dir.clone(),
            loaded_config: self.inner.loaded_config.clone(),
            task: self.inner.task.read().clone(),
            active_skills: self.inner.active_skills.read().clone(),
            session_id: self.inner.session_id.read().clone(),
        };

        let runtime = self.inner.runtime.clone();
        let frontend_thread = frontend.clone();
        let exit_writer = exit_slot.clone();
        let logical_owned = logical.to_string();
        let thread_name = format!("navi-ui-{logical}");
        let join = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                // Keep the CLI's tokio runtime available to the frontend: the
                // TUI spawns async work with `Handle::try_current()`.
                let _guard = runtime.as_ref().map(|handle| handle.enter());
                // A panicking frontend must still report an exit: otherwise the
                // supervisor would wait for a quiesce that can never come.
                let exit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    frontend_thread.run(host)
                }))
                .unwrap_or_else(|_| {
                    tracing::error!(component = %logical_owned, "frontend panicked");
                    UiExit::Error
                });
                *exit_writer.lock() = Some(exit);
            })
            .map_err(|err| anyhow!("failed to spawn frontend thread: {err}"))?;

        self.inner.running.lock().insert(
            logical.to_string(),
            RunningUi {
                generation,
                control,
                lease,
                resource,
                exit: exit_slot,
                join: Some(join),
            },
        );
        self.inner.started_ui.store(true, Ordering::SeqCst);
        tracing::info!(component = logical, generation, "frontend started");
        Ok(())
    }

    fn stop_frontend(&self, logical: &str) -> Option<UiExit> {
        let running = self.inner.running.lock().remove(logical);
        let Some(mut running) = running else {
            return None;
        };
        running.control.send(ControlMsg::Quiesce {
            reason: "generation withdrawn".to_string(),
        });
        tracing::debug!(
            component = logical,
            generation = running.generation,
            "quiescing frontend generation"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while running.exit.lock().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let exit = *running.exit.lock();
        tracing::info!(
            component = logical,
            generation = running.generation,
            exit = ?exit,
            "frontend quiesce settled"
        );
        if exit.is_some()
            && let Some(join) = running.join.take()
        {
            let _ = join.join();
        } else {
            tracing::warn!(
                component = logical,
                "frontend did not acknowledge quiesce before the deadline; \
                 releasing its terminal lease anyway"
            );
        }
        if let Some(lease) = running.lease.as_ref() {
            let _ = lease.release();
        }
        if let Some(handle) = running.resource {
            let _ = self.inner.kernel.release(handle);
        }
        exit
    }

    fn fail_activation(&self, logical: &str, reason: &str) {
        tracing::error!(component = logical, reason, "activation failed; withdrawing");
        *self.inner.last_error.lock() = Some(format!("{logical}: {reason}"));
        // Rollback: the kernel owns the cleanup (I05/I07). The failed generation
        // ends `Disposed`, never half-active. The definition is removed so the
        // reconciler cannot retry in a storm; the installed manifest stays on
        // disk, so `activate` can restore it once the cause is fixed.
        self.inner.kernel.dispose_plugin(logical);
    }

    /// Whether the host currently runs a frontend.
    pub fn has_running_frontend(&self) -> bool {
        !self.inner.running.lock().is_empty()
    }

    /// Reaps frontends that ended on their own (user quit, crash, shutdown).
    ///
    /// A frontend that *failed* is handed to supervision (restart per the
    /// manifest's `restart` policy, with budget and backoff); a frontend the
    /// user quit stays gone. Without this the supervisor would keep a dead
    /// generation in its running set and never exit when the last one quits.
    pub fn reap_exited_frontends(&self) -> Vec<String> {
        let exited: Vec<String> = {
            let running = self.inner.running.lock();
            running
                .iter()
                .filter(|(_, ui)| ui.exit.lock().is_some())
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in &exited {
            let exit = self.stop_frontend(id);
            tracing::info!(component = %id, exit = ?exit, "frontend exited on its own");
            if matches!(exit, Some(UiExit::Error)) {
                self.schedule_restart(id, "frontend failed");
            }
        }
        exited
    }

    /// Records a failed component and schedules its restart per policy.
    pub fn schedule_restart(&self, logical: &str, reason: &str) {
        let restart = self
            .inner
            .manifests
            .read()
            .get(logical)
            .map(|manifest| manifest.restart.clone())
            .unwrap_or_else(|| "on-failure".to_string());
        let now = Instant::now();
        let mut supervision = self.inner.supervision.lock();
        if restart == "never" {
            tracing::warn!(component = logical, reason, "component failed; policy is `never`");
            supervision
                .last_outcome
                .insert(logical.to_string(), RestartOutcome::PolicyNever);
            supervision.failed.insert(logical.to_string());
            return;
        }
        let window = supervision.policy.window;
        let max_attempts = supervision.policy.max_attempts;
        let recorded = {
            let attempts = supervision.attempts.entry(logical.to_string()).or_default();
            attempts.retain(|attempt| now.duration_since(*attempt) < window);
            attempts.len()
        };
        if recorded >= max_attempts {
            tracing::error!(
                component = logical,
                reason,
                attempts = recorded,
                "restart budget exhausted; the component stays down"
            );
            supervision
                .last_outcome
                .insert(logical.to_string(), RestartOutcome::BudgetExhausted);
            supervision.failed.insert(logical.to_string());
            return;
        }
        // Idempotent: a component that is already waiting for its backoff must
        // not consume another attempt (the supervisor polls while it is down).
        if supervision.pending.contains_key(logical) {
            tracing::debug!(component = logical, "restart already scheduled");
            return;
        }
        let exponent = (recorded + 1).min(4) as u32;
        let backoff = (supervision.policy.backoff * 2u32.pow(exponent))
            .min(supervision.policy.backoff_cap);
        supervision
            .attempts
            .entry(logical.to_string())
            .or_default()
            .push(now);
        supervision.pending.insert(logical.to_string(), now + backoff);
        tracing::info!(
            component = logical,
            reason,
            attempt = recorded + 1,
            backoff_ms = backoff.as_millis() as u64,
            "restart scheduled"
        );
    }

    /// Runs the restarts whose backoff elapsed. Returns the components
    /// restarted (or whose attempt failed).
    pub fn process_pending_restarts(&self) -> Vec<String> {
        let due: Vec<String> = {
            let mut supervision = self.inner.supervision.lock();
            let now = Instant::now();
            let due: Vec<String> = supervision
                .pending
                .iter()
                .filter(|(_, when)| **when <= now)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &due {
                supervision.pending.remove(id);
            }
            due
        };
        for id in &due {
            match self.activate(id) {
                Ok(outcome) => {
                    tracing::info!(
                        component = %id,
                        generation = ?outcome.generation,
                        "supervised component restarted"
                    );
                    self.inner.supervision.lock().last_outcome.insert(
                        id.clone(),
                        RestartOutcome::Restarted {
                            generation: outcome.generation,
                        },
                    );
                }
                Err(err) => {
                    tracing::error!(component = %id, error = %err, "restart failed");
                    self.inner.supervision.lock().last_outcome.insert(
                        id.clone(),
                        RestartOutcome::Failed {
                            reason: err.to_string(),
                        },
                    );
                }
            }
        }
        due
    }

    /// Whether a supervised restart is waiting for its backoff.
    pub fn has_pending_restart(&self) -> bool {
        !self.inner.supervision.lock().pending.is_empty()
    }

    /// Last restart decision for a component (tests, `components status`).
    pub fn restart_outcome(&self, logical: &str) -> Option<RestartOutcome> {
        self.inner
            .supervision
            .lock()
            .last_outcome
            .get(logical)
            .cloned()
    }

    /// Restart attempts recorded inside the current window.
    pub fn restart_attempts(&self, logical: &str) -> usize {
        self.inner
            .supervision
            .lock()
            .attempts
            .get(logical)
            .map(|attempts| attempts.len())
            .unwrap_or(0)
    }

    /// Whether supervision gave up on a component.
    pub fn is_supervised_failure(&self, logical: &str) -> bool {
        self.inner.supervision.lock().failed.contains(logical)
    }

    /// Sets the restart policy (budget, window, backoff).
    pub fn set_restart_policy(&self, policy: RestartPolicy) {
        self.inner.supervision.lock().policy = policy;
    }

    /// Current restart policy.
    pub fn restart_policy(&self) -> RestartPolicy {
        self.inner.supervision.lock().policy.clone()
    }

    /// Whether any frontend was ever started (used to decide process exit).
    pub fn started_any_frontend(&self) -> bool {
        self.inner.started_ui.load(Ordering::SeqCst)
    }

    /// Last host-side activation error, if any.
    pub fn last_error(&self) -> Option<String> {
        self.inner.last_error.lock().clone()
    }

    /// Commands admitted by the kernel and not yet executed.
    pub fn take_commands(&self) -> Vec<QueuedCommand> {
        let mut queue = self.inner.commands.lock();
        std::mem::take(&mut *queue)
    }

    /// Executes queued commands against the engine (async effects).
    pub async fn drain_commands(&self) -> Result<usize> {
        let commands = self.take_commands();
        let count = commands.len();
        for command in commands {
            let Some(driver) = self.inner.driver.read().clone() else {
                continue;
            };
            let Some(session_id) = self.session_id() else {
                continue;
            };
            tracing::info!(
                component = %command.component,
                generation = command.generation,
                "executing admitted command"
            );
            let request = NaviTurnRequest {
                session_id,
                message: command.text,
                content_parts: Vec::new(),
                context_packets: Vec::new(),
                thinking: None,
            };
            if let Err(err) = driver.send_turn(request).await {
                tracing::warn!(error = %err, "command execution failed");
            }
        }
        Ok(count)
    }

    /// Requests a reload from another process (CLI `navi components reload`).
    pub fn request_reload(&self, id: &str) -> Result<()> {
        let path = self.reload_request_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(id);
        existing.push('\n');
        std::fs::write(&path, existing)?;
        Ok(())
    }

    /// Consumes pending reload requests (called by the supervisor loop).
    pub fn take_reload_requests(&self) -> Vec<String> {
        let path = self.reload_request_path();
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let _ = std::fs::remove_file(&path);
        content
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect()
    }

    /// Path of the cross-process reload request file.
    pub fn reload_request_path(&self) -> PathBuf {
        self.inner.home.join("reload.requests")
    }

    /// Path of the kernel journal.
    pub fn journal_path(&self) -> PathBuf {
        self.inner.home.join("journal.jsonl")
    }

    /// Component status for the CLI/TUI.
    pub fn status(&self) -> Vec<ComponentStatus> {
        // Snapshot supervision first: `running` must not be held while taking
        // the supervision lock (restarts lock supervision and then start
        // frontends, which locks `running`).
        let supervision: HashMap<String, (usize, Option<String>)> = {
            let supervision = self.inner.supervision.lock();
            supervision
                .attempts
                .keys()
                .chain(supervision.last_outcome.keys())
                .map(|id| {
                    (
                        id.clone(),
                        (
                            supervision
                                .attempts
                                .get(id)
                                .map(|attempts| attempts.len())
                                .unwrap_or(0),
                            supervision.last_outcome.get(id).map(describe_restart),
                        ),
                    )
                })
                .collect()
        };
        let running = self.inner.running.lock();
        let mut statuses: Vec<ComponentStatus> = self
            .manifests()
            .into_iter()
            .map(|manifest| {
                let reference = self.inner.kernel.instance_ref_of(&manifest.id);
                let running_ui = running.get(&manifest.id);
                let (restarts, last_restart) = supervision
                    .get(&manifest.id)
                    .cloned()
                    .unwrap_or((0, None));
                ComponentStatus {
                    state: self
                        .inner
                        .kernel
                        .context_state_of(&manifest.id)
                        .unwrap_or_else(|| "NotLoaded".to_string()),
                    generation: reference.as_ref().map(|r| r.generation),
                    instance: reference.as_ref().map(|r| r.instance),
                    waiting_reason: self.inner.kernel.waiting_reason_of(&manifest.id),
                    ui_running: running_ui.is_some(),
                    tty: running_ui
                        .map(|ui| ui.lease.is_some())
                        .unwrap_or(false),
                    restarts,
                    last_restart,
                    capabilities: manifest.capabilities.clone(),
                    kind: manifest.kind,
                    id: manifest.id,
                }
            })
            .collect();
        statuses.sort_by(|a, b| a.id.cmp(&b.id));
        statuses
    }

    /// Raw kernel inventory (dependencies, resources, pending work).
    pub fn inventory(&self) -> Value {
        self.inner.kernel.inventory()
    }

    /// Publishes the live status snapshot read by `navi components status`.
    ///
    /// The kernel is in-process: another CLI process cannot inspect it, so the
    /// supervisor publishes generations/states/ownership for its clients.
    pub fn write_status_snapshot(&self) -> Result<()> {
        let components: Vec<Value> = self
            .status()
            .iter()
            .map(|status| {
                json!({
                    "id": status.id,
                    "kind": format!("{:?}", status.kind).to_lowercase(),
                    "state": status.state,
                    "generation": status.generation,
                    "instance": status.instance,
                    "waiting_reason": status.waiting_reason,
                    "ui_running": status.ui_running,
                    "tty": status.tty,
                    "restarts": status.restarts,
                    "last_restart": status.last_restart,
                    "capabilities": status.capabilities,
                })
            })
            .collect();
        let snapshot = json!({
            "pid": std::process::id(),
            "epoch": self.inner.kernel.epoch(),
            "session_id": self.session_id(),
            "running_frontend": self.running_frontend(),
            "components": components,
        });
        let path = self.inner.home.join("status.json");
        std::fs::write(&path, serde_json::to_string_pretty(&snapshot)?)
            .map_err(|err| anyhow!("failed to write status snapshot: {err}"))?;
        Ok(())
    }

    /// Reads the snapshot published by a running supervisor.
    pub fn read_status_snapshot(home: &Path) -> Option<Value> {
        let content = std::fs::read_to_string(home.join("status.json")).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Pending calls/effects known to the kernel.
    pub fn pending_work(&self) -> (usize, usize) {
        (
            self.inner.kernel.pending_calls().len(),
            self.inner.kernel.committed_effects(None).len(),
        )
    }

    /// Supervisor loop: reconciles lifecycle, drains effects, honors reload
    /// requests, and returns when no frontend is running.
    pub async fn run_until_exit(&self) -> Result<()> {
        self.install_shutdown_signals();
        loop {
            self.tick()?;
            self.reap_exited_frontends();
            self.supervise_process_components();
            self.process_pending_restarts();
            self.drain_commands().await?;
            for id in self.take_reload_requests() {
                match self.reload(&id) {
                    Ok(outcome) => tracing::info!(
                        component = %outcome.component,
                        generation = ?outcome.generation,
                        "runtime reload applied"
                    ),
                    Err(err) => tracing::error!(component = %id, error = %err, "reload failed"),
                }
            }
            self.publish_status_if_changed();
            // A failed frontend whose restart is scheduled keeps the session
            // alive: supervision gets its chance before we give up on it.
            if self.started_any_frontend()
                && !self.has_running_frontend()
                && !self.has_pending_restart()
            {
                self.shutdown();
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    /// Restarts process components whose child process died.
    ///
    /// `matrix-host` spawns and speaks the protocol but has no restart policy;
    /// supervision belongs to the composition owner. A child that exited while
    /// the kernel still considers the component `Active` is a failure: it is
    /// scheduled for a restart (new generation → fresh process) under the same
    /// budget as frontends. Components that never reported `Active` to this
    /// host, and ones still inside their startup grace, are ignored.
    pub fn supervise_process_components(&self) -> Vec<String> {
        // Snapshot first: never call into `status()` while holding a lock.
        let active: HashSet<String> = self
            .status()
            .into_iter()
            .filter(|component| component.state == "Active")
            .map(|component| component.id)
            .collect();
        let mut candidates: Vec<String> = {
            let mut seen = self.inner.seen_active.lock();
            // Forget components that are no longer active; components still
            // inside their startup grace stay tracked (just not supervised yet).
            seen.retain(|id, _| active.contains(id));
            let now = Instant::now();
            seen.iter()
                .filter(|(_, first_seen)| now.duration_since(**first_seen) >= PROCESS_GRACE)
                .map(|(id, _)| id.clone())
                .collect()
        };
        candidates.retain(|id| !self.inner.supervision.lock().failed.contains(id));
        let mut dead = Vec::new();
        for id in candidates {
            let running = self
                .inner
                .process_host
                .read()
                .as_ref()
                .map(|host| host.child_running(&id))
                .unwrap_or(true);
            if !running {
                tracing::warn!(component = %id, "process component child is gone");
                self.schedule_restart(&id, "child process exited");
                dead.push(id);
            }
        }
        dead
    }

    /// Writes the status snapshot only when the composition changed.
    fn publish_status_if_changed(&self) {
        let fingerprint = format!(
            "{:?}|{:?}",
            self.session_id(),
            self.status()
                .iter()
                .map(|status| (
                    status.id.clone(),
                    status.state.clone(),
                    status.generation,
                    status.instance,
                    status.ui_running,
                    status.tty,
                ))
                .collect::<Vec<_>>()
        );
        {
            let mut last = self.inner.last_status_fingerprint.lock();
            if *last == fingerprint {
                return;
            }
            *last = fingerprint;
        }
        if let Err(err) = self.write_status_snapshot() {
            tracing::warn!(error = %err, "failed to publish compose status snapshot");
        }
    }

    /// Convenience for hosts that need a session before the frontends start.
    pub async fn start_session_for_project(&self, active_skills: Vec<String>) -> Result<String> {
        let project_dir = Some(self.inner.project_dir.clone());
        self.start_session(project_dir, active_skills).await
    }

    /// Component id of the currently running frontend, if exactly one.
    pub fn running_frontend(&self) -> Option<String> {
        self.inner.running.lock().keys().next().cloned()
    }

    /// Registers process-tier frontends for installed `kind = ui` components.
    ///
    /// A frontend component whose `execution.entrypoint` is a real binary runs
    /// as a child that owns the terminal (`ProcessFrontend`). Call this after
    /// loading installed manifests and before activating the frontend.
    pub fn register_installed_frontends(&self) -> Vec<String> {
        let mut registered = Vec::new();
        for manifest in self.manifests() {
            if manifest.kind != ComponentKind::Ui || manifest.execution.is_inproc() {
                continue;
            }
            let Some(entrypoint) = manifest.execution.entrypoint() else {
                continue;
            };
            let args = match &manifest.execution {
                crate::manifest::Execution::Detailed { args, .. } => args.clone(),
                crate::manifest::Execution::Kind(_) => Vec::new(),
            };
            self.register_ui(Arc::new(crate::process_frontend::ProcessFrontend::new(
                manifest.id.clone(),
                entrypoint,
                args,
                self.inner.home.join("ui"),
            )));
            registered.push(manifest.id);
        }
        registered
    }

    /// The process host, when attached (Linux + `process_components`).
    pub fn process_host(&self) -> Option<Arc<ProcessHost>> {
        self.inner.process_host.read().clone()
    }

    /// Installed tool plugins (process tier, `kind = tool`).
    ///
    /// Convention: a tool plugin declares `navi.tool.<name>@1` (the tool) plus
    /// `navi.tool.<name>.definition@1` (its definition), so several plugins
    /// coexist without capability ambiguity.
    pub fn tool_plugins(&self) -> Vec<(String, String, String)> {
        self.manifests()
            .into_iter()
            .filter(|manifest| manifest.kind == ComponentKind::Tool && !manifest.execution.is_inproc())
            .filter_map(|manifest| {
                let definition = manifest
                    .capabilities
                    .iter()
                    .find(|capability| {
                        capability.starts_with("navi.tool.") && capability.ends_with(".definition@1")
                    })?
                    .clone();
                let tool = manifest
                    .capabilities
                    .iter()
                    .find(|capability| {
                        capability.starts_with("navi.tool.") && !capability.ends_with(".definition@1")
                    })?
                    .clone();
                Some((manifest.id, tool, definition))
            })
            .collect()
    }

    /// Fetches a plugin's tool definition, waiting for the child to connect.
    ///
    /// Activation spawns the process asynchronously, so the first calls can
    /// fail with `outcome-unknown` until the handshake completes.
    pub fn fetch_tool_spec(
        &self,
        component: &str,
        definition_capability: &str,
        timeout: Duration,
    ) -> Option<PluginToolSpec> {
        let deadline = Instant::now() + timeout;
        loop {
            let (value, ok) = self
                .inner
                .kernel
                .invoke(definition_capability, &json!({}));
            if ok && let Some(spec) = PluginToolSpec::from_response(component, &value) {
                return Some(spec);
            }
            if Instant::now() >= deadline {
                tracing::warn!(
                    component = %component,
                    capability = %definition_capability,
                    "plugin tool definition unavailable"
                );
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Activates every installed tool plugin and returns their tool proxies.
    ///
    /// Called by the CLI before building the engine, so the session's tool set
    /// includes the third-party tools.
    pub fn activate_tool_plugins(&self) -> Vec<(String, PluginToolSpec)> {
        let mut activated = Vec::new();
        for (component, _tool, definition) in self.tool_plugins() {
            match self.activate(&component) {
                Ok(outcome) => tracing::info!(
                    component = %component,
                    state = %outcome.state,
                    generation = ?outcome.generation,
                    "tool plugin activated"
                ),
                Err(err) => {
                    tracing::warn!(component = %component, error = %err, "tool plugin activation failed");
                    continue;
                }
            }
            if let Some(spec) = self.fetch_tool_spec(&component, &definition, Duration::from_secs(5)) {
                activated.push((component, spec));
            }
        }
        activated
    }

    /// Tears the composition down: kills process children and stops the host.
    ///
    /// The kernel journal and the installed manifests stay on disk.
    pub fn shutdown(&self) {
        #[cfg(target_os = "linux")]
        {
            let host = self.inner.process_host.read().clone();
            if let Some(host) = host {
                host.shutdown();
            }
        }
    }

    /// Asks every running frontend to quit (used by signal-driven shutdown).
    ///
    /// The frontends restore the terminal and return; the supervisor then exits
    /// once no frontend is left.
    pub fn shutdown_frontends(&self) {
        let running = self.inner.running.lock();
        for (id, ui) in running.iter() {
            tracing::info!(component = %id, "shutdown requested; asking the frontend to quit");
            ui.control.send(ControlMsg::Shutdown);
        }
    }

    /// Installs a one-shot SIGTERM/SIGINT/SIGHUP handler for the supervisor.
    ///
    /// Without it a composed session has no clean shutdown path: whichever
    /// frontend is running owns the input loop, and the process would only die
    /// on SIGKILL (leaving the terminal unrestored).
    fn install_shutdown_signals(&self) {
        if self.inner.signals_installed.swap(true, Ordering::SeqCst) {
            return;
        }
        let compose = self.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let mut terminate = match signal(SignalKind::terminate()) {
                    Ok(stream) => stream,
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to install SIGTERM handler");
                        return;
                    }
                };
                let mut interrupt = match signal(SignalKind::interrupt()) {
                    Ok(stream) => stream,
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to install SIGINT handler");
                        return;
                    }
                };
                let mut hangup = match signal(SignalKind::hangup()) {
                    Ok(stream) => stream,
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to install SIGHUP handler");
                        return;
                    }
                };
                tokio::select! {
                    _ = terminate.recv() => {}
                    _ = interrupt.recv() => {}
                    _ = hangup.recv() => {}
                }
                compose.shutdown_frontends();
            }
            #[cfg(not(unix))]
            {
                if tokio::signal::ctrl_c().await.is_ok() {
                    compose.shutdown_frontends();
                }
            }
        });
    }
}
