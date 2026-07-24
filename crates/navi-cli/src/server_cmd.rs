//! `navi server` — install and manage the remote `navi-server` as a systemd service.

use anyhow::{Context, Result, bail};
use navi_core::LoadedConfig;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SERVICE_NAME: &str = "navi-server.service";

#[derive(Debug, Clone)]
pub struct ServerInstallOpts {
    pub port: u16,
    pub bind: String,
    pub project: PathBuf,
    pub secret: Option<String>,
    /// Install as system unit (requires root). Default: user unit.
    pub system: bool,
    /// Force overwrite existing unit/env.
    pub force: bool,
}

pub fn handle_server_command(
    action: crate::ServerAction,
    loaded_config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    match action {
        crate::ServerAction::Install {
            port,
            bind,
            project,
            secret,
            system,
            force,
        } => {
            // Project is optional: mobile apps pick the workspace per session.
            // Default = $HOME so agent mode works without `--project`.
            let project = resolve_default_project(project, cwd)?;
            install(ServerInstallOpts {
                port,
                bind,
                project,
                secret,
                system,
                force,
            })?;
        }
        crate::ServerAction::Start { system } => {
            ensure_installed(system)?;
            systemctl(system, &["start", SERVICE_NAME])?;
            println!("Started {SERVICE_NAME}");
            print_status_hint(system);
        }
        crate::ServerAction::Stop { system } => {
            systemctl(system, &["stop", SERVICE_NAME])?;
            println!("Stopped {SERVICE_NAME}");
        }
        crate::ServerAction::Restart { system } => {
            ensure_installed(system)?;
            systemctl(system, &["restart", SERVICE_NAME])?;
            println!("Restarted {SERVICE_NAME}");
            print_status_hint(system);
        }
        crate::ServerAction::Status { system } => {
            status(system, loaded_config)?;
        }
        crate::ServerAction::Uninstall { system } => {
            uninstall(system)?;
        }
        crate::ServerAction::Logs { system, follow } => {
            logs(system, follow)?;
        }
    }
    Ok(())
}

/// Resolve the engine home workspace for the systemd unit.
///
/// - Explicit `--project` wins.
/// - Otherwise `$HOME` (agent home). Falls back to cwd if HOME is unset.
fn resolve_default_project(explicit: Option<PathBuf>, cwd: &Path) -> Result<PathBuf> {
    let raw = if let Some(p) = explicit {
        p
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
    } else {
        cwd.to_path_buf()
    };
    if !raw.exists() {
        fs::create_dir_all(&raw)
            .with_context(|| format!("create default project {}", raw.display()))?;
    }
    Ok(raw.canonicalize().unwrap_or(raw))
}

fn install(opts: ServerInstallOpts) -> Result<()> {
    let bin = resolve_navi_server_bin().context(
        "navi-server binary not found. Build it with `cargo build -p navi-server --release` \
         and ensure it is next to `navi` or on PATH.",
    )?;

    let secret = opts
        .secret
        .or_else(|| std::env::var("NAVI_SERVER_SECRET").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(generate_secret);

    let unit_path = unit_file_path(opts.system)?;
    let env_path = env_file_path(opts.system)?;

    if unit_path.exists() && !opts.force {
        bail!(
            "unit already exists at {}\nRe-run with --force to overwrite, or: navi server uninstall",
            unit_path.display()
        );
    }

    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if let Some(parent) = env_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let unit = render_unit(
        &bin,
        &env_path,
        opts.system,
        &opts.bind,
        opts.port,
        &opts.project,
    );
    let env = render_env(&secret);

    fs::write(&unit_path, unit).with_context(|| format!("write {}", unit_path.display()))?;
    fs::write(&env_path, env).with_context(|| format!("write {}", env_path.display()))?;
    // Restrict secret file permissions on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&env_path, fs::Permissions::from_mode(0o600));
    }

    systemctl(opts.system, &["daemon-reload"])?;
    systemctl(opts.system, &["enable", SERVICE_NAME])?;

    println!(
        "Installed systemd {} unit:",
        if opts.system { "system" } else { "user" }
    );
    println!("  unit:   {}", unit_path.display());
    println!("  env:    {}", env_path.display());
    println!("  binary: {}", bin.display());
    println!("  bind:   {}:{}", opts.bind, opts.port);
    println!(
        "  home:   {}  (default workspace; app can override per session)",
        opts.project.display()
    );
    println!();
    println!("Secret stored in env file (mode 0600):");
    println!("  {}", env_path.display());
    println!();
    println!("Phone setup:");
    println!("  1. navi server start");
    println!(
        "  2. In the app → Settings: host = this machine's Tailscale IP, port {}, secret from env file",
        opts.port
    );
    println!(
        "  3. Pick Home (agent) or a project path in the drawer — sent as projectDir on each session"
    );
    println!();
    println!(
        "Start with:  navi server start{}",
        if opts.system { " --system" } else { "" }
    );
    println!(
        "Status:      navi server status{}",
        if opts.system { " --system" } else { "" }
    );
    if !opts.system {
        println!();
        println!("Tip: keep the user service after logout:");
        println!("  loginctl enable-linger $USER");
    }
    Ok(())
}

