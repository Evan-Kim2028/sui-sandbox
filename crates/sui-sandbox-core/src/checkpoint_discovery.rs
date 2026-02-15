//! Shared checkpoint discovery planner for replay target selection.
//!
//! This module centralizes:
//! - Walrus client construction (network/custom endpoints)
//! - checkpoint spec parsing (`single`, `range`, `list`)
//! - package-filtered PTB target discovery
//! - digest/checkpoint auto-selection for replay

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use sui_resolver::is_framework_address;
use sui_transport::walrus::WalrusClient;
use sui_types::transaction::{Command as SuiCommand, TransactionDataAPI, TransactionKind};

/// Walrus archive network selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalrusArchiveNetwork {
    Mainnet,
    Testnet,
}

impl WalrusArchiveNetwork {
    pub fn parse(network: &str) -> Result<Self> {
        match network.trim().to_ascii_lowercase().as_str() {
            "mainnet" => Ok(Self::Mainnet),
            "testnet" => Ok(Self::Testnet),
            other => Err(anyhow!(
                "invalid walrus_network '{}': expected 'mainnet' or 'testnet'",
                other
            )),
        }
    }
}

/// Single MoveCall entry inside a discovered PTB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverMoveCall {
    pub command_index: usize,
    pub package: String,
    pub module: String,
    pub function: String,
}

/// Replay-ready transaction target discovered from checkpoint scans.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverTarget {
    pub checkpoint: u64,
    pub digest: String,
    pub sender: String,
    pub commands: usize,
    pub input_objects: usize,
    pub output_objects: usize,
    pub package_ids: Vec<String>,
    pub move_calls: Vec<DiscoverMoveCall>,
}

/// Discovery report payload for checkpoint scans.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverOutput {
    pub success: bool,
    pub checkpoints_scanned: usize,
    pub transactions_scanned: usize,
    pub ptbs_scanned: usize,
    pub matches: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_filter: Option<String>,
    pub include_framework: bool,
    pub limit: usize,
    pub truncated: bool,
    pub checkpoints: Vec<u64>,
    pub targets: Vec<DiscoverTarget>,
}

/// Parse checkpoint spec into concrete checkpoint numbers.
///
/// Supported formats:
/// - `239615926`
/// - `239615920..239615926`
/// - `239615920,239615923,239615926`
pub fn parse_checkpoint_spec(spec: &str) -> Result<Vec<u64>> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("checkpoint spec cannot be empty"));
    }

    if let Some((start, end)) = trimmed.split_once("..") {
        let start = start
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid checkpoint start in range: {}", start.trim()))?;
        let end = end
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid checkpoint end in range: {}", end.trim()))?;
        if end < start {
            return Err(anyhow!(
                "invalid checkpoint range {}..{}: end must be >= start",
                start,
                end
            ));
        }
        if end.saturating_sub(start) > 10_000 {
            return Err(anyhow!("checkpoint range too large (max span: 10,000)"));
        }
        let mut checkpoints: Vec<u64> = (start..=end).collect();
        checkpoints.sort_unstable();
        checkpoints.dedup();
        return Ok(checkpoints);
    }

    if trimmed.contains(',') {
        let mut checkpoints = Vec::new();
        for part in trimmed.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let checkpoint = part
                .parse::<u64>()
                .with_context(|| format!("invalid checkpoint in list: {}", part))?;
            checkpoints.push(checkpoint);
        }
        if checkpoints.is_empty() {
            return Err(anyhow!("checkpoint list is empty"));
        }
        checkpoints.sort_unstable();
        checkpoints.dedup();
        return Ok(checkpoints);
    }

    let single = trimmed
        .parse::<u64>()
        .with_context(|| format!("invalid checkpoint: {}", trimmed))?;
    Ok(vec![single])
}

