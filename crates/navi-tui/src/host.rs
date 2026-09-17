//! Frontend host contract.
//!
//! A NAVI frontend (TUI, plain REPL, web bridge, …) is a *component*: the
//! compose supervisor owns its lifecycle and hands it a [`UiHost`] with the
//! engine driver, the exclusive [`TerminalLease`], and a [`ControlHandle`]
//! used to quiesce the frontend before a runtime swap.
//!
//! Two invariants matter here:
//!
//! * **Exclusive terminal ownership (I01-like).** Exactly one live generation
//!   may own the terminal. [`TerminalLease::acquire`] fails closed when another
//!   lease is held, so a second frontend can never corrupt the screen of the
//!   active one.
//! * **Stale generations never clobber the successor.** The panic hook and the
//!   `Drop` guard only restore terminal modes while a lease is actually held;
//!   a released generation that panics after a swap is a no-op.

use std::collections::VecDeque;
use std::io;
use std::panic;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};

use anyhow::Result;
use navi_sdk::{EngineDriver, LoadedConfig};

use crate::event_loop::{enter_terminal_modes, restore_terminal_modes_best_effort};

/// Why a frontend stopped running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiExit {
    /// The user quit the frontend (session ends).
    Quit,
    /// The supervisor asked the frontend to release the terminal so a new
    /// generation can be activated (runtime swap).
    Quiesced,
    /// The frontend hit an error and stopped.
    Error,
}

/// Control messages delivered to a running frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMsg {
    /// Stop admitting input/events, restore the terminal and return
    /// [`UiExit::Quiesced`]. Sent by the compose supervisor right before
    /// withdrawing the generation.
    Quiesce { reason: String },
    /// Terminate the frontend (and, for the root frontend, the process).
    Shutdown,
}

/// Queue-based control channel.
///
/// The frontend event loop is synchronous (no nested tokio runtime), so the
/// channel is a plain `Mutex<VecDeque<..>>` polled non-blockingly each tick.
#[derive(Debug, Default)]
pub struct ControlChannel {
    queue: Mutex<VecDeque<ControlMsg>>,
    quiesce_requested: AtomicBool,
}

/// Shared handle to a [`ControlChannel`].
pub type ControlHandle = Arc<ControlChannel>;

impl ControlChannel {
    /// Creates a fresh control channel.
    pub fn new() -> ControlHandle {
        Arc::new(Self::default())
    }

    /// Enqueues a control message.
    ///
    /// Returns `false` when a quiesce was already requested: repeated quiesce
    /// requests are idempotent (I06) and the caller can treat the `false` as
    /// "already draining".
    pub fn send(&self, msg: ControlMsg) -> bool {
        if matches!(msg, ControlMsg::Quiesce { .. })
            && self.quiesce_requested.swap(true, Ordering::SeqCst)
        {
            return false;
        }
        self.queue.lock().expect("control queue").push_back(msg);
        true
    }

    /// Pops the next pending message, if any.
    pub fn poll(&self) -> Option<ControlMsg> {
        self.queue.lock().expect("control queue").pop_front()
    }

    /// True once a quiesce has been requested (even if already consumed).
    pub fn quiesce_requested(&self) -> bool {
        self.quiesce_requested.load(Ordering::SeqCst)
    }
}

/// Process-global terminal ownership.
static TTY_OWNER: Mutex<Option<String>> = Mutex::new(None);
static PANIC_HOOK: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseState {
    Held,
    Released,
}

/// Exclusive lease over the process terminal (raw mode + alternate screen).
///
/// The OS effect (termios / DEC modes) is performed here; ownership is also
/// recorded in the compose kernel (`acquire_external`), so the lifecycle and
/// the effect agree on the same owner and generation.
pub struct TerminalLease {
    owner: String,
    state: Mutex<LeaseState>,
}

impl TerminalLease {
    /// Acquires the terminal for `owner`, entering raw mode + alternate screen.
    ///
    /// Fails closed when another lease is live (exclusive resource).
    pub fn acquire(owner: impl Into<String>) -> Result<Arc<Self>> {
        let lease = Self::claim(owner)?;
        if let Err(err) = enter_terminal_modes(&mut io::stdout()) {
            // Partial acquisition must not leak ownership (I05).
            let _ = lease.release();
            return Err(err.into());
        }
        Ok(lease)
    }

