use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value as JsonValue, json};
use starlark::any::ProvidesStaticType;
use starlark::environment::{GlobalsBuilder, Module};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::Heap;
use starlark::values::Value as StarlarkValue;
use starlark::values::dict::AllocDict;
use starlark::values::none::NoneType;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::time::Instant;

use super::helpers;
use crate::security::SecurityPolicy;
use crate::tool::{Tool, ToolDefinition, ToolExecutor, ToolInvocation, ToolKind, ToolResult};

const MAX_SCRIPT_BYTES: usize = 32 * 1024;
const DEFAULT_MAX_TOOL_CALLS: usize = 200;
const MAX_TOOL_CALLS: usize = 1_000;
const DEFAULT_MAX_TICKS: u64 = 25_000;
const MAX_TICKS: u64 = 250_000;
const MAX_HEAP_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_MAX_RESULT_BYTES: usize = 128 * 1024;
const MAX_RESULT_BYTES: usize = 512 * 1024;
const DEFAULT_MAX_NESTED_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_NESTED_OUTPUT_BYTES: usize = 512 * 1024;

const WORKFLOW_PRELUDE: &str = r#"
def read_file(path, start_line = None, end_line = None):
    input = {"path": path}
    if start_line != None:
        input["start_line"] = start_line
    if end_line != None:
        input["end_line"] = end_line
    return tool("read_file", input)

def grep(pattern, path = ".", max_results = None):
    input = {"pattern": pattern, "path": path}
    if max_results != None:
        input["max_results"] = max_results
    return tool("grep", input)

def find(path = ".", pattern = None, hidden = False):
    input = {"action": "find", "path": path, "hidden": hidden}
    if pattern != None:
        input["pattern"] = pattern
    return tool("fs_browser", input)

def stat(path):
    return tool("fs_browser", {"action": "stat", "path": path})
"#;

pub(crate) struct ToolWorkflowTool {
    policy: SecurityPolicy,
}

impl ToolWorkflowTool {
    pub(crate) fn new(policy: SecurityPolicy) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl Tool for ToolWorkflowTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "tool_workflow",
            "Run a sandboxed Starlark workflow for batching read-only tool calls. Allowed nested tools: read_file, grep, fs_browser, git_ops read-only commands.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "Starlark script. Put loops inside def workflow(), then call workflow() as the final expression. Available helpers: tool(name, input), read_file(path), grep(pattern, path='.'), find(path='.', pattern=None), stat(path), emit(value), fail(message)."
                    },
                    "max_tool_calls": {
                        "type": "integer",
                        "description": "Maximum nested tool calls. Defaults to 200 and is capped at 1000."
                    },
                    "max_ticks": {
                        "type": "integer",
                        "description": "Maximum Starlark loop/function ticks. Defaults to 25000 and is capped at 250000."
                    },
                    "max_result_bytes": {
                        "type": "integer",
                        "description": "Maximum final result JSON bytes before truncating. Defaults to 128KiB and is capped at 512KiB."
                    },
                    "max_nested_output_bytes": {
                        "type": "integer",
                        "description": "Maximum nested tool output JSON bytes before truncating that nested result. Defaults to 128KiB and is capped at 512KiB."
                    }
                },
                "required": ["script"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let script = helpers::required_string(&invocation.input, "script")?.to_string();
        if script.len() > MAX_SCRIPT_BYTES {
            bail!("workflow script exceeds {MAX_SCRIPT_BYTES} bytes");
        }

        let limits = WorkflowLimits::from_input(&invocation.input);
        let nested_executor = ToolExecutor::new_workflow_host(self.policy.clone());
        let context = WorkflowContext::new(
            nested_executor,
            tokio::runtime::Handle::current(),
            limits.max_tool_calls,
            limits.max_nested_output_bytes,
        );

        let output = tokio::task::spawn_blocking(move || {
            run_starlark_workflow(&script, context, limits.max_ticks, limits.max_result_bytes)
        })
        .await
        .map_err(|e| anyhow!("workflow task join error: {e}"))??;

        Ok(helpers::ok(
            invocation.id,
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "result": output.result,
                "result_truncated": output.result_truncated,
                "tool_calls": output.tool_calls,
                "ticks": output.ticks,
                "emits": output.emits,
                "trace": output.trace,
            }),
        ))
    }
}