/// Build a Walrus client from network/defaults or explicit endpoint pair.
pub fn build_walrus_client(
    network: WalrusArchiveNetwork,
    caching_url: Option<&str>,
    aggregator_url: Option<&str>,
) -> Result<WalrusClient> {
    match (
        caching_url.map(str::trim).filter(|value| !value.is_empty()),
        aggregator_url
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) {
        (Some(caching), Some(aggregator)) => Ok(WalrusClient::new(
            caching.to_string(),
            aggregator.to_string(),
        )),
        (None, None) => Ok(match network {
            WalrusArchiveNetwork::Mainnet => WalrusClient::mainnet(),
            WalrusArchiveNetwork::Testnet => WalrusClient::testnet(),
        }),
        _ => Err(anyhow!(
            "provide both walrus_caching_url and walrus_aggregator_url for custom endpoints"
        )),
    }
}

/// Normalize package id to canonical hex-literal form.
pub fn normalize_package_id(package: &str) -> Result<String> {
    AccountAddress::from_hex_literal(package)
        .map(|address| address.to_hex_literal())
        .with_context(|| format!("invalid package id: {}", package))
}

/// Resolve concrete checkpoints to scan from explicit spec or rolling latest window.
pub fn resolve_discovery_checkpoints(
    walrus: &WalrusClient,
    checkpoint_spec: Option<&str>,
    latest: Option<u64>,
) -> Result<Vec<u64>> {
    if let Some(spec) = checkpoint_spec {
        return parse_checkpoint_spec(spec);
    }

    let latest_count = latest.unwrap_or(1);
    if latest_count == 0 {
        return Err(anyhow!("latest must be greater than zero"));
    }
    if latest_count > 500 {
        return Err(anyhow!("latest is capped at 500 checkpoints"));
    }

    let tip = walrus
        .get_latest_checkpoint()
        .context("failed to fetch latest checkpoint from Walrus")?;
    let start = tip.saturating_sub(latest_count.saturating_sub(1));
    Ok((start..=tip).collect())
}

/// Discover PTB replay targets across checkpoint(s), optionally package-filtered.
pub fn discover_checkpoint_targets(
    walrus: &WalrusClient,
    checkpoint_spec: Option<&str>,
    latest: Option<u64>,
    package_id: Option<&str>,
    include_framework: bool,
    limit: usize,
) -> Result<DiscoverOutput> {
    if limit == 0 {
        return Err(anyhow!("limit must be greater than zero"));
    }

    let checkpoints = resolve_discovery_checkpoints(walrus, checkpoint_spec, latest)?;
    let package_filter = match package_id {
        Some(pkg) => Some(normalize_package_id(pkg)?),
        None => None,
    };
    let filter_is_framework = package_filter
        .as_deref()
        .map(is_framework_package_id)
        .unwrap_or(false);

    let mut checkpoints_scanned = 0usize;
    let mut transactions_scanned = 0usize;
    let mut ptbs_scanned = 0usize;
    let mut targets = Vec::new();
    let mut truncated = false;

    'checkpoint_scan: for checkpoint in &checkpoints {
        checkpoints_scanned += 1;
        let checkpoint_data = walrus
            .get_checkpoint(*checkpoint)
            .with_context(|| format!("failed to fetch checkpoint {}", checkpoint))?;
        for tx in &checkpoint_data.transactions {
            transactions_scanned += 1;
            let tx_data = tx.transaction.data().transaction_data();
            let ptb = match tx_data.kind() {
                TransactionKind::ProgrammableTransaction(ptb) => ptb,
                _ => continue,
            };
            ptbs_scanned += 1;

            let mut move_calls = Vec::new();
            let mut package_ids: BTreeSet<String> = BTreeSet::new();
            for (command_index, command) in ptb.commands.iter().enumerate() {
                let SuiCommand::MoveCall(call) = command else {
                    continue;
                };
                let package = normalize_package_id(&call.package.to_hex_uncompressed())
                    .unwrap_or_else(|_| call.package.to_hex_uncompressed());
                let matches_filter = package_filter
                    .as_ref()
                    .map(|filter| filter == &package)
                    .unwrap_or(true);
                if !matches_filter {
                    continue;
                }
                if !include_framework && !filter_is_framework && is_framework_package_id(&package) {
                    continue;
                }
                package_ids.insert(package.clone());
                move_calls.push(DiscoverMoveCall {
                    command_index,
                    package,
                    module: call.module.to_string(),
                    function: call.function.to_string(),
                });
            }
            if move_calls.is_empty() {
                continue;
            }
            targets.push(DiscoverTarget {
                checkpoint: *checkpoint,
                digest: tx.transaction.digest().to_string(),
                sender: tx_data.sender().to_string(),
                commands: ptb.commands.len(),
                input_objects: tx.input_objects.len(),
                output_objects: tx.output_objects.len(),
                package_ids: package_ids.into_iter().collect(),
                move_calls,
            });
            if targets.len() >= limit {
                truncated = true;
                break 'checkpoint_scan;
            }
        }
    }

    Ok(DiscoverOutput {
        success: true,
        checkpoints_scanned,
        transactions_scanned,
        ptbs_scanned,
        matches: targets.len(),
        package_filter,
        include_framework,
        limit,
        truncated,
        checkpoints,
        targets,
    })
}

