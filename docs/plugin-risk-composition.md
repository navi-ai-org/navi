# NAVI Plugin Risk Composition

## Principle

A capability can be safe in isolation and dangerous in combination. If a plugin
can read X and send data to Y, it can exfiltrate X to Y. The system MUST
classify, warn, log, and restrict dangerous combinations.

Risk MUST be evaluated at the tool level, not the plugin level. A plugin that
contains ten tools with LOW risk is not itself HIGH risk. A single tool that
combines file read and network POST is CRITICAL regardless of the other tools
in the plugin.

## Single Capability Risk

| Capability | Risk | Reason |
|---|---|---|
| `filesystem: project/read-only` | MEDIUM | Can read source code, configs, secrets in project |
| `filesystem: project/read-write` | HIGH | Can modify project files, inject code |
| `network: specific hosts, GET` | MEDIUM | Can fetch external data, limited exfiltration |
| `network: specific hosts, POST` | HIGH | Can send data out to declared hosts |
| `network: wildcard` | CRITICAL | Can send data to any host |
| `tui: passive widget` | LOW | Display only, no data exfiltration path |
| `tui: interactive widget` | MEDIUM | Can receive user input, phishing vector |
| `git: read` | LOW | Status and diff only, no modification |
| `auth_binding` | MEDIUM | Secret injected by host, can authenticate requests |
| `shell` | FORBIDDEN | Full command execution, unrestricted |
| `process` | FORBIDDEN | Can spawn arbitrary processes |
| `env` (raw) | FORBIDDEN | Can read all environment variables including secrets |
| `model inject` | FORBIDDEN | Can manipulate agent behavior and context |

## Compound Risk Table

| Combination | Risk | Warning Text |
|---|---|---|
| `fs_read` + `network_GET` | HIGH | "This tool can read project files and send data to external servers. This enables data exfiltration." |
| `fs_read` + `network_POST` | CRITICAL | "CRITICAL: This tool can read project files and POST data to external servers. High risk of data exfiltration." |
| `fs_read` + `auth_binding` | HIGH | "This tool can read project files and has authenticated access to external services." |
| `fs_read` + `auth_binding` + `POST` | CRITICAL | "CRITICAL: This tool can read files, authenticate to services, and send data. Very high exfiltration risk." |
| `write` + `network` | CRITICAL | "CRITICAL: This tool can write files and access the network. Could write malicious content." |
| `fs_read` + `network_wildcard` | FORBIDDEN | Community plugins cannot combine read access with wildcard network. |

## Per-Tool Analysis

Risk MUST be computed per tool, not per plugin.

A plugin with two tools:

```toml
[[tools]]
id = "search_docs"
capabilities = ["fs_read"]  # MEDIUM

[[tools]]
id = "web_search"
capabilities = ["net_search"]  # MEDIUM
```

Is SAFER than a single tool with both:

```toml
[[tools]]
id = "check_config"
capabilities = ["fs_read", "net_api"]  # HIGH (compound)
```

The host MUST NOT elevate the risk of `search_docs` because `web_search`
exists in the same plugin. Each tool MUST be evaluated independently against
the compound risk table.

## Installation UI Requirements

### MEDIUM Risk Tools

For tools with MEDIUM risk the host MUST display a checkmark:

```
✓ filesystem: project (read-only)
```

No warning text is required. The capability is listed for transparency.

### HIGH Risk Tools

For tools with HIGH risk the host MUST display a warning:

```
⚠ HIGH RISK: This tool can read project files AND send data
  to external servers. This combination enables data exfiltration.
  ✓ filesystem: project (read-only)
  ⚠ network: api.example.com (GET, POST)
```

The user MUST acknowledge the warning before proceeding. The host MUST NOT
auto-approve HIGH risk tools.

### CRITICAL Risk Tools

For tools with CRITICAL risk the host MUST display a critical warning:

```
🔴 CRITICAL RISK: This tool can read files, authenticate to
  external services, and send data. Very high exfiltration risk.
  ✓ filesystem: project (read-only)
  🔴 network: api.example.com (POST)
  🔴 auth: github_token → Authorization header
```

The user MUST explicitly approve each CRITICAL capability. A single blanket
approval MUST NOT suffice; each CRITICAL capability requires individual
confirmation.

