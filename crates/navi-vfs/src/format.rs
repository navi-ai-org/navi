use anyhow::Result;
use std::path::Path;

use crate::lang::LangId;

/// Run the appropriate formatter on a file after VFS write.
///
/// Formatters are language-specific external tools that restore readable
/// formatting to the minified code written by the LLM.
pub fn format_file(path: &Path, lang: LangId) -> Result<()> {
    let spec = formatter_spec(lang);

    tracing::debug!(
        lang = %lang.name(),
        command = %spec.command,
        path = %path.display(),
        "running formatter"
    );

    let mut cmd = std::process::Command::new(spec.command);
    cmd.args(spec.args);
    cmd.arg(path);

    let output = cmd.output().map_err(|e| {
        anyhow::anyhow!(
            "formatter `{}` not found or failed to run: {e}",
            spec.command
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "formatter `{}` failed for {}: {}",
            spec.command,
            path.display(),
            stderr.trim()
        );
    }

    Ok(())
}

/// Run a formatter on multiple files (e.g., after apply_patch).
pub fn format_files(paths: &[&Path], lang: LangId) -> Vec<(String, Result<()>)> {
    paths
        .iter()
        .map(|p| {
            let result = format_file(p, lang);
            (p.display().to_string(), result)
        })
        .collect()
}

struct FormatterSpec {
    command: &'static str,
    args: &'static [&'static str],
}

fn formatter_spec(lang: LangId) -> FormatterSpec {
    match lang {
        // Tier 1
        LangId::Rust => FormatterSpec {
            command: "rustfmt",
            args: &[],
        },
        LangId::Go => FormatterSpec {
            command: "gofmt",
            args: &["-w"],
        },
        LangId::C | LangId::Cpp => FormatterSpec {
            command: "clang-format",
            args: &["-i", "--style=file"],
        },
        LangId::Python => FormatterSpec {
            command: "ruff",
            args: &["format", "--quiet"],
        },
        LangId::JavaScript | LangId::TypeScript => FormatterSpec {
            command: "biome",
            args: &["format", "--write", "--no-errors-on-unmatched"],
        },
        LangId::Java => FormatterSpec {
            command: "google-java-format",
            args: &["-i"],
        },
        // Tier 2
        LangId::Ruby => FormatterSpec {
            command: "rubocop",
            args: &["-A", "--format", "quiet"],
        },
        LangId::Php => FormatterSpec {
            command: "php-cs-fixer",
            args: &["fix", "--quiet"],
        },
        LangId::Bash => FormatterSpec {
            command: "shfmt",
            args: &["-w"],
        },
        LangId::Html => FormatterSpec {
            command: "prettier",
            args: &["--write", "--parser", "html"],
        },
        LangId::Css => FormatterSpec {
            command: "prettier",
            args: &["--write", "--parser", "css"],
        },
        LangId::Json => FormatterSpec {
            command: "prettier",
            args: &["--write", "--parser", "json"],
        },
        LangId::CSharp => FormatterSpec {
            command: "dotnet-format",
            args: &[],
        },
    }
}

/// Check if the formatter for a given language is available in PATH.
pub fn formatter_available(lang: LangId) -> bool {
    let spec = formatter_spec(lang);
    which::which(spec.command).is_some()
}

/// Simple `which` implementation — checks if a program exists in PATH.
mod which {
    use std::path::PathBuf;

    pub fn which(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let full = dir.join(name);
            if full.is_file() {
                return Some(full);
            }
            #[cfg(windows)]
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = full.with_extension(ext);
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatter_spec_returns_correct_command() {
        // Tier 1
        assert_eq!(formatter_spec(LangId::Rust).command, "rustfmt");
        assert_eq!(formatter_spec(LangId::Go).command, "gofmt");
        assert_eq!(formatter_spec(LangId::C).command, "clang-format");
        assert_eq!(formatter_spec(LangId::Python).command, "ruff");
        assert_eq!(formatter_spec(LangId::JavaScript).command, "biome");
        assert_eq!(formatter_spec(LangId::TypeScript).command, "biome");
        assert_eq!(formatter_spec(LangId::Java).command, "google-java-format");
        // Tier 2
        assert_eq!(formatter_spec(LangId::Ruby).command, "rubocop");
        assert_eq!(formatter_spec(LangId::Php).command, "php-cs-fixer");
        assert_eq!(formatter_spec(LangId::Bash).command, "shfmt");
        assert_eq!(formatter_spec(LangId::Html).command, "prettier");
        assert_eq!(formatter_spec(LangId::Css).command, "prettier");
        assert_eq!(formatter_spec(LangId::Json).command, "prettier");
        assert_eq!(formatter_spec(LangId::CSharp).command, "dotnet-format");
    }
}
