//! Sandboxed Lua 5.4 VM + host builtins.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{Lua, LuaOptions, StdLib, Table, Value as LuaValue};
use serde_json::{Value as JsonValue, json};
use tokio::sync::mpsc::UnboundedSender;

use super::policy::{AgentPolicyOpts, RunPolicy, intersect_agent_policy};
use super::types::{AGENT_RESULT_MAX_BYTES, AgentBackendResult, AgentRequest, WorkflowErrorCode};
use super::{AgentJob, WorkflowHostError};
use crate::cancel::CancelToken;

const INSTRUCTION_LIMIT: u32 = 5_000_000;

pub struct LuaRunInput {
    pub script: String,
    pub args: JsonValue,
    pub run_policy: RunPolicy,
    pub max_agents: usize,
    pub max_parallel: usize,
    pub job_tx: UnboundedSender<AgentJob>,
    pub cancel_token: CancelToken,
}

pub struct LuaRunOutcome {
    pub result: JsonValue,
    pub phases: Vec<String>,
    pub logs: Vec<String>,
    pub agents_started: usize,
    pub max_parallel_used: usize,
    pub error: Option<WorkflowHostError>,
}

struct HostState {
    job_tx: UnboundedSender<AgentJob>,
    cancel_token: CancelToken,
    run_policy: RunPolicy,
    max_agents: usize,
    agent_counter: AtomicU64,
    phases: Mutex<Vec<String>>,
    logs: Mutex<Vec<String>>,
    #[allow(dead_code)]
    max_parallel: usize,
    /// When > 0, `agent()` enqueues without waiting so `parallel` can fan out.
    defer_depth: AtomicU64,
    /// Deferred agent receivers (filled while defer_depth > 0).
    deferred: Mutex<Vec<DeferredAgent>>,
}

struct DeferredAgent {
    id: u64,
    response: std::sync::mpsc::Receiver<AgentBackendResult>,
}

pub fn run_lua_workflow(input: LuaRunInput) -> Result<LuaRunOutcome, WorkflowHostError> {
    let host = Arc::new(HostState {
        job_tx: input.job_tx,
        cancel_token: input.cancel_token,
        run_policy: input.run_policy,
        max_agents: input.max_agents,
        agent_counter: AtomicU64::new(0),
        phases: Mutex::new(Vec::new()),
        logs: Mutex::new(Vec::new()),
        max_parallel: input.max_parallel,
        defer_depth: AtomicU64::new(0),
        deferred: Mutex::new(Vec::new()),
    });

    let lua = new_sandbox()?;
    install_instruction_hook(&lua)?;
    strip_dangerous_globals(&lua)?;
    inject_args(&lua, &input.args)?;
    inject_budget(&lua)?;
    inject_host_builtins(&lua, host.clone())?;

    if let Err(err) = lua.load(&input.script).set_name("workflow").exec() {
        let msg = err.to_string();
        let code = classify_load_error(&msg);
        return Ok(outcome_err(
            &host,
            code,
            msg,
            Some("Fix Lua syntax or remove forbidden APIs."),
        ));
    }

    let result_lua = match call_entrypoint(&lua, &host) {
        Ok(v) => v,
        Err(e) => {
            return Ok(LuaRunOutcome {
                result: JsonValue::Null,
                phases: take_phases(&host),
                logs: take_logs(&host),
                agents_started: host.agent_counter.load(Ordering::SeqCst) as usize,
                max_parallel_used: 0,
                error: Some(e),
            });
        }
    };

    let result = lua_to_json(&lua, result_lua).unwrap_or(JsonValue::Null);
    Ok(LuaRunOutcome {
        result,
        phases: take_phases(&host),
        logs: take_logs(&host),
        agents_started: host.agent_counter.load(Ordering::SeqCst) as usize,
        max_parallel_used: 0,
        error: None,
    })
}

fn outcome_err(
    host: &HostState,
    code: WorkflowErrorCode,
    message: String,
    hint: Option<&str>,
) -> LuaRunOutcome {
    LuaRunOutcome {
        result: JsonValue::Null,
        phases: take_phases(host),
        logs: take_logs(host),
        agents_started: host.agent_counter.load(Ordering::SeqCst) as usize,
        max_parallel_used: 0,
        error: Some(WorkflowHostError {
            code,
            message,
            hint: hint.map(|s| s.to_string()),
        }),
    }
}

