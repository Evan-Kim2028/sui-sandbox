use crate::types::BytecodeStructRefJson;
use crate::utils::bytes_to_hex_prefixed;
use anyhow::{anyhow, Result};
use move_binary_format::file_format::{CompiledModule, SignatureToken};
use serde_json::Value;

pub fn normalize_address_str(addr: &str) -> Result<String> {
    let s = addr.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.is_empty() {
        return Err(anyhow!("empty address"));
    }
    let mut hex = s.to_ascii_lowercase();
    if hex.len() > 64 {
        return Err(anyhow!("address too long: {}", addr));
    }
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid hex address: {}", addr));
    }
    if hex.len() % 2 == 1 {
        hex = format!("0{}", hex);
    }
    Ok(format!("0x{:0>64}", hex))
}

pub fn rpc_visibility_to_string(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    match s {
        "Public" => Some("public".to_string()),
        "Friend" => Some("friend".to_string()),
        "Private" => Some("private".to_string()),
        _ => None,
    }
}

pub fn abilities_from_value(value: &Value) -> Vec<String> {
    if let Some(arr) = value.as_array() {
        let mut out: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_ascii_lowercase())
            .collect();
        out.sort();
        out.dedup();
        return out;
    }
    if let Some(obj) = value.as_object() {
        if let Some(v) = obj.get("abilities") {
            return abilities_from_value(v);
        }
        if let Some(v) = obj.get("constraints") {
            return abilities_from_value(v);
        }
    }
    Vec::new()
}

pub fn rpc_type_to_canonical_json(v: &Value) -> Result<Value> {
    if let Some(s) = v.as_str() {
        let out = match s {
            "Bool" => serde_json::json!({"kind": "bool"}),
            "U8" => serde_json::json!({"kind": "u8"}),
            "U16" => serde_json::json!({"kind": "u16"}),
            "U32" => serde_json::json!({"kind": "u32"}),
            "U64" => serde_json::json!({"kind": "u64"}),
            "U128" => serde_json::json!({"kind": "u128"}),
            "U256" => serde_json::json!({"kind": "u256"}),
            "Address" => serde_json::json!({"kind": "address"}),
            "Signer" => serde_json::json!({"kind": "signer"}),
            other => return Err(anyhow!("unknown RPC primitive type string: {}", other)),
        };
        return Ok(out);
    }

    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("RPC type is not an object: {}", v))?;
    if obj.len() != 1 {
        return Err(anyhow!("RPC type expected single-key object: {}", v));
    }
    let (k, inner) = obj.iter().next().expect("len=1");
    let out = match k.as_str() {
        "Bool" => serde_json::json!({"kind": "bool"}),
        "U8" => serde_json::json!({"kind": "u8"}),
        "U16" => serde_json::json!({"kind": "u16"}),
        "U32" => serde_json::json!({"kind": "u32"}),
        "U64" => serde_json::json!({"kind": "u64"}),
        "U128" => serde_json::json!({"kind": "u128"}),
        "U256" => serde_json::json!({"kind": "u256"}),
        "Address" => serde_json::json!({"kind": "address"}),
        "Signer" => serde_json::json!({"kind": "signer"}),
        "Vector" => {
            serde_json::json!({"kind": "vector", "type": rpc_type_to_canonical_json(inner)?})
        }
        "Reference" => {
            serde_json::json!({"kind": "ref", "mutable": false, "to": rpc_type_to_canonical_json(inner)?})
        }
        "MutableReference" => {
            serde_json::json!({"kind": "ref", "mutable": true, "to": rpc_type_to_canonical_json(inner)?})
        }
        "TypeParameter" => {
            let idx = inner
                .as_u64()
                .ok_or_else(|| anyhow!("TypeParameter index is not u64: {}", inner))?;
            serde_json::json!({"kind": "type_param", "index": idx})
        }
        "Struct" => {
            let s = inner
                .as_object()
                .ok_or_else(|| anyhow!("Struct payload is not object: {}", inner))?;
            let addr = s
                .get("address")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Struct missing address: {}", inner))?;
            let module = s
                .get("module")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Struct missing module: {}", inner))?;
            let name = s
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Struct missing name: {}", inner))?;
            let args = s
                .get("typeArguments")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Struct missing typeArguments: {}", inner))?;
            let args_canon: Vec<Value> = args
                .iter()
                .map(rpc_type_to_canonical_json)
                .collect::<Result<_>>()?;
            serde_json::json!({
                "kind": "datatype",
                "address": normalize_address_str(addr)?,
                "module": module,
                "name": name,
                "type_args": args_canon,
            })
        }
        _ => return Err(anyhow!("unknown RPC type tag: {}", k)),
    };
    Ok(out)
}

