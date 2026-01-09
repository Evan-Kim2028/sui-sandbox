use anyhow::{anyhow, bail, Context, Result};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use clap::{Parser, ValueEnum};
use move_binary_format::file_format::{Bytecode, CompiledModule, SignatureToken};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sui_json_rpc_types::SuiObjectDataOptions;
use sui_json_rpc_types::{Coin, DryRunTransactionBlockResponse, ObjectChange};
use sui_sdk::SuiClientBuilder;
use sui_types::base_types::{ObjectDigest, ObjectID, ObjectRef, SequenceNumber, SuiAddress};
use sui_types::programmable_transaction_builder::ProgrammableTransactionBuilder;
use sui_types::transaction::{
    Argument, CallArg, Command, ObjectArg, ProgrammableMoveCall, SharedObjectMutability,
    TransactionData, TransactionKind,
};
use sui_types::type_input::TypeInput;

#[derive(Debug, Copy, Clone, ValueEnum)]
enum Mode {
    DevInspect,
    DryRun,
    BuildOnly,
}

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Tx simulation helper (dev-inspect or dry-run) for Phase II inhabitation benchmarking"
)]
struct Args {
    /// RPC URL (default: mainnet fullnode)
    #[arg(long, default_value = "https://fullnode.mainnet.sui.io:443")]
    rpc_url: String,

    /// Transaction sender address.
    ///
    /// - For --mode dev-inspect: sender is used as a label (ownership is not enforced).
    /// - For --mode dry-run: sender must own a gas coin on the target network.
    #[arg(long)]
    sender: String,

    /// Simulation mode: dev-inspect or dry-run.
    #[arg(long, value_enum, default_value_t = Mode::DryRun)]
    mode: Mode,

    /// Gas budget (required for --mode dry-run).
    #[arg(long, default_value_t = 10_000_000)]
    gas_budget: u64,

    /// Optional gas coin object id to use (defaults to the first `Coin<SUI>` found for sender).
    #[arg(long)]
    gas_coin: Option<String>,

    /// JSON PTB spec path (use '-' for stdin).
    #[arg(long, value_name = "PATH")]
    ptb_spec: PathBuf,

    /// Optional local bytecode package dir (expects `bytecode_modules/*.mv`).
    /// If provided, the tool will also emit best-effort static created object types by scanning
    /// called function bytecode for `0x2::transfer::transfer<T>` / `public_transfer<T>` / `share_object<T>`.
    #[arg(long, value_name = "DIR")]
    bytecode_package_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct PtbSpec {
    calls: Vec<MoveCallSpec>,
}

#[derive(Debug, Deserialize)]
struct MoveCallSpec {
    /// "0xADDR::module::function"
    target: String,
    #[serde(default)]
    type_args: Vec<String>,
    #[serde(default)]
    args: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputJson {
    mode_used: String,
    created_object_types: Vec<String>,
    static_created_object_types: Vec<String>,
    programmable_transaction_bcs_base64: Option<String>,
    dry_run: Option<Value>,
    dev_inspect: Option<Value>,
}

fn read_json(path: &PathBuf) -> Result<Value> {
    let text = if path.as_os_str() == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("read stdin")?;
        buf
    } else {
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    };
    serde_json::from_str(&text).context("parse JSON")
}

fn parse_object_id(s: &str) -> Result<ObjectID> {
    ObjectID::from_str(s).with_context(|| format!("parse object id: {s}"))
}

const SUI_COIN_TYPE: &str = "0x2::sui::SUI";

fn normalize_sui_address(s: &str) -> Result<String> {
    let s = s.trim().to_lowercase();
    let h = s.strip_prefix("0x").unwrap_or(&s);
    if h.is_empty() {
        return Ok(format!("0x{}", "0".repeat(64)));
    }
    if h.len() > 64 {
        bail!("address too long: {s}");
    }
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("address is not hex: {s}");
    }
    Ok(format!("0x{:0>64}", h))
}

fn parse_identifier(s: &str) -> Result<Identifier> {
    Identifier::new(s).map_err(|e| anyhow!("invalid identifier {s:?}: {e}"))
}

fn parse_type_tag(s: &str) -> Result<TypeTag> {
    TypeTag::from_str(s).with_context(|| format!("parse type tag: {s}"))
}

