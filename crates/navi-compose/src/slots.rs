//! Host-side typed component slots.
//!
//! The kernel mediates composition — who is active, in which generation. Some
//! parts of NAVI are not JSON capabilities but Rust trait objects the engine
//! calls directly (the compaction strategy, the system-prompt builder). For
//! those, the host keeps a **typed slot keyed by the capability** the component
//! provides:
//!
//! * activating a provider installs its value for that generation;
//! * withdrawing it pops the slot back to the previous provider (a stack), so a
//!   swap is a stack push and a rollback is a pop;
//! * the engine holds a *proxy* that reads the slot on every call, so a runtime
//!   swap changes behavior on the next use without restarting anything.
//!
//! This is the local, generation-checked half of the composition contract: the
//! kernel owns lifecycle/admission, the host owns the typed handle.

use std::sync::Arc;

use navi_core::{
    CompactOutcome, CompactState, CompactionStrategy, ModelMessage, ModelProvider, PromptBuilder,
    PromptCache, RenderedPrompt, SystemPromptInput, config::HarnessConfig,
};
use parking_lot::RwLock;

/// One installed provider of a slot.
struct Entry<T: ?Sized> {
    component: String,
    generation: u64,
    value: Arc<T>,
}

/// A stack of providers for one typed role.
pub struct Slot<T: ?Sized> {
    entries: RwLock<Vec<Entry<T>>>,
    role: &'static str,
}

impl<T: ?Sized> Slot<T> {
    /// Creates an empty slot for `role` (used in diagnostics).
    pub fn new(role: &'static str) -> Arc<Self> {
        Arc::new(Self {
            entries: RwLock::new(Vec::new()),
            role,
        })
    }

    /// Installs (or re-installs) the value provided by `component`.
    ///
    /// Re-installing moves the component to the top of the stack: a reload (a
    /// new generation of the same component) takes the slot back from a
    /// provider installed after it. There is still exactly one entry per
    /// component, so a reload never grows the stack.
    pub fn install(&self, component: &str, generation: u64, value: Arc<T>) {
        let mut entries = self.entries.write();
        entries.retain(|entry| entry.component != component);
        entries.push(Entry {
            component: component.to_string(),
            generation,
            value,
        });
    }

    /// Removes `component`'s entry when it is still the owner of `generation`.
    ///
    /// Returns `true` when an entry was removed. A stale generation never
    /// removes its successor (I03).
    pub fn clear_if_owner(&self, component: &str, generation: u64) -> bool {
        let mut entries = self.entries.write();
        let Some(index) = entries
            .iter()
            .position(|entry| entry.component == component && entry.generation == generation)
        else {
            return false;
        };
        entries.remove(index);
        true
    }

    /// The active value (the most recently installed provider).
    pub fn current(&self) -> Option<(String, u64, Arc<T>)> {
        let entries = self.entries.read();
        entries
            .last()
            .map(|entry| (entry.component.clone(), entry.generation, entry.value.clone()))
    }

    /// Installed providers, oldest first (diagnostics/tests).
    pub fn providers(&self) -> Vec<(String, u64)> {
        self.entries
            .read()
            .iter()
            .map(|entry| (entry.component.clone(), entry.generation))
            .collect()
    }

    /// Role name (for logs).
    pub fn role(&self) -> &'static str {
        self.role
    }
}

/// Compaction strategy slot (`navi.compaction@1`).
pub type CompactionSlot = Slot<dyn CompactionStrategy>;

/// System-prompt builder slot (`navi.prompt@1`).
pub type PromptSlot = Slot<dyn PromptBuilder>;

/// The typed slots a compose host exposes to the engine.
#[derive(Clone)]
pub struct Slots {
    /// `navi.compaction@1`
    pub compaction: Arc<CompactionSlot>,
    /// `navi.prompt@1`
    pub prompt: Arc<PromptSlot>,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            compaction: CompactionSlot::new("navi.compaction@1"),
            prompt: PromptSlot::new("navi.prompt@1"),
        }
    }
}

impl Slots {
    /// Creates the default slot set.
    pub fn new() -> Self {
        Self::default()
    }
}

/// `CompactionStrategy` proxy: resolves the active provider per call.
pub struct ComposeCompaction {
    slots: Slots,
    fallback: Arc<dyn CompactionStrategy>,
}

