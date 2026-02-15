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

pub(super) struct LoadedFlowContext {
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
) -> Result<LoadedFlowContext> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read context file {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid context JSON in {}", path.display()))?;
    let parsed = parse_context_payload(&value).with_context(|| {
        format!(
            "Invalid context payload in {} (expected flow context wrapper or package map)",
            path.display()
        )
    })?;
    let context_packages = parsed.packages.clone();

    let mut loaded_count = 0usize;
    if !context_packages.is_empty() {
        loaded_count = load_context_packages_into_state(state, &context_packages)?;
        if verbose {
            eprintln!(
                "[flow] loaded {} packages directly from context {}",
                loaded_count,
                path.display()
            );
        }
    }

    if loaded_count == 0 {
        let package_id = parsed.package_id.as_deref().ok_or_else(|| {
            anyhow!(
                "Context {} has no portable bytecodes and no `package_id` for network refresh",
                path.display()
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
                "[flow] context had no portable package bytes; refreshed {} package(s) from network",
                loaded_count
            );
        }
    }

    Ok(LoadedFlowContext {
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

pub(super) fn default_flow_context_path(package_id: &str) -> PathBuf {
    let trimmed = package_id.trim();
    let no_prefix = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    let short = if no_prefix.is_empty() {
        "package".to_string()
    } else {
        no_prefix.chars().take(20).collect::<String>()
    };
    sandbox_home()
        .join("flow_contexts")
        .join(format!("flow_context.{short}.json"))
}
