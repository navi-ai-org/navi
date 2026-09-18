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
    PromptCache, RenderedPrompt, SecurityDecision, SecurityPolicy, SessionHooks,
    SystemPromptInput, ToolDefinition, ToolInvocation, ToolResult,
    config::HarnessConfig, harness::ToolLoopDecision, harness::{AgentRunState, HarnessPolicy},
    runtime_components::HarnessDriver, runtime_components::ToolSecurityPolicy,
    security::SecurityRisk,
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

/// Tool security policy slot (`navi.security@1`).
pub type SecuritySlot = Slot<dyn ToolSecurityPolicy>;

/// Harness driver slot (`navi.harness@1`).
pub type HarnessSlot = Slot<dyn HarnessDriver>;

/// Session hooks slot (`navi.hooks@1`).
pub type HooksSlot = Slot<dyn SessionHooks>;

/// The typed slots a compose host exposes to the engine.
#[derive(Clone)]
pub struct Slots {
    /// `navi.compaction@1`
    pub compaction: Arc<CompactionSlot>,
    /// `navi.prompt@1`
    pub prompt: Arc<PromptSlot>,
    /// `navi.security@1`
    pub security: Arc<SecuritySlot>,
    /// `navi.harness@1`
    pub harness: Arc<HarnessSlot>,
    /// `navi.hooks@1`
    pub hooks: Arc<HooksSlot>,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            compaction: CompactionSlot::new("navi.compaction@1"),
            prompt: PromptSlot::new("navi.prompt@1"),
            security: SecuritySlot::new("navi.security@1"),
            harness: HarnessSlot::new("navi.harness@1"),
            hooks: HooksSlot::new("navi.hooks@1"),
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

/// One capability call to a component in another process, with the instance
/// grant from activation (a stale generation is never reached by name).
fn call_component(
    component: &str,
    capability: &str,
    reference: &matrix_core::InstanceRef,
    kernel: &Arc<matrix_core::Kernel>,
    input: &serde_json::Value,
) -> Option<serde_json::Value> {
    let (value, ok) = kernel.invoke_for_ref(reference, capability, input);
    if ok {
        return Some(value);
    }
    tracing::warn!(
        component = %component,
        capability = %capability,
        response = %value,
        "process component did not answer; the engine keeps its default behaviour"
    );
    None
}

/// Tool security policy served by a **component in another process**.
///
/// Contract (`navi.security@1`):
///
/// ```json
/// // host → plugin
/// {"op":"validate_tool","tool":{"name":"web_fetch",…},"input":{"url":"…"}}
/// // plugin → host
/// {"decision":"allow"} | {"decision":"deny","reason":"…"}
///                      | {"decision":"approval","risk":"tool"}
/// ```
///
/// A missing or unreadable answer falls back to the engine's default policy:
/// a broken security plugin never silently allows everything.
pub struct ProcessSecurity {
    component: String,
    capability: String,
    reference: matrix_core::InstanceRef,
    kernel: Arc<matrix_core::Kernel>,
    fallback: Arc<dyn ToolSecurityPolicy>,
}

impl ProcessSecurity {
    /// Binds the slot value to a running component.
    pub fn new(
        component: impl Into<String>,
        capability: impl Into<String>,
        reference: matrix_core::InstanceRef,
        kernel: Arc<matrix_core::Kernel>,
        fallback: Arc<dyn ToolSecurityPolicy>,
    ) -> Self {
        Self {
            component: component.into(),
            capability: capability.into(),
            reference,
            kernel,
            fallback,
        }
    }
}

impl ToolSecurityPolicy for ProcessSecurity {
    fn validate_tool(
        &self,
        base_policy: &SecurityPolicy,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> SecurityDecision {
        let input = serde_json::json!({
            "op": "validate_tool",
            "tool": definition,
            "input": invocation.input,
        });
        let Some(answer) = call_component(
            &self.component,
            &self.capability,
            &self.reference,
            &self.kernel,
            &input,
        ) else {
            return self.fallback.validate_tool(base_policy, definition, invocation);
        };
        match answer.get("decision").and_then(|value| value.as_str()) {
            Some("allow") => SecurityDecision::Allow,
            Some("deny") => SecurityDecision::Deny(
                answer
                    .get("reason")
                    .and_then(|value| value.as_str())
                    .unwrap_or("denied by a security component")
                    .to_string(),
            ),
            Some("approval") => SecurityDecision::NeedsApproval(risk_from(&answer)),
            // No opinion: the base policy decides. A security plugin adds
            // restrictions without having to re-implement the host's rules.
            Some("abstain") => self.fallback.validate_tool(base_policy, definition, invocation),
            other => {
                tracing::warn!(
                    component = %self.component,
                    decision = ?other,
                    "unreadable security decision; falling back"
                );
                self.fallback.validate_tool(base_policy, definition, invocation)
            }
        }
    }
}

/// Maps the plugin's risk label onto the engine's enum.
fn risk_from(answer: &serde_json::Value) -> SecurityRisk {
    match answer.get("risk").and_then(|value| value.as_str()) {
        Some("write") => SecurityRisk::Write,
        Some("command") => SecurityRisk::Command,
        Some("guarded_command") => SecurityRisk::GuardedCommand,
        Some("external_plugin") => SecurityRisk::ExternalPlugin,
        Some("ui_automation") => SecurityRisk::UiAutomation,
        _ => SecurityRisk::Tool,
    }
}

/// Harness driver served by a **component in another process** for the part
/// that shapes the context.
///
/// Contract (`navi.harness@1`):
///
/// ```json
/// // host → plugin
/// {"op":"compact_observation","tool":"read_file","result":{…},"ok":true}
/// // plugin → host
/// {"observation":"…"}
/// ```
///
/// The stateful loop methods (`filter_tools`, `record_*`) stay with the
/// fallback driver: a process component owns the observation policy it declares.
pub struct ProcessHarness {
    component: String,
    capability: String,
    reference: matrix_core::InstanceRef,
    kernel: Arc<matrix_core::Kernel>,
    fallback: Arc<dyn HarnessDriver>,
}

impl ProcessHarness {
    /// Binds the slot value to a running component.
    pub fn new(
        component: impl Into<String>,
        capability: impl Into<String>,
        reference: matrix_core::InstanceRef,
        kernel: Arc<matrix_core::Kernel>,
        fallback: Arc<dyn HarnessDriver>,
    ) -> Self {
        Self {
            component: component.into(),
            capability: capability.into(),
            reference,
            kernel,
            fallback,
        }
    }
}

impl HarnessDriver for ProcessHarness {
    fn filter_tools(
        &self,
        tools: Vec<ToolDefinition>,
        allowed_tool_names: Option<&[String]>,
    ) -> Vec<ToolDefinition> {
        self.fallback.filter_tools(tools, allowed_tool_names)
    }