impl ComposeCompaction {
    /// Builds a proxy over `slots`, falling back to `fallback` when no component
    /// provides a compaction strategy.
    pub fn new(slots: Slots, fallback: Arc<dyn CompactionStrategy>) -> Self {
        Self { slots, fallback }
    }

    fn active(&self) -> Arc<dyn CompactionStrategy> {
        self.slots
            .compaction
            .current()
            .map(|(_, _, value)| value)
            .unwrap_or_else(|| self.fallback.clone())
    }
}

#[async_trait::async_trait]
impl CompactionStrategy for ComposeCompaction {
    fn micro_compact(&self, messages: &mut Vec<ModelMessage>, gap_threshold_minutes: u64) -> usize {
        self.active().micro_compact(messages, gap_threshold_minutes)
    }

    async fn auto_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> anyhow::Result<Option<CompactOutcome>> {
        self.active()
            .auto_compact(state, messages, provider, model, config)
            .await
    }

    async fn force_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> anyhow::Result<Option<CompactOutcome>> {
        self.active()
            .force_compact(state, messages, provider, model, config)
            .await
    }
}

/// `CompactionStrategy` served by a **component in another process**.
///
/// The plugin decides, the host applies: the host sends the conversation plus
/// the gap threshold, and the plugin answers with indices to drop or to clear.
/// Applying on the host side keeps every field the engine owns (`created_at`
/// is not serialized, for instance) exactly as it was.
///
/// Contract (`navi.compaction@1`):
///
/// ```json
/// // host → plugin
/// {"messages": [{"role":"tool","content":"…","tool_name":"read_file"}],
///  "timestamps": [1758000000000], "gap_threshold_minutes": 60}
/// // plugin → host
/// {"drop": [0], "cleared": [2]}
/// ```
///
/// Full compaction (`auto_compact`/`force_compact`) stays with the fallback
/// strategy: a process component owns the *micro* policy it declares, and the
/// engine keeps a working full-compaction path.
pub struct ProcessCompaction {
    component: String,
    generation: u64,
    capability: String,
    /// Authenticated instance grant: the call is routed to *this* generation of
    /// *this* component, never to a replacement generation by name (I03).
    reference: matrix_core::InstanceRef,
    kernel: Arc<matrix_core::Kernel>,
    fallback: Arc<dyn CompactionStrategy>,
}

impl ProcessCompaction {
    /// Binds a slot value to the capability of a running component.
    pub fn new(
        component: impl Into<String>,
        generation: u64,
        capability: impl Into<String>,
        reference: matrix_core::InstanceRef,
        kernel: Arc<matrix_core::Kernel>,
        fallback: Arc<dyn CompactionStrategy>,
    ) -> Self {
        Self {
            component: component.into(),
            generation,
            capability: capability.into(),
            reference,
            kernel,
            fallback,
        }
    }

    /// Component id providing this strategy.
    pub fn component(&self) -> &str {
        &self.component
    }

    /// Generation providing this strategy.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[async_trait::async_trait]
impl CompactionStrategy for ProcessCompaction {
    fn micro_compact(&self, messages: &mut Vec<ModelMessage>, gap_threshold_minutes: u64) -> usize {
        let timestamps: Vec<Option<u64>> = messages.iter().map(|message| message.created_at).collect();
        let input = serde_json::json!({
            "messages": messages,
            "timestamps": timestamps,
            "gap_threshold_minutes": gap_threshold_minutes,
        });
        let (value, ok) = self
            .kernel
            .invoke_for_ref(&self.reference, &self.capability, &input);
        if !ok {
            tracing::warn!(
                component = %self.component,
                capability = %self.capability,
                response = %value,
                "process compaction component did not answer; leaving the conversation as is"
            );
            return 0;
        }
        let mut changed = 0;
        // `cleared`: keep the message, empty its content (the default strategy's
        // move for stale tool results).
        for index in indices(&value, "cleared") {
            if let Some(message) = messages.get_mut(index)
                && !message.content.is_empty()
            {
                message.content.clear();
                message.content_parts.clear();
                changed += 1;
            }
        }
        // `drop`: remove the message entirely (highest index first).
        let mut drops = indices(&value, "drop");
        drops.sort_unstable_by(|a, b| b.cmp(a));
        for index in drops {
            if index < messages.len() {
                messages.remove(index);
                changed += 1;
            }
        }
        changed
    }

