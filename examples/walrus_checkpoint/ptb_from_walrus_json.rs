use anyhow::{anyhow, Context, Result};
use base64::Engine;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;

use sui_sandbox_core::ptb::{Argument, Command, InputValue, ObjectID, ObjectInput};
use sui_transport::grpc::{GrpcClient, GrpcOwner};
use super::replay_engine::ObjectCache;
use sui_transport::walrus::WalrusClient;
use sui_historical_cache::{CacheMetrics, ObjectVersionStore};

pub struct ParsedWalrusPtb {
    pub sender: AccountAddress,
    pub timestamp_ms: Option<u64>,
    pub gas_budget: Option<u64>,
    pub inputs: Vec<InputValue>,
    pub commands: Vec<Command>,
    pub package_ids: Vec<AccountAddress>,
}

pub fn parse_ptb_transaction(
    walrus: &WalrusClient,
    tx_json: &Value,
    grpc_fallback: Option<(&GrpcClient, &tokio::runtime::Runtime)>,
    object_cache: Option<&ObjectCache>,
    object_versions: Option<&HashMap<String, u64>>,
    disk_cache: Option<&dyn ObjectVersionStore>,
    metrics: Option<&CacheMetrics>,
) -> Result<ParsedWalrusPtb> {
    let v1 = tx_json
        .pointer("/transaction/data/0/intent_message/value/V1")
        .ok_or_else(|| anyhow!("missing /transaction/data/0/intent_message/value/V1"))?;

    let sender_str = v1
        .get("sender")
        .and_then(|v| v.as_str())
        .context("missing sender")?;
    let sender = AccountAddress::from_hex_literal(sender_str).context("invalid sender")?;

    let gas_budget = v1
        .get("gas_data")
        .and_then(|g| g.get("budget"))
        .and_then(|b| b.as_u64());

    let timestamp_ms = tx_json
        .get("timestamp_ms")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            // Some Walrus JSON embeds timestamp under checkpoint summary; keep this best-effort.
            tx_json
                .pointer("/checkpoint_summary/timestamp_ms")
                .and_then(|v| v.as_u64())
        });

    let ptb = v1
        .pointer("/kind/ProgrammableTransaction")
        .ok_or_else(|| anyhow!("not a ProgrammableTransaction"))?;

    let (inputs, gas_coin_arg_map) =
        parse_inputs(
            walrus,
            v1,
            ptb,
            tx_json,
            grpc_fallback,
            object_cache,
            object_versions,
            disk_cache,
            metrics,
        )?;
    let (commands, package_ids) = parse_commands(walrus, ptb, &gas_coin_arg_map)?;

    Ok(ParsedWalrusPtb {
        sender,
        timestamp_ms,
        gas_budget,
        inputs,
        commands,
        package_ids,
    })
}