fn take_phases(host: &HostState) -> Vec<String> {
    host.phases.lock().map(|p| p.clone()).unwrap_or_default()
}

fn take_logs(host: &HostState) -> Vec<String> {
    host.logs.lock().map(|l| l.clone()).unwrap_or_default()
}

fn new_sandbox() -> Result<Lua, WorkflowHostError> {
    let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE;
    Lua::new_with(libs, LuaOptions::default()).map_err(map_lua_err)
}

fn install_instruction_hook(lua: &Lua) -> Result<(), WorkflowHostError> {
    let _ = lua.set_memory_limit(32 * 1024 * 1024);
    lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(100_000),
        move |_lua, _dbg| {
            thread_local! {
                static INSTR: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
            }
            INSTR.with(|c| {
                let n = c.get().saturating_add(100_000);
                c.set(n);
                if n > INSTRUCTION_LIMIT {
                    Err(mlua::Error::RuntimeError(
                        "instruction limit exceeded (possible infinite loop)".into(),
                    ))
                } else {
                    Ok(mlua::VmState::Continue)
                }
            })
        },
    );
    Ok(())
}

fn strip_dangerous_globals(lua: &Lua) -> Result<(), WorkflowHostError> {
    let globals = lua.globals();
    for name in [
        "require",
        "dofile",
        "loadfile",
        "load",
        "loadstring",
        "io",
        "os",
        "debug",
        "package",
        "collectgarbage",
    ] {
        let _ = globals.set(name, LuaValue::Nil);
    }
    if let Ok(math) = globals.get::<Table>("math") {
        let _ = math.set("random", LuaValue::Nil);
        let _ = math.set("randomseed", LuaValue::Nil);
    }
    Ok(())
}

fn inject_args(lua: &Lua, args: &JsonValue) -> Result<(), WorkflowHostError> {
    let data = match json_to_lua(lua, args).map_err(map_lua_err)? {
        LuaValue::Table(t) => t,
        other => {
            let t = lua.create_table().map_err(map_lua_err)?;
            t.set("value", other).map_err(map_lua_err)?;
            t
        }
    };
    let proxy = lua.create_table().map_err(map_lua_err)?;
    let mt = lua.create_table().map_err(map_lua_err)?;
    let data_c = data.clone();
    mt.set(
        "__index",
        lua.create_function(move |_, (_t, key): (Table, LuaValue)| data_c.get::<LuaValue>(key))
            .map_err(map_lua_err)?,
    )
    .map_err(map_lua_err)?;
    mt.set(
        "__newindex",
        lua.create_function(|_, (_t, _k, _v): (Table, LuaValue, LuaValue)| {
            Err::<(), _>(mlua::Error::RuntimeError("args is read-only".into()))
        })
        .map_err(map_lua_err)?,
    )
    .map_err(map_lua_err)?;
    mt.set("__metatable", "args is read-only")
        .map_err(map_lua_err)?;
    proxy.set_metatable(Some(mt));
    lua.globals().set("args", proxy).map_err(map_lua_err)?;
    Ok(())
}

fn inject_budget(lua: &Lua) -> Result<(), WorkflowHostError> {
    let budget = lua.create_table().map_err(map_lua_err)?;
    budget.set("total", i64::MAX).map_err(map_lua_err)?;
    budget.set("spent", 0i64).map_err(map_lua_err)?;
    budget.set("remaining", i64::MAX).map_err(map_lua_err)?;
    let mt = lua.create_table().map_err(map_lua_err)?;
    mt.set(
        "__newindex",
        lua.create_function(|_, (_t, _k, _v): (Table, LuaValue, LuaValue)| {
            Err::<(), _>(mlua::Error::RuntimeError("budget is read-only".into()))
        })
        .map_err(map_lua_err)?,
    )
    .map_err(map_lua_err)?;
    budget.set_metatable(Some(mt));
    lua.globals().set("budget", budget).map_err(map_lua_err)?;
    Ok(())
}

