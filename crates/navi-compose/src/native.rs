//! Native (in-process) capability handlers.
//!
//! The Matrix kernel never executes component code: dispatch for a component
//! declared as external goes to `CallForwarder::forward`. NAVI's trusted
//! first-party components run in-process, so the host forwarder routes the
//! capability to a native [`Handler`] registered here.
//!
//! Handlers are synchronous by contract: they run while the kernel holds no
//! metadata lock, but they must not block on async work (see the design note in
//! `~/lab/navi-matrix/DESIGN.md` §5.2/§9). Async effects are queued and drained
//! by the supervisor.

use std::collections::HashMap;
use std::sync::Arc;

use matrix_core::external::{CallForwarder, ForwardError, ForwardOutcome, ForwardRequest};
use parking_lot::RwLock;
use serde_json::Value;

/// Identity of the activation that is being invoked.
///
/// The kernel already validated the generation at admission (I03): a handler
/// can trust `generation`/`instance` to describe the *live* activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Logical component id (`navi.engine`).
    pub component: String,
    /// Versioned capability being invoked (`navi.engine.driver@1`).
    pub capability: String,
    /// Kernel instance id of the activation.
    pub instance: u64,
    /// Kernel generation of the activation.
    pub generation: u64,
}

/// Handler failure: stable code + human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerError {
    /// Stable machine code (`dependency-unavailable`, `internal`, …).
    pub code: String,
    /// Human-readable message.
    pub message: String,
}

impl HandlerError {
    /// Builds an error.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Generic internal failure.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

/// Result of a native capability invocation.
pub type HandlerResult = Result<Value, HandlerError>;

/// A native capability implementation.
pub trait Handler: Send + Sync {
    /// Invokes the capability. Must be short and must not block on async I/O.
    fn invoke(&self, invocation: &Invocation, input: &Value) -> HandlerResult;
}

/// Capability → handler table.
#[derive(Default)]
pub struct HandlerTable {
    entries: RwLock<HashMap<String, Arc<dyn Handler>>>,
}

impl HandlerTable {
    /// Creates an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers (or replaces) the handler for a capability.
    pub fn register(&self, capability: impl Into<String>, handler: Arc<dyn Handler>) {
        self.entries.write().insert(capability.into(), handler);
    }

    /// Removes a handler.
    pub fn unregister(&self, capability: &str) -> bool {
        self.entries.write().remove(capability).is_some()
    }

    /// Looks up a handler.
    pub fn get(&self, capability: &str) -> Option<Arc<dyn Handler>> {
        self.entries.read().get(capability).cloned()
    }

    /// Registered capabilities.
    pub fn capabilities(&self) -> Vec<String> {
        let mut caps: Vec<String> = self.entries.read().keys().cloned().collect();
        caps.sort();
        caps
    }
}

/// Host forwarder: native handlers first, then an optional process forwarder.
pub struct NativeForwarder {
    table: Arc<HandlerTable>,
    process: Option<Arc<dyn CallForwarder>>,
}

impl NativeForwarder {
    /// Builds a forwarder over a handler table.
    pub fn new(table: Arc<HandlerTable>) -> Self {
        Self {
            table,
            process: None,
        }
    }

    /// Chains a process forwarder for components the host runs as child
    /// processes (`matrix-host` territory).
    pub fn with_process(mut self, process: Arc<dyn CallForwarder>) -> Self {
        self.process = Some(process);
        self
    }
}

impl CallForwarder for NativeForwarder {
    fn forward(&self, req: &ForwardRequest) -> ForwardOutcome {
        if let Some(handler) = self.table.get(&req.cap) {
            let invocation = Invocation {
                component: req.logical.clone(),
                capability: req.cap.clone(),
                instance: req.instance.0,
                generation: req.generation,
            };
            return match handler.invoke(&invocation, &req.input) {
                Ok(value) => ForwardOutcome::Ok(value),
                Err(err) => ForwardOutcome::Err {
                    code: err.code,
                    message: err.message,
                },
            };
        }
        if let Some(process) = &self.process {
            return process.forward(req);
        }
        ForwardOutcome::Failed(ForwardError::Protocol(format!(
            "no host handler for capability `{}` (component `{}`)",
            req.cap, req.logical
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Echo(AtomicU64);

    impl Handler for Echo {
        fn invoke(&self, invocation: &Invocation, input: &Value) -> HandlerResult {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({
                "component": invocation.component,
                "cap": invocation.capability,
                "generation": invocation.generation,
                "echo": input,
            }))
        }
    }

    fn request(cap: &str) -> ForwardRequest {
        ForwardRequest {
            ticket: matrix_core::TicketId(1),
            cap: cap.to_string(),
            input: serde_json::json!({"ping": 1}),
            logical: "navi.test".into(),
            instance: matrix_core::InstanceId(7),
            generation: 3,
            timeout_ms: 1_000,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    #[test]
    fn forwarder_routes_to_registered_handler() {
        let table = Arc::new(HandlerTable::new());
        let handler = Arc::new(Echo(AtomicU64::new(0)));
        table.register("navi.test.echo@1", handler.clone());
        let forwarder = NativeForwarder::new(table);

        let out = forwarder.forward(&request("navi.test.echo@1"));
        match out {
            ForwardOutcome::Ok(value) => {
                assert_eq!(value["generation"], 3);
                assert_eq!(value["echo"]["ping"], 1);
            }
            other => panic!("expected ok, got {other:?}"),
        }
        assert_eq!(handler.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn forwarder_fails_closed_without_handler() {
        let forwarder = NativeForwarder::new(Arc::new(HandlerTable::new()));
        match forwarder.forward(&request("navi.missing@1")) {
            ForwardOutcome::Failed(ForwardError::Protocol(message)) => {
                assert!(message.contains("no host handler"));
            }
            other => panic!("expected protocol failure, got {other:?}"),
        }
    }

    #[test]
    fn handler_error_keeps_stable_code() {
        struct Failing;
        impl Handler for Failing {
            fn invoke(&self, _: &Invocation, _: &Value) -> HandlerResult {
                Err(HandlerError::new("dependency-unavailable", "engine not bound"))
            }
        }
        let table = Arc::new(HandlerTable::new());
        table.register("navi.test.fail@1", Arc::new(Failing));
        let forwarder = NativeForwarder::new(table);
        match forwarder.forward(&request("navi.test.fail@1")) {
            ForwardOutcome::Err { code, message } => {
                assert_eq!(code, "dependency-unavailable");
                assert_eq!(message, "engine not bound");
            }
            other => panic!("expected err, got {other:?}"),
        }
    }
}
