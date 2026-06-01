use serde_json::Value;

pub(super) fn find_npm_package(manifest: &str, package: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(manifest).ok()?;
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if parsed
            .get(section)
            .and_then(Value::as_object)
            .is_some_and(|deps| deps.contains_key(package))
        {
            return Some(section.to_string());
        }
    }
    None
}

pub(super) fn find_cargo_package(manifest: &str, package: &str) -> Option<String> {
    let parsed: toml::Value = toml::from_str(manifest).ok()?;
    for section in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "workspace.dependencies",
    ] {
        let table = section
            .split('.')
            .try_fold(&parsed, |value, key| value.get(key));
        if table
            .and_then(toml::Value::as_table)
            .is_some_and(|deps| deps.contains_key(package))
        {
            return Some(section.to_string());
        }
    }
    None
}

pub(super) fn find_go_package(manifest: &str, package: &str) -> bool {
    let mut in_require_block = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with("require (") {
            in_require_block = true;
            continue;
        }
        if in_require_block && line == ")" {
            in_require_block = false;
            continue;
        }
        let require_line = if let Some(rest) = line.strip_prefix("require ") {
            rest.trim()
        } else if in_require_block {
            line
        } else {
            continue;
        };
        if require_line.split_whitespace().next() == Some(package) {
            return true;
        }
    }
    false
}
