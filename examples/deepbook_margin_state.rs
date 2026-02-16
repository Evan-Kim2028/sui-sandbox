//! DeepBook Margin Manager State Query Example via generic historical-view API.
//!
//! Run:
//!   cargo run --example deepbook_margin_state
//!   DEEPBOOK_SCENARIO=position_a_snapshot cargo run --example deepbook_margin_state

use anyhow::{anyhow, Context, Result};
use std::path::Path;

use sui_sandbox_core::historical_view::HistoricalViewRequest;
use sui_sandbox_core::orchestrator::{ReplayOrchestrator, ReturnDecodeField};

mod deepbook_scenarios;

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    let scenario = deepbook_scenarios::resolve_scenario(Some("position_a_snapshot"))
        .and_then(|scenario| deepbook_scenarios::require_kind(&scenario, "snapshot").cloned())
        .map_err(|err| anyhow!("{}", err))?;
    let versions_path = scenario
        .versions_file
        .as_deref()
        .map(deepbook_scenarios::scenario_data_path)
        .ok_or_else(|| anyhow!("Scenario '{}' is missing 'versions_file'", scenario.id))?;
    let request_path = scenario
        .request_file
        .as_deref()
        .map(deepbook_scenarios::scenario_data_path)
        .ok_or_else(|| anyhow!("Scenario '{}' is missing 'request_file'", scenario.id))?;
    let request: HistoricalViewRequest = serde_json::from_str(
        &std::fs::read_to_string(&request_path)
            .with_context(|| format!("read request file: {}", request_path.display()))?,
    )
    .with_context(|| format!("parse request file: {}", request_path.display()))?;

    let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT").ok();
    let grpc_api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let request = maybe_update_target_manager(request, scenario.target_margin_manager.as_deref())
        .context("apply scenario target manager override")?;

    println!("\n=== DeepBook Margin manager_state (generic historical view) ===\n");
    println!(
        "scenario: {} ({})",
        scenario.id,
        scenario
            .description
            .unwrap_or_else(|| "no description".to_string())
    );
    println!("versions_file: {}", versions_path.display());
    println!("request_file: {}", request_path.display());

    let out = ReplayOrchestrator::execute_historical_view_from_versions(
        Path::new(&versions_path),
        &request,
        grpc_endpoint.as_deref(),
        grpc_api_key.as_deref(),
    )?;

    println!("checkpoint:   {}", out.checkpoint);
    println!("endpoint:     {}", out.grpc_endpoint);
    println!("success:      {}", out.success);
    println!("gas_used:     {}", out.gas_used.unwrap_or(0));

    if let Some(decoded) = decode_margin_state(&out.raw)? {
        println!("\ndecoded_margin_state:");
        println!("  risk_ratio_pct:   {:.6}", decoded.risk_ratio_pct);
        println!("  base_asset_sui:   {:.9}", decoded.base_asset_sui);
        println!("  quote_asset_usdc: {:.6}", decoded.quote_asset_usdc);
        println!("  base_debt_sui:    {:.9}", decoded.base_debt_sui);
        println!("  quote_debt_usdc:  {:.6}", decoded.quote_debt_usdc);
        println!("  current_price:    {:.6}", decoded.current_price);
    }

    if let Some(error) = out.error {
        println!("\nerror: {}", error);
    }
    if let Some(hint) = out.hint {
        println!("hint: {}", hint);
    }

    Ok(())
}

#[derive(Debug)]
struct MarginStateDecoded {
    risk_ratio_pct: f64,
    base_asset_sui: f64,
    quote_asset_usdc: f64,
    base_debt_sui: f64,
    quote_debt_usdc: f64,
    current_price: f64,
}

fn decode_margin_state(result: &serde_json::Value) -> Result<Option<MarginStateDecoded>> {
    let schema: Vec<ReturnDecodeField> = vec![
        ReturnDecodeField::scaled_u64(2, "risk_ratio_pct", 10_000_000.0),
        ReturnDecodeField::scaled_u64(3, "base_asset_sui", 1_000_000_000.0),
        ReturnDecodeField::scaled_u64(4, "quote_asset_usdc", 1_000_000.0),
        ReturnDecodeField::scaled_u64(5, "base_debt_sui", 1_000_000_000.0),
        ReturnDecodeField::scaled_u64(6, "quote_debt_usdc", 1_000_000.0),
        ReturnDecodeField::scaled_u64(11, "current_price", 1_000_000.0),
    ];
    let Some(decoded) = ReplayOrchestrator::decode_command_return_schema(result, 0, &schema)?
    else {
        return Ok(None);
    };

    Ok(Some(MarginStateDecoded {
        risk_ratio_pct: ReplayOrchestrator::decoded_number_field(&decoded, "risk_ratio_pct")?,
        base_asset_sui: ReplayOrchestrator::decoded_number_field(&decoded, "base_asset_sui")?,
        quote_asset_usdc: ReplayOrchestrator::decoded_number_field(&decoded, "quote_asset_usdc")?,
        base_debt_sui: ReplayOrchestrator::decoded_number_field(&decoded, "base_debt_sui")?,
        quote_debt_usdc: ReplayOrchestrator::decoded_number_field(&decoded, "quote_debt_usdc")?,
        current_price: ReplayOrchestrator::decoded_number_field(&decoded, "current_price")?,
    }))
}

fn maybe_update_target_manager(
    mut request: HistoricalViewRequest,
    target_margin_manager: Option<&str>,
) -> Result<HistoricalViewRequest> {
    let Some(target_margin_manager) = target_margin_manager else {
        return Ok(request);
    };

    let Some(pos) = request
        .required_objects
        .iter()
        .position(|object_id| object_id == target_margin_manager)
    else {
        if request.required_objects.is_empty() {
            return Err(anyhow!(
                "Scenario target margin manager '{}' does not match required_objects in request",
                target_margin_manager
            ));
        }
        request.required_objects[0] = target_margin_manager.to_string();
        return Ok(request);
    };

    request.required_objects[pos] = target_margin_manager.to_string();
    Ok(request)
}
