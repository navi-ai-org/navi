use crate::error::RuntimeError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wasmtime::*;

/// Data stored in the WASM store, holding resource limits and host callbacks.
struct StoreData {
    limits: StoreLimits,
    callbacks: HostCallbacks,
}

/// Host callback functions that the WASM module can call.
///
/// Each callback receives a string input and returns a string output.
/// Errors are returned as JSON: `{"error": "message"}`.
#[derive(Clone)]
pub struct HostCallbacks {
    /// Read a project file. Input: path. Output: file content or error JSON.
    pub fs_read: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// List a project directory. Input: path. Output: JSON array of names or error JSON.
    pub fs_list: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Make an HTTP request. Input: JSON `{method, url, body}`. Output: JSON response or error JSON.
    pub http_request: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// Get git status. Input: empty. Output: git status text or error JSON.
    pub git_status: Arc<dyn Fn() -> String + Send + Sync>,
    /// Get git diff. Input: empty. Output: git diff text or error JSON.
    pub git_diff: Arc<dyn Fn() -> String + Send + Sync>,
}

impl Default for HostCallbacks {
    fn default() -> Self {
        Self {
            fs_read: Arc::new(|_| r#"{"error":"not implemented"}"#.into()),
            fs_list: Arc::new(|_| r#"{"error":"not implemented"}"#.into()),
            http_request: Arc::new(|_| r#"{"error":"not implemented"}"#.into()),
            git_status: Arc::new(|| r#"{"error":"not implemented"}"#.into()),
            git_diff: Arc::new(|| r#"{"error":"not implemented"}"#.into()),
        }
    }
}

/// Configuration for the WASM plugin runtime.
/// All limits are mandatory and cannot be disabled.
#[derive(Debug, Clone)]
pub struct ToolRuntimeConfig {
    /// Wall-clock timeout per invocation.
    pub timeout: Duration,
    /// Linear memory limit in bytes.
    pub memory_limit_bytes: u64,
    /// Fuel (instruction budget) per invocation.
    pub fuel: u64,
    /// Max tool output size in bytes.
    pub max_output_bytes: usize,
    /// Stack size in bytes.
    pub stack_size_bytes: usize,
}

impl Default for ToolRuntimeConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            memory_limit_bytes: 64 * 1024 * 1024, // 64 MB
            fuel: 10_000_000,
            max_output_bytes: 32 * 1024,   // 32 KB
            stack_size_bytes: 1024 * 1024, // 1 MB
        }
    }
}

/// Result of a tool invocation.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// The JSON output string from the plugin.
    pub output: String,
    /// Fuel consumed during execution.
    pub fuel_consumed: u64,
    /// Wall-clock duration of execution.
    pub duration: Duration,
}

/// WASM plugin runtime with mandatory resource limits.
pub struct PluginRuntime {
    config: ToolRuntimeConfig,
}

impl PluginRuntime {
    /// Create a new runtime with the given configuration.
    pub fn new(config: ToolRuntimeConfig) -> Self {
        Self { config }
    }

    /// Create a runtime with default security limits.
    pub fn with_defaults() -> Self {
        Self::new(ToolRuntimeConfig::default())
    }

    /// Execute a tool in a WASM module with host callbacks.
    pub fn execute(
        &self,
        wasm_bytes: &[u8],
        tool_name: &str,
        input_json: &str,
        callbacks: HostCallbacks,
    ) -> Result<RunResult, RuntimeError> {
        let start = Instant::now();

        let mut engine_config = Config::new();
        engine_config.consume_fuel(true);

        let engine = Engine::new(&engine_config)?;

        let limits = StoreLimitsBuilder::new()
            .memory_size(self.config.memory_limit_bytes as usize)
            .build();
        let mut store = Store::new(&engine, StoreData { limits, callbacks });

        store.set_fuel(self.config.fuel)?;
        store.limiter(|data: &mut StoreData| &mut data.limits);

        let module = Module::new(&engine, wasm_bytes)?;
        let mut linker = Linker::new(&engine);

        // Register host imports
        add_host_imports(&mut linker)?;

        let instance = linker.instantiate(&mut store, &module)?;

        let run_tool = instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "run_tool")
            .map_err(|_| RuntimeError::ToolNotFound {
                tool_name: tool_name.into(),
            })?;

        let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| {
            RuntimeError::ToolNotFound {
                tool_name: "memory".into(),
            }
        })?;

        // Write tool_name and input_json into WASM memory
        let tool_name_bytes = tool_name.as_bytes();
        let input_bytes = input_json.as_bytes();

        let tool_name_ptr = 0i32;
        let input_ptr = tool_name_bytes.len() as i32;

