# ADR 0008 — Plugin Signing and Verification

## Status
Accepted

## Context
Community and signed plugins must be tamper-proof. A plugin downloaded from a registry
could be modified in transit or replaced with a malicious version. The system needs
cryptographic verification that the WASM binary, capability declarations, and tool
definitions have not been altered since the publisher signed them.

## Decision
Plugins at `Community` and `Signed` trust levels MUST be verified using Ed25519
signatures over a deterministic hash bundle. `LocalDev` trust level skips verification.

### Hash Bundle

The signature covers a 96-byte message formed by concatenating three SHA-256 digests:

```
message = wasm_hash_bytes ++ capabilities_hash_bytes ++ tools_hash_bytes
```

Each hash is computed as `sha256(<canonical content>)` and encoded as `sha256:<hex>`.

| Hash | Covers |
|------|--------|
| `wasm_hash` | Raw `.wasm` binary bytes |
| `capabilities_hash` | Canonical TOML serialization of the `[[capabilities]]` array |
| `tools_hash` | Canonical TOML serialization of the `[[tools]]` array |

This means a signature is invalidated if the WASM binary, any capability declaration,
or any tool definition is modified.

### Signature Format

Signatures and public keys use the format `ed25519:<base64>`.

- Signature: 64 bytes Ed25519, base64-encoded with `ed25519:` prefix
- Public key: 32 bytes Ed25519 verifying key, base64-encoded with `ed25519:` prefix

### Verification Flow

1. Load the plugin manifest (`plugin.toml`).
2. Verify `wasm_hash` matches the actual `.wasm` binary (`verify_wasm_hash`).
3. Compute `capabilities_hash` and `tools_hash` from the manifest.
4. Parse the signature and public key from the manifest.
5. Build the 96-byte hash bundle message.
6. Verify the Ed25519 signature over the message.
7. If any step fails, reject the plugin.

### Key Management

- Plugin publishers generate an Ed25519 keypair.
- The public key is embedded in the manifest (`plugin.public_key`).
- The private key is used to sign the hash bundle (`plugin.signature`).
- NAVI does not manage a PKI or trust anchor — trust is derived from the registry
  (marketplace) or from the user's explicit approval at install time.
- A deterministic test keypair (`[0x42; 32]`) exists for unit tests and local fixtures.
  It is NOT used in production.

### Trust Level Behavior

| Trust Level | Signature Check | WASM Hash Check |
|-------------|----------------|-----------------|
| `Core` | N/A (bundled) | N/A |
| `Signed` | Required | Required |
| `Community` | Required | Required |
| `LocalDev` | Skipped | Skipped |

## Consequences
Positive:
- Tamper detection: any modification to WASM, capabilities, or tools invalidates the signature
- Publisher identity bound to the signing key
- Deterministic hash bundle enables reproducible verification
- No dependency on external PKI — trust is user-mediated

Negative:
- Key management is manual (publishers must safeguard private keys)
- No key revocation mechanism (relies on registry removing compromised plugins)
- No certificate chain — a compromised key cannot be distinguished from a legitimate one
  without registry-side revocation
- `LocalDev` skips all verification — developers must understand this is unsafe
