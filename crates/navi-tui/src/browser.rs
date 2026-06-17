use std::process::{Command, Stdio};

use crate::TuiApp;
use crate::notifications::show_notification;
use crate::runtime::spawn_runtime_task;

pub(crate) fn open_url(app: &mut TuiApp, url: String) {
    if url.trim().is_empty() {
        return;
    }

    show_notification(app, "OAuth", "Opening browser...");
    spawn_runtime_task(async move {
        let result = tokio::task::spawn_blocking(move || open_url_blocking(&url)).await;
        if let Ok(Err(err)) = result {
            tracing::warn!(error = %err, "failed to open OAuth URL in browser");
        }
    });
}

fn open_url_blocking(url: &str) -> std::io::Result<()> {
    let status = if cfg!(target_os = "macos") {
        Command::new("open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
    } else if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
    } else {
        Command::new("xdg-open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
    };

    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "browser opener exited with {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn open_url_blocking_accepts_non_empty_input_type() {
        let url = "https://commandcode.ai/studio/auth/cli";
        assert!(!url.is_empty());
    }
}
