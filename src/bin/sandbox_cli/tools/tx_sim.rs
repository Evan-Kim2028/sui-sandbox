//! Transaction simulation tool using gRPC
//!
//! Simulates Sui transactions (dev-inspect or dry-run) for benchmarking.
//! This tool uses gRPC exclusively, replacing the deprecated JSON-RPC approach.

use anyhow::{anyhow, bail, Context, Result};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use clap::{Parser, ValueEnum};
use move_binary_format::file_format::{Bytecode, CompiledModule, SignatureToken};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use sui_transport::grpc::generated::sui_rpc_v2 as proto;
use sui_transport::grpc::GrpcClient;

type ProtoArgument = proto::Argument;
type ProtoCommand = proto::Command;
type ProtoInput = proto::Input;
type ProtoInputKind = proto::input::InputKind;
type ProtoMoveCall = proto::MoveCall;
type ProtoProgrammableTransaction = proto::ProgrammableTransaction;
type ProtoTransaction = proto::Transaction;
type ProtoTransactionKind = proto::TransactionKind;

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
    about = "Tx simulation helper (dev-inspect or dry-run) using gRPC"
)]
pub struct TxSimCmd {
    /// gRPC endpoint URL (default: mainnet fullnode)
    #[arg(long, default_value = "https://fullnode.mainnet.sui.io:443")]
    grpc_url: String,

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
    simulation_result: Option<Value>,
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

fn parse_move_target(s: &str) -> Result<(String, String, String)> {
    // Expected: 0xADDR::module::function
    let parts: Vec<&str> = s.split("::").collect();
    if parts.len() != 3 {
        bail!("invalid move target (expected 0xADDR::module::function): {s}");
    }
    let package = normalize_sui_address(parts[0])?;
    let module = parse_identifier(parts[1])?.to_string();
    let function = parse_identifier(parts[2])?.to_string();
    Ok((package, module, function))
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
    let m = match modules.get(&module_name) {
        Some(m) => m,
        None => return Ok(out),
    };

    // Resolve function definition by name.
    let mut fn_def_idx = None;
    for (i, def) in m.function_defs().iter().enumerate() {
        let fh = m.function_handle_at(def.function);
        let name = m.identifier_at(fh.name).to_string();
        if name == function_name {
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
    let bytecode_dir = bytecode_package_dir.join("bytecode_modules");
    let mut out = HashMap::new();
    let rd = std::fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read_dir {}", bytecode_dir.display()))?;
    for entry in rd {
        let entry = entry?;
        let path = entry.path();
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
    Ok(out)
}

/// Build a gRPC proto Input from a JSON argument spec
fn build_proto_input(
    v: &Value,
    input_index: &mut u32,
) -> Result<(ProtoInput, Option<ProtoArgument>)> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("PTB arg must be an object (got {v:?})"))?;

    // Handle result references (these don't create inputs)
    if let Some(res) = obj.get("result") {
        let i = res
            .as_u64()
            .ok_or_else(|| anyhow!("result index must be u16"))?;
        let arg = ProtoArgument {
            kind: Some(proto::argument::ArgumentKind::Result as i32),
            result: Some(i as u32),
            input: None,
            subresult: None,
        };
        return Ok((ProtoInput::default(), Some(arg)));
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
        let arg = ProtoArgument {
            kind: Some(proto::argument::ArgumentKind::Result as i32),
            result: Some(cmd as u32),
            input: None,
            subresult: Some(res as u32),
        };
        return Ok((ProtoInput::default(), Some(arg)));
    }

    // Otherwise, this creates an input
    if obj.len() != 1 {
        bail!("PTB arg must have exactly 1 key (got {obj:?})");
    }
    let (k, vv) = obj.iter().next().expect("len==1");

    let idx = *input_index;
    *input_index += 1;

    let input = match k.as_str() {
        "u8" | "u16" | "u32" | "u64" | "bool" | "address" | "vector_u8_utf8" | "vector_u8_hex"
        | "vector_address" | "vector_bool" | "vector_u16" | "vector_u32" | "vector_u64" => {
            // Pure value - serialize to BCS
            let bcs_bytes = serialize_pure_value(k, vv)?;
            ProtoInput {
                kind: Some(ProtoInputKind::Pure as i32),
                pure: Some(bcs_bytes),
                object_id: None,
                version: None,
                digest: None,
                mutable: None,
                mutability: None,
                funds_withdrawal: None,
                literal: None,
            }
        }
        "imm_or_owned_object" => {
            let s = vv
                .as_str()
                .ok_or_else(|| anyhow!("imm_or_owned_object must be a string object id"))?;
            let object_id = normalize_sui_address(s)?;
            ProtoInput {
                kind: Some(ProtoInputKind::ImmutableOrOwned as i32),
                pure: None,
                object_id: Some(object_id),
                version: None, // Will be resolved by the fullnode
                digest: None,
                mutable: None,
                mutability: None,
                funds_withdrawal: None,
                literal: None,
            }
        }
        "shared_object" => {
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
            let object_id = normalize_sui_address(id_s)?;
            // For shared objects, version is the initial_shared_version
            // The fullnode will resolve it if not provided
            ProtoInput {
                kind: Some(ProtoInputKind::Shared as i32),
                pure: None,
                object_id: Some(object_id),
                version: None, // Will be resolved
                digest: None,
                mutable: Some(mutable),
                mutability: None,
                funds_withdrawal: None,
                literal: None,
            }
        }
        other => bail!("unsupported PTB arg kind: {other}"),
    };

    let arg = ProtoArgument {
        kind: Some(proto::argument::ArgumentKind::Input as i32),
        input: Some(idx),
        result: None,
        subresult: None,
    };

    Ok((input, Some(arg)))
}

/// Serialize a pure value to BCS bytes
fn serialize_pure_value(kind: &str, value: &Value) -> Result<Vec<u8>> {
    match kind {
        "u8" => {
            let n = value
                .as_u64()
                .ok_or_else(|| anyhow!("u8 must be an integer"))?;
            let x: u8 = n.try_into().context("u8 out of range")?;
            bcs::to_bytes(&x).context("bcs u8")
        }
        "u16" => {
            let n = value
                .as_u64()
                .ok_or_else(|| anyhow!("u16 must be an integer"))?;
            let x: u16 = n.try_into().context("u16 out of range")?;
            bcs::to_bytes(&x).context("bcs u16")
        }
        "u32" => {
            let n = value
                .as_u64()
                .ok_or_else(|| anyhow!("u32 must be an integer"))?;
            let x: u32 = n.try_into().context("u32 out of range")?;
            bcs::to_bytes(&x).context("bcs u32")
        }
        "u64" => {
            let n = value
                .as_u64()
                .ok_or_else(|| anyhow!("u64 must be an integer"))?;
            bcs::to_bytes(&n).context("bcs u64")
        }
        "bool" => {
            let b = value
                .as_bool()
                .ok_or_else(|| anyhow!("bool must be true/false"))?;
            bcs::to_bytes(&b).context("bcs bool")
        }
        "address" => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow!("address must be a string"))?;
            let addr = normalize_sui_address(s)?;
            // Address is 32 bytes
            let hex = addr.strip_prefix("0x").unwrap_or(&addr);
            let bytes = hex::decode(hex).context("parse address hex")?;
            bcs::to_bytes(&bytes).context("bcs address")
        }
        "vector_u8_utf8" => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow!("vector_u8_utf8 must be a string"))?;
            let bytes = s.as_bytes().to_vec();
            bcs::to_bytes(&bytes).context("bcs vector<u8>")
        }
        "vector_u8_hex" => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow!("vector_u8_hex must be a string"))?;
            let s = s.strip_prefix("0x").unwrap_or(s);
            if s.len() % 2 != 0 {
                bail!("vector_u8_hex must have even number of hex chars");
            }
            let bytes = hex::decode(s).context("parse hex bytes")?;
            bcs::to_bytes(&bytes).context("bcs vector<u8>")
        }
        "vector_address" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("vector_address must be an array"))?;
            let mut out: Vec<[u8; 32]> = Vec::with_capacity(arr.len());
            for el in arr {
                let s = el
                    .as_str()
                    .ok_or_else(|| anyhow!("vector_address elements must be strings"))?;
                let addr = normalize_sui_address(s)?;
                let hex = addr.strip_prefix("0x").unwrap_or(&addr);
                let bytes = hex::decode(hex).context("parse address hex")?;
                let arr: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| anyhow!("address must be 32 bytes"))?;
                out.push(arr);
            }
            bcs::to_bytes(&out).context("bcs vector<address>")
        }
        "vector_bool" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("vector_bool must be an array"))?;
            let mut out: Vec<bool> = Vec::with_capacity(arr.len());
            for el in arr {
                out.push(
                    el.as_bool()
                        .ok_or_else(|| anyhow!("vector_bool elements must be true/false"))?,
                );
            }
            bcs::to_bytes(&out).context("bcs vector<bool>")
        }
        "vector_u16" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("vector_u16 must be an array"))?;
            let mut out: Vec<u16> = Vec::with_capacity(arr.len());
            for el in arr {
                let n = el
                    .as_u64()
                    .ok_or_else(|| anyhow!("vector_u16 elements must be integers"))?;
                out.push(n.try_into().context("vector_u16 element out of range")?);
            }
            bcs::to_bytes(&out).context("bcs vector<u16>")
        }
        "vector_u32" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("vector_u32 must be an array"))?;
            let mut out: Vec<u32> = Vec::with_capacity(arr.len());
            for el in arr {
                let n = el
                    .as_u64()
                    .ok_or_else(|| anyhow!("vector_u32 elements must be integers"))?;
                out.push(n.try_into().context("vector_u32 element out of range")?);
            }
            bcs::to_bytes(&out).context("bcs vector<u32>")
        }
        "vector_u64" => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow!("vector_u64 must be an array"))?;
            let mut out: Vec<u64> = Vec::with_capacity(arr.len());
            for el in arr {
                out.push(
                    el.as_u64()
                        .ok_or_else(|| anyhow!("vector_u64 elements must be integers"))?,
                );
            }
            bcs::to_bytes(&out).context("bcs vector<u64>")
        }
        other => bail!("unsupported pure type: {other}"),
    }
}