fn inject_host_builtins(lua: &Lua, host: Arc<HostState>) -> Result<(), WorkflowHostError> {
    {
        let host = host.clone();
        let f = lua
            .create_function(move |_, title: String| {
                if let Ok(mut p) = host.phases.lock() {
                    p.push(title);
                }
                Ok(())
            })
            .map_err(map_lua_err)?;
        lua.globals().set("phase", f).map_err(map_lua_err)?;
    }
    {
        let host = host.clone();
        let f = lua
            .create_function(move |_, message: String| {
                if let Ok(mut l) = host.logs.lock() {
                    l.push(message);
                }
                Ok(())
            })
            .map_err(map_lua_err)?;
        lua.globals().set("log", f).map_err(map_lua_err)?;
    }
    {
        let host = host.clone();
        let f = lua
            .create_function(move |lua, (prompt, opts): (String, Option<Table>)| {
                call_agent(lua, &host, prompt, opts)
            })
            .map_err(map_lua_err)?;
        lua.globals().set("agent", f).map_err(map_lua_err)?;
    }
    {
        let host = host.clone();
        let f = lua
            .create_function(move |lua, (thunks, maybe_fn): (LuaValue, Option<LuaValue>)| {
                if maybe_fn.is_some() {
                    return Err(mlua::Error::RuntimeError(
                        "invalid_host_call: parallel accepts only an array of zero-arg functions; use pipeline(items, fn) for map".into(),
                    ));
                }
                let table = match thunks {
                    LuaValue::Table(t) => t,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "invalid_host_call: parallel expects a table of functions".into(),
                        ));
                    }
                };
                let mut fns = Vec::new();
                for pair in table.sequence_values::<LuaValue>() {
                    let v = pair.map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
                    match v {
                        LuaValue::Function(f) => fns.push(f),
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "invalid_host_call: parallel elements must be zero-arg functions"
                                    .into(),
                            ));
                        }
                    }
                }

                // Defer agent waits so all thunks can enqueue under the host
                // semaphore (true concurrency without multi-state dump).
                host.defer_depth.fetch_add(1, Ordering::SeqCst);
                let results = lua.create_table()?;
                let mut run_err: Option<mlua::Error> = None;
                for (i, func) in fns.into_iter().enumerate() {
                    if host.cancel_token.is_requested() {
                        run_err = Some(mlua::Error::RuntimeError("cancelled".into()));
                        break;
                    }
                    match func.call::<LuaValue>(()) {
                        Ok(ret) => {
                            if let Err(e) = results.set(i + 1, ret) {
                                run_err = Some(e);
                                break;
                            }
                        }
                        Err(e) => {
                            run_err = Some(e);
                            break;
                        }
                    }
                }
                host.defer_depth.fetch_sub(1, Ordering::SeqCst);
                if let Some(e) = run_err {
                    // Drain deferred receivers so workers do not hang.
                    if let Ok(mut d) = host.deferred.lock() {
                        d.clear();
                    }
                    return Err(e);
                }
                let resolved = resolve_deferred(lua, &host, LuaValue::Table(results))?;
                Ok(resolved)
            })
            .map_err(map_lua_err)?;
        lua.globals().set("parallel", f).map_err(map_lua_err)?;
    }
    {
        let host = host.clone();
        let f = lua
            .create_function(move |lua, (items, map_fn): (Table, mlua::Function)| {
                let mut collected = Vec::new();
                for pair in items.sequence_values::<LuaValue>() {
                    collected.push(pair.map_err(|e| mlua::Error::RuntimeError(e.to_string()))?);
                }
                // Defer agent waits for concurrent pipeline fan-out.
                host.defer_depth.fetch_add(1, Ordering::SeqCst);
                let results = lua.create_table()?;
                let mut run_err: Option<mlua::Error> = None;
                for (i, item) in collected.into_iter().enumerate() {
                    if host.cancel_token.is_requested() {
                        run_err = Some(mlua::Error::RuntimeError("cancelled".into()));
                        break;
                    }
                    match map_fn.call::<LuaValue>(item) {
                        Ok(ret) => {
                            if let Err(e) = results.set(i + 1, ret) {
                                run_err = Some(e);
                                break;
                            }
                        }
                        Err(e) => {
                            run_err = Some(e);
                            break;
                        }
                    }
                }
                host.defer_depth.fetch_sub(1, Ordering::SeqCst);
                if let Some(e) = run_err {
                    if let Ok(mut d) = host.deferred.lock() {
                        d.clear();
                    }
                    return Err(e);
                }
                let resolved = resolve_deferred(lua, &host, LuaValue::Table(results))?;
                match resolved {
                    LuaValue::Table(t) => Ok(t),
                    other => {
                        let t = lua.create_table()?;
                        t.set(1, other)?;
                        Ok(t)
                    }
                }
            })
            .map_err(map_lua_err)?;
        lua.globals().set("pipeline", f).map_err(map_lua_err)?;
    }
    Ok(())
}