#[derive(Clone, Copy)]
struct WorkflowLimits {
    max_tool_calls: usize,
    max_ticks: u64,
    max_result_bytes: usize,
    max_nested_output_bytes: usize,
}

impl WorkflowLimits {
    fn from_input(input: &JsonValue) -> Self {
        Self {
            max_tool_calls: helpers::optional_u64(input, "max_tool_calls")
                .unwrap_or(DEFAULT_MAX_TOOL_CALLS as u64)
                .min(MAX_TOOL_CALLS as u64) as usize,
            max_ticks: helpers::optional_u64(input, "max_ticks")
                .unwrap_or(DEFAULT_MAX_TICKS)
                .min(MAX_TICKS),
            max_result_bytes: helpers::optional_u64(input, "max_result_bytes")
                .unwrap_or(DEFAULT_MAX_RESULT_BYTES as u64)
                .min(MAX_RESULT_BYTES as u64) as usize,
            max_nested_output_bytes: helpers::optional_u64(input, "max_nested_output_bytes")
                .unwrap_or(DEFAULT_MAX_NESTED_OUTPUT_BYTES as u64)
                .min(MAX_NESTED_OUTPUT_BYTES as u64) as usize,
        }
    }
}

#[derive(ProvidesStaticType)]
struct WorkflowContext {
    executor: ToolExecutor,
    runtime: tokio::runtime::Handle,
    tool_calls: Cell<usize>,
    max_tool_calls: usize,
    max_nested_output_bytes: usize,
    trace: RefCell<Vec<JsonValue>>,
    emits: RefCell<Vec<JsonValue>>,
    allowed_tools: BTreeSet<&'static str>,
}

impl WorkflowContext {
    fn new(
        executor: ToolExecutor,
        runtime: tokio::runtime::Handle,
        max_tool_calls: usize,
        max_nested_output_bytes: usize,
    ) -> Self {
        Self {
            executor,
            runtime,
            tool_calls: Cell::new(0),
            max_tool_calls,
            max_nested_output_bytes,
            trace: RefCell::new(Vec::new()),
            emits: RefCell::new(Vec::new()),
            allowed_tools: BTreeSet::from(["read_file", "grep", "fs_browser", "git_ops"]),
        }
    }

    fn call_tool(&self, name: &str, input: JsonValue) -> Result<JsonValue> {
        if !self.allowed_tools.contains(name) {
            bail!(
                "tool_workflow only allows read_file, grep, fs_browser, and read-only git_ops; got `{name}`"
            );
        }
        if name == "git_ops" {
            let command = input.get("command").and_then(JsonValue::as_str);
            if !matches!(command, Some("status" | "diff" | "log" | "branch")) {
                bail!("tool_workflow only allows read-only git_ops commands");
            }
        }

        let next = self.tool_calls.get() + 1;
        if next > self.max_tool_calls {
            bail!("workflow exceeded max_tool_calls ({})", self.max_tool_calls);
        }
        self.tool_calls.set(next);

        let invocation = ToolInvocation {
            id: format!("workflow-{next}"),
            tool_name: name.to_string(),
            input,
        };
        let started = Instant::now();
        let result = self
            .runtime
            .block_on(self.executor.invoke(invocation.clone()));
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let output_bytes = result.output.to_string().len();
        let (output, output_truncated) =
            truncate_json_value(result.output, self.max_nested_output_bytes);

        self.trace.borrow_mut().push(json!({
            "id": invocation.id,
            "tool": invocation.tool_name,
            "ok": result.ok,
            "elapsed_ms": elapsed_ms,
            "output_bytes": output_bytes,
            "output_truncated": output_truncated,
        }));

        if !result.ok {
            bail!("nested tool `{}` failed: {}", name, output);
        }

        Ok(output)
    }

    fn emit(&self, value: JsonValue) {
        let (value, _) = truncate_json_value(value, self.max_nested_output_bytes);
        self.emits.borrow_mut().push(value);
    }
}