/// Resolve digest/checkpoint for replay when digest was omitted and discovery is requested.
pub fn resolve_replay_target_from_discovery(
    digest: Option<&str>,
    checkpoint: Option<u64>,
    state_supplied: bool,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    walrus: &WalrusClient,
) -> Result<(Option<String>, Option<u64>)> {
    if let Some(raw_digest) = digest {
        let trimmed = raw_digest.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("digest cannot be empty"));
        }
        return Ok((Some(trimmed.to_string()), checkpoint));
    }

    if state_supplied {
        return Ok((None, checkpoint));
    }

    let latest = discover_latest.ok_or_else(|| {
        anyhow!("provide digest, state_file, or discover_latest for replay target selection")
    })?;
    let package_id = discover_package_id
        .ok_or_else(|| anyhow!("discover_package_id is required when discover_latest is set"))?;

    let discovered =
        discover_checkpoint_targets(walrus, None, Some(latest), Some(package_id), false, 1)?;
    let target = discovered.targets.into_iter().next().ok_or_else(|| {
        anyhow!(
            "no digest discovered for package {} in latest {} checkpoint(s)",
            package_id,
            latest
        )
    })?;
    Ok((Some(target.digest), Some(target.checkpoint)))
}

fn is_framework_package_id(package: &str) -> bool {
    is_framework_address(package)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_checkpoint_range_spec() {
        let checkpoints = parse_checkpoint_spec("239615920..239615923").expect("range parse");
        assert_eq!(
            checkpoints,
            vec![239615920, 239615921, 239615922, 239615923]
        );
    }

    #[test]
    fn parses_checkpoint_list_spec_and_dedups() {
        let checkpoints = parse_checkpoint_spec("5, 3,5,7").expect("list parse");
        assert_eq!(checkpoints, vec![3, 5, 7]);
    }

    #[test]
    fn rejects_inverted_checkpoint_range() {
        let err = parse_checkpoint_spec("20..10").expect_err("inverted range should fail");
        assert!(err.to_string().contains("end must be >= start"));
    }

    #[test]
    fn rejects_partial_custom_endpoint_pair() {
        let err = build_walrus_client(
            WalrusArchiveNetwork::Mainnet,
            Some("https://example-caching.invalid"),
            None,
        )
        .expect_err("missing aggregator should fail");
        assert!(err.to_string().contains("provide both walrus_caching_url"));
    }

    #[test]
    fn accepts_full_custom_endpoint_pair() {
        let client = build_walrus_client(
            WalrusArchiveNetwork::Mainnet,
            Some("https://example-caching.invalid"),
            Some("https://example-aggregator.invalid"),
        )
        .expect("custom pair should construct client");
        let _ = client;
    }
}
