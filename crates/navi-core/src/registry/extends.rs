//! Resolve `extends` overlays against `bases/` (and provider) definitions.
//!
//! Mirrors navi-registry `scripts/validate.py` deep-merge semantics so provider
//! JSON files can inherit shared model catalogs (e.g. mimo-anthropic regions).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use super::types::RegistryProvider;

/// Flatten a provider JSON value by resolving its `extends` chain.
///
/// `bases` maps base/provider id → raw JSON object. Lookups prefer this map;
/// when missing, the original value is returned unchanged (aside from removing
/// a dangling `extends` key is NOT done — unresolved extends leave models empty).
pub fn resolve_extends_value(data: &Value, bases: &HashMap<String, Value>) -> Result<Value> {
    resolve_extends_value_inner(data, bases, &mut Vec::new())
}

fn resolve_extends_value_inner(
    data: &Value,
    bases: &HashMap<String, Value>,
    stack: &mut Vec<String>,
) -> Result<Value> {
    let Value::Object(map) = data else {
        bail!("provider definition must be a JSON object");
    };

    let Some(extends_val) = map.get("extends") else {
        return Ok(data.clone());
    };
    let extends = extends_val
        .as_str()
        .filter(|s| !s.is_empty())
        .context("extends: must be a non-empty string")?;

    if stack.iter().any(|s| s == extends) {
        bail!(
            "extends cycle detected: {} -> {}",
            stack.join(" -> "),
            extends
        );
    }

    let base = bases
        .get(extends)
        .with_context(|| format!("extends: base '{extends}' not found in bases/ or providers/"))?;

    stack.push(extends.to_string());
    let resolved_base = resolve_extends_value_inner(base, bases, stack)?;
    stack.pop();

    let mut overlay = Map::new();
    for (k, v) in map {
        if k != "extends" {
            overlay.insert(k.clone(), v.clone());
        }
    }
    Ok(deep_merge(&resolved_base, &Value::Object(overlay)))
}

/// Deep-merge `overlay` onto `base` (object keys only; non-objects replace).
pub fn deep_merge(base: &Value, overlay: &Value) -> Value {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            let mut result = base_map.clone();
            for (key, value) in overlay_map {
                match result.get(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        result.insert(key.clone(), deep_merge(existing, value));
                    }
                    _ => {
                        result.insert(key.clone(), value.clone());
                    }
                }
            }
            Value::Object(result)
        }
        (_, overlay) => overlay.clone(),
    }
}

/// Parse provider JSON, resolve `extends`, then deserialize to [`RegistryProvider`].
pub fn parse_provider_json(json: &str, bases: &HashMap<String, Value>) -> Result<RegistryProvider> {
    let raw: Value = serde_json::from_str(json).context("failed to parse provider JSON")?;
    let flattened = resolve_extends_value(&raw, bases)?;
    let provider: RegistryProvider = serde_json::from_value(flattened)
        .context("failed to deserialize flattened provider JSON")?;
    Ok(provider)
}

/// Load all `bases/*.json` and `providers/*.json` as raw objects keyed by file stem / id.
pub fn load_local_base_map(registry_dir: &Path) -> Result<HashMap<String, Value>> {
    let mut map = HashMap::new();
    for sub in ["bases", "providers"] {
        let dir = registry_dir.join(sub);
        if !dir.is_dir() {
            continue;
        }
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let value: Value = serde_json::from_str(&text)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            let id = value
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_default();
            if !id.is_empty() {
                map.insert(id, value);
            }
        }
    }
    Ok(map)
}

/// Build a base map from embedded `(id, json)` pairs (bases first, then providers).
pub fn base_map_from_embedded(
    base_files: &[(&str, &str)],
    provider_files: &[(&str, &str)],
) -> Result<HashMap<String, Value>> {
    let mut map = HashMap::new();
    for (id, json) in base_files.iter().chain(provider_files.iter()) {
        let value: Value = serde_json::from_str(json)
            .with_context(|| format!("failed to parse embedded base/provider '{id}'"))?;
        let key = value
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(id)
            .to_string();
        map.insert(key, value);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deep_merge_objects_and_replaces_scalars() {
        let base = json!({"a": 1, "nested": {"x": 1, "y": 2}, "models": [1]});
        let overlay = json!({"a": 2, "nested": {"y": 9, "z": 3}, "models": [2, 3]});
        let merged = deep_merge(&base, &overlay);
        assert_eq!(
            merged,
            json!({"a": 2, "nested": {"x": 1, "y": 9, "z": 3}, "models": [2, 3]})
        );
    }

    #[test]
    fn resolve_extends_merges_base_models() {
        let mut bases = HashMap::new();
        bases.insert(
            "mimo-anthropic".into(),
            json!({
                "id": "mimo-anthropic",
                "label": "MiMo base",
                "kind": "anthropic-messages",
                "api_key_env": "MIMO_API_KEY",
                "models": [{"ref": "mimo-v2.5"}, {"ref": "mimo-v2-flash"}]
            }),
        );
        let child = json!({
            "id": "mimo-anthropic-ams",
            "label": "MiMo Europe",
            "extends": "mimo-anthropic",
            "kind": "anthropic-messages",
            "api_key_env": "MIMO_API_KEY",
            "base_url": "https://example.com"
        });
        let flat = resolve_extends_value(&child, &bases).expect("resolve");
        let provider: RegistryProvider = serde_json::from_value(flat).expect("deserialize");
        assert_eq!(provider.id, "mimo-anthropic-ams");
        assert_eq!(provider.label, "MiMo Europe");
        assert_eq!(provider.base_url.as_deref(), Some("https://example.com"));
        assert_eq!(provider.models.len(), 2);
        assert_eq!(provider.models[0].model_ref.as_deref(), Some("mimo-v2.5"));
        assert!(provider.extends.is_none());
    }
}
