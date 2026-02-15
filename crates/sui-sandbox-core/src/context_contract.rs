//! Shared context payload contract used by CLI `context/*` and Python bindings.
//!
//! The canonical payload shape is v2 with:
//! - `packages`: array of `{address, modules, bytecodes}`
//! - compatibility mirrors: `with_deps` and `resolve_deps`
//!
//! Parsers also accept legacy/map shapes for backward compatibility.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use serde::{Deserialize, Serialize};

/// Canonical context payload schema version.
pub const CONTEXT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextPackage {
    pub address: String,
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default)]
    pub bytecodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextPayloadV2 {
    pub version: u32,
    pub package_id: String,
    pub with_deps: bool,
    pub resolve_deps: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,
    pub generated_at_ms: u64,
    #[serde(default)]
    pub packages_fetched: Vec<String>,
    #[serde(default)]
    pub packages: Vec<ContextPackage>,
    pub count: usize,
}

impl ContextPayloadV2 {
    pub fn new(
        package_id: impl Into<String>,
        with_deps: bool,
        generated_at_ms: u64,
        rpc_url: Option<String>,
        packages: Vec<ContextPackage>,
    ) -> Self {
        let packages_fetched = packages.iter().map(|pkg| pkg.address.clone()).collect();
        let count = packages.len();
        Self {
            version: CONTEXT_SCHEMA_VERSION,
            package_id: package_id.into(),
            with_deps,
            resolve_deps: with_deps,
            rpc_url,
            generated_at_ms,
            packages_fetched,
            packages,
            count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedContextPayload {
    pub package_id: Option<String>,
    pub with_deps: bool,
    pub packages: Vec<ContextPackage>,
}

/// Parse a context payload from wrapper (v1/v2) or direct package map/array form.
pub fn parse_context_payload(value: &serde_json::Value) -> Result<ParsedContextPayload> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(packages) = map.get("packages") {
                let package_id = map
                    .get("package_id")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
                let with_deps = map
                    .get("with_deps")
                    .and_then(serde_json::Value::as_bool)
                    .or_else(|| map.get("resolve_deps").and_then(serde_json::Value::as_bool))
                    .unwrap_or(true);
                return Ok(ParsedContextPayload {
                    package_id,
                    with_deps,
                    packages: decode_context_packages(packages)?,
                });
            }

            if looks_like_package_map(map) {
                return Ok(ParsedContextPayload {
                    package_id: None,
                    with_deps: true,
                    packages: decode_context_packages(value)?,
                });
            }

            Err(anyhow!("missing `packages` field"))
        }
        serde_json::Value::Array(_) => Ok(ParsedContextPayload {
            package_id: None,
            with_deps: true,
            packages: decode_context_packages(value)?,
        }),
        _ => Err(anyhow!(
            "unsupported context JSON type (expected object or array)"
        )),
    }
}

/// Decode context package payload from:
/// - `null`
/// - v2 array payload
/// - legacy package map payload: `{"0x..": ["base64...", ...]}`
pub fn decode_context_packages(value: &serde_json::Value) -> Result<Vec<ContextPackage>> {
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(_) => serde_json::from_value::<Vec<ContextPackage>>(value.clone())
            .context("array `packages` payload is invalid"),
        serde_json::Value::Object(map) => context_packages_from_package_map(map),
        _ => Err(anyhow!(
            "unsupported `packages` payload type in context payload"
        )),
    }
}

/// Convert a legacy package map payload into canonical context packages.
pub fn context_packages_from_package_map(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<ContextPackage>> {
    let mut out = Vec::with_capacity(map.len());
    for (address, encoded_modules) in map {
        let entries = encoded_modules
            .as_array()
            .ok_or_else(|| anyhow!("package {} in map payload must be an array", address))?;
        let mut modules = Vec::with_capacity(entries.len());
        let mut bytecodes = Vec::with_capacity(entries.len());
        for (idx, value) in entries.iter().enumerate() {
            let b64 = value
                .as_str()
                .ok_or_else(|| anyhow!("package {} module #{} is not a string", address, idx))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .with_context(|| {
                    format!(
                        "invalid base64 bytecode in package {} module #{}",
                        address, idx
                    )
                })?;
            modules.push(inferred_module_name(&bytes, idx));
            bytecodes.push(b64.to_string());
        }
        out.push(ContextPackage {
            address: address.clone(),
            modules,
            bytecodes,
        });
    }
    Ok(out)
}

/// Decode one context package into `(module_name, module_bytecode)` entries.
pub fn decode_context_package_modules(package: &ContextPackage) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::with_capacity(package.bytecodes.len());
    for (idx, encoded) in package.bytecodes.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .with_context(|| {
                format!(
                    "invalid base64 bytecode for {} module #{} in context payload",
                    package.address, idx
                )
            })?;
        let name = package
            .modules
            .get(idx)
            .cloned()
            .unwrap_or_else(|| inferred_module_name(&bytes, idx));
        out.push((name, bytes));
    }
    Ok(out)
}

pub fn inferred_module_name(bytes: &[u8], idx: usize) -> String {
    CompiledModule::deserialize_with_defaults(bytes)
        .ok()
        .map(|module| module.self_id().name().to_string())
        .unwrap_or_else(|| format!("module_{}", idx))
}

fn looks_like_package_map(map: &serde_json::Map<String, serde_json::Value>) -> bool {
    !map.is_empty() && map.keys().all(|key| key.starts_with("0x"))
}

#[cfg(test)]
mod tests {
    use super::{
        context_packages_from_package_map, decode_context_package_modules, parse_context_payload,
        ContextPayloadV2,
    };
    use serde_json::json;

    #[test]
    fn parses_python_v1_wrapper_shape() {
        let value = json!({
            "version": 1,
            "package_id": "0x2",
            "resolve_deps": true,
            "packages": {
                "0x2": []
            }
        });
        let parsed = parse_context_payload(&value).expect("parse python v1 wrapper");
        assert_eq!(parsed.package_id.as_deref(), Some("0x2"));
        assert!(parsed.with_deps);
        assert_eq!(parsed.packages.len(), 1);
        assert_eq!(parsed.packages[0].address, "0x2");
    }

    #[test]
    fn parses_cli_v2_wrapper_shape() {
        let payload = ContextPayloadV2::new(
            "0x2",
            true,
            0,
            Some("https://archive.mainnet.sui.io:443".to_string()),
            vec![],
        );
        let value = serde_json::to_value(payload).expect("serialize payload");
        let parsed = parse_context_payload(&value).expect("parse cli v2 wrapper");
        assert_eq!(parsed.package_id.as_deref(), Some("0x2"));
        assert!(parsed.with_deps);
        assert!(parsed.packages.is_empty());
    }

    #[test]
    fn parses_direct_package_map_shape() {
        let value = json!({
            "0x2": []
        });
        let parsed = parse_context_payload(&value).expect("parse map payload");
        assert_eq!(parsed.package_id, None);
        assert!(parsed.with_deps);
        assert_eq!(parsed.packages.len(), 1);
        assert_eq!(parsed.packages[0].address, "0x2");
    }

    #[test]
    fn decodes_context_package_modules() {
        let map = serde_json::json!({
            "0x2": ["AQIDBA=="] // [1,2,3,4]
        });
        let packages =
            context_packages_from_package_map(map.as_object().expect("object package map"))
                .expect("decode package map");
        let modules = decode_context_package_modules(&packages[0]).expect("decode module payload");
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].1, vec![1, 2, 3, 4]);
    }
}
