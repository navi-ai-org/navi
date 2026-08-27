use sha2::{Digest, Sha256};

/// Compute SHA-256 hash of raw bytes, returned as "sha256:<hex>".
pub fn compute_wasm_hash(wasm_bytes: &[u8]) -> String {
    let hash = Sha256::digest(wasm_bytes);
    format!("sha256:{}", hex_encode(&hash))
}

/// Compute SHA-256 hash of a normalized TOML string.
/// Normalization: keys sorted, no trailing whitespace, LF endings, UTF-8 no BOM.
pub fn compute_content_hash(content: &str) -> String {
    let normalized = normalize_toml(content);
    let hash = Sha256::digest(normalized.as_bytes());
    format!("sha256:{}", hex_encode(&hash))
}

/// Verify that a wasm_hash matches the actual file bytes.
pub fn verify_wasm_hash(wasm_bytes: &[u8], expected_hash: &str) -> bool {
    let actual = compute_wasm_hash(wasm_bytes);
    actual == expected_hash
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Normalize TOML content for deterministic hashing.
/// This is a simplified normalization: sort keys at each level,
/// remove trailing whitespace, ensure LF line endings.
fn normalize_toml(content: &str) -> String {
    // Parse and re-serialize with sorted keys
    match toml::from_str::<toml::Value>(content) {
        Ok(value) => {
            let normalized = toml::to_string(&value).unwrap_or_else(|_| content.to_string());
            // Ensure LF line endings and no trailing whitespace per line
            normalized
                .lines()
                .map(|line| line.trim_end())
                .collect::<Vec<_>>()
                .join("\n")
        }
        Err(_) => {
            // If parsing fails, do basic normalization
            content
                .lines()
                .map(|line| line.trim_end())
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasm_hash_format() {
        let hash = compute_wasm_hash(b"hello");
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), 71); // "sha256:" (7) + 64 hex chars
    }

    #[test]
    fn wasm_hash_deterministic() {
        let h1 = compute_wasm_hash(b"test data");
        let h2 = compute_wasm_hash(b"test data");
        assert_eq!(h1, h2);
    }

    #[test]
    fn wasm_hash_different_data() {
        let h1 = compute_wasm_hash(b"data1");
        let h2 = compute_wasm_hash(b"data2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn verify_wasm_hash_correct() {
        let data = b"wasm binary";
        let hash = compute_wasm_hash(data);
        assert!(verify_wasm_hash(data, &hash));
    }

    #[test]
    fn verify_wasm_hash_incorrect() {
        let data = b"wasm binary";
        let hash = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        assert!(!verify_wasm_hash(data, hash));
    }

    #[test]
    fn content_hash_deterministic() {
        let content = r#"
[[tools]]
id = "a"
summary = "A"
risk = "read_only"
capabilities = []

[[tools]]
id = "b"
summary = "B"
risk = "read_only"
capabilities = []
"#;
        let h1 = compute_content_hash(content);
        let h2 = compute_content_hash(content);
        assert_eq!(h1, h2);
    }

    #[test]
    fn empty_wasm_hash() {
        let hash = compute_wasm_hash(b"");
        // SHA-256 of empty input
        assert_eq!(
            hash,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