/// Build a gRPC Transaction from a PTB spec
fn build_transaction(
    sender: &str,
    spec: &PtbSpec,
    _gas_budget: u64,
) -> Result<(ProtoTransaction, Vec<u8>)> {
    let mut inputs: Vec<ProtoInput> = Vec::new();
    let mut commands: Vec<ProtoCommand> = Vec::new();
    let mut input_index: u32 = 0;

    for call in &spec.calls {
        let (package, module, function) = parse_move_target(&call.target)?;

        let mut arguments: Vec<ProtoArgument> = Vec::new();
        for arg_json in &call.args {
            let (input, arg) = build_proto_input(arg_json, &mut input_index)?;
            if input.kind.is_some() {
                inputs.push(input);
            }
            if let Some(a) = arg {
                arguments.push(a);
            }
        }

        let move_call = ProtoMoveCall {
            package: Some(package),
            module: Some(module),
            function: Some(function),
            type_arguments: call.type_args.clone(),
            arguments,
        };

        commands.push(ProtoCommand {
            command: Some(proto::command::Command::MoveCall(move_call)),
        });
    }

    let ptb = ProtoProgrammableTransaction { inputs, commands };

    // Serialize PTB to BCS for the output
    // Note: We can't easily serialize proto to BCS, but we can provide the proto structure
    // For now, we'll skip the BCS output in build-only mode
    let ptb_bcs: Vec<u8> = Vec::new(); // Placeholder - gRPC handles serialization internally

    let tx_kind = ProtoTransactionKind {
        kind: Some(proto::transaction_kind::Kind::ProgrammableTransaction as i32),
        data: Some(proto::transaction_kind::Data::ProgrammableTransaction(ptb)),
    };

    let transaction = ProtoTransaction {
        bcs: None,
        digest: None,
        version: None,
        kind: Some(tx_kind),
        sender: Some(sender.to_string()),
        gas_payment: None, // Let the fullnode handle gas selection
        expiration: None,
    };

    Ok((transaction, ptb_bcs))
}

