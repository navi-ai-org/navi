use crate::config::LoggingConfig;
use crate::security::redact_secrets;
use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

/// Guard that keeps the tracing file appender alive. Dropping this stops logging.
pub struct LoggingGuard {
    path: Option<PathBuf>,
    _guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// Runtime logging configuration that can be adjusted without restarting.
#[derive(Debug, Clone)]
pub struct LoggingRuntimeConfig {
    /// Whether to log to stdout.
    pub stdout_enabled: bool,
    /// Whether to log to a file.
    pub file_enabled: bool,
    /// Override log level filter, or `None` to keep the current level.
    pub level: Option<String>,
    /// Whether to include raw payloads in log output.
    pub include_payloads: bool,
}

impl LoggingGuard {
    /// Returns the path to the log file, if file logging is active.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl Default for LoggingRuntimeConfig {
    fn default() -> Self {
        Self {
            stdout_enabled: false,
            file_enabled: true,
            level: None,
            include_payloads: false,
        }
    }
}

/// Returns the log directory path: `<data_dir>/logs/`.
pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// Returns the log file path: `<data_dir>/logs/navi.log`.
pub fn log_path(data_dir: &Path) -> PathBuf {
    log_dir(data_dir).join("navi.log")
}

/// Initializes the tracing subscriber with file and/or stdout layers based on
/// the logging config. Returns a [`LoggingGuard`] that must be kept alive.
pub fn init_logging(
    config: &LoggingConfig,
    data_dir: &Path,
    runtime: LoggingRuntimeConfig,
) -> Result<LoggingGuard> {
    if !config.enabled {
        return Ok(LoggingGuard {
            path: None,
            _guard: None,
        });
    }

    let level = runtime.level.unwrap_or_else(|| config.level.clone());
    let filter =
        EnvFilter::try_new(level.clone()).unwrap_or_else(|_| EnvFilter::new("navi=info,info"));
    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::new();
    let mut guard = None;
    let mut path = None;

    if config.file_enabled && runtime.file_enabled {
        let dir = log_dir(data_dir);
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        crate::fs_util::set_private_dir_permissions(&dir)?;
        cleanup_old_logs(&dir, config.max_files)?;

        let path_for_writer = log_path(data_dir);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path_for_writer)
            .with_context(|| format!("failed to open {}", path_for_writer.display()))?;
        crate::fs_util::set_private_file_permissions(&path_for_writer)?;
        let (writer, writer_guard) = tracing_appender::non_blocking(file);
        layers.push(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_writer(writer)
                .boxed(),
        );
        guard = Some(writer_guard);
        path = Some(path_for_writer);
    }

    if config.stdout_enabled || runtime.stdout_enabled {
        layers.push(
            tracing_subscriber::fmt::layer()
                .with_ansi(true)
                .with_target(true)
                .boxed(),
        );
    }

    if layers.is_empty() {
        return Ok(LoggingGuard {
            path: None,
            _guard: None,
        });
    }

    let subscriber = Registry::default().with(layers).with(filter);
    let _ = tracing::subscriber::set_global_default(subscriber);

    tracing::info!(
        log_path = path.as_ref().map(|p| p.display().to_string()),
        level,
        include_payloads = config.include_payloads || runtime.include_payloads,
        "logging initialized"
    );

    Ok(LoggingGuard {
        path,
        _guard: guard,
    })
}

/// Redacts secrets from a log value string.
pub fn redact_log_value(value: impl AsRef<str>) -> String {
    redact_secrets(value.as_ref())
}

fn cleanup_old_logs(dir: &Path, max_files: usize) -> Result<()> {
    if max_files == 0 {
        return Ok(());
    }
    let mut logs = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) != Some("navi.log") {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        logs.push((modified, path));
    }
    logs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in logs.into_iter().skip(max_files) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_lives_under_data_dir() {
        let path = log_path(Path::new("/tmp/navi-data"));
        assert_eq!(path, PathBuf::from("/tmp/navi-data/logs/navi.log"));
    }

    #[test]
    fn redacts_secret_values_for_logs() {
        assert_eq!(
            redact_log_value("OPENAI_API_KEY=sk-proj-1234567890abcdef"),
            "OPENAI_API_KEY=<redacted>"
        );
    }

    #[cfg(unix)]
    #[test]
    fn init_logging_creates_private_log_file() {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let config = LoggingConfig::default();
        let guard = init_logging(
            &config,
            tempdir.path(),
            LoggingRuntimeConfig {
                stdout_enabled: false,
                file_enabled: true,
                level: Some("info".to_string()),
                include_payloads: false,
            },
        )
        .expect("init logging");
        drop(guard);

        let dir_mode = fs::metadata(log_dir(tempdir.path()))
            .expect("dir metadata")
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(log_path(tempdir.path()))
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }
}
