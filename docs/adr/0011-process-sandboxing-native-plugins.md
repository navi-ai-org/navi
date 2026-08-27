# ADR 0011 — Process Sandboxing for Native Plugins

## Status
Accepted

## Context
Native plugins (`Core` and `LocalDev` trust levels) run in the host process with
full memory access. While the host broker layer mediates API calls, a native plugin
can bypass userspace security by direct syscalls. Process-level sandboxing restricts
filesystem access at the kernel level, reducing the blast radius of a compromised
native plugin.

## Decision
On Linux, native plugin loading applies Landlock filesystem sandboxing to restrict
the process to a defined set of allowed paths. On other platforms, sandboxing is
documented but not enforced at the OS level.

### Landlock Implementation

The sandbox (`apply_filesystem_sandbox`) creates a Landlock ruleset (ABI v5) that
restricts filesystem access to:

**User-specified paths:**
- Project root
- Data directory
- Plugin's own directory

**System paths (auto-included):**
- `/usr`
- `/lib`, `/lib64`
- `/bin`
- `/etc/ld.so.cache`

All other filesystem paths are denied by the kernel after `restrict_self()` is called.

### Sandbox Status

The sandbox returns one of three outcomes:

| Status | Meaning |
|--------|---------|
| `Active` | Sandbox applied successfully, all paths accepted |
| `ActiveWithWarnings` | Sandbox applied but some paths were rejected |
| `Unavailable(reason)` | Sandbox could not be applied |

`Unavailable` reasons include:
- Linux kernel < 5.13 (Landlock not supported)
- `landlock` feature not compiled in
- Landlock setup failed (permission error, seccomp interference)

### Platform Limitations

| Platform | Sandboxing | Notes |
|----------|-----------|-------|
| Linux >= 5.13 | Landlock (kernel-enforced) | Best case — filesystem restriction at syscall level |
| Linux < 5.13 | Not available | Returns `Unavailable` |
| macOS | Not implemented | Recommends caller wrap invocation in a sandbox profile |
| Other | No-op | Returns `Unavailable` |

### Post-Load Restriction

Landlock is applied **after** the native library is loaded (`libloading`). This means
the plugin's initialization code runs before the sandbox is active. This is an
accepted limitation — the sandbox protects against persistent filesystem abuse during
the plugin's runtime, not against malicious initialization.

Future improvement: load the plugin in a pre-sandboxed subprocess (ADR 0012 covers
the dual execution path model).

## Consequences
Positive:
- Kernel-enforced filesystem restriction on Linux
- Defense-in-depth: even if host broker is bypassed, the kernel blocks unauthorized paths
- Graceful degradation: `Unavailable` does not prevent plugin loading
- System paths are auto-included so standard library loading works

Negative:
- Linux-only (and requires kernel >= 5.13)
- Post-load application means initialization code is unsandboxed
- No network sandboxing (Landlock ABI v5 does not cover network)
- macOS and other platforms rely on userspace security only
- `ActiveWithWarnings` status may confuse users about actual protection level