        let total_needed = tool_name_bytes.len() + input_bytes.len();
        let pages_needed = total_needed.div_ceil(65536);
        let current_pages = memory.size(&store) as usize;
        if total_needed > current_pages * 65536 {
            memory.grow(&mut store, pages_needed as u64)?;
        }

        let mem_data = memory.data_mut(&mut store);
        mem_data[..tool_name_bytes.len()].copy_from_slice(tool_name_bytes);
        mem_data[tool_name_bytes.len()..tool_name_bytes.len() + input_bytes.len()]
            .copy_from_slice(input_bytes);

        // Timeout thread
        let timed_out = Arc::new(AtomicBool::new(false));
        let timed_out_clone = timed_out.clone();
        let timeout = self.config.timeout;
        let timeout_thread = std::thread::spawn(move || {
            std::thread::sleep(timeout);
            timed_out_clone.store(true, Ordering::Relaxed);
        });

        let result = run_tool.call(
            &mut store,
            (
                tool_name_ptr,
                tool_name_bytes.len() as i32,
                input_ptr,
                input_bytes.len() as i32,
            ),
        );

        let timed_out_flag = timed_out.load(Ordering::Relaxed);
        drop(timeout_thread);

        let fuel_after = self
            .config
            .fuel
            .saturating_sub(store.get_fuel().unwrap_or(0));
        let duration = start.elapsed();

        match result {
            Ok(output_ptr) => {
                let mem_data = memory.data(&store);
                let ptr = output_ptr as usize;

                if ptr + 4 > mem_data.len() {
                    return Err(RuntimeError::PluginError("invalid output pointer".into()));
                }

                let len = u32::from_le_bytes([
                    mem_data[ptr],
                    mem_data[ptr + 1],
                    mem_data[ptr + 2],
                    mem_data[ptr + 3],
                ]) as usize;

                if ptr + 4 + len > mem_data.len() {
                    return Err(RuntimeError::PluginError(
                        "output extends beyond memory".into(),
                    ));
                }

                let output_bytes = &mem_data[ptr + 4..ptr + 4 + len];
                let output = String::from_utf8_lossy(output_bytes).to_string();

                if output.len() > self.config.max_output_bytes {
                    return Err(RuntimeError::OutputTooLarge {
                        size_bytes: output.len(),
                        limit_bytes: self.config.max_output_bytes,
                    });
                }

                Ok(RunResult {
                    output,
                    fuel_consumed: fuel_after,
                    duration,
                })
            }
            Err(e) => {
                if timed_out_flag {
                    return Err(RuntimeError::Timeout {
                        timeout_secs: self.config.timeout.as_secs(),
                    });
                }

                let e_str = format!("{:?}", e);
                if e_str.contains("fuel") || e_str.contains("all fuel consumed") {
                    return Err(RuntimeError::FuelExhausted);
                }

                if e_str.contains("memory") || e_str.contains("out of bounds") {
                    return Err(RuntimeError::MemoryLimitExceeded {
                        limit_mb: self.config.memory_limit_bytes / (1024 * 1024),
                    });
                }

                Err(RuntimeError::Engine(e))
            }
        }
    }
}

/// Helper: read a string from WASM memory.
fn read_wasm_string(mem: &[u8], ptr: i32, len: i32) -> String {
    let start = ptr as usize;
    let end = start + len as usize;
    if end > mem.len() {
        return String::new();
    }
    String::from_utf8_lossy(&mem[start..end]).to_string()
}

/// Helper: write a string to WASM memory and return the pointer.
/// Format: [4 bytes length LE][content bytes]
fn write_wasm_string(mem: &mut [u8], content: &str, offset: usize) -> i32 {
    let bytes = content.as_bytes();
    let len = bytes.len() as u32;
    mem[offset..offset + 4].copy_from_slice(&len.to_le_bytes());
    mem[offset + 4..offset + 4 + bytes.len()].copy_from_slice(bytes);
    offset as i32
}

