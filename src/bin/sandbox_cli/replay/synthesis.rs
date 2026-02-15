use std::collections::HashMap;

use anyhow::{anyhow, Result};
use base64::Engine;
#[cfg(feature = "mm2")]
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
#[cfg(feature = "mm2")]
use sui_sandbox_core::mm2::{TypeModel, TypeSynthesizer};
use sui_sandbox_core::resolver::LocalModuleResolver;
use sui_sandbox_core::tx_replay::MissingInputObject;
use sui_sandbox_core::types::{format_type_tag, parse_type_tag};
use sui_sandbox_core::utilities::rewrite_type_tag;
use sui_state_fetcher::HistoricalStateProvider;

#[cfg(feature = "mm2")]
pub(super) fn synthesize_missing_inputs(
    missing: &[MissingInputObject],
    cached_objects: &mut HashMap<String, String>,
    version_map: &mut HashMap<String, u64>,
    resolver: &LocalModuleResolver,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    provider: &HistoricalStateProvider,
    verbose: bool,
) -> Result<Vec<String>> {
    if missing.is_empty() {
        return Ok(Vec::new());
    }

    let modules: Vec<CompiledModule> = resolver.iter_modules().cloned().collect();
    if modules.is_empty() {
        return Err(anyhow!("no modules loaded for synthesis"));
    }
    let type_model = TypeModel::from_modules(modules)
        .map_err(|e| anyhow!("failed to build type model: {}", e))?;
    let mut synthesizer = TypeSynthesizer::new(&type_model);

    let gql = provider.graphql();
    let mut logs = Vec::new();

    for entry in missing {
        let object_id = entry.object_id.as_str();
        let version = entry.version;
        let mut type_string = gql
            .fetch_object_at_version(object_id, version)
            .ok()
            .and_then(|obj| obj.type_string)
            .or_else(|| {
                gql.fetch_object(object_id)
                    .ok()
                    .and_then(|obj| obj.type_string)
            });

        let Some(type_str) = type_string.take() else {
            if verbose {
                logs.push(format!(
                    "missing_type object={} version={} (skipped)",
                    object_id, version
                ));
            }
            continue;
        };

        let mut synth_type = type_str.clone();
        if let Ok(tag) = parse_type_tag(&type_str) {
            let rewritten = rewrite_type_tag(tag, aliases);
            synth_type = format_type_tag(&rewritten);
        }

        let mut result = synthesizer.synthesize_with_fallback(&synth_type);
        if let Ok(id) = AccountAddress::from_hex_literal(object_id) {
            if result.bytes.len() >= 32 {
                result.bytes[..32].copy_from_slice(id.as_ref());
            }
        }

        let encoded = base64::engine::general_purpose::STANDARD.encode(&result.bytes);
        let normalized = sui_sandbox_core::utilities::normalize_address(object_id);
        cached_objects.insert(normalized.clone(), encoded.clone());
        cached_objects.insert(object_id.to_string(), encoded.clone());
        if let Some(short) = sui_sandbox_core::types::normalize_address_short(object_id) {
            cached_objects.insert(short, encoded.clone());
        }
        version_map.insert(normalized.clone(), version);

        logs.push(format!(
            "synthesized object={} version={} type={} stub={} ({})",
            normalized, version, synth_type, result.is_stub, result.description
        ));
    }

    Ok(logs)
}

#[cfg(not(feature = "mm2"))]
pub(super) fn synthesize_missing_inputs(
    _missing: &[MissingInputObject],
    _cached_objects: &mut HashMap<String, String>,
    _version_map: &mut HashMap<String, u64>,
    _resolver: &LocalModuleResolver,
    _aliases: &HashMap<AccountAddress, AccountAddress>,
    _provider: &HistoricalStateProvider,
    _verbose: bool,
) -> Result<Vec<String>> {
    Err(anyhow!(
        "missing input synthesis requires the `mm2` feature"
    ))
}
