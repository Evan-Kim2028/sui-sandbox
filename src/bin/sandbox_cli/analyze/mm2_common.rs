use anyhow::{Context, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use super::super::network::resolve_graphql_endpoint;
use crate::sandbox_cli::SandboxState;
use sui_package_extractor::bytecode::read_local_compiled_modules;
use sui_transport::graphql::GraphQLClient;

pub(super) fn normalize_package_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let lower = hex.to_ascii_lowercase();
    if lower.len() > 64 {
        return None;
    }
    Some(format!("0x{:0>64}", lower))
}

pub(super) fn parse_bcs_linkage_upgraded_ids(package_dir: &Path) -> Result<Vec<String>> {
    let bcs_path = package_dir.join("bcs.json");
    if !bcs_path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&bcs_path)
        .with_context(|| format!("read {}", bcs_path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", bcs_path.display()))?;
    let Some(linkage) = v.get("linkageTable").and_then(serde_json::Value::as_object) else {
        return Ok(Vec::new());
    };

    let mut out = BTreeSet::new();
    for (k, entry) in linkage {
        if let Some(id) = entry
            .get("upgraded_id")
            .and_then(serde_json::Value::as_str)
            .and_then(normalize_package_id)
        {
            out.insert(id);
            continue;
        }
        if let Some(id) = normalize_package_id(k) {
            out.insert(id);
        }
    }
    Ok(out.into_iter().collect())
}

fn resolve_corpus_dep_dir(package_dir: &Path, dep_id: &str) -> Option<PathBuf> {
    let shard_dir = package_dir.parent()?;
    let corpus_root = shard_dir.parent()?;
    let shard_name = shard_dir.file_name()?.to_str()?;
    if !(shard_name.starts_with("0x") && shard_name.len() == 4) {
        return None;
    }
    let hex = dep_id.strip_prefix("0x").unwrap_or(dep_id);
    if hex.len() != 64 {
        return None;
    }
    let prefix = format!("0x{}", &hex[0..2]);
    let suffix = &hex[2..];
    let candidate = corpus_root.join(prefix).join(suffix);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn insert_compiled_modules_dedup(
    dst: &mut Vec<CompiledModule>,
    seen: &mut BTreeSet<String>,
    modules: Vec<CompiledModule>,
) {
    for module in modules {
        let module_id = module.self_id();
        let key = format!("{}::{}", module_id.address(), module_id.name());
        if seen.insert(key) {
            dst.push(module);
        }
    }
}

fn fetch_graphql_package_modules(
    graphql: &GraphQLClient,
    package_id: &str,
) -> Result<Vec<CompiledModule>> {
    let pkg = graphql
        .fetch_package(package_id)
        .with_context(|| format!("fetch package {}", package_id))?;
    let mut compiled_modules = Vec::with_capacity(pkg.modules.len());
    for module in pkg.modules {
        let Some(b64) = module.bytecode_base64 else {
            continue;
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("decode module bytecode")?;
        let compiled =
            CompiledModule::deserialize_with_defaults(&bytes).context("deserialize module")?;
        compiled_modules.push(compiled);
    }
    Ok(compiled_modules)
}

pub(super) fn expand_local_modules_for_mm2(
    package_dir: &Path,
    state: &SandboxState,
    local_modules: &[CompiledModule],
    verbose: bool,
) -> Result<Vec<CompiledModule>> {
    let mut modules = local_modules.to_vec();
    let mut seen = BTreeSet::new();
    for module in &modules {
        let module_id = module.self_id();
        seen.insert(format!("{}::{}", module_id.address(), module_id.name()));
    }

    let mut missing_network_deps = BTreeSet::new();
    let mut processed = BTreeSet::new();
    let mut queue = VecDeque::new();
    for dep in parse_bcs_linkage_upgraded_ids(package_dir)? {
        queue.push_back(dep);
    }

    while let Some(dep_id) = queue.pop_front() {
        if !processed.insert(dep_id.clone()) {
            continue;
        }

        if let Some(dep_dir) = resolve_corpus_dep_dir(package_dir, &dep_id) {
            match read_local_compiled_modules(&dep_dir) {
                Ok(dep_modules) => {
                    if verbose {
                        eprintln!(
                            "[mm2] local dependency {} => {} modules",
                            dep_id,
                            dep_modules.len()
                        );
                    }
                    insert_compiled_modules_dedup(&mut modules, &mut seen, dep_modules);
                    if let Ok(transitive) = parse_bcs_linkage_upgraded_ids(&dep_dir) {
                        for next in transitive {
                            if !processed.contains(&next) {
                                queue.push_back(next);
                            }
                        }
                    }
                }
                Err(_) => {
                    missing_network_deps.insert(dep_id);
                }
            }
        } else {
            missing_network_deps.insert(dep_id);
        }
    }

    if !missing_network_deps.is_empty() {
        let graphql_endpoint = resolve_graphql_endpoint(&state.rpc_url);
        let graphql = GraphQLClient::new(&graphql_endpoint);
        for dep_id in missing_network_deps {
            match fetch_graphql_package_modules(&graphql, &dep_id) {
                Ok(dep_modules) => {
                    if verbose {
                        eprintln!(
                            "[mm2] graphql dependency {} => {} modules",
                            dep_id,
                            dep_modules.len()
                        );
                    }
                    insert_compiled_modules_dedup(&mut modules, &mut seen, dep_modules);
                }
                Err(err) => {
                    if verbose {
                        eprintln!("[mm2] dependency {} unavailable: {}", dep_id, err);
                    }
                }
            }
        }
    }

    Ok(modules)
}

pub(super) fn build_mm2_summary(
    enabled: bool,
    modules: Vec<CompiledModule>,
    verbose: bool,
) -> (Option<bool>, Option<String>) {
    if !enabled {
        return (None, None);
    }
    #[cfg(feature = "mm2")]
    {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sui_sandbox_core::mm2::TypeModel::from_modules(modules)
        }));
        match result {
            Ok(Ok(_)) => (Some(true), None),
            Ok(Err(err)) => {
                if verbose {
                    eprintln!("[mm2] type model build failed: {}", err);
                }
                (Some(false), Some(err.to_string()))
            }
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic payload".to_string()
                };
                if verbose {
                    eprintln!("[mm2] type model panicked: {}", msg);
                }
                (Some(false), Some(format!("mm2 panic: {}", msg)))
            }
        }
    }
    #[cfg(not(feature = "mm2"))]
    {
        let _ = modules;
        (Some(false), Some("mm2 feature disabled".to_string()))
    }
}