fn parse_move_target(s: &str) -> Result<(ObjectID, Identifier, Identifier)> {
    // Expected: 0xADDR::module::function
    let parts: Vec<&str> = s.split("::").collect();
    if parts.len() != 3 {
        bail!("invalid move target (expected 0xADDR::module::function): {s}");
    }
    let package = parse_object_id(parts[0])?;
    let module = parse_identifier(parts[1])?;
    let function = parse_identifier(parts[2])?;
    Ok((package, module, function))
}

fn parse_move_struct_tag(s: &str) -> Result<(ObjectID, Identifier, Identifier)> {
    // Expected: 0xADDR::module::Struct
    let parts: Vec<&str> = s.split("::").collect();
    if parts.len() != 3 {
        bail!("invalid move struct tag (expected 0xADDR::module::Struct): {s}");
    }
    let package = parse_object_id(parts[0])?;
    let module = parse_identifier(parts[1])?;
    let struct_name = parse_identifier(parts[2])?;
    Ok((package, module, struct_name))
}

fn addr_eq(addr: &AccountAddress, hex_literal: &str) -> bool {
    AccountAddress::from_hex_literal(hex_literal)
        .ok()
        .map(|a| &a == addr)
        .unwrap_or(false)
}

fn account_address_to_hex(addr: &AccountAddress) -> String {
    format!("0x{}", addr.to_hex())
}

fn sig_token_to_type_string(m: &CompiledModule, tok: &SignatureToken) -> String {
    match tok {
        SignatureToken::Bool => "bool".to_string(),
        SignatureToken::U8 => "u8".to_string(),
        SignatureToken::U16 => "u16".to_string(),
        SignatureToken::U32 => "u32".to_string(),
        SignatureToken::U64 => "u64".to_string(),
        SignatureToken::U128 => "u128".to_string(),
        SignatureToken::U256 => "u256".to_string(),
        SignatureToken::Address => "address".to_string(),
        SignatureToken::Signer => "signer".to_string(),
        SignatureToken::Vector(inner) => format!("vector<{}>", sig_token_to_type_string(m, inner)),
        SignatureToken::Reference(inner) => format!("&{}", sig_token_to_type_string(m, inner)),
        SignatureToken::MutableReference(inner) => {
            format!("&mut {}", sig_token_to_type_string(m, inner))
        }
        SignatureToken::TypeParameter(i) => format!("T{i}"),
        SignatureToken::Datatype(dt_idx) => {
            let dh = m.datatype_handle_at(*dt_idx);
            let mh = m.module_handle_at(dh.module);
            let addr = m.address_identifier_at(mh.address);
            let mod_name = m.identifier_at(mh.name).to_string();
            let st_name = m.identifier_at(dh.name).to_string();
            format!(
                "{}::{}::{}",
                account_address_to_hex(addr),
                mod_name,
                st_name
            )
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (dt_idx, tys) = &**inst;
            let base = sig_token_to_type_string(m, &SignatureToken::Datatype(*dt_idx));
            let args = tys
                .iter()
                .map(|t| sig_token_to_type_string(m, t))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{base}<{args}>")
        }
    }
}

fn static_created_types_for_call(
    modules: &HashMap<String, CompiledModule>,
    call: &MoveCallSpec,
) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::<String>::new();
    let (_package, module_name, function_name) = parse_move_target(&call.target)?;
    let m = match modules.get(module_name.as_str()) {
        Some(m) => m,
        None => return Ok(out),
    };

    // Resolve function definition by name.
    let mut fn_def_idx = None;
    for (i, def) in m.function_defs().iter().enumerate() {
        let fh = m.function_handle_at(def.function);
        let name = m.identifier_at(fh.name).to_string();
        if name == function_name.as_str() {
            fn_def_idx = Some(i);
            break;
        }
    }
    let Some(def_i) = fn_def_idx else {
        return Ok(out);
    };
    let def = m.function_def_at(move_binary_format::file_format::FunctionDefinitionIndex(
        def_i as u16,
    ));
    let Some(code) = &def.code else {
        return Ok(out);
    };

    for bc in &code.code {
        let inst_idx = match bc {
            Bytecode::CallGeneric(i) => Some(*i),
            _ => None,
        };
        let Some(inst_idx) = inst_idx else {
            continue;
        };
        let inst = m.function_instantiation_at(inst_idx);
        let fh = m.function_handle_at(inst.handle);
        let mh = m.module_handle_at(fh.module);
        let addr = m.address_identifier_at(mh.address);
        let mod_name = m.identifier_at(mh.name).to_string();
        let fun_name = m.identifier_at(fh.name).to_string();

        let is_transfer_like = addr_eq(addr, "0x2")
            && mod_name == "transfer"
            && matches!(
                fun_name.as_str(),
                "transfer" | "public_transfer" | "share_object"
            );
        if !is_transfer_like {
            continue;
        }

        let sig = m.signature_at(inst.type_parameters);
        for tok in &sig.0 {
            out.insert(sig_token_to_type_string(m, tok));
        }
    }

    Ok(out)
}