fn uninstall(system: bool) -> Result<()> {
    let _ = systemctl(system, &["stop", SERVICE_NAME]);
    let _ = systemctl(system, &["disable", SERVICE_NAME]);

    let unit_path = unit_file_path(system)?;
    if unit_path.exists() {
        fs::remove_file(&unit_path).with_context(|| format!("remove {}", unit_path.display()))?;
        println!("Removed {}", unit_path.display());
    } else {
        println!("Unit not found at {}", unit_path.display());
    }

    systemctl(system, &["daemon-reload"])?;
    println!("Uninstalled {SERVICE_NAME}");
    println!(
        "Note: env file left in place (may contain secret): {}",
        env_file_path(system)?.display()
    );
    Ok(())
}

fn status(system: bool, loaded_config: &LoadedConfig) -> Result<()> {
    let unit_path = unit_file_path(system)?;
    let env_path = env_file_path(system)?;
    let bin = resolve_navi_server_bin();

    println!("NAVI remote server");
    println!("  scope:    {}", if system { "system" } else { "user" });
    println!(
        "  unit:     {} {}",
        unit_path.display(),
        if unit_path.exists() {
            "(installed)"
        } else {
            "(missing)"
        }
    );
    println!(
        "  env:      {} {}",
        env_path.display(),
        if env_path.exists() {
            "(present)"
        } else {
            "(missing)"
        }
    );
    match &bin {
        Some(p) => println!("  binary:   {}", p.display()),
        None => println!("  binary:   (not found on PATH / next to navi)"),
    }
    println!("  data_dir: {}", loaded_config.data_dir.display());
    println!();

    if unit_path.exists() {
        let _ = systemctl(system, &["--no-pager", "status", SERVICE_NAME]);
    } else {
        println!("Not installed. Run: navi server install");
    }
    Ok(())
}

fn logs(system: bool, follow: bool) -> Result<()> {
    let mut cmd = Command::new("journalctl");
    if !system {
        cmd.arg("--user");
    }
    cmd.arg("--no-pager");
    cmd.arg("-u").arg(SERVICE_NAME);
    if follow {
        cmd.arg("-f");
    } else {
        cmd.arg("-n").arg("80");
    }
    let status = cmd
        .status()
        .context("run journalctl (is systemd/journald available?)")?;
    if !status.success() {
        bail!("journalctl failed with {status}");
    }
    Ok(())
}

fn ensure_installed(system: bool) -> Result<()> {
    let unit = unit_file_path(system)?;
    if !unit.exists() {
        bail!(
            "service not installed (missing {}).\nRun: navi server install{}",
            unit.display(),
            if system { " --system" } else { "" }
        );
    }
    Ok(())
}

fn print_status_hint(system: bool) {
    let flag = if system { " --system" } else { "" };
    println!("Health: curl -sS http://127.0.0.1:9800/health  # port may differ");
    println!("Logs:   navi server logs{flag}");
    println!("Status: navi server status{flag}");
}

fn systemctl(system: bool, args: &[&str]) -> Result<()> {
    let mut cmd = Command::new("systemctl");
    if !system {
        cmd.arg("--user");
    }
    cmd.args(args);
    let status = cmd
        .status()
        .with_context(|| format!("systemctl {} (is systemd available?)", args.join(" ")))?;
    if !status.success() {
        // status subcommand returns non-zero when inactive — callers that need
        // strict success should check; for `status` we still print output.
        if args.contains(&"status") {
            return Ok(());
        }
        bail!("systemctl {} failed with {status}", args.join(" "));
    }
    Ok(())
}

