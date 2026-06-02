use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::store::{capabilities_hash_from_manifest, tools_hash_from_manifest};
use crate::types::{PluginManifest, TrustLevel};
use crate::{compute_wasm_hash, verify_wasm_hash};

/// Verify an Ed25519 signature over a manifest's hash bundle.
///
/// The signature covers: wasm_hash_bytes ++ capabilities_hash_bytes ++ tools_hash_bytes.
///
/// The signature string is expected in the format "ed25519:<base64>".
/// The public key is expected in the format "ed25519:<base64>".
pub fn verify_manifest_signature(
    wasm_hash: &str,
    capabilities_hash: &str,
    tools_hash: &str,
    signature_str: &str,
    public_key_str: &str,
) -> Result<bool, String> {
    // Parse signature (64 bytes)
    let sig_bytes = parse_ed25519_bytes(signature_str, "ed25519:")?;
    if sig_bytes.len() != 64 {
        return Err(format!(
            "signature must be 64 bytes, got {}",
            sig_bytes.len()
        ));
    }
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| "signature conversion failed".to_string())?;
    let signature = Signature::from_bytes(&sig_array);

    // Parse public key (32 bytes)
    let key_bytes = parse_ed25519_bytes(public_key_str, "ed25519:")?;
    if key_bytes.len() != 32 {
        return Err(format!(
            "public key must be 32 bytes, got {}",
            key_bytes.len()
        ));
    }
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| "public key conversion failed".to_string())?;
    let public_key =
        VerifyingKey::from_bytes(&key_array).map_err(|e| format!("invalid public key: {}", e))?;

    let message = hash_bundle_message(wasm_hash, capabilities_hash, tools_hash)?;

    // Verify
    match public_key.verify(&message, &signature) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Compute the hash bundle for signing.
///
/// Returns (wasm_hash, capabilities_hash, tools_hash) as "sha256:<hex>" strings.
pub fn compute_hash_bundle(
    wasm_bytes: &[u8],
    capabilities_content: &str,
    tools_content: &str,
) -> (String, String, String) {
    let wasm_hash = compute_sha256(wasm_bytes);
    let cap_hash = compute_sha256(capabilities_content.as_bytes());
    let tools_hash = compute_sha256(tools_content.as_bytes());
    (wasm_hash, cap_hash, tools_hash)
}