fn call_agent(
    lua: &Lua,
    host: &HostState,
    prompt: String,
    opts: Option<Table>,
) -> mlua::Result<LuaValue> {
    if host.cancel_token.is_requested() {
        return Err(mlua::Error::RuntimeError("cancelled".into()));
    }
    let idx = host.agent_counter.fetch_add(1, Ordering::SeqCst) + 1;
    if idx as usize > host.max_agents {
        return Err(mlua::Error::RuntimeError(format!(
            "agent_cap_exceeded: max_agents={}",
            host.max_agents
        )));
    }
    let opts = parse_agent_opts(opts);
    let effective = intersect_agent_policy(&host.run_policy, &opts);
    let (tx, rx) = std::sync::mpsc::channel::<AgentBackendResult>();
    let request = AgentRequest {
        agent_index: idx,
        prompt,
        label: opts.label.clone(),
        model: opts.model.clone(),
        max_tokens: opts.max_tokens,
        effective,
        cancel_token: host.cancel_token.clone(),
    };
    host.job_tx
        .send(AgentJob {
            request,
            response: tx,
        })
        .map_err(|_| mlua::Error::RuntimeError("workflow agent channel closed".into()))?;

    // Deferred mode (inside parallel/pipeline fan-out): return a marker so many
    // agents can be in-flight under the host semaphore; resolve after thunks.
    if host.defer_depth.load(Ordering::SeqCst) > 0 {
        let defer_id = idx;
        if let Ok(mut d) = host.deferred.lock() {
            d.push(DeferredAgent {
                id: defer_id,
                response: rx,
            });
        }
        let marker = lua.create_table()?;
        marker.set("__wf_pending", defer_id as i64)?;
        return Ok(LuaValue::Table(marker));
    }

    let result = rx
        .recv()
        .map_err(|_| mlua::Error::RuntimeError("workflow agent response dropped".into()))?;
    if host.cancel_token.is_requested() {
        return Err(mlua::Error::RuntimeError("cancelled".into()));
    }
    if !result.ok {
        if let Some(err) = &result.error {
            if err.contains("cancelled") {
                return Err(mlua::Error::RuntimeError("cancelled".into()));
            }
        }
    }
    let truncated = truncate_json_value(result.output, AGENT_RESULT_MAX_BYTES);
    json_to_lua(lua, &truncated)
}

fn resolve_deferred(lua: &Lua, host: &HostState, value: LuaValue) -> mlua::Result<LuaValue> {
    match value {
        LuaValue::Table(t) => {
            if let Ok(id) = t.get::<i64>("__wf_pending") {
                let rx = {
                    let mut deferred = host
                        .deferred
                        .lock()
                        .map_err(|e| mlua::Error::RuntimeError(format!("deferred lock: {e}")))?;
                    let pos = deferred.iter().position(|d| d.id == id as u64);
                    match pos {
                        Some(i) => deferred.remove(i).response,
                        None => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "missing deferred agent {id}"
                            )));
                        }
                    }
                };
                let result = rx.recv().map_err(|_| {
                    mlua::Error::RuntimeError("workflow agent response dropped".into())
                })?;
                if host.cancel_token.is_requested() {
                    return Err(mlua::Error::RuntimeError("cancelled".into()));
                }
                let truncated = truncate_json_value(result.output, AGENT_RESULT_MAX_BYTES);
                return json_to_lua(lua, &truncated);
            }
            // Recurse into array-like tables.
            let len = t.len()?;
            if len > 0 {
                let out = lua.create_table()?;
                for i in 1..=len {
                    let v: LuaValue = t.get(i)?;
                    out.set(i, resolve_deferred(lua, host, v)?)?;
                }
                return Ok(LuaValue::Table(out));
            }
            Ok(LuaValue::Table(t))
        }
        other => Ok(other),
    }
}

