# Contributing to NAVI

Thank you for your interest in contributing to NAVI! This document provides guidelines and information for contributors.

## Getting Started

### Prerequisites

- Rust 1.85+ (edition 2024)
- Git

### Building

```bash
cargo build
```

### Running

```bash
# Interactive TUI
cargo run -p navi-cli -- "explain this codebase"

# Headless mode
cargo run -p navi-cli -- --no-tui "write a hello world in Rust"

# Inspect config/providers
cargo run -p navi-cli -- --print-config
cargo run -p navi-cli -- --print-providers
```

## Development Workflow

### 1. Fork and Clone

```bash
git clone https://github.com/your-username/navi.git
cd navi
```

### 2. Create a Branch

```bash
git checkout -b feature/your-feature-name
```

### 3. Make Changes

- Follow existing code conventions
- Add tests for new functionality
- Update documentation if needed

### 4. Verify Your Changes

```bash
# Format code
cargo fmt

# Check for errors
cargo check --all-targets

# Run tests (with resource limits)
CARGO_TEST_THREADS=4 cargo test

# Run clippy
cargo clippy --all-targets
```

### 5. Commit and Push

```bash
git add .
git commit -m "feat: describe your change"
git push origin feature/your-feature-name
```

### 6. Open a Pull Request

- Describe what your PR does
- Reference any related issues
- Wait for CI checks to pass

## Code Style

### Rust

- Use `cargo fmt` to format code
- Follow existing naming conventions
- Prefer `?` and `.context()` over `.unwrap()` in production code
- Use `#[allow(dead_code)]` sparingly and only with justification

### Error Handling

- Use `anyhow` for application errors
- Use `thiserror` for library errors
- Provide meaningful error messages with context

### Testing

- Write focused unit tests for new functionality
- Use `#[tokio::test]` for async tests
- Respect resource limits: `CARGO_TEST_THREADS=4`
- Test edge cases and error conditions

## Project Structure

```
navi/
├── crates/
│   ├── navi-cli/          # Entry binary, CLI parsing
│   ├── navi-core/         # Core domain: config, tools, security, runtime
│   ├── navi-mcp/          # MCP client integration
│   ├── navi-openai/       # OpenAI-compatible provider implementation
│   ├── navi-plugin-api/   # Plugin trait and API version
│   ├── navi-plugin-host/  # Native library loading
│   ├── navi-providers/    # Provider facade
│   ├── navi-sdk/          # Public embedding facade
│   └── navi-tui/          # Terminal UI
├── docs/                  # Technical documentation
├── AGENTS.md              # Agent development guide
└── README.md              # Project overview
```

## Documentation

- **README.md** — User-facing overview, quickstart, configuration
- **docs/architecture.md** — Crate boundaries and runtime flow
- **docs/providers.md** — Provider protocols and configuration
- **docs/tools-security.md** — Built-in tools and security policy
- **docs/tui.md** — TUI state, keybindings, rendering
- **AGENTS.md** — Agent development guide

## Reporting Issues

When reporting issues, please include:

- Steps to reproduce
- Expected behavior
- Actual behavior
- Your environment (OS, Rust version)
- Relevant logs (use `navi --print-log-path` to find them)

## Security

- Never commit secrets or API keys
- Use environment variables for sensitive configuration
- Report security vulnerabilities privately

## License

By contributing to NAVI, you agree that your contributions will be licensed under the MIT License.

## Questions?

If you have questions about contributing, feel free to open an issue or reach out to the maintainers.