fn parse_inputs(
    walrus: &WalrusClient,
    v1: &Value,
    ptb: &Value,
    tx_json: &Value,
    grpc_fallback: Option<(&GrpcClient, &tokio::runtime::Runtime)>,
    object_cache: Option<&ObjectCache>,
    object_versions: Option<&HashMap<String, u64>>,
    disk_cache: Option<&dyn ObjectVersionStore>,
    metrics: Option<&CacheMetrics>,
) -> Result<(Vec<InputValue>, GasCoinArgMap)> {
    let input_objects = tx_json
        .get("input_objects")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing input_objects"))?;

    let mut object_data_by_id: HashMap<ObjectID, WalrusObjectData> = HashMap::new();
    for obj_json in input_objects {
        let Some(move_obj) = obj_json.get("data").and_then(|d| d.get("Move")) else {
            continue;
        };

        // Some Walrus entries may include a `Move` wrapper but omit `contents` (e.g. redacted or
        // non-materialized). Skip these and let the later fallback paths (cache/gRPC) fill them in.
        let Some(contents_b64) = move_obj.get("contents").and_then(|c| c.as_str()) else {
            continue;
        };
        let bcs_bytes = base64::engine::general_purpose::STANDARD
            .decode(contents_b64)
            .context("base64 decode Move.contents")?;

        if bcs_bytes.len() < 32 {
            return Err(anyhow!("Move.contents too short to contain object id"));
        }
        let object_id = AccountAddress::new({
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&bcs_bytes[0..32]);
            bytes
        });

        let Some(version) = move_obj.get("version").and_then(|v| v.as_u64()) else {
            continue;
        };

        let owner_json = obj_json.get("owner").unwrap_or(&Value::Null);
        let is_immutable = owner_json.get("Immutable").is_some();

        let Some(type_json) = move_obj.get("type_") else {
            continue;
        };
        let type_tag = walrus_type_tag(walrus, type_json)?;

        object_data_by_id.insert(
            object_id,
            WalrusObjectData {
                bcs_bytes,
                type_tag,
                version,
                is_immutable,
            },
        );
    }

    // Inputs array contains both Pure and Object entries.
    let ptb_inputs = ptb
        .get("inputs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing ptb.inputs"))?;

    // Gas coin is referenced via Argument::GasCoin in Sui transaction format; our sandbox
    // representation doesn't have that variant, so we materialize it as an extra input.
    let gas_payment_ref = v1
        .get("gas_data")
        .and_then(|g| g.get("payment"))
        .and_then(|p| p.as_array())
        .and_then(|p| p.first())
        .and_then(extract_object_ref_id_version);
    let gas_coin_id = gas_payment_ref
        .as_ref()
        .and_then(|(id, _)| AccountAddress::from_hex_literal(id).ok());

    let timestamp_ms = tx_json
        .get("timestamp_ms")
        .and_then(|v| v.as_u64())
        .or_else(|| tx_json.pointer("/checkpoint_summary/timestamp_ms").and_then(|v| v.as_u64()));

    let mut inputs: Vec<InputValue> = Vec::with_capacity(ptb_inputs.len() + 1);
    for inp in ptb_inputs {
        if let Some(pure) = inp.get("Pure") {
            let bytes = decode_pure_bytes(pure)?;
            inputs.push(InputValue::Pure(bytes));
            continue;
        }

        if let Some(obj) = inp.get("Object") {
            let (id, mode, version_hint) = parse_object_ref(obj)?;
            let data = if let Some(d) = object_data_by_id.get(&id) {
                if let Some(m) = metrics {
                    m.record_walrus_hit();
                }
                d.clone()
            } else if let Some(cache) = object_cache {
                let cached = if let Some(ver) = version_hint.or_else(|| {
                    object_versions
                        .and_then(|m| m.get(&normalize_addr(&id.to_hex_literal())))
                        .copied()
                }) {
                    cache.get(id, ver).map(|entry| WalrusObjectData {
                        bcs_bytes: entry.bytes.clone(),
                        type_tag: entry.type_tag.clone(),
                        version: entry.version,
                        is_immutable: false,
                    })
                } else {
                    cache.get_any(id).map(|entry| WalrusObjectData {
                        bcs_bytes: entry.bytes.clone(),
                        type_tag: entry.type_tag.clone(),
                        version: entry.version,
                        is_immutable: false,
                    })
                };
                if let Some(cached) = cached {
                    if let Some(m) = metrics {
                        m.record_memory_hit();
                    }
                    cached
                } else if let Some(disk) = disk_cache {
                    // Try disk cache before gRPC
                    let version = version_hint.or_else(|| {
                        object_versions
                            .and_then(|m| m.get(&normalize_addr(&id.to_hex_literal())))
                            .copied()
                    });
                    if let Some(ver) = version {
                        if let Ok(Some(cached_obj)) = disk.get(id, ver) {
                            if let Some(m) = metrics {
                                m.record_disk_hit();
                            }
                            let type_tag = TypeTag::from_str(&cached_obj.meta.type_tag)
                                .map_err(|e| anyhow!("Failed to parse type tag from disk cache: {}", e))?;
                            let data = WalrusObjectData {
                                bcs_bytes: cached_obj.bcs_bytes,
                                type_tag,
                                version: ver,
                                is_immutable: false,
                            };
                            object_data_by_id.insert(id, data.clone());
                            data
                        } else if let Some(sys) =
                            synthesize_system_object(id, timestamp_ms, version_hint, object_versions)
                        {
                            object_data_by_id.insert(id, sys.clone());
                            sys
                        } else if let Some((grpc, rt)) = grpc_fallback {
                            if let Some(m) = metrics {
                                m.record_grpc_fetch();
                            }
                            let fetched = fetch_missing_object_data(
                                grpc,
                                rt,
                                tx_json,
                                id,
                                version_hint,
                                object_versions,
                            )?;
                            object_data_by_id.insert(id, fetched.clone());
                            fetched
                        } else {
                            return Err(anyhow!("missing object data for {}", id.to_hex_literal()));
                        }
                    } else if let Some(sys) =
                        synthesize_system_object(id, timestamp_ms, version_hint, object_versions)
                    {
                        object_data_by_id.insert(id, sys.clone());
                        sys
                    } else if let Some((grpc, rt)) = grpc_fallback {
                        if let Some(m) = metrics {
                            m.record_grpc_fetch();
                        }
                        let fetched = fetch_missing_object_data(
                            grpc,
                            rt,
                            tx_json,
                            id,
                            version_hint,
                            object_versions,
                        )?;
                        object_data_by_id.insert(id, fetched.clone());
                        fetched
                    } else {
                        return Err(anyhow!("missing object data for {}", id.to_hex_literal()));
                    }
                } else if let Some(sys) =
                    synthesize_system_object(id, timestamp_ms, version_hint, object_versions)
                {
                    object_data_by_id.insert(id, sys.clone());
                    sys
                } else if let Some((grpc, rt)) = grpc_fallback {
                    if let Some(m) = metrics {
                        m.record_grpc_fetch();
                    }
                    let fetched = fetch_missing_object_data(
                        grpc,
                        rt,
                        tx_json,
                        id,
                        version_hint,
                        object_versions,
                    )?;
                    object_data_by_id.insert(id, fetched.clone());
                    fetched
                } else {
                    return Err(anyhow!("missing object data for {}", id.to_hex_literal()));
                }
            } else if let Some(disk) = disk_cache {
                // Try disk cache before gRPC
                let version = version_hint.or_else(|| {
                    object_versions
                        .and_then(|m| m.get(&normalize_addr(&id.to_hex_literal())))
                        .copied()
                });
                if let Some(ver) = version {
                    if let Ok(Some(cached_obj)) = disk.get(id, ver) {
                        if let Some(m) = metrics {
                            m.record_disk_hit();
                        }
                        let type_tag = TypeTag::from_str(&cached_obj.meta.type_tag)
                            .map_err(|e| anyhow!("Failed to parse type tag from disk cache: {}", e))?;
                        let data = WalrusObjectData {
                            bcs_bytes: cached_obj.bcs_bytes,
                            type_tag,
                            version: ver,
                            is_immutable: false,
                        };
                        object_data_by_id.insert(id, data.clone());
                        data
                    } else if let Some((grpc, rt)) = grpc_fallback {
                        if let Some(m) = metrics {
                            m.record_grpc_fetch();
                        }
                        let fetched = fetch_missing_object_data(
                            grpc,
                            rt,
                            tx_json,
                            id,
                            version_hint,
                            object_versions,
                        )?;
                        object_data_by_id.insert(id, fetched.clone());
                        fetched
                    } else {
                        return Err(anyhow!("missing object data for {}", id.to_hex_literal()));
                    }
                } else if let Some((grpc, rt)) = grpc_fallback {
                    if let Some(m) = metrics {
                        m.record_grpc_fetch();
                    }
                    let fetched = fetch_missing_object_data(
                        grpc,
                        rt,
                        tx_json,
                        id,
                        version_hint,
                        object_versions,
                    )?;
                    object_data_by_id.insert(id, fetched.clone());
                    fetched
                } else {
                    return Err(anyhow!("missing object data for {}", id.to_hex_literal()));
                }
            } else if let Some((grpc, rt)) = grpc_fallback {
                if let Some(m) = metrics {
                    m.record_grpc_fetch();
                }
                let fetched = fetch_missing_object_data(
                    grpc,
                    rt,
                    tx_json,
                    id,
                    version_hint,
                    object_versions,
                )?;
                object_data_by_id.insert(id, fetched.clone());
                fetched
            } else {
                return Err(anyhow!("missing object data for {}", id.to_hex_literal()));
            };

            let object_input = match mode {
                WalrusObjectMode::Shared => ObjectInput::Shared {
                    id,
                    bytes: data.bcs_bytes.clone(),
                    type_tag: Some(data.type_tag.clone()),
                    version: Some(data.version),
                },
                WalrusObjectMode::Receiving => ObjectInput::Receiving {
                    id,
                    bytes: data.bcs_bytes.clone(),
                    type_tag: Some(data.type_tag.clone()),
                    parent_id: None,
                    version: Some(data.version),
                },
                WalrusObjectMode::ImmOrOwned => {
                    if data.is_immutable {
                        ObjectInput::ImmRef {
                            id,
                            bytes: data.bcs_bytes.clone(),
                            type_tag: Some(data.type_tag.clone()),
                            version: Some(data.version),
                        }
                    } else {
                        // Treat ImmOrOwned objects as Owned. This matches Sui's “ImmOrOwnedObject”
                        // semantics: the transaction can borrow immutably or take by value depending
                        // on command usage; Owned is the most permissive input mode in our harness.
                        ObjectInput::Owned {
                            id,
                            bytes: data.bcs_bytes.clone(),
                            type_tag: Some(data.type_tag.clone()),
                            version: Some(data.version),
                        }
                    }
                }
            };
            inputs.push(InputValue::Object(object_input));
            continue;
        }

        return Err(anyhow!("unsupported ptb input: {}", inp));
    }

    // Append gas coin input if we can resolve it from input_objects.
    let gas_coin_input_idx = if let Some(gas_id) = gas_coin_id {
        if !inputs.iter().any(|iv| matches!(iv, InputValue::Object(oi) if oi.id() == &gas_id)) {
            // Try to resolve gas coin bytes from Walrus content, cache, or gRPC.
            let version_hint = gas_payment_ref.as_ref().and_then(|(_, v)| *v);
            let data = if let Some(d) = object_data_by_id.get(&gas_id) {
                if let Some(m) = metrics {
                    m.record_walrus_hit();
                }
                Some(d.clone())
            } else if let Some(cache) = object_cache {
                cache.get_any(gas_id).map(|entry| {
                    if let Some(m) = metrics {
                        m.record_memory_hit();
                    }
                    WalrusObjectData {
                    bcs_bytes: entry.bytes.clone(),
                    type_tag: entry.type_tag.clone(),
                    version: entry.version,
                    is_immutable: false,
                    }
                })
            } else if let Some(disk) = disk_cache {
                // Try disk cache for gas coin
                let version = version_hint.or_else(|| {
                    object_versions
                        .and_then(|m| m.get(&normalize_addr(&gas_id.to_hex_literal())))
                        .copied()
                });
                if let Some(ver) = version {
                    if let Ok(Some(cached_obj)) = disk.get(gas_id, ver) {
                        if let Some(m) = metrics {
                            m.record_disk_hit();
                        }
                        let type_tag = TypeTag::from_str(&cached_obj.meta.type_tag)
                            .map_err(|e| anyhow!("Failed to parse type tag from disk cache: {}", e))?;
                        Some(WalrusObjectData {
                            bcs_bytes: cached_obj.bcs_bytes,
                            type_tag,
                            version: ver,
                            is_immutable: false,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let data = if let Some(d) = data {
                d
            } else if let Some(disk) = disk_cache {
                // Try disk cache with version hint
                let version = version_hint.or_else(|| {
                    object_versions
                        .and_then(|m| m.get(&normalize_addr(&gas_id.to_hex_literal())))
                        .copied()
                });
                if let Some(ver) = version {
                    if let Ok(Some(cached_obj)) = disk.get(gas_id, ver) {
                        if let Some(m) = metrics {
                            m.record_disk_hit();
                        }
                        let type_tag = TypeTag::from_str(&cached_obj.meta.type_tag)
                            .map_err(|e| anyhow!("Failed to parse type tag from disk cache: {}", e))?;
                        let data = WalrusObjectData {
                            bcs_bytes: cached_obj.bcs_bytes,
                            type_tag,
                            version: ver,
                            is_immutable: false,
                        };
                        object_data_by_id.insert(gas_id, data.clone());
                        data
                    } else if let Some((grpc, rt)) = grpc_fallback {
                        if let Some(m) = metrics {
                            m.record_grpc_fetch();
                        }
                        let fetched = fetch_missing_object_data(
                            grpc,
                            rt,
                            tx_json,
                            gas_id,
                            version_hint,
                            object_versions,
                        )?;
                        object_data_by_id.insert(gas_id, fetched.clone());
                        fetched
                    } else {
                        return Ok((inputs, GasCoinArgMap { gas_coin_input_idx: None }));
                    }
                } else if let Some((grpc, rt)) = grpc_fallback {
                    if let Some(m) = metrics {
                        m.record_grpc_fetch();
                    }
                    let fetched = fetch_missing_object_data(
                        grpc,
                        rt,
                        tx_json,
                        gas_id,
                        version_hint,
                        object_versions,
                    )?;
                    object_data_by_id.insert(gas_id, fetched.clone());
                    fetched
                } else {
                    return Ok((inputs, GasCoinArgMap { gas_coin_input_idx: None }));
                }
            } else if let Some((grpc, rt)) = grpc_fallback {
                if let Some(m) = metrics {
                    m.record_grpc_fetch();
                }
                let fetched = fetch_missing_object_data(
                    grpc,
                    rt,
                    tx_json,
                    gas_id,
                    version_hint,
                    object_versions,
                )?;
                object_data_by_id.insert(gas_id, fetched.clone());
                fetched
            } else {
                // We know the tx references GasCoin, but we can't materialize it.
                return Ok((inputs, GasCoinArgMap { gas_coin_input_idx: None }));
            };

                inputs.push(InputValue::Object(ObjectInput::Owned {
                    id: gas_id,
                    bytes: data.bcs_bytes.clone(),
                    type_tag: Some(data.type_tag.clone()),
                    version: Some(data.version),
                }));
                Some((gas_id, (inputs.len() - 1) as u16))
        } else {
            // gas coin already included as a normal PTB input
            None
        }
    } else {
        None
    };

    Ok((inputs, GasCoinArgMap { gas_coin_input_idx }))
}

fn extract_object_ref_id(v: &Value) -> Option<String> {
    // Common shape: [ "<id>", <version>, "<digest>" ]
    if let Some(arr) = v.as_array() {
        if let Some(id) = arr.first().and_then(|x| x.as_str()) {
            return Some(id.to_string());
        }
    }
    // Alternative shapes: { "ObjectRef": [ ... ] } or { "id": "..."} or { "object_id": "..." }
    if let Some(obj) = v.as_object() {
        if let Some(or) = obj.get("ObjectRef") {
            return extract_object_ref_id(or);
        }
        if let Some(id) = obj.get("id").and_then(|x| x.as_str()) {
            return Some(id.to_string());
        }
        if let Some(id) = obj.get("object_id").and_then(|x| x.as_str()) {
            return Some(id.to_string());
        }
    }
    None
}

fn extract_object_ref_id_version(v: &Value) -> Option<(String, Option<u64>)> {
    // Common shape: [ "<id>", <version>, "<digest>" ]
    if let Some(arr) = v.as_array() {
        if let Some(id) = arr.first().and_then(|x| x.as_str()) {
            let ver = arr
                .get(1)
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
            return Some((id.to_string(), ver));
        }
    }
    if let Some(obj) = v.as_object() {
        if let Some(or) = obj.get("ObjectRef") {
            return extract_object_ref_id_version(or);
        }
        if let Some(id) = obj.get("id").and_then(|x| x.as_str()) {
            let ver = obj.get("version").and_then(|v| v.as_u64());
            return Some((id.to_string(), ver));
        }
        if let Some(id) = obj.get("object_id").and_then(|x| x.as_str()) {
            let ver = obj.get("version").and_then(|v| v.as_u64());
            return Some((id.to_string(), ver));
        }
    }
    None
}

fn decode_pure_bytes(pure: &Value) -> Result<Vec<u8>> {
    // Shape 1: base64 string (common in walrus-sui-archival `show_content=true` JSON)
    if let Some(data_b64) = pure.as_str() {
        return base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .context("base64 decode Pure");
    }

    // Shape 2: list of byte values (can happen when serializing CheckpointData via `serde_json::to_value`)
    if let Some(arr) = pure.as_array() {
        let mut out = Vec::with_capacity(arr.len());
        for v in arr {
            let n = v
                .as_u64()
                .ok_or_else(|| anyhow!("Pure byte array contains non-u64 element: {v}"))?;
            if n > 255 {
                return Err(anyhow!("Pure byte value out of range: {n}"));
            }
            out.push(n as u8);
        }
        return Ok(out);
    }

    // Shape 3: object wrapper with `bytes: [...]` or `bytes: "..."`.
    if let Some(obj) = pure.as_object() {
        if let Some(bytes) = obj.get("bytes") {
            return decode_pure_bytes(bytes);
        }
        if let Some(b64) = obj.get("b64").and_then(|v| v.as_str()) {
            return base64::engine::general_purpose::STANDARD
                .decode(b64)
                .context("base64 decode Pure.b64");
        }
    }

    Err(anyhow!("Pure input has unsupported shape: {}", pure))
}

fn parse_commands(
    walrus: &WalrusClient,
    ptb: &Value,
    gas_coin: &GasCoinArgMap,
) -> Result<(Vec<Command>, Vec<AccountAddress>)> {
    let commands_json = ptb
        .get("commands")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing ptb.commands"))?;

    let mut commands = Vec::with_capacity(commands_json.len());
    let mut package_ids: Vec<AccountAddress> = Vec::new();

    for cmd in commands_json {
        if let Some(mc) = cmd.get("MoveCall") {
            let package_str = mc
                .get("package")
                .and_then(|v| v.as_str())
                .context("MoveCall.package missing")?;
            let package = AccountAddress::from_hex_literal(package_str)
                .context("invalid MoveCall.package")?;

            let module = mc
                .get("module")
                .and_then(|v| v.as_str())
                .context("MoveCall.module missing")?;
            let function = mc
                .get("function")
                .and_then(|v| v.as_str())
                .context("MoveCall.function missing")?;

            let type_args = mc
                .get("type_arguments")
                .and_then(|v| v.as_array())
                .unwrap_or(&vec![])
                .iter()
                .map(|t| walrus_type_tag(walrus, t))
                .collect::<Result<Vec<TypeTag>>>()?;

            let args = mc
                .get("arguments")
                .and_then(|v| v.as_array())
                .unwrap_or(&vec![])
                .iter()
                .map(|a| parse_argument(a, gas_coin))
                .collect::<Result<Vec<Argument>>>()?;

            let pkg_id = AccountAddress::from_hex_literal(package_str)
                .context("parse MoveCall.package")?;
            if !package_ids.contains(&pkg_id) {
                package_ids.push(pkg_id);
            }

            commands.push(Command::MoveCall {
                package,
                module: Identifier::new(module).context("invalid module identifier")?,
                function: Identifier::new(function).context("invalid function identifier")?,
                type_args,
                args,
            });
            continue;
        }

        if let Some(split) = cmd.get("SplitCoins") {
            let (coin, amounts_val) = if let Some(arr) = split.as_array() {
                let coin = arr
                    .get(0)
                    .ok_or_else(|| anyhow!("SplitCoins[0] missing coin"))?;
                let amounts = arr
                    .get(1)
                    .ok_or_else(|| anyhow!("SplitCoins[1] missing amounts"))?;
                (coin, amounts)
            } else {
                let coin = split
                    .get("coin")
                    .or_else(|| split.get("coins").and_then(|v| v.as_array()).and_then(|v| v.first()))
                    .ok_or_else(|| anyhow!("SplitCoins.coin missing"))?;
                let amounts = split
                    .get("amounts")
                    .or_else(|| split.get("amount"))
                    .ok_or_else(|| anyhow!("SplitCoins.amounts missing"))?;
                (coin, amounts)
            };
            let amounts_vec: Vec<&Value> = if let Some(arr) = amounts_val.as_array() {
                arr.iter().collect()
            } else {
                vec![amounts_val]
            };
            commands.push(Command::SplitCoins {
                coin: parse_argument(coin, gas_coin)?,
                amounts: amounts_vec
                    .into_iter()
                    .map(|a| parse_argument(a, gas_coin))
                    .collect::<Result<Vec<_>>>()?,
            });
            continue;
        }

        if let Some(merge) = cmd.get("MergeCoins") {
            let (destination, sources_val) = if let Some(arr) = merge.as_array() {
                let destination = arr
                    .get(0)
                    .ok_or_else(|| anyhow!("MergeCoins[0] missing destination"))?;
                let sources = arr
                    .get(1)
                    .ok_or_else(|| anyhow!("MergeCoins[1] missing sources"))?;
                (destination, sources)
            } else {
                let destination = merge
                    .get("destination")
                    .ok_or_else(|| anyhow!("MergeCoins.destination missing"))?;
                let sources = merge
                    .get("sources")
                    .ok_or_else(|| anyhow!("MergeCoins.sources missing"))?;
                (destination, sources)
            };
            let sources = sources_val
                .as_array()
                .map(|a| a.iter().collect::<Vec<_>>())
                .unwrap_or_else(|| vec![sources_val]);
            commands.push(Command::MergeCoins {
                destination: parse_argument(destination, gas_coin)?,
                sources: sources
                    .into_iter()
                    .map(|a| parse_argument(a, gas_coin))
                    .collect::<Result<Vec<_>>>()?,
            });
            continue;
        }

        if let Some(xfer) = cmd.get("TransferObjects") {
            let (objects_val, address) = if let Some(arr) = xfer.as_array() {
                let objects = arr
                    .get(0)
                    .ok_or_else(|| anyhow!("TransferObjects[0] missing objects"))?;
                let address = arr
                    .get(1)
                    .ok_or_else(|| anyhow!("TransferObjects[1] missing address"))?;
                (objects, address)
            } else {
                let objects = xfer
                    .get("objects")
                    .or_else(|| xfer.get("object"))
                    .ok_or_else(|| anyhow!("TransferObjects.objects missing"))?;
                let address = xfer
                    .get("address")
                    .ok_or_else(|| anyhow!("TransferObjects.address missing"))?;
                (objects, address)
            };
            let objects_vec: Vec<&Value> = if let Some(arr) = objects_val.as_array() {
                arr.iter().collect()
            } else {
                vec![objects_val]
            };
            commands.push(Command::TransferObjects {
                objects: objects_vec
                    .into_iter()
                    .map(|a| parse_argument(a, gas_coin))
                    .collect::<Result<Vec<_>>>()?,
                address: parse_argument(address, gas_coin)?,
            });
            continue;
        }

        if let Some(mmv) = cmd.get("MakeMoveVec") {
            let type_tag = if let Some(type_json) = mmv.get("type_") {
                if type_json.is_null() {
                    None
                } else {
                    Some(walrus_type_tag(walrus, type_json)?)
                }
            } else {
                None
            };

            let elements = mmv
                .get("elements")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("MakeMoveVec.elements missing"))?;

            commands.push(Command::MakeMoveVec {
                type_tag,
                elements: elements
                    .iter()
                    .map(|a| parse_argument(a, gas_coin))
                    .collect::<Result<Vec<_>>>()?,
            });
            continue;
        }

        // Publish / Upgrade / Receive exist in transaction format, but Walrus JSON varies by endpoint.
        // We keep these as explicit “unsupported” for now; the main replay example will count them.
        return Err(anyhow!("unsupported PTB command variant: {}", cmd));
    }

    Ok((commands, package_ids))
}

fn parse_argument(arg: &Value, gas_coin: &GasCoinArgMap) -> Result<Argument> {
    if let Some(s) = arg.as_str() {
        if s == "GasCoin" {
            if let Some((_, idx)) = gas_coin.gas_coin_input_idx {
                return Ok(Argument::Input(idx));
            }
            return Err(anyhow!("transaction references GasCoin but gas coin input not found"));
        }
    }
    if let Some(i) = arg.get("Input").and_then(|v| v.as_u64()) {
        return Ok(Argument::Input(i as u16));
    }
    if let Some(r) = arg.get("Result").and_then(|v| v.as_u64()) {
        return Ok(Argument::Result(r as u16));
    }
    if let Some(nr_val) = arg.get("NestedResult") {
        if let Some(nr) = nr_val.as_array() {
            let a = nr.get(0).and_then(|v| v.as_u64()).context("NestedResult[0]")?;
            let b = nr.get(1).and_then(|v| v.as_u64()).context("NestedResult[1]")?;
            return Ok(Argument::NestedResult(a as u16, b as u16));
        }
        if let Some(s) = nr_val.as_str() {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(s)
                .context("base64 decode NestedResult")?;
            let (a, b) = match bytes.len() {
                2 => (bytes[0] as u16, bytes[1] as u16),
                4 => {
                    let a = u16::from_le_bytes([bytes[0], bytes[1]]);
                    let b = u16::from_le_bytes([bytes[2], bytes[3]]);
                    (a, b)
                }
                _ => return Err(anyhow!("NestedResult base64 has unexpected length {}", bytes.len())),
            };
            return Ok(Argument::NestedResult(a, b));
        }
        return Err(anyhow!("NestedResult has unsupported shape: {}", nr_val));
    }
    if arg.get("GasCoin").is_some() {
        if let Some((_, idx)) = gas_coin.gas_coin_input_idx {
            return Ok(Argument::Input(idx));
        }
        return Err(anyhow!("transaction references GasCoin but gas coin input not found"));
    }
    Err(anyhow!("unsupported PTB argument: {}", arg))
}

fn parse_object_ref(obj: &Value) -> Result<(ObjectID, WalrusObjectMode, Option<u64>)> {
    if let Some(shared) = obj.get("SharedObject") {
        let id = shared
            .get("id")
            .and_then(|v| v.as_str())
            .context("SharedObject.id missing")?;
        let version = shared
            .get("initial_shared_version")
            .and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_u64()));
        return Ok((
            AccountAddress::from_hex_literal(id)?,
            WalrusObjectMode::Shared,
            version,
        ));
    }
    if let Some(imm_or_owned) = obj.get("ImmOrOwnedObject").and_then(|v| v.as_array()) {
        let id = imm_or_owned
            .first()
            .and_then(|v| v.as_str())
            .context("ImmOrOwnedObject[0] missing")?;
        let version = imm_or_owned
            .get(1)
            .and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_u64()));
        return Ok((
            AccountAddress::from_hex_literal(id)?,
            WalrusObjectMode::ImmOrOwned,
            version,
        ));
    }
    if let Some(receiving) = obj.get("Receiving").and_then(|v| v.as_array()) {
        let id = receiving
            .first()
            .and_then(|v| v.as_str())
            .context("Receiving[0] missing")?;
        let version = receiving
            .get(1)
            .and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_u64()));
        return Ok((
            AccountAddress::from_hex_literal(id)?,
            WalrusObjectMode::Receiving,
            version,
        ));
    }
    Err(anyhow!("unsupported Object input: {}", obj))
}