    fn record_tool_call(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
    ) -> ToolLoopDecision {
        self.fallback.record_tool_call(state, policy, invocation)
    }

    fn record_tool_result(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
        result: &ToolResult,
    ) -> ToolLoopDecision {
        self.fallback
            .record_tool_result(state, policy, invocation, result)
    }

    fn compact_tool_observation(
        &self,
        invocation: &ToolInvocation,
        result: &ToolResult,
        policy: HarnessPolicy,
    ) -> String {
        let input = serde_json::json!({
            "op": "compact_observation",
            "tool": invocation.tool_name,
            "ok": result.ok,
            "result": result.output,
        });
        let Some(answer) = call_component(
            &self.component,
            &self.capability,
            &self.reference,
            &self.kernel,
            &input,
        ) else {
            return self
                .fallback
                .compact_tool_observation(invocation, result, policy);
        };
        match answer.get("observation").and_then(|value| value.as_str()) {
            Some(observation) => observation.to_string(),
            None => self
                .fallback
                .compact_tool_observation(invocation, result, policy),
        }
    }
}

/// Session hooks served by a **component in another process** (fire and forget).
///
/// Contract (`navi.hooks@1`): `{"op":"event","event":"session_start",
/// "session_id":"…"}` (plus `tool`, `output_chars` where they apply). The engine
/// never blocks on the answer; a dead component only costs its own logging.
pub struct ProcessHooks {
    component: String,
    capability: String,
    reference: matrix_core::InstanceRef,
    kernel: Arc<matrix_core::Kernel>,
}

impl ProcessHooks {
    /// Binds the slot value to a running component.
    pub fn new(
        component: impl Into<String>,
        capability: impl Into<String>,
        reference: matrix_core::InstanceRef,
        kernel: Arc<matrix_core::Kernel>,
    ) -> Self {
        Self {
            component: component.into(),
            capability: capability.into(),
            reference,
            kernel,
        }
    }

