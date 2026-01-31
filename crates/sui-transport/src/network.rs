const MAINNET_GRAPHQL: &str = "https://graphql.mainnet.sui.io/graphql";
const TESTNET_GRAPHQL: &str = "https://graphql.testnet.sui.io/graphql";
const DEVNET_GRAPHQL: &str = "https://graphql.devnet.sui.io/graphql";

pub fn infer_network_from_url(url: &str) -> Option<&'static str> {
    let lower = url.to_lowercase();
    if lower.contains("testnet") {
        Some("testnet")
    } else if lower.contains("devnet") {
        Some("devnet")
    } else if lower.contains("mainnet") {
        Some("mainnet")
    } else {
        None
    }
}

pub fn infer_network_from_endpoints(
    rpc_url: Option<&str>,
    graphql_url: Option<&str>,
) -> Option<&'static str> {
    rpc_url
        .and_then(infer_network_from_url)
        .or_else(|| graphql_url.and_then(infer_network_from_url))
}

pub fn infer_network(rpc_url: &str, graphql_url: &str) -> String {
    infer_network_from_endpoints(Some(rpc_url), Some(graphql_url))
        .unwrap_or("mainnet")
        .to_string()
}

pub fn default_graphql_endpoint(network: &str) -> String {
    match network {
        "testnet" => TESTNET_GRAPHQL.to_string(),
        "devnet" => DEVNET_GRAPHQL.to_string(),
        _ => MAINNET_GRAPHQL.to_string(),
    }
}

pub fn resolve_graphql_endpoint(rpc_url: &str) -> String {
    if let Ok(value) = std::env::var("SUI_GRAPHQL_ENDPOINT") {
        if !value.trim().is_empty() {
            return value;
        }
    }

    let rpc_lower = rpc_url.to_lowercase();
    if rpc_lower.contains("graphql") {
        return rpc_url.to_string();
    }

    match infer_network_from_url(rpc_url) {
        Some("testnet") => TESTNET_GRAPHQL.to_string(),
        Some("devnet") => DEVNET_GRAPHQL.to_string(),
        _ => MAINNET_GRAPHQL.to_string(),
    }
}