pub fn bytecode_type_to_canonical_json(v: &Value) -> Result<Value> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("bytecode type is not object: {}", v))?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("bytecode type missing kind: {}", v))?;
    match kind {
        "datatype" => {
            let addr = obj
                .get("address")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bytecode datatype missing address: {}", v))?;
            let module = obj
                .get("module")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bytecode datatype missing module: {}", v))?;
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bytecode datatype missing name: {}", v))?;
            let args = obj
                .get("type_args")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("bytecode datatype missing type_args: {}", v))?;
            let args_canon: Vec<Value> = args
                .iter()
                .map(bytecode_type_to_canonical_json)
                .collect::<Result<_>>()?;
            Ok(serde_json::json!({
                "kind": "datatype",
                "address": normalize_address_str(addr)?,
                "module": module,
                "name": name,
                "type_args": args_canon,
            }))
        }
        "vector" => {
            let inner = obj
                .get("type")
                .ok_or_else(|| anyhow!("vector missing type: {}", v))?;
            Ok(
                serde_json::json!({"kind": "vector", "type": bytecode_type_to_canonical_json(inner)?}),
            )
        }
        "ref" => {
            let mutable = obj
                .get("mutable")
                .and_then(Value::as_bool)
                .ok_or_else(|| anyhow!("ref missing mutable bool: {}", v))?;
            let inner = obj
                .get("to")
                .ok_or_else(|| anyhow!("ref missing to: {}", v))?;
            Ok(
                serde_json::json!({"kind":"ref","mutable":mutable,"to": bytecode_type_to_canonical_json(inner)?}),
            )
        }
        "type_param" => {
            let idx = obj
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("type_param missing numeric index: {}", v))?;
            Ok(serde_json::json!({"kind":"type_param","index": idx}))
        }
        "bool" | "u8" | "u16" | "u32" | "u64" | "u128" | "u256" | "address" | "signer" => {
            Ok(serde_json::json!({"kind": kind}))
        }
        _ => Err(anyhow!("unknown bytecode type kind: {}", kind)),
    }
}

pub fn signature_token_to_json(module: &CompiledModule, tok: &SignatureToken) -> Value {
    match tok {
        SignatureToken::Bool => serde_json::json!({"kind": "bool"}),
        SignatureToken::U8 => serde_json::json!({"kind": "u8"}),
        SignatureToken::U16 => serde_json::json!({"kind": "u16"}),
        SignatureToken::U32 => serde_json::json!({"kind": "u32"}),
        SignatureToken::U64 => serde_json::json!({"kind": "u64"}),
        SignatureToken::U128 => serde_json::json!({"kind": "u128"}),
        SignatureToken::U256 => serde_json::json!({"kind": "u256"}),
        SignatureToken::Address => serde_json::json!({"kind": "address"}),
        SignatureToken::Signer => serde_json::json!({"kind": "signer"}),
        SignatureToken::Vector(inner) => {
            serde_json::json!({"kind": "vector", "type": signature_token_to_json(module, inner)})
        }
        SignatureToken::Reference(inner) => {
            serde_json::json!({"kind": "ref", "mutable": false, "to": signature_token_to_json(module, inner)})
        }
        SignatureToken::MutableReference(inner) => {
            serde_json::json!({"kind": "ref", "mutable": true, "to": signature_token_to_json(module, inner)})
        }
        SignatureToken::TypeParameter(idx) => {
            serde_json::json!({"kind": "type_param", "index": idx})
        }
        SignatureToken::Datatype(idx) => {
            let tref = module_id_for_datatype_handle(module, *idx);
            serde_json::json!({"kind": "datatype", "address": tref.address, "module": tref.module, "name": tref.name, "type_args": []})
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, tys) = &**inst;
            let tref = module_id_for_datatype_handle(module, *idx);
            let args: Vec<Value> = tys
                .iter()
                .map(|t| signature_token_to_json(module, t))
                .collect();
            serde_json::json!({"kind": "datatype", "address": tref.address, "module": tref.module, "name": tref.name, "type_args": args})
        }
    }
}

fn module_id_for_datatype_handle(
    module: &CompiledModule,
    datatype_handle_index: move_binary_format::file_format::DatatypeHandleIndex,
) -> BytecodeStructRefJson {
    let datatype_handle = module.datatype_handle_at(datatype_handle_index);
    let module_handle = module.module_handle_at(datatype_handle.module);
    let addr = module.address_identifier_at(module_handle.address);
    let module_name = module.identifier_at(module_handle.name).to_string();
    let struct_name = module.identifier_at(datatype_handle.name).to_string();
    BytecodeStructRefJson {
        address: bytes_to_hex_prefixed(addr.as_ref()),
        module: module_name,
        name: struct_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_address_str_pads_to_32_bytes() {
        assert_eq!(
            normalize_address_str("0x2").unwrap(),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        assert_eq!(
            normalize_address_str("2").unwrap(),
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_rpc_type_to_canonical_handles_string_primitives() {
        assert_eq!(
            rpc_type_to_canonical_json(&serde_json::json!("U64")).unwrap(),
            serde_json::json!({"kind":"u64"})
        );
        assert_eq!(
            rpc_type_to_canonical_json(&serde_json::json!("Address")).unwrap(),
            serde_json::json!({"kind":"address"})
        );
    }

    #[test]
    fn test_rpc_type_to_canonical_handles_struct_object() {
        let t = serde_json::json!({
            "Struct": {
                "address": "0x2",
                "module": "object",
                "name": "UID",
                "typeArguments": []
            }
        });
        let canon = rpc_type_to_canonical_json(&t).unwrap();
        assert_eq!(
            canon,
            serde_json::json!({
                "kind": "datatype",
                "address": "0x0000000000000000000000000000000000000000000000000000000000000002",
                "module": "object",
                "name": "UID",
                "type_args": []
            })
        );
    }
}