fn call_entrypoint(lua: &Lua, host: &HostState) -> Result<LuaValue, WorkflowHostError> {
    if host.cancel_token.is_requested() {
        return Err(WorkflowHostError {
            code: WorkflowErrorCode::Cancelled,
            message: "cancelled".into(),
            hint: None,
        });
    }
    if let Ok(func) = lua.globals().get::<mlua::Function>("workflow") {
        return func.call(()).map_err(map_runtime_err);
    }
    Ok(LuaValue::Nil)
}

fn parse_agent_opts(opts: Option<Table>) -> AgentPolicyOpts {
    let Some(t) = opts else {
        return AgentPolicyOpts::default();
    };
    let mut out = AgentPolicyOpts::default();
    if let Ok(Some(p)) = t.get::<Option<String>>("profile") {
        out.profile = Some(p);
    }
    if let Ok(Some(m)) = t.get::<Option<String>>("model") {
        out.model = Some(m);
    }
    if let Ok(Some(l)) = t.get::<Option<String>>("label") {
        out.label = Some(l);
    }
    if let Ok(Some(a)) = t.get::<Option<String>>("approval") {
        out.approval = Some(a);
    }
    if let Ok(Some(b)) = t.get::<Option<bool>>("create_files") {
        out.create_files = Some(b);
    }
    if let Ok(Some(b)) = t.get::<Option<bool>>("create_dirs") {
        out.create_dirs = Some(b);
    }
    if let Ok(Some(n)) = t.get::<Option<i64>>("max_tokens") {
        out.max_tokens = Some(n.max(0) as usize);
    }
    out.tools = read_string_array(&t, "tools");
    out.write_allow = read_string_array(&t, "write_allow").or_else(|| {
        // Distinguish missing vs empty: if key exists as empty table, return Some([])
        if t.contains_key("write_allow").unwrap_or(false) {
            Some(Vec::new())
        } else {
            None
        }
    });
    out.path_allow = read_string_array(&t, "path_allow");
    out.path_deny = read_string_array(&t, "path_deny");
    out
}

fn read_string_array(t: &Table, key: &str) -> Option<Vec<String>> {
    let tbl: Table = t.get(key).ok()?;
    let mut v = Vec::new();
    for pair in tbl.sequence_values::<String>() {
        if let Ok(s) = pair {
            v.push(s);
        }
    }
    Some(v)
}

fn classify_load_error(msg: &str) -> WorkflowErrorCode {
    if msg.contains("sandbox_violation") {
        WorkflowErrorCode::SandboxViolation
    } else {
        WorkflowErrorCode::ScriptParseError
    }
}

fn map_runtime_err(err: mlua::Error) -> WorkflowHostError {
    let msg = err.to_string();
    if msg.contains("agent_cap_exceeded") {
        return WorkflowHostError {
            code: WorkflowErrorCode::AgentCapExceeded,
            message: msg,
            hint: Some("Increase max_agents or reduce agent() calls.".into()),
        };
    }
    if msg.contains("cancelled") {
        return WorkflowHostError {
            code: WorkflowErrorCode::Cancelled,
            message: msg,
            hint: None,
        };
    }
    if msg.contains("invalid_host_call") {
        return WorkflowHostError {
            code: WorkflowErrorCode::InvalidHostCall,
            message: msg,
            hint: None,
        };
    }
    if msg.contains("sandbox_violation")
        || msg.contains("args is read-only")
        || (msg.contains("attempt to call a nil value")
            && (msg.contains("require") || msg.contains("io") || msg.contains("os")))
        || msg.contains("instruction limit")
    {
        let code = if msg.contains("instruction limit") {
            WorkflowErrorCode::ScriptRuntimeError
        } else {
            WorkflowErrorCode::SandboxViolation
        };
        return WorkflowHostError {
            code,
            message: msg,
            hint: Some("Do not use require/io/os/debug; avoid infinite loops.".into()),
        };
    }
    WorkflowHostError {
        code: WorkflowErrorCode::ScriptRuntimeError,
        message: msg,
        hint: None,
    }
}

