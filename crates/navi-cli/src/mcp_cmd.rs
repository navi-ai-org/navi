use anyhow::Result;
use navi_core::LoadedConfig;

use crate::McpAction;

pub async fn handle_mcp_command(action: McpAction, config: &LoadedConfig) -> Result<()> {
    match action {
        McpAction::List => list_mcp_servers(config).await,
    }
}

async fn list_mcp_servers(config: &LoadedConfig) -> Result<()> {
    let mcp_config = &config.config.mcp;

    if !mcp_config.enabled {
        println!("MCP integration is disabled.");
        return Ok(());
    }

    if mcp_config.servers.is_empty() {
        println!("No MCP servers configured.");
        return Ok(());
    }

    println!("Connecting to {} server(s)...\n", mcp_config.servers.len());

    let loaded = navi_mcp::load_configured_mcp_servers(
        mcp_config,
        &config.config.security.allowed_mcp_servers,
    )
    .await;

    let connected_ids: std::collections::HashSet<String> =
        loaded.servers.iter().map(|s| s.id.clone()).collect();

    for server in &mcp_config.servers {
        let status = if !server.enabled {
            "disabled"
        } else if connected_ids.contains(&server.id) {
            "connected"
        } else {
            "failed"
        };

        let tag = match status {
            "disabled" => "  ",
            "connected" => "OK",
            _ => "!!",
        };

        println!("[{}] {} ({})", tag, server.id, status);

        if let Some(url) = &server.url {
            println!("      url: {url}");
        }
        if let Some(cmd) = &server.command {
            if server.args.is_empty() {
                println!("      cmd: {cmd}");
            } else {
                println!("      cmd: {} {}", cmd, server.args.join(" "));
            }
        }

        if let Some(info) = loaded.servers.iter().find(|s| s.id == server.id) {
            if let Some(version) = &info.protocol_version {
                let era = if info.legacy { "legacy" } else { "modern" };
                println!("      protocol: {version} ({era})");
            }
            if !info.supported_protocol_versions.is_empty() {
                println!(
                    "      supported versions: {}",
                    info.supported_protocol_versions.join(", ")
                );
            }
            if let Some(capabilities) = info.capabilities.as_object() {
                if !capabilities.is_empty() {
                    let mut names = capabilities.keys().cloned().collect::<Vec<_>>();
                    names.sort();
                    println!("      capabilities: {}", names.join(", "));
                }
            }
            if info.tools.is_empty() {
                println!("      tools: (none)");
            } else {
                println!("      tools: {}", info.tools.join(", "));
            }
        }

        println!();
    }

    loaded.shutdown();
    Ok(())
}
