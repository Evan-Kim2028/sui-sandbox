//! DeepBook Margin Manager State Query Example via generic historical-view API.
//!
//! Run:
//!   cargo run --example deepbook_margin_state
//!   VERSIONS_FILE=./examples/advanced/deepbook_margin_state/data/deepbook_versions_240733000.json cargo run --example deepbook_margin_state

use anyhow::{anyhow, Result};
use std::path::PathBuf;

use sui_sandbox_core::historical_view::HistoricalViewRequest;
use sui_sandbox_core::orchestrator::{ReplayOrchestrator, ReturnDecodeField};

const MARGIN_PACKAGE: &str = "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b";
const DEEPBOOK_SPOT_PACKAGE: &str =
    "0x337f4f4f6567fcd778d5454f27c16c70e2f274cc6377ea6249ddf491482ef497";
const MARGIN_REGISTRY: &str = "0x0e40998b359a9ccbab22a98ed21bd4346abf19158bc7980c8291908086b3a742";
const TARGET_MARGIN_MANAGER: &str =
    "0xed7a38b242141836f99f16ea62bd1182bcd8122d1de2f1ae98b80acbc2ad5c80";
const DEEPBOOK_POOL: &str = "0xe05dafb5133bcffb8d59f4e12465dc0e9faeaa05e3e342a08fe135800e3e4407";
const BASE_MARGIN_POOL: &str = "0x53041c6f86c4782aabbfc1d4fe234a6d37160310c7ee740c915f0a01b7127344";
const QUOTE_MARGIN_POOL: &str =
    "0xba473d9ae278f10af75c50a8fa341e9c6a1c087dc91a3f23e8048baf67d0754f";
const CLOCK: &str = "0x6";
const SUI_PYTH_PRICE_INFO: &str =
    "0x801dbc2f0053d34734814b2d6df491ce7807a725fe9a01ad74a07e9c51396c37";
const USDC_PYTH_PRICE_INFO: &str =
    "0x5dec622733a204ca27f5a90d8c2fad453cc6665186fd5dff13a83d0b6c9027ab";
const SUI_TYPE: &str = "0x2::sui::SUI";
const USDC_TYPE: &str =
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";
const DEFAULT_VERSIONS_FILE: &str =
    "examples/advanced/deepbook_margin_state/data/deepbook_versions_240733000.json";

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    let versions_file = std::env::var("VERSIONS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_VERSIONS_FILE));
    let grpc_endpoint = std::env::var("SUI_GRPC_ENDPOINT").ok();
    let grpc_api_key = std::env::var("SUI_GRPC_API_KEY").ok();

    let request = HistoricalViewRequest {
        package_id: MARGIN_PACKAGE.to_string(),
        module: "margin_manager".to_string(),
        function: "manager_state".to_string(),
        type_args: vec![SUI_TYPE.to_string(), USDC_TYPE.to_string()],
        required_objects: vec![
            TARGET_MARGIN_MANAGER.to_string(),
            MARGIN_REGISTRY.to_string(),
            SUI_PYTH_PRICE_INFO.to_string(),
            USDC_PYTH_PRICE_INFO.to_string(),
            DEEPBOOK_POOL.to_string(),
            BASE_MARGIN_POOL.to_string(),
            QUOTE_MARGIN_POOL.to_string(),
            CLOCK.to_string(),
        ],
        package_roots: vec![
            MARGIN_PACKAGE.to_string(),
            DEEPBOOK_SPOT_PACKAGE.to_string(),
        ],
        type_refs: vec![SUI_TYPE.to_string(), USDC_TYPE.to_string()],
        fetch_child_objects: true,
    };

    println!("\n=== DeepBook Margin manager_state (generic historical view) ===\n");
    println!("versions_file: {}", versions_file.display());

    let out = ReplayOrchestrator::execute_historical_view_from_versions(
        &versions_file,
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
    let schema = vec![
        ReturnDecodeField {
            index: 2,
            name: "risk_ratio_pct".to_string(),
            type_hint: Some("u64".to_string()),
            scale: Some(10_000_000.0), // u64 / 1e9 * 100
        },
        ReturnDecodeField {
            index: 3,
            name: "base_asset_sui".to_string(),
            type_hint: Some("u64".to_string()),
            scale: Some(1_000_000_000.0),
        },
        ReturnDecodeField {
            index: 4,
            name: "quote_asset_usdc".to_string(),
            type_hint: Some("u64".to_string()),
            scale: Some(1_000_000.0),
        },
        ReturnDecodeField {
            index: 5,
            name: "base_debt_sui".to_string(),
            type_hint: Some("u64".to_string()),
            scale: Some(1_000_000_000.0),
        },
        ReturnDecodeField {
            index: 6,
            name: "quote_debt_usdc".to_string(),
            type_hint: Some("u64".to_string()),
            scale: Some(1_000_000.0),
        },
        ReturnDecodeField {
            index: 11,
            name: "current_price".to_string(),
            type_hint: Some("u64".to_string()),
            scale: Some(1_000_000.0),
        },
    ];

    let Some(decoded) = ReplayOrchestrator::decode_command_return_schema(result, 0, &schema)?
    else {
        return Ok(None);
    };

    Ok(Some(MarginStateDecoded {
        risk_ratio_pct: json_number(&decoded, "risk_ratio_pct")?,
        base_asset_sui: json_number(&decoded, "base_asset_sui")?,
        quote_asset_usdc: json_number(&decoded, "quote_asset_usdc")?,
        base_debt_sui: json_number(&decoded, "base_debt_sui")?,
        quote_debt_usdc: json_number(&decoded, "quote_debt_usdc")?,
        current_price: json_number(&decoded, "current_price")?,
    }))
}

fn json_number(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Result<f64> {
    let value = map
        .get(key)
        .ok_or_else(|| anyhow!("decoded field '{}' missing", key))?;
    match value {
        serde_json::Value::Number(num) => num
            .as_f64()
            .ok_or_else(|| anyhow!("decoded field '{}' is not f64-representable", key)),
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|e| anyhow!("decoded field '{}' parse error: {}", key, e)),
        serde_json::Value::Null => Ok(0.0),
        other => Err(anyhow!(
            "decoded field '{}' has non-numeric type: {}",
            key,
            other
        )),
    }
}
