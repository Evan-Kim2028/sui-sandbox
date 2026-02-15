use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::fetch::{fetch_package_into_state, fetch_package_with_bytecodes_into_state};
use super::super::network::sandbox_home;
use super::super::SandboxState;
use sui_sandbox_core::context_contract::{
    decode_context_package_modules, parse_context_payload, ContextPackage, ContextPayloadV2,
};

pub(super) struct LoadedContext {
    pub(super) package_id: String,
    pub(super) packages_count: usize,
}

pub(super) fn prepare_context_data(
    state: &mut SandboxState,
    package_id: &str,
    with_deps: bool,
    verbose: bool,
) -> Result<(ContextPayloadV2, Vec<String>)> {
    let fetched = fetch_package_with_bytecodes_into_state(state, package_id, with_deps, verbose)
        .with_context(|| format!("Failed to prepare package context for {}", package_id))?;
    let packages: Vec<ContextPackage> = fetched
        .packages_fetched
        .iter()
        .map(|pkg| ContextPackage {
            address: pkg.address.clone(),
            modules: pkg.modules.clone(),
            bytecodes: pkg.bytecodes.clone().unwrap_or_default(),
        })
        .collect();
    let packages_fetched: Vec<String> = packages.iter().map(|pkg| pkg.address.clone()).collect();
    let context = ContextPayloadV2::new(
        package_id.to_string(),
        with_deps,
        now_ms(),
        Some(state.rpc_url.clone()),
        packages,
    );
    Ok((context, packages_fetched))
}

pub(super) fn write_context_file(
    path: &Path,
    context: &ContextPayloadV2,
    force: bool,
) -> Result<()> {
    if path.exists() && !force {
        return Err(anyhow!(
            "Refusing to overwrite existing context at {} (pass --force)",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create context directory {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(context)?)
        .with_context(|| format!("Failed to write context {}", path.display()))?;
    Ok(())
}

pub(super) fn load_context_file_into_state(
    state: &mut SandboxState,
    path: &Path,
    verbose: bool,
) -> Result<LoadedContext> {
    let resolved_path = resolve_context_read_path(path);
    if verbose && resolved_path != path {
        eprintln!(
            "[context] requested {} but loaded compatibility path {}",
            path.display(),
            resolved_path.display()
        );
    }
    let raw = fs::read_to_string(&resolved_path)
        .with_context(|| format!("Failed to read context file {}", resolved_path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid context JSON in {}", resolved_path.display()))?;
    let parsed = parse_context_payload(&value).with_context(|| {
        format!(
            "Invalid context payload in {} (expected context wrapper or package map)",
            resolved_path.display()
        )
    })?;
    let context_packages = parsed.packages.clone();

    let mut loaded_count = 0usize;
    if !context_packages.is_empty() {
        loaded_count = load_context_packages_into_state(state, &context_packages)?;
        if verbose {
            eprintln!(
                "[context] loaded {} packages directly from context {}",
                loaded_count,
                resolved_path.display()
            );
        }
    }

    if loaded_count == 0 {
        let package_id = parsed.package_id.as_deref().ok_or_else(|| {
            anyhow!(
                "Context {} has no portable bytecodes and no `package_id` for network refresh",
                resolved_path.display()
            )
        })?;
        let fetched = fetch_package_into_state(state, package_id, parsed.with_deps, verbose)
            .with_context(|| {
                format!(
                    "Failed to refresh prepared package context for {}",
                    package_id
                )
            })?;
        loaded_count = fetched.packages_fetched.len();
        if verbose {
            eprintln!(
                "[context] context had no portable package bytes; refreshed {} package(s) from network",
                loaded_count
            );
        }
    }

    Ok(LoadedContext {
        package_id: parsed
            .package_id
            .unwrap_or_else(|| "<context-packages>".to_string()),
        packages_count: loaded_count,
    })
}

fn load_context_packages_into_state(
    state: &mut SandboxState,
    packages: &[ContextPackage],
) -> Result<usize> {
    let mut loaded = 0usize;
    for package in packages {
        if package.bytecodes.is_empty() {
            continue;
        }
        let address = AccountAddress::from_hex_literal(&package.address)
            .with_context(|| format!("invalid package address in context: {}", package.address))?;
        let decoded_modules = decode_context_package_modules(package).with_context(|| {
            format!(
                "failed decoding context package modules for {}",
                package.address
            )
        })?;
        if decoded_modules.is_empty() {
            continue;
        }
        state.add_package(address, decoded_modules);
        loaded += 1;
    }
    Ok(loaded)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn default_context_path(package_id: &str) -> PathBuf {
    let trimmed = package_id.trim();
    let no_prefix = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    let short = if no_prefix.is_empty() {
        "package".to_string()
    } else {
        no_prefix.chars().take(20).collect::<String>()
    };
    sandbox_home()
        .join("contexts")
        .join(format!("context.{short}.json"))
}

fn resolve_context_read_path(path: &Path) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    if let Some(candidate) = legacy_or_canonical_counterpart(path) {
        if candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn legacy_or_canonical_counterpart(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let parent_name = parent.file_name()?.to_str()?;
    let grandparent = parent.parent()?;

    if parent_name == "contexts" && file_name.starts_with("context.") {
        let legacy_name = file_name.replacen("context.", "flow_context.", 1);
        return Some(grandparent.join("flow_contexts").join(legacy_name));
    }
    if parent_name == "flow_contexts" && file_name.starts_with("flow_context.") {
        let canonical_name = file_name.replacen("flow_context.", "context.", 1);
        return Some(grandparent.join("contexts").join(canonical_name));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::resolve_context_read_path;
    use std::fs;

    #[test]
    fn resolves_legacy_when_canonical_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = dir.path().join("contexts").join("context.2.json");
        let legacy = dir.path().join("flow_contexts").join("flow_context.2.json");
        fs::create_dir_all(legacy.parent().expect("parent")).expect("mkdir");
        fs::write(&legacy, "{}").expect("write");
        let resolved = resolve_context_read_path(&canonical);
        assert_eq!(resolved, legacy);
    }

    #[test]
    fn resolves_canonical_when_legacy_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = dir.path().join("contexts").join("context.2.json");
        let legacy = dir.path().join("flow_contexts").join("flow_context.2.json");
        fs::create_dir_all(canonical.parent().expect("parent")).expect("mkdir");
        fs::write(&canonical, "{}").expect("write");
        let resolved = resolve_context_read_path(&legacy);
        assert_eq!(resolved, canonical);
    }
}