/// Register host imports with real implementations backed by HostCallbacks.
fn add_host_imports(linker: &mut Linker<StoreData>) -> anyhow::Result<()> {
    // fs.read-project-file(path_ptr, path_len) -> result_ptr
    linker.func_wrap(
        "fs",
        "read-project-file",
        |mut caller: Caller<'_, StoreData>, ptr: i32, len: i32| -> i32 {
            let path = {
                let mem = caller.get_export("memory").and_then(|e| e.into_memory());
                match mem {
                    Some(m) => read_wasm_string(m.data(&caller), ptr, len),
                    None => return -1,
                }
            };

            let result = (caller.data().callbacks.fs_read)(&path);

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            // Write result at the end of current memory (after input area)
            let offset = mem.data_size(&caller).saturating_sub(4096);
            write_wasm_string(mem.data_mut(&mut caller), &result, offset)
        },
    )?;

    // fs.list-project-dir(path_ptr, path_len) -> result_ptr
    linker.func_wrap(
        "fs",
        "list-project-dir",
        |mut caller: Caller<'_, StoreData>, ptr: i32, len: i32| -> i32 {
            let path = {
                let mem = caller.get_export("memory").and_then(|e| e.into_memory());
                match mem {
                    Some(m) => read_wasm_string(m.data(&caller), ptr, len),
                    None => return -1,
                }
            };

            let result = (caller.data().callbacks.fs_list)(&path);

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let offset = mem.data_size(&caller).saturating_sub(4096);
            write_wasm_string(mem.data_mut(&mut caller), &result, offset)
        },
    )?;

    // http.request(method_ptr, method_len, url_ptr, url_len, body_ptr, body_len) -> result_ptr
    linker.func_wrap(
        "http",
        "request",
        |mut caller: Caller<'_, StoreData>,
         m_ptr: i32,
         m_len: i32,
         u_ptr: i32,
         u_len: i32,
         b_ptr: i32,
         b_len: i32|
         -> i32 {
            let (method, url, body) = {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let data = mem.data(&caller);
                let method = read_wasm_string(data, m_ptr, m_len);
                let url = read_wasm_string(data, u_ptr, u_len);
                let body = if b_ptr >= 0 && b_len > 0 {
                    read_wasm_string(data, b_ptr, b_len)
                } else {
                    String::new()
                };
                (method, url, body)
            };

            let input = if body.is_empty() {
                format!(r#"{{"method":"{}","url":"{}"}}"#, method, url)
            } else {
                format!(
                    r#"{{"method":"{}","url":"{}","body":"{}"}}"#,
                    method, url, body
                )
            };

            let result = (caller.data().callbacks.http_request)(&input);

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let offset = mem.data_size(&caller).saturating_sub(4096);
            write_wasm_string(mem.data_mut(&mut caller), &result, offset)
        },
    )?;

    // git.status() -> result_ptr
    linker.func_wrap(
        "git",
        "status",
        |mut caller: Caller<'_, StoreData>| -> i32 {
            let result = (caller.data().callbacks.git_status)();

            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let offset = mem.data_size(&caller).saturating_sub(4096);
            write_wasm_string(mem.data_mut(&mut caller), &result, offset)
        },
    )?;

    // git.diff() -> result_ptr
    linker.func_wrap("git", "diff", |mut caller: Caller<'_, StoreData>| -> i32 {
        let result = (caller.data().callbacks.git_diff)();

        let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
            Some(m) => m,
            None => return -1,
        };
        let offset = mem.data_size(&caller).saturating_sub(4096);
        write_wasm_string(mem.data_mut(&mut caller), &result, offset)
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_spec() {
        let config = ToolRuntimeConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.memory_limit_bytes, 64 * 1024 * 1024);
        assert_eq!(config.fuel, 10_000_000);
        assert_eq!(config.max_output_bytes, 32 * 1024);
        assert_eq!(config.stack_size_bytes, 1024 * 1024);
    }

    #[test]
    fn runtime_creation() {
        let runtime = PluginRuntime::with_defaults();
        assert_eq!(runtime.config.timeout, Duration::from_secs(30));
    }

    #[test]
    fn runtime_with_custom_config() {
        let config = ToolRuntimeConfig {
            timeout: Duration::from_secs(5),
            memory_limit_bytes: 16 * 1024 * 1024,
            fuel: 1_000_000,
            max_output_bytes: 8192,
            stack_size_bytes: 512 * 1024,
        };
        let runtime = PluginRuntime::new(config.clone());
        assert_eq!(runtime.config.timeout, Duration::from_secs(5));
        assert_eq!(runtime.config.memory_limit_bytes, 16 * 1024 * 1024);
    }

    #[test]
    fn execute_echo_plugin() {
        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)

                (func $write_u32 (param $ptr i32) (param $val i32)
                    (i32.store (local.get $ptr) (local.get $val))
                )

                (func (export "run_tool")
                    (param $name_ptr i32) (param $name_len i32)
                    (param $input_ptr i32) (param $input_len i32)
                    (result i32)

                    (local $output_ptr i32)
                    (local $i i32)

                    (local.set $output_ptr
                        (i32.add (local.get $input_ptr) (local.get $input_len))
                    )

                    (call $write_u32 (local.get $output_ptr) (local.get $input_len))

                    (local.set $i (i32.const 0))
                    (block $break
                        (loop $copy
                            (br_if $break (i32.ge_u (local.get $i) (local.get $input_len)))
                            (i32.store8
                                (i32.add (i32.add (local.get $output_ptr) (i32.const 4)) (local.get $i))
                                (i32.load8_u (i32.add (local.get $input_ptr) (local.get $i)))
                            )
                            (local.set $i (i32.add (local.get $i) (i32.const 1)))
                            (br $copy)
                        )
                    )

                    (local.get $output_ptr)
                )
            )
            "#,
        )
        .expect("valid WAT");

        let runtime = PluginRuntime::with_defaults();
        let result = runtime.execute(
            &wasm,
            "echo",
            r#"{"text":"hello"}"#,
            HostCallbacks::default(),
        );
        assert!(
            result.is_ok(),
            "execution should succeed: {:?}",
            result.err()
        );
        let result = result.unwrap();
        assert_eq!(result.output, r#"{"text":"hello"}"#);
    }

    #[test]
    fn execute_fuel_exhaustion() {
        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (func (export "run_tool")
                    (param $name_ptr i32) (param $name_len i32)
                    (param $input_ptr i32) (param $input_len i32)
                    (result i32)
                    (block $break
                        (loop $loop
                            (br $loop)
                        )
                    )
                    (i32.const 0)
                )
            )
            "#,
        )
        .expect("valid WAT");

        let config = ToolRuntimeConfig {
            fuel: 100,
            timeout: Duration::from_secs(3600),
            ..Default::default()
        };
        let runtime = PluginRuntime::new(config);
        let result = runtime.execute(&wasm, "loop", "{}", HostCallbacks::default());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, RuntimeError::FuelExhausted),
            "should be fuel exhaustion, got: {:?}",
            err
        );
    }

    #[test]
    fn execute_timeout() {
        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (func (export "run_tool")
                    (param $name_ptr i32) (param $name_len i32)
                    (param $input_ptr i32) (param $input_len i32)
                    (result i32)
                    (local $i i32)
                    (block $break
                        (loop $loop
                            (local.set $i (i32.add (local.get $i) (i32.const 1)))
                            (br_if $break (i32.ge_u (local.get $i) (i32.const 1000000000)))
                            (br $loop)
                        )
                    )
                    (i32.const 0)
                )
            )
            "#,
        )
        .expect("valid WAT");

        let config = ToolRuntimeConfig {
            timeout: Duration::from_millis(1),
            fuel: 100_000_000,
            ..Default::default()
        };
        let runtime = PluginRuntime::new(config);
        let result = runtime.execute(&wasm, "slow", "{}", HostCallbacks::default());
        if let Err(e) = result {
            assert!(
                matches!(e, RuntimeError::Timeout { .. }),
                "should be timeout"
            );
        }
    }

    #[test]
    fn tool_not_found_error() {
        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (func (export "other_func") (result i32)
                    (i32.const 0)
                )
            )
            "#,
        )
        .expect("valid WAT");

        let runtime = PluginRuntime::with_defaults();
        let result = runtime.execute(&wasm, "test", "{}", HostCallbacks::default());
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), RuntimeError::ToolNotFound { .. }),
            "should be tool not found"
        );
    }

    #[test]
    fn host_callback_fs_read() {
        // WASM module that calls fs.read-project-file and returns the result
        let wasm = wat::parse_str(
            r#"
            (module
                ;; Import fs.read-project-file BEFORE memory
                (import "fs" "read-project-file" (func $fs_read (param i32 i32) (result i32)))

                (memory (export "memory") 1)

                (func (export "run_tool")
                    (param $name_ptr i32) (param $name_len i32)
                    (param $input_ptr i32) (param $input_len i32)
                    (result i32)

                    ;; Write "test.txt" at address 0
                    (i32.store8 (i32.const 0) (i32.const 116))  ;; t
                    (i32.store8 (i32.const 1) (i32.const 101))  ;; e
                    (i32.store8 (i32.const 2) (i32.const 115))  ;; s
                    (i32.store8 (i32.const 3) (i32.const 116))  ;; t
                    (i32.store8 (i32.const 4) (i32.const 46))   ;; .
                    (i32.store8 (i32.const 5) (i32.const 116))  ;; t
                    (i32.store8 (i32.const 6) (i32.const 120))  ;; x
                    (i32.store8 (i32.const 7) (i32.const 116))  ;; t

                    ;; Call fs.read-project-file(0, 8)
                    (call $fs_read (i32.const 0) (i32.const 8))
                    ;; Return the result pointer
                )
            )
            "#,
        )
        .expect("valid WAT");

        let callbacks = HostCallbacks {
            fs_read: Arc::new(|path| {
                if path == "test.txt" {
                    r#"{"content":"hello from broker"}"#.into()
                } else {
                    r#"{"error":"not found"}"#.into()
                }
            }),
            ..Default::default()
        };

        let runtime = PluginRuntime::with_defaults();
        let result = runtime.execute(&wasm, "read", "{}", callbacks);
        assert!(result.is_ok(), "should succeed: {:?}", result.err());
    }
}