fn load_bytecode_modules(bytecode_package_dir: &Path) -> Result<HashMap<String, CompiledModule>> {
    fn load_dir(dir: &Path, out: &mut HashMap<String, CompiledModule>) -> Result<()> {
        let rd = std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))?;
        for entry in rd {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                load_dir(&path, out)?;
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("mv") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| anyhow!("invalid module filename: {}", path.display()))?
                .to_string();
            let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            let m = CompiledModule::deserialize_with_defaults(&bytes)
                .with_context(|| format!("deserialize {}", path.display()))?;
            out.insert(name, m);
        }
        Ok(())
    }

    let bytecode_dir = bytecode_package_dir.join("bytecode_modules");
    let mut out = HashMap::new();
    load_dir(&bytecode_dir, &mut out)?;
    Ok(out)
}

fn mock_object_ref(id: ObjectID) -> ObjectRef {
    (id, SequenceNumber::from_u64(1), ObjectDigest::new([0; 32]))
}

async fn resolve_object_ref(
    client: &Option<sui_sdk::SuiClient>,
    id: ObjectID,
) -> Result<ObjectRef> {
    if let Some(client) = client {
        let resp = client
            .read_api()
            .get_object_with_options(id, SuiObjectDataOptions::new().with_owner())
            .await
            .with_context(|| format!("get_object {id}"))?;
        let Some(data) = resp.data else {
            bail!("object not found: {id}");
        };
        Ok(data.object_ref())
    } else {
        // BuildOnly mode: return mock ref
        Ok(mock_object_ref(id))
    }
}

struct ResolveCtx<'a> {
    client: &'a Option<sui_sdk::SuiClient>,
    sender: SuiAddress,
    gas_coin_id: Option<ObjectID>,
    sender_sui_coins: Vec<ObjectRef>,
}

fn load_sender_sui_coin(
    ctx: &ResolveCtx<'_>,
    index: usize,
    exclude_gas: bool,
) -> Result<ObjectRef> {
    if ctx.client.is_none() {
        // BuildOnly mode: return mock coin
        // Use a deterministic ID based on index to differentiate
        let mut bytes = [0u8; 32];
        bytes[31] = index as u8; // simple hack
                                 // or just use 0x2::sui::SUI ID if it was an object? No, coins have random IDs.
                                 // Let's use 0x...01, 0x...02
        let id = ObjectID::from_bytes(bytes).unwrap();
        return Ok(mock_object_ref(id));
    }

    let mut coins: Vec<ObjectRef> = ctx.sender_sui_coins.clone();
    if exclude_gas {
        if let Some(gas_id) = ctx.gas_coin_id {
            coins.retain(|r| r.0 != gas_id);
        }
    }
    if coins.is_empty() {
        bail!(
            "no Coin<SUI> available for sender {} (after exclude_gas)",
            ctx.sender
        );
    }
    let i = index.min(coins.len() - 1);
    Ok(coins[i])
}

async fn resolve_argument(
    ptb: &mut ProgrammableTransactionBuilder,
    ctx: Option<&ResolveCtx<'_>>,
    v: &Value,
) -> Result<Argument> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("PTB arg must be an object (got {v:?})"))?;

    if let Some(res) = obj.get("result") {
        let i = res
            .as_u64()
            .ok_or_else(|| anyhow!("result index must be u16"))?;
        return Ok(Argument::Result(i as u16));
    }

    if let Some(nested) = obj.get("nested_result") {
        let arr = nested
            .as_array()
            .ok_or_else(|| anyhow!("nested_result must be [cmd, res]"))?;
        if arr.len() != 2 {
            bail!("nested_result must be [u16, u16]");
        }
        let cmd = arr[0]
            .as_u64()
            .ok_or_else(|| anyhow!("nested_result cmd index must be u16"))?;
        let res = arr[1]
            .as_u64()
            .ok_or_else(|| anyhow!("nested_result res index must be u16"))?;
        return Ok(Argument::NestedResult(cmd as u16, res as u16));
    }

    // Otherwise, resolve as CallArg and add as Input
    resolve_call_arg_as_input(ptb, ctx, v).await
}