fn unit_file_path(system: bool) -> Result<PathBuf> {
    if system {
        Ok(PathBuf::from("/etc/systemd/system").join(SERVICE_NAME))
    } else {
        let home = dirs_home().context("HOME not set")?;
        Ok(home.join(".config/systemd/user").join(SERVICE_NAME))
    }
}

fn env_file_path(system: bool) -> Result<PathBuf> {
    if system {
        Ok(PathBuf::from("/etc/navi/server.env"))
    } else {
        let home = dirs_home().context("HOME not set")?;
        Ok(home.join(".config/navi/server.env"))
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn resolve_navi_server_bin() -> Option<PathBuf> {
    // 1) Sibling of current navi binary (release/debug install layouts).
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join("navi-server");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // 2) PATH
    if let Ok(output) = Command::new("which").arg("navi-server").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            let p = PathBuf::from(path);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // 3) Common cargo targets relative to cwd
    for rel in ["target/release/navi-server", "target/debug/navi-server"] {
        let p = PathBuf::from(rel);
        if p.is_file() {
            return Some(p.canonicalize().unwrap_or(p));
        }
    }
    None
}

fn render_unit(
    bin: &Path,
    env_path: &Path,
    system: bool,
    bind: &str,
    port: u16,
    project: &Path,
) -> String {
    let wanted = if system {
        "multi-user.target"
    } else {
        "default.target"
    };
    format!(
        r#"[Unit]
Description=NAVI remote HTTP/WebSocket server
Documentation=https://github.com/navi-ai-org/navi
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=-{env}
ExecStart={bin} --bind {bind} --port {port} --project {project}
Restart=on-failure
RestartSec=3
# Soft limits — raise if needed for long sessions / browser tools
LimitNOFILE=65536

[Install]
WantedBy={wanted}
"#,
        env = env_path.display(),
        bin = bin.display(),
        bind = bind,
        port = port,
        project = project.display(),
        wanted = wanted,
    )
}

/// Provider / auth env vars to copy from the installer's shell into server.env
/// so the systemd unit sees the same keys as an interactive TUI session.
const PROVIDER_ENV_KEYS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENROUTER_API_KEY",
    "OPENCODE_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
    "DEEPSEEK_API_KEY",
    "MISTRAL_API_KEY",
    "CMD_API_KEY",
    "CHARM_HYPER_API_KEY",
    "NVIDIA_NIM_API_KEY",
    "GITHUB_COPILOT_TOKEN",
    "COHERE_API_KEY",
    "TOGETHER_API_KEY",
    "FIREWORKS_API_KEY",
    "PERPLEXITY_API_KEY",
    "ZAI_API_KEY",
    "MINIMAX_API_KEY",
];

fn render_env(secret: &str) -> String {
    let mut out = String::from(
        "# Generated by `navi server install` — do not commit.\n\
         # Auth header for clients: X-Navi-Secret\n\
         # Provider keys below were copied from the install shell so the gateway\n\
         # matches TUI credentials (systemd does not inherit your login env).\n",
    );
    out.push_str(&format!("NAVI_SERVER_SECRET={secret}\n"));
    out.push_str("RUST_LOG=navi_server=info,navi_sdk=info\n");
    out.push('\n');
    let mut any = false;
    for key in PROVIDER_ENV_KEYS {
        if let Ok(val) = std::env::var(key) {
            let val = val.trim();
            if val.is_empty() {
                continue;
            }
            // Basic shell-safe quoting for values with spaces/special chars.
            if val
                .chars()
                .any(|c| c.is_whitespace() || "\"'\\$".contains(c))
            {
                let escaped = val.replace('\\', "\\\\").replace('"', "\\\"");
                out.push_str(&format!("{key}=\"{escaped}\"\n"));
            } else {
                out.push_str(&format!("{key}={val}\n"));
            }
            any = true;
        }
    }
    if !any {
        out.push_str(
            "# (no provider API keys found in install environment)\n\
             # Add keys here or save them via `navi` TUI / app Credentials.\n",
        );
    }
    out
}

fn generate_secret() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Prefer getrandom via fastrand when available; mix time for uniqueness.
    format!("navi_{:x}_{:x}", fastrand::u128(..), t)
}
