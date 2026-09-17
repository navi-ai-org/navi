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
    /// Re-activating the same component replaces its entry in place, so a
    /// reload does not grow the stack.
    pub fn install(&self, component: &str, generation: u64, value: Arc<T>) {
        let mut entries = self.entries.write();
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.component == component)
        {
            existing.generation = generation;
            existing.value = value;
            return;
        }
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
