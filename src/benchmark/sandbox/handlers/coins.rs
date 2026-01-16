//! Coin-related sandbox handlers.
//!
//! Handles register_coin, get_coin_metadata, and list_coins operations.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;

/// Register a custom coin with its metadata.
pub fn execute_register_coin(
    env: &mut SimulationEnvironment,
    coin_type: &str,
    decimals: u8,
    symbol: &str,
    name: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Registering coin: {} ({} decimals)", coin_type, decimals);
    }

    env.register_coin(coin_type, decimals, symbol, name);

    SandboxResponse::success_with_data(serde_json::json!({
        "coin_type": coin_type,
        "decimals": decimals,
        "symbol": symbol,
        "name": name,
    }))
}

/// Get coin metadata for a registered coin.
pub fn execute_get_coin_metadata(
    env: &SimulationEnvironment,
    coin_type: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting coin metadata for: {}", coin_type);
    }

    match env.get_coin_metadata(coin_type) {
        Some(metadata) => SandboxResponse::success_with_data(serde_json::json!({
            "coin_type": coin_type,
            "decimals": metadata.decimals,
            "symbol": metadata.symbol,
            "name": metadata.name,
        })),
        None => SandboxResponse::error(format!("Coin {} not found in registry", coin_type)),
    }
}

/// List all registered coins.
pub fn execute_list_coins(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing registered coins");
    }

    let coins: Vec<serde_json::Value> = env
        .list_registered_coins()
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "coin_type": m.type_tag,
                "decimals": m.decimals,
                "symbol": m.symbol,
                "name": m.name,
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "coins": coins,
        "count": coins.len(),
    }))
}
