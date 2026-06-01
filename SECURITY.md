# Security Policy

## Supported Versions

NAVI is currently pre-1.0. Security fixes are applied to the `main` branch.

## Reporting a Vulnerability

Please do not report security vulnerabilities in public issues.

Report vulnerabilities privately to the maintainers. Include as much detail as you can safely share:

- affected commit or version
- operating system and Rust version
- steps to reproduce
- expected and actual behavior
- impact assessment
- relevant logs with secrets removed

If you are unsure whether something is a security issue, report it privately first.

## Scope

Security-sensitive areas include:

- provider credentials and credential storage
- tool execution, command approval, and blocked commands
- filesystem path restrictions and `.git` protection
- plugin loading and native library trust boundaries
- MCP server configuration and tool registration
- session persistence and secret redaction
- provider request/response logging

## Handling Secrets

Never include API keys, OAuth tokens, bearer tokens, private keys, or full session transcripts in public issues, pull requests, or logs.

NAVI resolves credentials from environment variables first, then from its credential store. Session persistence and diagnostics are intended to redact likely secrets, but reporters should still review all shared output manually.

## Disclosure

The maintainers will acknowledge valid reports, investigate impact, and coordinate a fix before public disclosure when appropriate.
