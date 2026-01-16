//! Sui JSON-RPC client helpers for fetching package data.
//!
//! Provides functions for resolving package addresses, fetching BCS module bytes,
//! and building normalized interface JSON from on-chain packages.

use crate::args::RetryConfig;
use crate::types::PackageInterfaceJson;
use crate::utils::{canonicalize_json_value, with_retries};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;
use sui_sdk::types::base_types::ObjectID;

pub async fn resolve_package_address_from_package_info(
    client: Arc<sui_sdk::SuiClient>,
    package_info_id: ObjectID,
    retry: RetryConfig,
) -> Result<ObjectID> {
    let options = sui_sdk::rpc_types::SuiObjectDataOptions::new()
        .with_type()
        .with_content();

    let resp = with_retries(
        retry.retries,
        retry.initial_backoff,
        retry.max_backoff,
        || {
            let client = Arc::clone(&client);
            let options = options.clone();
            async move {
                client
                    .read_api()
                    .get_object_with_options(package_info_id, options)
                    .await
                    .with_context(|| format!("fetch object {}", package_info_id))
            }
        },
    )
    .await?;

    let value = serde_json::to_value(&resp).context("serialize object response")?;
    let package_address = value
        .get("data")
        .and_then(|d| d.get("content"))
        .and_then(|c| c.get("fields"))
        .and_then(|f| f.get("package_address"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!(
                "object {} missing content.fields.package_address",
                package_info_id
            )
        })?;

    ObjectID::from_str(package_address)
        .map_err(|e| anyhow!("invalid package_address {}: {}", package_address, e))
}

pub async fn fetch_bcs_module_names(
    client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<Vec<String>> {
    let options = sui_sdk::rpc_types::SuiObjectDataOptions::new().with_bcs();
    let resp = with_retries(
        retry.retries,
        retry.initial_backoff,
        retry.max_backoff,
        || {
            let client = Arc::clone(&client);
            let options = options.clone();
            async move {
                client
                    .read_api()
                    .get_object_with_options(package_id, options)
                    .await
                    .with_context(|| format!("fetch package bcs {}", package_id))
            }
        },
    )
    .await?;

    let value = serde_json::to_value(&resp).context("serialize object response")?;
    let module_map = value
        .get("data")
        .and_then(|d| d.get("bcs"))
        .and_then(|b| b.get("moduleMap"))
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("object {} missing data.bcs.moduleMap", package_id))?;

    let mut names: Vec<String> = module_map.keys().cloned().collect();
    names.sort();
    Ok(names)
}

pub async fn fetch_bcs_module_map_bytes(
    client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<Vec<(String, Vec<u8>)>> {
    use base64::Engine;
    let options = sui_sdk::rpc_types::SuiObjectDataOptions::new().with_bcs();
    let resp = with_retries(
        retry.retries,
        retry.initial_backoff,
        retry.max_backoff,
        || {
            let client = Arc::clone(&client);
            let options = options.clone();
            async move {
                client
                    .read_api()
                    .get_object_with_options(package_id, options)
                    .await
                    .with_context(|| format!("fetch package bcs {}", package_id))
            }
        },
    )
    .await?;

    let value = serde_json::to_value(&resp).context("serialize object response")?;
    let module_map = value
        .get("data")
        .and_then(|d| d.get("bcs"))
        .and_then(|b| b.get("moduleMap"))
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("object {} missing data.bcs.moduleMap", package_id))?;

    let mut out: Vec<(String, Vec<u8>)> = Vec::with_capacity(module_map.len());
    for (name, v) in module_map {
        let bytes: Vec<u8> = match v {
            Value::String(s) => base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .with_context(|| format!("base64 decode moduleMap[{}] for {}", name, package_id))?,
            Value::Array(arr) => {
                let mut b = Vec::with_capacity(arr.len());
                for x in arr {
                    let n = x
                        .as_u64()
                        .ok_or_else(|| anyhow!("moduleMap[{}] contains non-u64 byte", name))?;
                    if n > 255 {
                        return Err(anyhow!(
                            "moduleMap[{}] contains out-of-range byte {}",
                            name,
                            n
                        ));
                    }
                    b.push(n as u8);
                }
                b
            }
            _ => {
                return Err(anyhow!(
                    "moduleMap[{}] unexpected JSON type (expected string/array)",
                    name
                ))
            }
        };
        out.push((name.clone(), bytes));
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub async fn build_interface_value_for_package(
    client: Arc<sui_sdk::SuiClient>,
    package_id: ObjectID,
    retry: RetryConfig,
) -> Result<(Vec<String>, Value)> {
    let modules = with_retries(
        retry.retries,
        retry.initial_backoff,
        retry.max_backoff,
        || {
            let client = Arc::clone(&client);
            async move {
                client
                    .read_api()
                    .get_normalized_move_modules_by_package(package_id)
                    .await
                    .with_context(|| format!("fetch normalized modules for {}", package_id))
            }
        },
    )
    .await?;

    let mut module_names: Vec<String> = modules.keys().cloned().collect();
    module_names.sort();

    let mut modules_value =
        serde_json::to_value(&modules).context("serialize normalized modules")?;
    canonicalize_json_value(&mut modules_value);

    let interface = PackageInterfaceJson {
        schema_version: 1,
        package_id: package_id.to_string(),
        module_names: module_names.clone(),
        modules: modules_value,
    };

    let mut interface_value = serde_json::to_value(interface).context("build interface JSON")?;
    canonicalize_json_value(&mut interface_value);

    Ok((module_names, interface_value))
}
