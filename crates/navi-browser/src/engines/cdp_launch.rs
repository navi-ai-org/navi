// Discover and launch Chromium-compatible browsers for CDP.

use anyhow::{Context, Result, bail};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserBackendKind {
    CloakBrowser,
    Chrome,
    Chromium,
    Custom,
}

#[derive(Debug, Clone)]
pub struct BrowserBinary {
    pub path: PathBuf,
    pub kind: BrowserBackendKind,
}

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub headless: bool,
    pub user_data_dir: PathBuf,
    pub proxy: Option<String>,
    /// Extra CLI args (e.g. Cloak fingerprint flags).
    pub extra_args: Vec<String>,
}

#[derive(Debug)]
pub struct LaunchedBrowser {
    pub child: Child,
    pub debug_port: u16,
    pub binary: BrowserBinary,
    pub user_data_dir: PathBuf,
}

impl Drop for LaunchedBrowser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Find a browser binary: CloakBrowser first, then Chrome/Chromium.
pub fn discover_browser(
    cloak_binary_path: Option<&Path>,
    preferred: &str,
) -> Option<BrowserBinary> {
    let preferred = preferred.trim().to_ascii_lowercase();
    match preferred.as_str() {
        "chrome" => find_system_chrome(),
        "chromium" => find_system_chromium().or_else(find_system_chrome),
        "cloakbrowser" => find_cloakbrowser(cloak_binary_path),
        _ => find_cloakbrowser(cloak_binary_path)
            .or_else(find_system_chrome)
            .or_else(find_system_chromium),
    }
}

fn find_cloakbrowser(explicit: Option<&Path>) -> Option<BrowserBinary> {
    if let Some(p) = explicit {
        if p.is_file() {
            return Some(BrowserBinary {
                path: p.to_path_buf(),
                kind: BrowserBackendKind::CloakBrowser,
            });
        }
    }
    if let Ok(env) = std::env::var("CLOAKBROWSER_BINARY_PATH") {
        let p = PathBuf::from(env);
        if p.is_file() {
            return Some(BrowserBinary {
                path: p,
                kind: BrowserBackendKind::CloakBrowser,
            });
        }
    }
    // Common CloakBrowser cache layouts (~/.cloakbrowser/chromium-*/...)
    let home = dirs_home()?;
    let cache = home.join(".cloakbrowser");
    if cache.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&cache) {
            let mut candidates: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("chromium"))
                })
                .collect();
            candidates.sort();
            candidates.reverse();
            for dir in candidates {
                for name in [
                    "chrome",
                    "chrome.exe",
                    "Chromium",
                    "Chromium.app/Contents/MacOS/Chromium",
                ] {
                    let bin = dir.join(name);
                    if bin.is_file() {
                        return Some(BrowserBinary {
                            path: bin,
                            kind: BrowserBackendKind::CloakBrowser,
                        });
                    }
                }
            }
        }
    }
    None
}

fn find_system_chrome() -> Option<BrowserBinary> {
    for candidate in chrome_candidates() {
        if which_exists(&candidate) {
            return Some(BrowserBinary {
                path: PathBuf::from(candidate),
                kind: BrowserBackendKind::Chrome,
            });
        }
    }
    None
}

fn find_system_chromium() -> Option<BrowserBinary> {
    for candidate in ["chromium", "chromium-browser", "/usr/bin/chromium"] {
        if which_exists(candidate) {
            return Some(BrowserBinary {
                path: PathBuf::from(candidate),
                kind: BrowserBackendKind::Chromium,
            });
        }
    }
    None
}

fn chrome_candidates() -> Vec<&'static str> {
    #[cfg(target_os = "macos")]
    {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "google-chrome",
            "chrome",
        ]
    }
    #[cfg(target_os = "windows")]
    {
        vec![
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            "chrome.exe",
        ]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec![
            "google-chrome",
            "google-chrome-stable",
            "chrome",
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
        ]
    }
}

fn which_exists(cmd: &str) -> bool {
    let path = Path::new(cmd);
    if path.is_absolute() || cmd.contains('/') || cmd.contains('\\') {
        return path.is_file();
    }
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind ephemeral port")?;
    Ok(listener.local_addr()?.port())
}

/// Launch browser with remote debugging enabled.
pub async fn launch_browser(binary: &BrowserBinary, opts: LaunchOptions) -> Result<LaunchedBrowser> {
    std::fs::create_dir_all(&opts.user_data_dir).context("create browser user-data-dir")?;
    let port = pick_free_port()?;

    let mut args = vec![
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={}", opts.user_data_dir.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-background-networking".into(),
        "--disable-sync".into(),
        "--disable-extensions".into(),
        "--disable-component-update".into(),
        "about:blank".into(),
    ];
    if opts.headless {
        args.push("--headless=new".into());
        args.push("--disable-gpu".into());
    }
    if let Some(proxy) = &opts.proxy {
        if !proxy.trim().is_empty() {
            args.push(format!("--proxy-server={}", proxy.trim()));
        }
    }
    args.extend(opts.extra_args.iter().cloned());

    let mut cmd = Command::new(&binary.path);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to launch browser binary {}", binary.path.display()))?;

    // Wait until CDP HTTP is up.
    let http = format!("http://127.0.0.1:{port}/json/version");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let mut ready = false;
    for _ in 0..50 {
        if let Ok(resp) = client.get(&http).send().await {
            if resp.status().is_success() {
                ready = true;
                break;
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        bail!(
            "browser CDP did not become ready on port {port} (binary: {})",
            binary.path.display()
        );
    }

    Ok(LaunchedBrowser {
        child,
        debug_port: port,
        binary: binary.clone(),
        user_data_dir: opts.user_data_dir,
    })
}

/// Probe whether CDP is reachable at a base URL (e.g. `http://127.0.0.1:9222`).
pub async fn cdp_http_ready(cdp_http_base: &str) -> bool {
    let base = cdp_http_base.trim_end_matches('/');
    let url = format!("{base}/json/version");
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}