struct WorkflowOutput {
    result: JsonValue,
    result_truncated: bool,
    tool_calls: usize,
    ticks: u64,
    emits: Vec<JsonValue>,
    trace: Vec<JsonValue>,
}

fn run_starlark_workflow(
    script: &str,
    context: WorkflowContext,
    max_ticks: u64,
    max_result_bytes: usize,
) -> Result<WorkflowOutput> {
    let source = format!("{WORKFLOW_PRELUDE}\n{script}");
    let ast = AstModule::parse("tool_workflow.star", source, &Dialect::Standard)
        .map_err(|e| anyhow!(e.to_string()))?;
    let globals = GlobalsBuilder::standard().with(workflow_globals).build();

    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        eval.extra = Some(&context);
        eval.set_max_tick_count(max_ticks)?;
        eval.set_max_heap_size(MAX_HEAP_BYTES)?;
        eval.set_max_callstack_size(64)?;

        let value = eval
            .eval_module(ast, &globals)
            .map_err(|e| anyhow!(e.to_string()))?;
        let ticks = eval.get_total_tick_count();
        let json_text = value.to_json()?;
        let result = serde_json::from_str(&json_text)
            .with_context(|| format!("workflow result is not JSON-serializable: {json_text}"))?;
        let (result, result_truncated) = truncate_json_value(result, max_result_bytes);
        Ok(WorkflowOutput {
            result,
            result_truncated,
            tool_calls: context.tool_calls.get(),
            ticks,
            emits: context.emits.borrow().clone(),
            trace: context.trace.borrow().clone(),
        })
    })
}

#[starlark_module]
fn workflow_globals(builder: &mut GlobalsBuilder) {
    fn tool<'v>(
        name: &str,
        input: StarlarkValue<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<StarlarkValue<'v>> {
        let context = workflow_context(eval)?;
        let input_json = serde_json::from_str(&input.to_json()?)?;
        let output = context.call_tool(name, input_json)?;
        Ok(alloc_json(eval.heap(), output))
    }

    fn emit(value: StarlarkValue, eval: &mut Evaluator) -> anyhow::Result<NoneType> {
        let context = workflow_context(eval)?;
        let value_json = serde_json::from_str(&value.to_json()?)?;
        context.emit(value_json);
        Ok(NoneType)
    }

    fn fail(message: &str) -> anyhow::Result<NoneType> {
        bail!(message.to_string())
    }
}

fn workflow_context<'a, 'v>(eval: &'a Evaluator<'v, '_, '_>) -> Result<&'a WorkflowContext> {
    eval.extra
        .ok_or_else(|| anyhow!("missing workflow context"))?
        .downcast_ref::<WorkflowContext>()
        .ok_or_else(|| anyhow!("invalid workflow context"))
}

fn alloc_json<'v>(heap: Heap<'v>, value: JsonValue) -> StarlarkValue<'v> {
    match value {
        JsonValue::Null => heap.alloc(NoneType),
        JsonValue::Bool(value) => heap.alloc(value),
        JsonValue::Number(value) => {
            if let Some(value) = value.as_i64() {
                heap.alloc(value)
            } else if let Some(value) = value.as_u64() {
                heap.alloc(value)
            } else if let Some(value) = value.as_f64() {
                heap.alloc(value)
            } else {
                heap.alloc(value.to_string())
            }
        }
        JsonValue::String(value) => heap.alloc(value),
        JsonValue::Array(values) => {
            let values = values
                .into_iter()
                .map(|value| alloc_json(heap, value))
                .collect::<Vec<_>>();
            heap.alloc(values)
        }
        JsonValue::Object(values) => {
            let values = values
                .into_iter()
                .map(|(key, value)| (key, alloc_json(heap, value)))
                .collect::<Vec<_>>();
            heap.alloc(AllocDict(values))
        }
    }
}

fn truncate_json_value(value: JsonValue, max_bytes: usize) -> (JsonValue, bool) {
    let serialized = value.to_string();
    if serialized.len() <= max_bytes {
        return (value, false);
    }
    let mut content = serialized;
    content.truncate(max_bytes);
    while !content.is_char_boundary(content.len()) {
        content.pop();
    }
    content.push_str("\n<truncated>");
    (json!({ "truncated": true, "content": content }), true)
}
