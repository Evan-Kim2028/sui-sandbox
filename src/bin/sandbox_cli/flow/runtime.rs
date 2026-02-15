use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use std::path::PathBuf;

use super::WalrusArchiveNetwork;
#[cfg(test)]
use sui_sandbox_core::checkpoint_discovery::parse_checkpoint_spec as core_parse_checkpoint_spec;
use sui_sandbox_core::checkpoint_discovery::{
    build_walrus_client as core_build_walrus_client,
    discover_checkpoint_targets as core_discover_checkpoint_targets,
    resolve_replay_target_from_discovery as core_resolve_replay_target_from_discovery,
    DiscoverOutput as CoreDiscoverOutput, WalrusArchiveNetwork as CoreWalrusArchiveNetwork,
};
use sui_sandbox_core::environment_bootstrap::MainnetObjectRequest;
use sui_transport::walrus::WalrusClient;

pub(super) type FlowDiscoverOutput = CoreDiscoverOutput;

#[cfg(test)]
pub(super) fn parse_checkpoint_spec(spec: &str) -> Result<Vec<u64>> {
    core_parse_checkpoint_spec(spec)
}

pub(super) fn build_walrus_client(
    network: WalrusArchiveNetwork,
    caching_url: Option<&str>,
    aggregator_url: Option<&str>,
) -> Result<WalrusClient> {
    core_build_walrus_client(
        match network {
            WalrusArchiveNetwork::Mainnet => CoreWalrusArchiveNetwork::Mainnet,
            WalrusArchiveNetwork::Testnet => CoreWalrusArchiveNetwork::Testnet,
        },
        caching_url,
        aggregator_url,
    )
    .map_err(|err| {
        let message = err.to_string();
        if message.contains("provide both walrus_caching_url and walrus_aggregator_url") {
            anyhow!(
                "provide both --walrus-caching-url and --walrus-aggregator-url for custom endpoints"
            )
        } else {
            err
        }
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_replay_target(
    digest: Option<&str>,
    state_json: Option<&PathBuf>,
    checkpoint: Option<u64>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    walrus_network: WalrusArchiveNetwork,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<(Option<String>, Option<u64>)> {
    let walrus = build_walrus_client(walrus_network, walrus_caching_url, walrus_aggregator_url)?;
    core_resolve_replay_target_from_discovery(
        digest,
        checkpoint,
        state_json.is_some(),
        discover_latest,
        discover_package_id,
        &walrus,
    )
    .map_err(|err| {
        let message = err.to_string();
        if message == "digest cannot be empty" {
            anyhow!("--digest cannot be empty")
        } else if message
            == "provide digest, state_file, or discover_latest for replay target selection"
        {
            anyhow!("Provide --digest, --state-json, or --discover-latest for replay target selection")
        } else if message == "discover_package_id is required when discover_latest is set" {
            anyhow!(
                "--discover-latest requires package context; pass --package-id or --context with package_id"
            )
        } else {
            err
        }
    })
}

pub(super) fn discover_flow_targets(
    walrus: &WalrusClient,
    checkpoint_spec: Option<&str>,
    latest: Option<u64>,
    package_id: Option<&str>,
    include_framework: bool,
    limit: usize,
) -> Result<FlowDiscoverOutput> {
    core_discover_checkpoint_targets(
        walrus,
        checkpoint_spec,
        latest,
        package_id,
        include_framework,
        limit,
    )
}

pub(super) fn parse_object_at_spec(spec: &str) -> Result<MainnetObjectRequest> {
    let trimmed = spec.trim();
    let (object_id, version_raw) = trimmed.rsplit_once('@').ok_or_else(|| {
        anyhow!(
            "invalid --object-at value `{}` (expected <OBJECT_ID>@<VERSION>)",
            spec
        )
    })?;
    validate_hex_address(object_id, "--object-at")?;
    let version: u64 = version_raw.trim().parse().with_context(|| {
        format!(
            "invalid version `{}` in --object-at value `{}`",
            version_raw, spec
        )
    })?;
    Ok(MainnetObjectRequest {
        object_id: object_id.trim().to_string(),
        version: Some(version),
    })
}

pub(super) fn validate_hex_address(raw: &str, flag: &str) -> Result<()> {
    let trimmed = raw.trim();
    AccountAddress::from_hex_literal(trimmed)
        .with_context(|| format!("invalid {} value {}", flag, raw))?;
    Ok(())
}