async fn resolve_call_arg_as_input(
    ptb: &mut ProgrammableTransactionBuilder,
    ctx: Option<&ResolveCtx<'_>>,
    v: &Value,
) -> Result<Argument> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("PTB arg must be an object (got {v:?})"))?;
    if obj.len() != 1 {
        bail!("PTB arg must have exactly 1 key (got {obj:?})");
    }
    let (k, vv) = obj.iter().next().expect("len==1");
    let call_arg = match k.as_str() {
        "u8" => {
            let n = vv
                .as_u64()
                .ok_or_else(|| anyhow!("u8 must be an integer"))?;
            let x: u8 = n.try_into().context("u8 out of range")?;
            CallArg::Pure(bcs::to_bytes(&x).context("bcs u8")?)
        }
        "u16" => {
            let n = vv
                .as_u64()
                .ok_or_else(|| anyhow!("u16 must be an integer"))?;
            let x: u16 = n.try_into().context("u16 out of range")?;
            CallArg::Pure(bcs::to_bytes(&x).context("bcs u16")?)
        }
        "u32" => {
            let n = vv
                .as_u64()
                .ok_or_else(|| anyhow!("u32 must be an integer"))?;
            let x: u32 = n.try_into().context("u32 out of range")?;
            CallArg::Pure(bcs::to_bytes(&x).context("bcs u32")?)
        }
        "u64" => CallArg::Pure(
            bcs::to_bytes(
                &vv.as_u64()
                    .ok_or_else(|| anyhow!("u64 must be an integer"))?,
            )
            .context("bcs u64")?,
        ),
        "bool" => CallArg::Pure(
            bcs::to_bytes(
                &vv.as_bool()
                    .ok_or_else(|| anyhow!("bool must be true/false"))?,
            )
            .context("bcs bool")?,
        ),
        "address" => {
            let s = vv
                .as_str()
                .ok_or_else(|| anyhow!("address must be a string"))?;
            let s = normalize_sui_address(s)?;
            let addr = SuiAddress::from_str(&s).with_context(|| format!("parse address: {s}"))?;
            CallArg::Pure(bcs::to_bytes(&addr).context("bcs address")?)
        }
        "vector_u8_utf8" => {
            let s = vv
                .as_str()
                .ok_or_else(|| anyhow!("vector_u8_utf8 must be a string"))?;
            let bytes = s.as_bytes().to_vec();
            CallArg::Pure(bcs::to_bytes(&bytes).context("bcs vector<u8>")?)
        }
        "vector_u8_hex" => {
            let s = vv
                .as_str()
                .ok_or_else(|| anyhow!("vector_u8_hex must be a string"))?;
            let s = s.strip_prefix("0x").unwrap_or(s);
            if s.len() % 2 != 0 {
                bail!("vector_u8_hex must have even number of hex chars");
            }
            let bytes = (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
                .collect::<std::result::Result<Vec<u8>, _>>()
                .context("parse hex bytes")?;
            CallArg::Pure(bcs::to_bytes(&bytes).context("bcs vector<u8>")?)
        }
        "vector_address" => {
            let arr = vv
                .as_array()
                .ok_or_else(|| anyhow!("vector_address must be an array"))?;
            let mut out: Vec<SuiAddress> = Vec::with_capacity(arr.len());
            for el in arr {
                let s = el
                    .as_str()
                    .ok_or_else(|| anyhow!("vector_address elements must be strings"))?;
                let s = normalize_sui_address(s)?;
                out.push(SuiAddress::from_str(&s).with_context(|| format!("parse address: {s}"))?);
            }
            CallArg::Pure(bcs::to_bytes(&out).context("bcs vector<address>")?)
        }
        "vector_bool" => {
            let arr = vv
                .as_array()
                .ok_or_else(|| anyhow!("vector_bool must be an array"))?;
            let mut out: Vec<bool> = Vec::with_capacity(arr.len());
            for el in arr {
                out.push(
                    el.as_bool()
                        .ok_or_else(|| anyhow!("vector_bool elements must be true/false"))?,
                );
            }
            CallArg::Pure(bcs::to_bytes(&out).context("bcs vector<bool>")?)
        }
        "option_vector_u8_utf8" => {
            if vv.is_null() {
                let none: Option<Vec<u8>> = None;
                CallArg::Pure(bcs::to_bytes(&none).context("bcs option<vector<u8>>")?)
            } else {
                let s = vv
                    .as_str()
                    .ok_or_else(|| anyhow!("option_vector_u8_utf8 must be a string or null"))?;
                let some: Option<Vec<u8>> = Some(s.as_bytes().to_vec());
                CallArg::Pure(bcs::to_bytes(&some).context("bcs option<vector<u8>>")?)
            }
        }
        "one_time_witness" => {
            let s = vv
                .as_str()
                .ok_or_else(|| anyhow!("one_time_witness must be a string like 0xPKG::module::STRUCT"))?;
            let (pkg, mod_name, struct_name) = parse_move_struct_tag(s)?;

            // We only need a BCS value whose layout matches the zero-sized witness struct.
            // For `struct X has drop {}`, BCS value is empty bytes.
            // We still validate the tag parses, so callers get a good error for malformed input.
            let type_str = format!("{}::{}::{}", pkg, mod_name, struct_name);
            let _tag = parse_type_tag(&type_str)
                .with_context(|| format!("parse one_time_witness type tag: {type_str}"))?;
            CallArg::Pure(bcs::to_bytes(&()).context("bcs unit for OTW")?)
        }
        "vector_u16" => {
            let arr = vv
                .as_array()
                .ok_or_else(|| anyhow!("vector_u16 must be an array"))?;
            let mut out: Vec<u16> = Vec::with_capacity(arr.len());
            for el in arr {
                let n = el
                    .as_u64()
                    .ok_or_else(|| anyhow!("vector_u16 elements must be integers"))?;
                out.push(n.try_into().context("vector_u16 element out of range")?);
            }
            CallArg::Pure(bcs::to_bytes(&out).context("bcs vector<u16>")?)
        }
        "vector_u32" => {
            let arr = vv
                .as_array()
                .ok_or_else(|| anyhow!("vector_u32 must be an array"))?;
            let mut out: Vec<u32> = Vec::with_capacity(arr.len());
            for el in arr {
                let n = el
                    .as_u64()
                    .ok_or_else(|| anyhow!("vector_u32 elements must be integers"))?;
                out.push(n.try_into().context("vector_u32 element out of range")?);
            }
            CallArg::Pure(bcs::to_bytes(&out).context("bcs vector<u32>")?)
        }
        "vector_u64" => {
            let arr = vv
                .as_array()
                .ok_or_else(|| anyhow!("vector_u64 must be an array"))?;
            let mut out: Vec<u64> = Vec::with_capacity(arr.len());
            for el in arr {
                out.push(
                    el.as_u64()
                        .ok_or_else(|| anyhow!("vector_u64 elements must be integers"))?,
                );
            }
            CallArg::Pure(bcs::to_bytes(&out).context("bcs vector<u64>")?)
        }
        "gas_coin" => {
            let Some(ctx) = ctx else {
                // Should not happen if gas_coin is passed, but just in case
                bail!("gas_coin missing context");
            };
            let Some(gas_id) = ctx.gas_coin_id else {
                // If gas_coin was requested but not provided/resolved?
                // In BuildOnly, gas_coin is None unless provided explicitly.
                // If provided explicitly, we resolve it.
                // If client is None, we return mock.
                // So we need resolve_object_ref to handle client=None.
                // And we need gas_id.
                // If gas_id is None, we can't fetch it.
                bail!("gas_coin requires a resolved gas coin id");
            };
            let oref = resolve_object_ref(ctx.client, gas_id).await?;
            CallArg::Object(ObjectArg::ImmOrOwnedObject(oref))
        }
        "sender_sui_coin" => {
            let Some(ctx) = ctx else {
                bail!("sender_sui_coin context missing");
            };
            let o = vv
                .as_object()
                .ok_or_else(|| anyhow!("sender_sui_coin must be an object"))?;
            let index = o.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let exclude_gas = o
                .get("exclude_gas")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let oref = load_sender_sui_coin(ctx, index, exclude_gas)?;
            CallArg::Object(ObjectArg::ImmOrOwnedObject(oref))
        }
        "imm_or_owned_object" => {
            let Some(ctx) = ctx else {
                bail!("imm_or_owned_object context missing");
            };
            let s = vv
                .as_str()
                .ok_or_else(|| anyhow!("imm_or_owned_object must be a string object id"))?;
            let id = parse_object_id(s)?;
            let oref = resolve_object_ref(ctx.client, id).await?;
            CallArg::Object(ObjectArg::ImmOrOwnedObject(oref))
        }
        "shared_object" => {
            let Some(ctx) = ctx else {
                bail!("shared_object context missing");
            };
            let o = vv
                .as_object()
                .ok_or_else(|| anyhow!("shared_object must be an object"))?;
            let id_s = o
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("shared_object.id must be a string"))?;
            let mutable = o
                .get("mutable")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| anyhow!("shared_object.mutable must be true/false"))?;
            let id = parse_object_id(id_s)?;

            let initial_shared_version = if let Some(client) = ctx.client {
                let resp = client
                    .read_api()
                    .get_object_with_options(id, SuiObjectDataOptions::new().with_owner())
                    .await
                    .with_context(|| format!("get_object {id}"))?;
                let Some(data) = resp.data else {
                    bail!("object not found: {id}");
                };
                let owner = data
                    .owner
                    .ok_or_else(|| anyhow!("object missing owner: {id}"))?;
                match owner {
                    sui_types::object::Owner::Shared {
                        initial_shared_version,
                    } => initial_shared_version,
                    _ => bail!("object is not shared: {id}"),
                }
            } else {
                // BuildOnly mode: mock shared version
                SequenceNumber::from_u64(1)
            };

            CallArg::Object(ObjectArg::SharedObject {
                id,
                initial_shared_version,
                mutability: if mutable {
                    SharedObjectMutability::Mutable
                } else {
                    SharedObjectMutability::Immutable
                },
            })
        }
        other => bail!("unsupported PTB arg kind: {other}"),
    };
    ptb.input(call_arg).context("ptb.input")
}