fn walrus_type_tag(walrus: &WalrusClient, type_json: &Value) -> Result<TypeTag> {
    // WalrusClient already implements a robust type parser for its checkpoint JSON.
    // We keep this adapter to avoid duplicating that logic inside the example.
    //
    // Note: WalrusClient currently exposes type parsing only internally; for the example
    // we fall back to the common “Other/struct” JSON shapes and primitives.
    //
    // If you extend WalrusClient to expose `parse_type_tag`, switch this to call it.
    let _ = walrus; // keep signature stable; parser does not currently depend on client state

    if let Some(s) = type_json.as_str() {
        if s == "GasCoin" {
            return TypeTag::from_str("0x2::coin::Coin<0x2::sui::SUI>")
                .map_err(|e| anyhow!("parse GasCoin TypeTag: {e}"));
        }
        return TypeTag::from_str(s).map_err(|e| anyhow!("parse TypeTag {s:?}: {e}"));
    }

    if let Some(vec_json) = type_json.get("vector") {
        let inner = walrus_type_tag(walrus, vec_json)?;
        return Ok(TypeTag::Vector(Box::new(inner)));
    }

    if let Some(coin_json) = type_json.get("Coin") {
        if let Some(struct_json) = coin_json.get("struct") {
            let inner = walrus_type_tag(walrus, &serde_json::json!({ "struct": struct_json }))?;
            let inner_str = format!("{inner}");
            let s = format!("0x2::coin::Coin<{inner_str}>");
            return TypeTag::from_str(&s)
                .map_err(|e| anyhow!("parse Coin TypeTag from {s:?}: {e}"));
        }
    }

    // Common shapes: { "struct": { address, module, name, type_args } } and { "Other": { ... } }
    let struct_json = if let Some(other) = type_json.get("Other") {
        other
    } else if let Some(s) = type_json.get("struct") {
        s
    } else if type_json.get("address").is_some() {
        type_json
    } else {
        return Err(anyhow!("unsupported type tag JSON: {}", type_json));
    };

    let address = struct_json
        .get("address")
        .and_then(|a| a.as_str())
        .ok_or_else(|| anyhow!("Missing address in type"))?;
    let module = struct_json
        .get("module")
        .and_then(|m| m.as_str())
        .ok_or_else(|| anyhow!("Missing module in type"))?;
    let name = struct_json
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow!("Missing name in type"))?;

    let type_args = struct_json
        .get("type_args")
        .and_then(|t| t.as_array())
        .unwrap_or(&vec![])
        .iter()
        .map(|arg| walrus_type_tag(walrus, arg))
        .collect::<Result<Vec<_>>>()?;

    // Use the TypeTag string parser for correctness.
    let address = if address.starts_with("0x") {
        address.to_string()
    } else {
        format!("0x{address}")
    };
    let mut s = format!("{address}::{module}::{name}");
    if !type_args.is_empty() {
        let inner = type_args
            .iter()
            .map(|t| format!("{t}"))
            .collect::<Vec<_>>()
            .join(", ");
        s.push('<');
        s.push_str(&inner);
        s.push('>');
    }
    TypeTag::from_str(&s).map_err(|e| anyhow!("parse TypeTag from {s:?}: {e}"))
}