fn map_lua_err(err: mlua::Error) -> WorkflowHostError {
    WorkflowHostError {
        code: WorkflowErrorCode::ScriptRuntimeError,
        message: err.to_string(),
        hint: None,
    }
}

fn json_to_lua(lua: &Lua, value: &JsonValue) -> mlua::Result<LuaValue> {
    match value {
        JsonValue::Null => Ok(LuaValue::Nil),
        JsonValue::Bool(b) => Ok(LuaValue::Boolean(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else if let Some(u) = n.as_u64() {
                if u <= i64::MAX as u64 {
                    Ok(LuaValue::Integer(u as i64))
                } else {
                    Ok(LuaValue::Number(u as f64))
                }
            } else {
                Ok(LuaValue::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        JsonValue::String(s) => Ok(LuaValue::String(lua.create_string(s)?)),
        JsonValue::Array(arr) => {
            let t = lua.create_table_with_capacity(arr.len(), 0)?;
            for (i, v) in arr.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, v)?)?;
            }
            Ok(LuaValue::Table(t))
        }
        JsonValue::Object(map) => {
            let t = lua.create_table_with_capacity(0, map.len())?;
            for (k, v) in map {
                t.set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            Ok(LuaValue::Table(t))
        }
    }
}

fn lua_to_json(lua: &Lua, value: LuaValue) -> mlua::Result<JsonValue> {
    match value {
        LuaValue::Nil => Ok(JsonValue::Null),
        LuaValue::Boolean(b) => Ok(JsonValue::Bool(b)),
        LuaValue::Integer(i) => Ok(json!(i)),
        LuaValue::Number(n) => {
            if let Some(num) = serde_json::Number::from_f64(n) {
                Ok(JsonValue::Number(num))
            } else {
                Ok(JsonValue::Null)
            }
        }
        LuaValue::String(s) => Ok(JsonValue::String(s.to_str()?.to_string())),
        LuaValue::Table(t) => {
            let len = t.len()?;
            if len > 0 {
                let mut arr = Vec::new();
                let mut is_array = true;
                for i in 1..=len {
                    match t.get::<LuaValue>(i) {
                        Ok(v) => arr.push(lua_to_json(lua, v)?),
                        Err(_) => {
                            is_array = false;
                            break;
                        }
                    }
                }
                if is_array {
                    for pair in t.pairs::<LuaValue, LuaValue>() {
                        let (k, _) = pair?;
                        match k {
                            LuaValue::Integer(i) if i >= 1 && i <= len => {}
                            LuaValue::Number(n)
                                if n.fract() == 0.0 && n >= 1.0 && n <= len as f64 => {}
                            _ => {
                                is_array = false;
                                break;
                            }
                        }
                    }
                }
                if is_array {
                    return Ok(JsonValue::Array(arr));
                }
            }
            let mut map = serde_json::Map::new();
            for pair in t.pairs::<LuaValue, LuaValue>() {
                let (k, v) = pair?;
                let key = match k {
                    LuaValue::String(s) => s.to_str()?.to_string(),
                    LuaValue::Integer(i) => i.to_string(),
                    LuaValue::Number(n) => n.to_string(),
                    _ => continue,
                };
                map.insert(key, lua_to_json(lua, v)?);
            }
            Ok(JsonValue::Object(map))
        }
        other => Ok(JsonValue::String(format!("{other:?}"))),
    }
}

fn truncate_json_value(value: JsonValue, max_bytes: usize) -> JsonValue {
    let Ok(s) = serde_json::to_string(&value) else {
        return value;
    };
    if s.len() <= max_bytes {
        return value;
    }
    json!({
        "truncated": true,
        "original_bytes": s.len(),
        "preview": s.chars().take(4096).collect::<String>(),
    })
}