async fn pick_gas_coin(
    client: &Option<sui_sdk::SuiClient>,
    sender: SuiAddress,
    gas_coin: Option<&str>,
) -> Result<ObjectRef> {
    let Some(rpc) = client else {
        // BuildOnly: mock gas coin
        let id = ObjectID::from_hex_literal("0x1234").unwrap();
        return Ok(mock_object_ref(id));
    };

    if let Some(id_s) = gas_coin {
        let id = parse_object_id(id_s)?;
        return resolve_object_ref(client, id).await;
    }

    let page = rpc
        .coin_read_api()
        .get_coins(sender, None, None, Some(1))
        .await
        .context("get_coins")?;
    let Some(first) = page.data.into_iter().next() else {
        bail!("no Coin<SUI> gas coins found for sender: {sender}");
    };
    let coin: Coin = first;
    Ok(coin.object_ref())
}

async fn load_sender_sui_coins(
    client: &Option<sui_sdk::SuiClient>,
    sender: SuiAddress,
) -> Result<Vec<ObjectRef>> {
    let Some(client) = client else {
        return Ok(vec![]);
    };
    // Pull a small page of SUI coins. This is only for benchmarking/heuristics.
    let page = client
        .coin_read_api()
        .get_coins(sender, Some(SUI_COIN_TYPE.to_string()), None, Some(50))
        .await
        .context("get_coins (Coin<SUI>)")?;
    Ok(page
        .data
        .into_iter()
        .map(|c: Coin| c.object_ref())
        .collect())
}

