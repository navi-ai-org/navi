use anyhow::Result;
use navi_browser::{BrowserRuntimeConfig, doctor_report};
use navi_core::LoadedConfig;

pub async fn handle_browser_command(
    action: crate::BrowserAction,
    loaded_config: &LoadedConfig,
) -> Result<()> {
    let c = &loaded_config.config.browser;
    let runtime = BrowserRuntimeConfig {
        enabled: c.enabled,
        backend: c.backend.clone(),
        cdp_url: c.cdp_url.clone(),
        headless: c.headless,
        allow_private_network: c.allow_private_network,
        proxy: c.proxy.clone(),
        timeout_ms: c.timeout_ms,
        binary_path: c.binary_path.clone(),
    };

    match action {
        crate::BrowserAction::Doctor => {
            let report = doctor_report(&runtime);
            println!("{}", serde_json::to_string_pretty(&report)?);
            if let Some(hints) = report.get("hints").and_then(|h| h.as_array())
                && !hints.is_empty()
            {
                println!();
                println!("Hints:");
                for h in hints {
                    if let Some(s) = h.as_str() {
                        println!("  - {s}");
                    }
                }
            }
        }
        crate::BrowserAction::Install => {
            println!("Browser backends for the NAVI `browser` tool:\n");
            println!("  CDP (Chrome DevTools Protocol)");
            println!("     - Google Chrome / Chromium installed locally, or:");
            println!("     - Any CDP-compatible browser running with remote debugging:");
            println!("       google-chrome --remote-debugging-port=9222");
            println!("       # [browser] backend = \"cdp\", cdp_url = \"http://127.0.0.1:9222\"");
            println!();
            println!("  Then run: navi browser doctor");
        }
        crate::BrowserAction::Status => {
            let report = doctor_report(&runtime);
            println!("Browser tool");
            println!("  enabled: {}", runtime.enabled);
            println!("  backend: {}", runtime.backend);
            println!("  headless: {}", runtime.headless);
            println!("  allow_private_network: {}", runtime.allow_private_network);
            if !runtime.cdp_url.is_empty() {
                println!("  cdp_url: {}", runtime.cdp_url);
            }
            if let Some(bin) = report.get("binary") {
                println!(
                    "  binary: {}",
                    bin.get("path").and_then(|p| p.as_str()).unwrap_or("(none)")
                );
                println!(
                    "  kind: {}",
                    bin.get("kind")
                        .and_then(|p| p.as_str())
                        .unwrap_or("unknown")
                );
            } else {
                println!("  binary: (none found)");
            }
            println!(
                "  data_dir/browser: {}",
                loaded_config.data_dir.join("browser").display()
            );
        }
    }
    Ok(())
}
