use super::*;

pub(crate) fn parse_walrus_archive_network(network: &str) -> Result<CoreWalrusArchiveNetwork> {
    CoreWalrusArchiveNetwork::parse(network)
}

pub(crate) fn build_walrus_client(
    network: CoreWalrusArchiveNetwork,
    caching_url: Option<&str>,
    aggregator_url: Option<&str>,
) -> Result<WalrusClient> {
    core_build_walrus_client(network, caching_url, aggregator_url)
}

pub(crate) fn resolve_protocol_package_id(
    protocol: &str,
    package_id: Option<&str>,
) -> Result<String> {
    let parsed = CoreProtocolAdapter::parse(protocol)?;
    core_resolve_required_package_id(parsed, package_id)
}

pub(crate) fn resolve_protocol_discovery_package_filter(
    protocol: &str,
    package_id: Option<&str>,
) -> Result<Option<String>> {
    let parsed = CoreProtocolAdapter::parse(protocol)?;
    core_resolve_discovery_package_filter(parsed, package_id)
}

pub(crate) fn discover_checkpoint_targets_inner(
    checkpoint: Option<&str>,
    latest: Option<u64>,
    package_id: Option<&str>,
    include_framework: bool,
    limit: usize,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<serde_json::Value> {
    let network = parse_walrus_archive_network(walrus_network)?;
    let walrus = build_walrus_client(network, walrus_caching_url, walrus_aggregator_url)?;
    let output = core_discover_checkpoint_targets(
        &walrus,
        checkpoint,
        latest,
        package_id,
        include_framework,
        limit,
    )?;
    serde_json::to_value(output).context("failed to serialize checkpoint discovery output")
}

pub(crate) fn resolve_replay_target_from_discovery(
    digest: Option<&str>,
    checkpoint: Option<u64>,
    state_file: Option<&str>,
    discover_latest: Option<u64>,
    discover_package_id: Option<&str>,
    walrus_network: &str,
    walrus_caching_url: Option<&str>,
    walrus_aggregator_url: Option<&str>,
) -> Result<(Option<String>, Option<u64>)> {
    let network = parse_walrus_archive_network(walrus_network)?;
    let walrus = build_walrus_client(network, walrus_caching_url, walrus_aggregator_url)?;
    core_resolve_replay_target_from_discovery(
        digest,
        checkpoint,
        state_file.is_some(),
        discover_latest,
        discover_package_id,
        &walrus,
    )
}

pub(crate) fn resolve_grpc_endpoint_and_key(
    endpoint: Option<&str>,
    api_key: Option<&str>,
) -> (String, Option<String>) {
    const MAINNET_ARCHIVE_GRPC: &str = "https://archive.mainnet.sui.io:443";
    let endpoint_explicit = endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some();
    let (default_endpoint, default_api_key) = historical_endpoint_and_api_key_from_env();
    let mut resolved_endpoint = endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or(default_endpoint);
    if resolved_endpoint
        .to_ascii_lowercase()
        .contains("fullnode.mainnet.sui.io")
    {
        resolved_endpoint = MAINNET_ARCHIVE_GRPC.to_string();
    }
    let explicit_api_key = api_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let sui_api_key = std::env::var("SUI_GRPC_API_KEY")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let resolved_api_key = if endpoint_explicit {
        // Keep explicit endpoint behavior predictable:
        // explicit arg > SUI_GRPC_API_KEY > no key.
        explicit_api_key.or(sui_api_key)
    } else {
        explicit_api_key.or(default_api_key)
    };
    (resolved_endpoint, resolved_api_key)
}