fn created_types_from_object_changes(changes: &[ObjectChange]) -> BTreeSet<String> {
    let mut out = BTreeSet::<String>::new();
    for ch in changes {
        if let ObjectChange::Created { object_type, .. } = ch {
            out.insert(object_type.to_string());
        }
    }
    out
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let sender_s = normalize_sui_address(&args.sender)?;
    let sender = SuiAddress::from_str(&sender_s)
        .with_context(|| format!("parse sender address: {sender_s}"))?;

    let v = read_json(&args.ptb_spec)?;
    let spec: PtbSpec = serde_json::from_value(v).context("PTB spec must be {\"calls\": [...]}")?;

    let client = match args.mode {
        Mode::BuildOnly => None,
        Mode::DevInspect | Mode::DryRun => Some(
            SuiClientBuilder::default()
                .build(&args.rpc_url)
                .await
                .with_context(|| format!("connect rpc: {}", args.rpc_url))?,
        ),
    };

    let (gas_coin_id, sender_sui_coins) = {
        let gas = if matches!(args.mode, Mode::DryRun) {
            Some(pick_gas_coin(&client, sender, args.gas_coin.as_deref()).await?)
        } else if let Some(id_s) = args.gas_coin.as_deref() {
            Some(resolve_object_ref(&client, parse_object_id(id_s)?).await?)
        } else if matches!(args.mode, Mode::BuildOnly) {
            // For build-only, we might need a dummy gas coin ID for context?
            // Or just pick one via mock logic.
            Some(pick_gas_coin(&client, sender, args.gas_coin.as_deref()).await?)
        } else {
            None
        };
        let coins = load_sender_sui_coins(&client, sender).await?;
        (gas.map(|r| r.0), coins)
    };

    let resolve_ctx = ResolveCtx {
        client: &client,
        sender,
        gas_coin_id,
        sender_sui_coins,
    };

    let mut ptb = ProgrammableTransactionBuilder::new();
    for call in &spec.calls {
        let (package, module, function) = parse_move_target(&call.target)?;
        let type_args: Vec<TypeTag> = call
            .type_args
            .iter()
            .map(|s| parse_type_tag(s))
            .collect::<Result<_>>()?;
        let mut call_args: Vec<Argument> = Vec::with_capacity(call.args.len());
        for a in &call.args {
            call_args.push(resolve_argument(&mut ptb, Some(&resolve_ctx), a).await?);
        }
        ptb.command(Command::MoveCall(Box::new(ProgrammableMoveCall {
            package,
            module: module.to_string(),
            function: function.to_string(),
            type_arguments: type_args.into_iter().map(TypeInput::from).collect(),
            arguments: call_args,
        })));
    }

    let pt = ptb.finish();
    let pt_bcs = bcs::to_bytes(&pt).context("bcs programmable transaction")?;
    let pt_b64 = BASE64_STANDARD.encode(pt_bcs);
    let mut static_created = BTreeSet::<String>::new();
    if let Some(dir) = args.bytecode_package_dir.as_ref() {
        let modules = load_bytecode_modules(dir)?;
        for call in &spec.calls {
            static_created.extend(static_created_types_for_call(&modules, call)?);
        }
    }

    let mut created = BTreeSet::<String>::new();
    let mut pt_b64_opt: Option<String> = None;
    let mut dry_run_json: Option<Value> = None;
    let mut dev_inspect_json: Option<Value> = None;
    let mode_used: String;

    match args.mode {
        Mode::DevInspect => {
            let tx = TransactionKind::ProgrammableTransaction(pt);
            let rpc = client.as_ref().expect("client present");
            let res = rpc
                .read_api()
                .dev_inspect_transaction_block(sender, tx, None, None, None)
                .await
                .context("dev_inspect_transaction_block")?;
            dev_inspect_json =
                Some(serde_json::to_value(&res).context("serialize devInspect JSON")?);
            created.extend(static_created.iter().cloned());
            mode_used = "dev_inspect".to_string();
        }
        Mode::DryRun => {
            let rpc = client.as_ref().expect("client present");
            let gas_price = rpc
                .read_api()
                .get_reference_gas_price()
                .await
                .context("get_reference_gas_price")?;
            // Pass the outer 'client' Option to pick_gas_coin
            let gas = pick_gas_coin(&client, sender, args.gas_coin.as_deref()).await?;
            let tx_data = TransactionData::new_programmable(
                sender,
                vec![gas],
                pt,
                args.gas_budget,
                gas_price,
            );
            let res: DryRunTransactionBlockResponse = rpc
                .read_api()
                .dry_run_transaction_block(tx_data)
                .await
                .context("dry_run_transaction_block")?;
            created.extend(created_types_from_object_changes(&res.object_changes));
            dry_run_json = Some(serde_json::to_value(&res).context("serialize dry-run JSON")?);
            mode_used = "dry_run".to_string();
        }
        Mode::BuildOnly => {
            pt_b64_opt = Some(pt_b64);
            created.extend(static_created.iter().cloned());
            mode_used = "build_only".to_string();
        }
    }

    let out = OutputJson {
        mode_used,
        created_object_types: created.into_iter().collect(),
        static_created_object_types: static_created.into_iter().collect(),
        programmable_transaction_bcs_base64: pt_b64_opt,
        dry_run: dry_run_json,
        dev_inspect: dev_inspect_json,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&out).context("serialize output JSON")?
    );
    Ok(())
}