    async fn auto_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> anyhow::Result<Option<CompactOutcome>> {
        self.fallback
            .auto_compact(state, messages, provider, model, config)
            .await
    }
}

/// Reads an index array out of a plugin response, ignoring junk.
fn indices(value: &serde_json::Value, field: &str) -> Vec<usize> {
    value
        .get(field)
        .and_then(|entries| entries.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_u64())
                .map(|index| index as usize)
                .collect()
        })
        .unwrap_or_default()
}

/// `PromptBuilder` proxy: resolves the active provider per call.
pub struct ComposePrompt {
    slots: Slots,
    fallback: Arc<dyn PromptBuilder>,
}

impl ComposePrompt {
    /// Builds a proxy over `slots`, falling back to `fallback` when no component
    /// provides a prompt builder.
    pub fn new(slots: Slots, fallback: Arc<dyn PromptBuilder>) -> Self {
        Self { slots, fallback }
    }

    fn active(&self) -> Arc<dyn PromptBuilder> {
        self.slots
            .prompt
            .current()
            .map(|(_, _, value)| value)
            .unwrap_or_else(|| self.fallback.clone())
    }
}

impl PromptBuilder for ComposePrompt {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> RenderedPrompt {
        self.active().build(input, cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counter(AtomicUsize);

    impl PromptBuilder for Counter {
        fn build(&self, _input: SystemPromptInput, _cache: Arc<PromptCache>) -> RenderedPrompt {
            self.0.fetch_add(1, Ordering::SeqCst);
            RenderedPrompt::default()
        }
    }

    #[test]
    fn slot_install_and_clear_follow_generations() {
        let slot: Arc<Slot<Counter>> = Slot::new("test");
        let first = Arc::new(Counter(AtomicUsize::new(0)));
        let second = Arc::new(Counter(AtomicUsize::new(0)));

        assert!(slot.current().is_none());
        slot.install("a", 1, first.clone());
        assert_eq!(slot.providers(), vec![("a".to_string(), 1)]);

        // A stale generation must not clear its successor (I03).
        slot.install("a", 2, second.clone());
        assert!(!slot.clear_if_owner("a", 1), "stale generation must not clear");
        assert_eq!(slot.current().map(|(_, generation, _)| generation), Some(2));
        assert!(slot.clear_if_owner("a", 2));
        assert!(slot.current().is_none());
    }

    #[test]
    fn reinstalling_a_provider_takes_the_slot_back() {
        let slot: Arc<Slot<Counter>> = Slot::new("test");
        let default = Arc::new(Counter(AtomicUsize::new(0)));
        let plugin = Arc::new(Counter(AtomicUsize::new(0)));

        slot.install("navi.compaction.default", 1, default.clone());
        slot.install("navi.plugin.compaction", 1, plugin.clone());
        assert_eq!(
            slot.current().map(|(component, _, _)| component).as_deref(),
            Some("navi.plugin.compaction")
        );

        // Reloading the built-in component (a new generation) takes the slot
        // back — the most recent activation is the active provider.
        slot.install("navi.compaction.default", 2, default.clone());
        assert_eq!(
            slot.current().map(|(component, generation, _)| (component, generation)),
            Some(("navi.compaction.default".to_string(), 2))
        );
        assert_eq!(
            slot.providers(),
            vec![
                ("navi.plugin.compaction".to_string(), 1),
                ("navi.compaction.default".to_string(), 2)
            ],
            "one entry per component: reloading does not grow the stack"
        );
    }

    #[test]
    fn most_recent_provider_wins_and_withdraw_restores_the_previous() {
        let slot: Arc<Slot<Counter>> = Slot::new("test");
        let default = Arc::new(Counter(AtomicUsize::new(0)));
        let alternative = Arc::new(Counter(AtomicUsize::new(0)));

        slot.install("navi.compaction.default", 1, default.clone());
        slot.install("navi.compaction.terse", 1, alternative.clone());
        assert_eq!(
            slot.current().map(|(component, _, _)| component).as_deref(),
            Some("navi.compaction.terse")
        );

        assert!(slot.clear_if_owner("navi.compaction.terse", 1));
        assert_eq!(
            slot.current().map(|(component, _, _)| component).as_deref(),
            Some("navi.compaction.default"),
            "withdrawing the active provider falls back to the previous one"
        );
    }
}