pub async fn run(cmd: &TxSimCmd) -> Result<()> {
    let args = cmd;

    let sender = normalize_sui_address(&args.sender)?;

    let v = read_json(&args.ptb_spec)?;
    let spec: PtbSpec = serde_json::from_value(v).context("PTB spec must be {\"calls\": [...]}")?;

    // Static analysis of created types from bytecode
    let mut static_created = BTreeSet::<String>::new();
    if let Some(dir) = args.bytecode_package_dir.as_ref() {
        let modules = load_bytecode_modules(dir)?;
        for call in &spec.calls {
            static_created.extend(static_created_types_for_call(&modules, call)?);
        }
    }

    // Build the transaction
    let (transaction, ptb_bcs) = build_transaction(&sender, &spec, args.gas_budget)?;

    let mut created = BTreeSet::<String>::new();
    let mut pt_b64_opt: Option<String> = None;
    let mut simulation_json: Option<Value> = None;
    let mode_used: String;

    match args.mode {
        Mode::BuildOnly => {
            // In build-only mode, we can only show static analysis results
            // since we can't easily serialize proto to BCS
            created.extend(static_created.iter().cloned());
            mode_used = "build_only".to_string();
            if !ptb_bcs.is_empty() {
                pt_b64_opt = Some(BASE64_STANDARD.encode(&ptb_bcs));
            }
        }
        Mode::DevInspect | Mode::DryRun => {
            // Connect to gRPC endpoint
            let client = GrpcClient::new(&args.grpc_url)
                .await
                .with_context(|| format!("connect gRPC: {}", args.grpc_url))?;

            let checks = match args.mode {
                Mode::DevInspect => {
                    mode_used = "dev_inspect".to_string();
                    proto::simulate_transaction_request::TransactionChecks::Disabled
                }
                Mode::DryRun => {
                    mode_used = "dry_run".to_string();
                    proto::simulate_transaction_request::TransactionChecks::Enabled
                }
                Mode::BuildOnly => unreachable!(),
            };

            let response = client
                .simulate_transaction(transaction, checks, matches!(args.mode, Mode::DryRun))
                .await?;

            // Also include static analysis
            created.extend(static_created.iter().cloned());

            let (success, error) = response
                .transaction
                .as_ref()
                .and_then(|tx| tx.effects.as_ref())
                .and_then(|effects| effects.status.as_ref())
                .map(|status| {
                    let success = status.success.unwrap_or(false);
                    let error = status.error.as_ref().and_then(|e| e.description.clone());
                    (success, error)
                })
                .unwrap_or((false, None));

            // Serialize simulation result to JSON
            simulation_json = Some(serde_json::json!({
                "success": success,
                "error": error,
                "command_outputs_count": response.command_outputs.len(),
            }));
        }
    }

    let out = OutputJson {
        mode_used,
        created_object_types: created.into_iter().collect(),
        static_created_object_types: static_created.into_iter().collect(),
        programmable_transaction_bcs_base64: pt_b64_opt,
        simulation_result: simulation_json,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&out).context("serialize output JSON")?
    );
    Ok(())
}

impl TxSimCmd {
    pub async fn execute(&self) -> Result<()> {
        run(self).await
    }
}