#[derive(Clone)]
struct WalrusObjectData {
    bcs_bytes: Vec<u8>,
    type_tag: TypeTag,
    version: u64,
    is_immutable: bool,
}

#[derive(Clone, Copy)]
enum WalrusObjectMode {
    ImmOrOwned,
    Shared,
    Receiving,
}

#[derive(Default)]
struct GasCoinArgMap {
    gas_coin_input_idx: Option<(ObjectID, u16)>,
}

fn fetch_missing_object_data(
    grpc: &GrpcClient,
    rt: &tokio::runtime::Runtime,
    tx_json: &Value,
    id: ObjectID,
    version_hint: Option<u64>,
    object_versions: Option<&HashMap<String, u64>>,
) -> Result<WalrusObjectData> {
    let id_hex = id.to_hex_literal();
    let version = version_hint
        .or_else(|| {
            object_versions
                .and_then(|m| m.get(&normalize_addr(&id_hex)))
                .copied()
        })
        .or_else(|| find_historical_version(tx_json, id));
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..3 {
        let result = rt.block_on(async { grpc.get_object_at_version(&id_hex, version).await });
        match result {
            Ok(Some(obj)) => {
                let bcs_bytes = obj
                    .bcs
                    .ok_or_else(|| anyhow!("gRPC object missing bcs for {}", id_hex))?;
                let type_str = obj
                    .type_string
                    .ok_or_else(|| anyhow!("gRPC object missing type_string for {}", id_hex))?;
                let type_tag =
                    TypeTag::from_str(&type_str).map_err(|e| anyhow!("parse type {type_str}: {e}"))?;
                let is_immutable = matches!(obj.owner, GrpcOwner::Immutable);
                return Ok(WalrusObjectData {
                    bcs_bytes,
                    type_tag,
                    version: obj.version,
                    is_immutable,
                });
            }
            Ok(None) => {
                last_err = Some(anyhow!("gRPC missing object {}", id_hex));
            }
            Err(e) => {
                let msg = format!("{e:#}");
                // Retry on rate limiting / transient errors
                if msg.contains("429") || msg.contains("Unavailable") || msg.contains("UNAVAILABLE")
                {
                    let backoff_ms = match attempt {
                        0 => 200,
                        1 => 500,
                        _ => 1000,
                    };
                    std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                    last_err = Some(e);
                    continue;
                }
                return Err(anyhow!("gRPC error fetching object {}: {e}", id_hex));
            }
        }
    }
    if let Some(err) = last_err {
        return Err(anyhow!("gRPC error fetching object {}: {err}", id_hex));
    }
    return Err(anyhow!("gRPC missing object {}", id_hex));
}

