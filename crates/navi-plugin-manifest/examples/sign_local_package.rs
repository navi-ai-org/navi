//! Sign a local WASM plugin package for marketplace fixtures / LocalDev demos.
//!
//! ```bash
//! cargo run -p navi-plugin-manifest --example sign_local_package -- \
//!   marketplace/artifacts/hello-echo/0.1.0
//! ```

use navi_plugin_manifest::{
    PluginManifest, PluginMeta, RuntimeKind, ToolDef, ToolRisk, parse_manifest,
    sign_plugin_manifest_for_tests,
};
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env::args().nth(1).expect("usage: sign_local_package <plugin-dir>"));
    let wasm_path = dir.join("plugin.wasm");
    let wasm = fs::read(&wasm_path).expect("read plugin.wasm");
    let toml_path = dir.join("plugin.toml");

    let mut manifest = if toml_path.is_file() {
        let content = fs::read_to_string(&toml_path).expect("read plugin.toml");
        parse_manifest(&content).expect("parse plugin.toml")
    } else {
        PluginManifest {
            plugin: PluginMeta {
                id: dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "plugin".into()),
                name: "Package".into(),
                version: "0.1.0".into(),
                publisher: "gh:navi-ai-org".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: String::new(),
                signature: String::new(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![],
            tools: vec![ToolDef {
                id: "echo".into(),
                summary: "Echo JSON input".into(),
                risk: ToolRisk::ReadOnly,
                input_schema: None,
                capabilities: vec![],
            }],
        }
    };

    sign_plugin_manifest_for_tests(&mut manifest, &wasm);
    let out = toml::to_string_pretty(&manifest).expect("serialize");
    fs::write(&toml_path, out).expect("write plugin.toml");
    println!(
        "signed {} (hash={})",
        toml_path.display(),
        manifest.plugin.wasm_hash
    );
}
