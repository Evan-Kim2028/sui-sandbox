use std::path::PathBuf;
pub use sui_transport::network::infer_network;
use sui_transport::network::resolve_graphql_endpoint as resolve_graphql;

pub fn sandbox_home() -> PathBuf {
    std::env::var("SUI_SANDBOX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".sui-sandbox")
        })
}

pub fn cache_dir(network: &str) -> PathBuf {
    sandbox_home().join("cache").join(network)
}

pub fn resolve_graphql_endpoint(rpc_url: &str) -> String {
    resolve_graphql(rpc_url)
}