fn find_historical_version(tx_json: &Value, id: ObjectID) -> Option<u64> {
    let id_str = id.to_hex_literal();

    if let Some(changed) = tx_json
        .pointer("/effects/V2/changed_objects")
        .and_then(|v| v.as_array())
    {
        for entry in changed {
            let arr = entry.as_array()?;
            let id_s = arr.get(0)?.as_str()?;
            if id_s.eq_ignore_ascii_case(&id_str) {
                let meta = arr.get(1)?;
                if let Some(exist) = meta.pointer("/input_state/Exist") {
                    let ver = exist
                        .as_array()
                        .and_then(|v| v.get(0))
                        .and_then(|v| v.as_array())
                        .and_then(|v| v.get(0))
                        .and_then(|v| v.as_u64());
                    if ver.is_some() {
                        return ver;
                    }
                }
            }
        }
    }

    if let Some(unchanged) = tx_json
        .pointer("/effects/V2/unchanged_consensus_objects")
        .and_then(|v| v.as_array())
    {
        for entry in unchanged {
            let arr = entry.as_array()?;
            let id_s = arr.get(0)?.as_str()?;
            if id_s.eq_ignore_ascii_case(&id_str) {
                let meta = arr.get(1)?;
                if let Some(readonly) = meta.get("ReadOnlyRoot") {
                    let ver = readonly
                        .as_array()
                        .and_then(|v| v.get(0))
                        .and_then(|v| v.as_u64());
                    if ver.is_some() {
                        return ver;
                    }
                }
                if let Some(mutable) = meta.get("MutableRoot") {
                    let ver = mutable
                        .as_array()
                        .and_then(|v| v.get(0))
                        .and_then(|v| v.as_u64());
                    if ver.is_some() {
                        return ver;
                    }
                }
            }
        }
    }
    None
}

