#![recursion_limit = "512"]
use clap::Parser;
use navi_server::{NaviServer, NaviServerConfig};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "navi-server", about = "NAVI remote server for mobile clients")]
struct Args {
    /// Port to listen on
    #[arg(long, default_value = "9800")]
    port: u16,

    /// Address to bind to (0.0.0.0 for Tailscale access)
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,

    /// Shared secret for authentication. Falls back to NAVI_SERVER_SECRET env var.
    #[arg(long)]
    secret: Option<String>,

    /// Project directory for the NaviEngine
    #[arg(long, default_value = ".")]
    project: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("navi_server=info".parse()?))
        .init();

    let args = Args::parse();
    let secret = args
        .secret
        .or_else(|| std::env::var("NAVI_SERVER_SECRET").ok())
        .unwrap_or_else(|| {
            eprintln!("WARNING: No secret set. Use --secret or NAVI_SERVER_SECRET env var.");
            String::new()
        });

    let config = NaviServerConfig {
        bind: args.bind,
        port: args.port,
        shared_secret: secret,
        project_dir: args.project,
    };

    let server = NaviServer::new(config).await?;
    server.run().await
}