    fn event(&self, event: &str, extra: serde_json::Value) {
        let mut input = serde_json::json!({"op": "event", "event": event});
        if let (Some(map), Some(extra)) = (input.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                map.insert(key.clone(), value.clone());
            }
        }
        let _ = call_component(
            &self.component,
            &self.capability,
            &self.reference,
            &self.kernel,
            &input,
        );
    }
}

impl SessionHooks for ProcessHooks {
    fn on_session_start(&self, session_id: &str) {
        self.event("session_start", serde_json::json!({"session_id": session_id}));
    }

    fn on_turn_start(&self, session_id: &str, task: &str) {
        self.event(
            "turn_start",
            serde_json::json!({"session_id": session_id, "task": task}),
        );
    }

    fn on_tool_call(&self, invocation: &ToolInvocation) {
        self.event(
            "tool_call",
            serde_json::json!({"tool": invocation.tool_name}),
        );
    }

    fn on_tool_result(&self, result: &ToolResult) {
        self.event("tool_result", serde_json::json!({"ok": result.ok}));
    }

    fn on_turn_end(&self, session_id: &str, output: &str) {
        self.event(
            "turn_end",
            serde_json::json!({
                "session_id": session_id,
                "output_chars": output.chars().count(),
            }),
        );
    }

    fn on_session_end(&self, session_id: &str) {
        self.event("session_end", serde_json::json!({"session_id": session_id}));
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

/// `ToolSecurityPolicy` proxy: resolves the active provider per call.
pub struct ComposeSecurity {
    slots: Slots,
    fallback: Arc<dyn ToolSecurityPolicy>,
}

impl ComposeSecurity {
    /// Builds a proxy over `slots`, falling back when no component provides a
    /// policy.
    pub fn new(slots: Slots, fallback: Arc<dyn ToolSecurityPolicy>) -> Self {
        Self { slots, fallback }
    }

    fn active(&self) -> Arc<dyn ToolSecurityPolicy> {
        self.slots
            .security
            .current()
            .map(|(_, _, value)| value)
            .unwrap_or_else(|| self.fallback.clone())
    }
}

impl ToolSecurityPolicy for ComposeSecurity {
    fn validate_tool(
        &self,
        base_policy: &SecurityPolicy,
        definition: &ToolDefinition,
        invocation: &ToolInvocation,
    ) -> SecurityDecision {
        self.active()
            .validate_tool(base_policy, definition, invocation)
    }
}

/// `HarnessDriver` proxy: resolves the active provider per call.
pub struct ComposeHarness {
    slots: Slots,
    fallback: Arc<dyn HarnessDriver>,
}

impl ComposeHarness {
    /// Builds a proxy over `slots`, falling back when no component provides a
    /// driver.
    pub fn new(slots: Slots, fallback: Arc<dyn HarnessDriver>) -> Self {
        Self { slots, fallback }
    }

    fn active(&self) -> Arc<dyn HarnessDriver> {
        self.slots
            .harness
            .current()
            .map(|(_, _, value)| value)
            .unwrap_or_else(|| self.fallback.clone())
    }
}

impl HarnessDriver for ComposeHarness {
    fn filter_tools(
        &self,
        tools: Vec<ToolDefinition>,
        allowed_tool_names: Option<&[String]>,
    ) -> Vec<ToolDefinition> {
        self.active().filter_tools(tools, allowed_tool_names)
    }

    fn record_tool_call(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
    ) -> ToolLoopDecision {
        self.active().record_tool_call(state, policy, invocation)
    }

    fn record_tool_result(
        &self,
        state: &mut AgentRunState,
        policy: HarnessPolicy,
        invocation: &ToolInvocation,
        result: &ToolResult,
    ) -> ToolLoopDecision {
        self.active()
            .record_tool_result(state, policy, invocation, result)
    }

    fn compact_tool_observation(
        &self,
        invocation: &ToolInvocation,
        result: &ToolResult,
        policy: HarnessPolicy,
    ) -> String {
        self.active()
            .compact_tool_observation(invocation, result, policy)
    }
}

/// `PromptBuilder` served by a **component in another process**.
///
/// `SystemPromptInput` carries live objects (the prompt cache, the whole
/// config), so the host sends a **projection** of it — the data a prompt builder
/// needs — and the plugin answers with the base instructions plus optional
/// developer blocks:
///
/// ```json
/// // host → plugin
/// {"op":"build","project_dir":"/p","tools":["read_file"],"memory":null,
///  "active_skills":[…],"context_packets":[…]}
/// // plugin → host
/// {"instructions":"…","developer_messages":[{"role":"developer","content":"…"}]}
/// ```
///
/// An empty or unreadable answer falls back to the engine's builder.
pub struct ProcessPrompt {
    component: String,
    capability: String,
    reference: matrix_core::InstanceRef,
    kernel: Arc<matrix_core::Kernel>,
    fallback: Arc<dyn PromptBuilder>,
}

impl ProcessPrompt {
    /// Binds the slot value to a running component.
    pub fn new(
        component: impl Into<String>,
        capability: impl Into<String>,
        reference: matrix_core::InstanceRef,
        kernel: Arc<matrix_core::Kernel>,
        fallback: Arc<dyn PromptBuilder>,
    ) -> Self {
        Self {
            component: component.into(),
            capability: capability.into(),
            reference,
            kernel,
            fallback,
        }
    }
}

impl PromptBuilder for ProcessPrompt {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> RenderedPrompt {
        let payload = serde_json::json!({
            "op": "build",
            "project_dir": input.project_dir.display().to_string(),
            "memory": input.memory_injection.clone(),
            "harness_card": input.harness_card.clone(),
            "system_prompt": input.system_prompt.clone(),
            "tools": input
                .tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>(),
            "context_packets": input.context_packets.clone(),
            "available_skills": input.available_skills.clone(),
            "active_skills": input.active_skills.clone(),
        });
        let Some(answer) = call_component(
            &self.component,
            &self.capability,
            &self.reference,
            &self.kernel,
            &payload,
        ) else {
            return self.fallback.build(input, cache);
        };
        let instructions = answer
            .get("instructions")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        if instructions.trim().is_empty() {
            tracing::warn!(
                component = %self.component,
                "prompt component returned no instructions; falling back"
            );
            return self.fallback.build(input, cache);
        }
        let developer_messages = answer
            .get("developer_messages")
            .and_then(|value| serde_json::from_value::<Vec<ModelMessage>>(value.clone()).ok())
            .unwrap_or_default();
        RenderedPrompt {
            instructions,
            developer_messages,
        }
    }
}

/// `SessionHooks` proxy: forwards every callback to the active provider.
pub struct ComposeHooks {
    slots: Slots,
    fallback: Arc<dyn SessionHooks>,
}

impl ComposeHooks {
    /// Builds a proxy over `slots`, falling back when no component provides
    /// hooks.
    pub fn new(slots: Slots, fallback: Arc<dyn SessionHooks>) -> Self {
        Self { slots, fallback }
    }

    fn active(&self) -> Arc<dyn SessionHooks> {
        self.slots
            .hooks
            .current()
            .map(|(_, _, value)| value)
            .unwrap_or_else(|| self.fallback.clone())
    }
}

impl SessionHooks for ComposeHooks {
    fn on_session_start(&self, session_id: &str) {
        self.active().on_session_start(session_id)
    }

    fn on_turn_start(&self, session_id: &str, task: &str) {
        self.active().on_turn_start(session_id, task)
    }

    fn on_tool_call(&self, invocation: &ToolInvocation) {
        self.active().on_tool_call(invocation)
    }

    fn on_tool_result(&self, result: &ToolResult) {
        self.active().on_tool_result(result)
    }

    fn on_turn_end(&self, session_id: &str, output: &str) {
        self.active().on_turn_end(session_id, output)
    }

    fn on_session_end(&self, session_id: &str) {
        self.active().on_session_end(session_id)
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