fn compute_sha256(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    format!("sha256:{}", hex_encode(&hash))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn parse_ed25519_bytes(s: &str, prefix: &str) -> Result<Vec<u8>, String> {
    let b64 = s
        .strip_prefix(prefix)
        .ok_or_else(|| format!("expected '{}' prefix", prefix))?;
    BASE64
        .decode(b64)
        .map_err(|e| format!("invalid base64: {}", e))
}

/// Verify manifest signature and WASM integrity for the given trust level.
///
/// `LocalDev` skips cryptographic verification (development only).
pub fn verify_plugin_signature(
    manifest: &PluginManifest,
    wasm_bytes: &[u8],
    trust_level: TrustLevel,
) -> Result<(), String> {
    if trust_level == TrustLevel::LocalDev {
        return Ok(());
    }

    let public_key = manifest
        .plugin
        .public_key
        .as_deref()
        .ok_or_else(|| "plugin.public_key is required".to_string())?;

    if !verify_wasm_hash(wasm_bytes, &manifest.plugin.wasm_hash) {
        return Err("WASM hash does not match plugin.wasm_hash".into());
    }

    let capabilities_hash = capabilities_hash_from_manifest(manifest);
    let tools_hash = tools_hash_from_manifest(manifest);

    let valid = verify_manifest_signature(
        &manifest.plugin.wasm_hash,
        &capabilities_hash,
        &tools_hash,
        &manifest.plugin.signature,
        public_key,
    )?;

    if valid {
        Ok(())
    } else {
        Err("manifest Ed25519 signature verification failed".into())
    }
}

/// Deterministic signing key for tests and local fixtures (not a production secret).
const TEST_SIGNING_KEY_BYTES: [u8; 32] = [0x42; 32];

/// Sign a manifest with the deterministic test key.
pub fn sign_plugin_manifest_for_tests(manifest: &mut PluginManifest, wasm_bytes: &[u8]) {
    let signing_key = SigningKey::from_bytes(&TEST_SIGNING_KEY_BYTES);
    sign_plugin_manifest(manifest, wasm_bytes, &signing_key);
}

/// Sign a manifest and populate `wasm_hash`, `public_key`, and `signature`.
pub fn sign_plugin_manifest(
    manifest: &mut PluginManifest,
    wasm_bytes: &[u8],
    signing_key: &SigningKey,
) {
    manifest.plugin.wasm_hash = compute_wasm_hash(wasm_bytes);
    let capabilities_hash = capabilities_hash_from_manifest(manifest);
    let tools_hash = tools_hash_from_manifest(manifest);

    let verifying_key = signing_key.verifying_key();
    manifest.plugin.public_key = Some(format!(
        "ed25519:{}",
        BASE64.encode(verifying_key.to_bytes())
    ));

    let message = hash_bundle_message(&manifest.plugin.wasm_hash, &capabilities_hash, &tools_hash)
        .expect("valid hashes for signing");

    let signature = signing_key.sign(&message);
    manifest.plugin.signature = format!("ed25519:{}", BASE64.encode(signature.to_bytes()));
}

fn hash_bundle_message(
    wasm_hash: &str,
    capabilities_hash: &str,
    tools_hash: &str,
) -> Result<Vec<u8>, String> {
    let wasm_digest = parse_sha256_bytes(wasm_hash)?;
    let cap_digest = parse_sha256_bytes(capabilities_hash)?;
    let tools_digest = parse_sha256_bytes(tools_hash)?;
    let mut message = Vec::with_capacity(96);
    message.extend_from_slice(&wasm_digest);
    message.extend_from_slice(&cap_digest);
    message.extend_from_slice(&tools_digest);
    Ok(message)
}

fn parse_sha256_bytes(s: &str) -> Result<[u8; 32], String> {
    let hex = s
        .strip_prefix("sha256:")
        .ok_or_else(|| "expected 'sha256:' prefix".to_string())?;
    let bytes = hex::decode(hex).map_err(|e| format!("invalid hex: {}", e))?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "invalid sha256 length".to_string())?;
    Ok(array)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    fn generate_keypair() -> (SigningKey, VerifyingKey) {
        let mut csprng = OsRng;
        let secret_bytes: [u8; 32] = rand::Rng::r#gen(&mut csprng);
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        let verifying_key = signing_key.verifying_key();
        (signing_key, verifying_key)
    }

    #[test]
    fn verify_valid_signature() {
        let (signing_key, verifying_key) = generate_keypair();

        let wasm_hash = compute_sha256(b"fake wasm");
        let cap_hash = compute_sha256(b"caps");
        let tools_hash = compute_sha256(b"tools");

        // Build message
        let wasm_d = parse_sha256_bytes(&wasm_hash).unwrap();
        let cap_d = parse_sha256_bytes(&cap_hash).unwrap();
        let tools_d = parse_sha256_bytes(&tools_hash).unwrap();
        let mut message = Vec::new();
        message.extend_from_slice(&wasm_d);
        message.extend_from_slice(&cap_d);
        message.extend_from_slice(&tools_d);

        let signature = signing_key.sign(&message);
        let sig_str = format!("ed25519:{}", BASE64.encode(signature.to_bytes()));
        let key_str = format!("ed25519:{}", BASE64.encode(verifying_key.to_bytes()));

        let result =
            verify_manifest_signature(&wasm_hash, &cap_hash, &tools_hash, &sig_str, &key_str);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn reject_invalid_signature() {
        let (signing_key, verifying_key) = generate_keypair();

        let wasm_hash = compute_sha256(b"fake wasm");
        let cap_hash = compute_sha256(b"caps");
        let tools_hash = compute_sha256(b"tools");

        // Sign different data
        let wrong_sig = signing_key.sign(b"wrong data");
        let sig_str = format!("ed25519:{}", BASE64.encode(wrong_sig.to_bytes()));
        let key_str = format!("ed25519:{}", BASE64.encode(verifying_key.to_bytes()));

        let result =
            verify_manifest_signature(&wasm_hash, &cap_hash, &tools_hash, &sig_str, &key_str);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn reject_wrong_key() {
        let (signing_key, _) = generate_keypair();
        let (_, wrong_verifying) = generate_keypair();

        let wasm_hash = compute_sha256(b"fake wasm");
        let cap_hash = compute_sha256(b"caps");
        let tools_hash = compute_sha256(b"tools");

        let wasm_d = parse_sha256_bytes(&wasm_hash).unwrap();
        let cap_d = parse_sha256_bytes(&cap_hash).unwrap();
        let tools_d = parse_sha256_bytes(&tools_hash).unwrap();
        let mut message = Vec::new();
        message.extend_from_slice(&wasm_d);
        message.extend_from_slice(&cap_d);
        message.extend_from_slice(&tools_d);

        let signature = signing_key.sign(&message);
        let sig_str = format!("ed25519:{}", BASE64.encode(signature.to_bytes()));
        let key_str = format!("ed25519:{}", BASE64.encode(wrong_verifying.to_bytes()));

        let result =
            verify_manifest_signature(&wasm_hash, &cap_hash, &tools_hash, &sig_str, &key_str);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn reject_invalid_signature_format() {
        let result = verify_manifest_signature(
            "sha256:abc",
            "sha256:def",
            "sha256:ghi",
            "not-valid",
            "ed25519:abc",
        );
        assert!(result.is_err());
    }

    #[test]
    fn reject_invalid_key_format() {
        let result = verify_manifest_signature(
            "sha256:abc",
            "sha256:def",
            "sha256:ghi",
            "ed25519:abc",
            "not-valid",
        );
        assert!(result.is_err());
    }

    #[test]
    fn compute_hash_bundle_deterministic() {
        let (h1, h2, h3) = compute_hash_bundle(b"wasm", "caps", "tools");
        let (h4, h5, h6) = compute_hash_bundle(b"wasm", "caps", "tools");
        assert_eq!(h1, h4);
        assert_eq!(h2, h5);
        assert_eq!(h3, h6);
    }

    #[test]
    fn verify_plugin_signature_roundtrip() {
        use crate::types::{PluginMeta, RuntimeKind};

        let wasm = b"wasm-bytes";
        let mut manifest = PluginManifest {
            plugin: PluginMeta {
                id: "p".into(),
                name: "p".into(),
                version: "1.0.0".into(),
                publisher: "gh:t".into(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".into(),
                wasm_hash: String::new(),
                signature: String::new(),
                public_key: None,
                minimum_navi: "0.1.0".into(),
            },
            capabilities: vec![],
            tools: vec![],
        };
        let (signing_key, _) = generate_keypair();
        sign_plugin_manifest(&mut manifest, wasm, &signing_key);
        verify_plugin_signature(&manifest, wasm, TrustLevel::Community).unwrap();
    }

    #[test]
    fn compute_hash_bundle_different_data() {
        let (h1, _, _) = compute_hash_bundle(b"wasm1", "caps", "tools");
        let (h2, _, _) = compute_hash_bundle(b"wasm2", "caps", "tools");
        assert_ne!(h1, h2);
    }
}
