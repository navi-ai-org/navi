## Highlights

**0.6.0** renames the `bash` tool to `run` with dynamic shell detection, per-shell syntax descriptions, and a new `[shell]` config section. The tool now auto-detects the user's preferred shell (bash, zsh, pwsh, PowerShell 5.1, nu, cmd, fish) and generates a description that tells the model which syntax to write — preventing the common bug where the model wrote bash syntax for PowerShell.

Full changelog: https://github.com/navi-ai-org/navi/compare/v0.5.0...v0.6.0

### Breaking

- **`bash` tool renamed to `run`** — the tool name, struct (`RunTool`), file
  (`run.rs`), and all 44 files referencing the tool name have been updated
  across `navi-core`, `navi-tui`, `navi-openai`, `navi-cli`, `navi-sdk`,
  `navi-lite`, `navi-plugin-broker`, and `navi-plugin-manifest`. Any code or
  config referencing the tool name `"bash"` must be updated to `"run"`.

### Changed

- **Dynamic shell detection** — the `run` tool now dynamically detects the
  user's shell instead of hardcoding PowerShell on Windows / bash on Unix.
  Resolution order: `[shell].program` in config.toml → `NAVI_SHELL` env →
  `SHELL` env → `NAVI_BASH_SHELL` env (legacy) → platform default (bash on
  Unix, pwsh→powershell on Windows).
- **Dynamic tool descriptions per shell** — the tool description is generated
  dynamically per shell so the model knows which syntax to write (`$VAR` vs
  `$env:VAR` vs `%VAR%`, etc.), preventing the bug where the model wrote bash
  syntax for PowerShell.
- **`ShellKind` expanded** from 3 to 8 variants: Bash, Zsh, Pwsh, PowerShell5,
  Nu, Cmd, Fish, Unknown — each with correct argv prefixes and syntax hints.

### Added

- **`[shell]` config section** — new `[shell]` section in `config.toml` with
  `program` and `args` fields to pin a specific shell (e.g.
  `program = "nu"`, `args = ["-c"]`).
- **`ShellConfig` struct** in `config/types.rs` with serde defaults.
- **`RunTool::with_shell_config()`** constructor for config injection.
- **Comprehensive Layer 1/2/3 tests** — ShellKind::from_program_name (all
  names + .exe), argv_prefix (all variants), shell_description (all 8
  variants), detect_shell_kind_with, shell_argv_prefix with config override,
  edge cases (empty/whitespace/unicode/long paths), and integration tests for
  simple commands, shell identity, PowerShell env var syntax, bash syntax,
  empty command error, and stderr handling.

### Fixed

- **Background task output race** — added 50ms drain delay after process exit
  so the output reader finishes consuming the pipe before snapshotting.
- **Cross-platform cancel test** — use `Start-Sleep -Seconds 5` on Windows
  instead of `sleep 5` (which doesn't exist in PowerShell).
- **SHELL env var on Windows** — now goes through `resolve_shell_path` so
  `SHELL=bash` correctly finds Git Bash via well-known installation paths.
- **Unknown shell description** — now conservative, advises simple commands
  instead of defaulting to POSIX syntax that may not work.
- **Sudo wrapper limitation documented** — `export -f` works with bash/zsh but
  not fish; sudo is Unix-only.

### Bindings

- `@navi-agent/napi` **0.6.0** and platform packages
- `@navi-agent/navi` **0.6.0** CLI packages
- Workspace crate versions bumped to **0.6.0**

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh -s -- --version 0.6.0
```

```bash
npm install -g @navi-agent/navi@0.6.0
npm install @navi-agent/napi@0.6.0
```

## Changelog

- Tag range: https://github.com/navi-ai-org/navi/compare/v0.5.0...v0.6.0
- See [CHANGELOG.md](https://github.com/navi-ai-org/navi/blob/v0.6.0/CHANGELOG.md)