    /// Acquires the lease **without** touching terminal modes.
    ///
    /// The exclusivity contract is identical; only the OS effect is skipped.
    /// Used by headless hosts and tests (CI has no TTY) that still need
    /// one-owner-per-terminal bookkeeping.
    pub fn acquire_detached(owner: impl Into<String>) -> Result<Arc<Self>> {
        Self::claim(owner)
    }

    fn claim(owner: impl Into<String>) -> Result<Arc<Self>> {
        let owner = owner.into();
        {
            let mut guard = TTY_OWNER.lock().expect("tty owner lock");
            if let Some(current) = guard.as_ref() {
                anyhow::bail!(
                    "terminal lease unavailable: already owned by `{current}` (exclusive resource)"
                );
            }
            *guard = Some(owner.clone());
        }
        install_restore_panic_hook();
        Ok(Arc::new(Self {
            owner,
            state: Mutex::new(LeaseState::Held),
        }))
    }

    /// Restores the terminal and drops ownership. Idempotent.
    pub fn release(&self) -> io::Result<()> {
        {
            let mut state = self.state.lock().expect("lease state");
            if *state == LeaseState::Released {
                return Ok(());
            }
            *state = LeaseState::Released;
        }
        let restore = restore_terminal_modes_best_effort();
        let mut guard = TTY_OWNER.lock().expect("tty owner lock");
        if guard.as_deref() == Some(self.owner.as_str()) {
            *guard = None;
        }
        restore
    }

    /// True while this lease still owns the terminal.
    pub fn is_held(&self) -> bool {
        *self.state.lock().expect("lease state") == LeaseState::Held
    }

    /// Owner label recorded at acquisition (component id + generation).
    pub fn owner(&self) -> &str {
        &self.owner
    }
}

impl Drop for TerminalLease {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Installs a panic hook that restores terminal modes only while a lease is
/// actually held.
///
/// Without the ownership check, a panic in an *old* generation (already
/// quiesced and swapped out) would restore the terminal on top of the new
/// generation's screen.
pub fn install_restore_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let held = TTY_OWNER.lock().map(|g| g.is_some()).unwrap_or(false);
            if held {
                let _ = restore_terminal_modes_best_effort();
            }
            previous(info);
        }));
    });
}

/// Sink that admits a UI command through the compose kernel.
///
/// The kernel revalidates the frontend's activation on every call (I02/I03):
/// after a generation is withdrawn, `submit` fails with `context-not-active`
/// instead of silently reaching the engine.
pub trait UiCommandSink: Send + Sync {
    /// Submits a user command. Returns a stable error code + message on denial.
    fn submit(&self, command: &str) -> Result<(), String>;
}

/// Everything a frontend needs to run: engine access, terminal ownership,
/// control channel, and the session it should attach to.
pub struct UiHost {
    /// Engine facade (`EngineDriver` is the versioned UI↔engine contract).
    pub driver: Arc<dyn EngineDriver>,
    /// Exclusive terminal lease held by this generation (`None` for frontends
    /// that do not own the terminal, e.g. line-based/headless frontends).
    pub lease: Option<Arc<TerminalLease>>,
    /// Control channel polled by the frontend loop.
    pub control: ControlHandle,
    /// Component id that owns this run (e.g. `navi.ui.tui`).
    pub component_id: String,
    /// Kernel generation of this activation (for diagnostics/UI labels).
    pub generation: u64,
    /// Versioned capability this frontend's commands are admitted through.
    pub command_capability: String,
    /// Admission sink for user commands (kernel-mediated).
    pub commands: Arc<dyn UiCommandSink>,
    /// Project root.
    pub project_dir: PathBuf,
    /// Loaded configuration.
    pub loaded_config: LoadedConfig,
    /// Initial task typed on the CLI, if any.
    pub task: Option<String>,
    /// Session-active skills (`--skill`).
    pub active_skills: Vec<String>,
    /// Session the frontend must attach to. `None` starts a fresh session.
    pub session_id: Option<String>,
}