fn normalize_addr(addr: &str) -> String {
    let hex = addr.strip_prefix("0x").unwrap_or(addr);
    format!("0x{}", hex.to_lowercase())
}

fn synthesize_system_object(
    id: ObjectID,
    timestamp_ms: Option<u64>,
    version_hint: Option<u64>,
    object_versions: Option<&HashMap<String, u64>>,
) -> Option<WalrusObjectData> {
    let clock_id = AccountAddress::from_hex_literal("0x6").ok()?;
    let random_id = AccountAddress::from_hex_literal("0x8").ok()?;
    let version = version_hint
        .or_else(|| {
            object_versions
                .and_then(|m| m.get(&normalize_addr(&id.to_hex_literal())))
                .copied()
        })
        .unwrap_or(1);

    if id == clock_id {
        let ts = timestamp_ms.unwrap_or(0);
        let mut clock_bytes = Vec::with_capacity(40);
        clock_bytes.extend_from_slice(clock_id.as_ref());
        clock_bytes.extend_from_slice(&ts.to_le_bytes());
        let type_tag = TypeTag::from_str("0x2::clock::Clock").ok()?;
        return Some(WalrusObjectData {
            bcs_bytes: clock_bytes,
            type_tag,
            version,
            is_immutable: false,
        });
    }

    if id == random_id {
        let mut random_bytes = Vec::with_capacity(72);
        random_bytes.extend_from_slice(random_id.as_ref());
        random_bytes.extend_from_slice(random_id.as_ref());
        random_bytes.extend_from_slice(&1u64.to_le_bytes());
        let type_tag = TypeTag::from_str("0x2::random::Random").ok()?;
        return Some(WalrusObjectData {
            bcs_bytes: random_bytes,
            type_tag,
            version,
            is_immutable: false,
        });
    }

    None
}