## Risk Classifier Implementation

The risk classifier MUST be a pure function:

- Input: list of capabilities declared for a single tool.
- Output: risk level (`LOW`, `MEDIUM`, `HIGH`, `CRITICAL`, `FORBIDDEN`) and
  warning text.

The classifier MUST:

1. Check each capability against the single-capability risk table.
2. Check all pairs of capabilities against the compound risk table.
3. Check all triples of capabilities against the compound risk table.
4. Return the highest risk level found.
5. Return the warning text associated with the highest-risk combination.

The classifier MUST NOT:

- Rely on plugin metadata beyond the capability list.
- Cache results across invocations with different capability sets.
- Return a risk level lower than the highest single-capability risk.

### Test Requirements

The risk classifier MUST be tested with all combinations in the red-team suite.
The red-team suite MUST cover:

- Every single capability in isolation.
- Every pair in the compound risk table.
- Every triple in the compound risk table.
- Empty capability lists (MUST return `LOW`).
- Unknown capabilities (MUST return `FORBIDDEN`).

## Escalation Paths

If a tool's risk level is `FORBIDDEN` for the current trust level:

- The host MUST reject the tool at manifest validation time.
- The host MUST NOT load the plugin containing the tool.
- The host MUST display an error message listing the forbidden capabilities.

If a tool's risk level is `CRITICAL`:

- The host MUST require explicit user approval at install time.
- The host MUST log every invocation.
- The host MUST NOT allow auto-approval in any mode.

If a tool's risk level is `HIGH`:

- The host MUST require user acknowledgment at install time.
- The host SHOULD log every invocation.
- The host MAY allow session-granted approval.

## Risk Scoring Methodology

The risk score for a tool MUST be computed as follows:

1. Start with a base score of 0.
2. For each single capability, add the capability's score:

| Risk Level | Score |
|---|---|
| LOW | 1 |
| MEDIUM | 2 |
| HIGH | 4 |
| CRITICAL | 8 |
| FORBIDDEN | 16 |

3. For each compound pair present in the tool's capability set, apply the
   compound rule. The compound risk level OVERRIDES the maximum of the
   individual scores if it is higher.
4. The final risk level is the highest level among all single capabilities
   and all compound combinations.

Example:

```
Tool: check_config
Capabilities: [fs_read, network_POST]

Single: fs_read = MEDIUM (2), network_POST = HIGH (4)
Compound: fs_read + network_POST = CRITICAL (8)

Final: CRITICAL
```

The host MUST NOT round down. If any combination yields CRITICAL, the tool
is CRITICAL.

## Logging Requirements

Every risk classification decision MUST be logged. The log entry MUST contain:

| Field | Description |
|---|---|
| `plugin_id` | ID of the plugin being evaluated |
| `tool_id` | ID of the tool being evaluated |
| `capabilities` | Full list of capabilities for the tool |
| `single_risks` | Risk level for each individual capability |
| `compound_risks` | Risk level for each compound combination |
| `final_risk` | The computed final risk level |
| `warning_text` | The warning text shown to the user |

Logs MUST be written at `debug` level during classification and at `info`
level when the risk level is HIGH or above.

## Edge Cases

### Empty Capability List

A tool with no capabilities MUST be classified as LOW risk. The host MUST
allow such tools without prompting.

### Unknown Capabilities

A tool that declares a capability not in the risk table MUST be classified
as FORBIDDEN. The host MUST reject the tool and log the unknown capability.

### Duplicate Capabilities

A tool that declares the same capability multiple times MUST be treated as
declaring it once. Duplicates MUST NOT inflate the risk level.

### Capability Removal

If a tool's risk decreases due to capability removal during an update:

- The host SHOULD inform the user that the tool's risk has decreased.
- The host MUST NOT require reconsent for risk decreases.
- The host MUST update the stored risk level.

## Risk Level Transitions

Risk levels MUST NOT decrease during a session. If a plugin requests additional
capabilities at runtime:

- The host MUST recompute the risk level with the new capabilities.
- If the risk level increases, the host MUST require reconsent.
- If the new risk level is FORBIDDEN for the trust level, the host MUST deny
  the request.
