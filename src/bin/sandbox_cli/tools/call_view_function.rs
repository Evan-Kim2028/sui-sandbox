//! Execute a Move function via the local VM from supplied bytecode.

use anyhow::{Context, Result};
use base64::Engine;
use clap::Parser;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::TypeTag;
use serde::Deserialize;
use std::collections::HashSet;
use std::collections::{BTreeSet, HashMap, VecDeque};

use sui_package_extractor::extract_module_dependency_ids;
use sui_package_extractor::utils::is_framework_address;
use sui_sandbox_core::ptb::{Argument, Command, ObjectInput, PTBExecutor};
use sui_sandbox_core::resolver::{LocalModuleResolver, ModuleProvider};
use sui_sandbox_core::types::parse_type_tag;
use sui_sandbox_core::vm::{SimulationConfig, VMHarness};
use sui_transport::decode_graphql_modules;
use sui_transport::graphql::GraphQLClient;

#[derive(Debug, Parser)]
#[command(
    name = "call-view-function",
    about = "Execute a Move function using local module bytecode and return base64 return values"
)]
pub struct CallViewFunctionCmd {
    /// Package ID containing the target view function
    #[arg(long, value_name = "ID")]
    package_id: String,

    /// Target module name
    #[arg(long, value_name = "MODULE")]
    module: String,

    /// Target function name
    #[arg(long, value_name = "FUNCTION")]
    function: String,

    /// Type arguments (e.g., "0x2::coin::Coin")
    #[arg(long, value_name = "TYPE", num_args(1))]
    type_args: Vec<String>,

    /// Object inputs JSON array
    #[arg(long, value_name = "JSON")]
    object_inputs: Option<String>,

    /// Pure inputs base64 JSON array
    #[arg(long, value_name = "JSON")]
    pure_inputs: Option<String>,

    /// Child-object map JSON object
    #[arg(long, value_name = "JSON")]
    child_objects: Option<String>,

    /// Package bytecode map JSON object: {"0x2":["base64module",...]}
    #[arg(long, value_name = "JSON")]
    package_bytecodes: Option<String>,

    /// Resolve transitive dependencies using GraphQL
    #[arg(long, default_value_t = true, value_name = "BOOL")]
    fetch_deps: bool,
}

#[derive(Debug, Deserialize)]
struct ObjectInputSpec {
    object_id: String,
    #[serde(rename = "bcs_bytes")]
    bcs_bytes: String,
    type_tag: String,
    #[serde(default)]
    is_shared: bool,
    #[serde(default)]
    mutable: bool,
}

#[derive(Debug, Deserialize)]
struct ChildInputSpec {
    child_id: String,
    #[serde(rename = "bcs_bytes")]
    bcs_bytes: String,
    type_tag: String,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct PackageBytecodeMap(HashMap<String, Vec<String>>);

impl CallViewFunctionCmd {
    pub async fn execute(&self, json_output: bool) -> Result<()> {
        let value = run(self)?;
        let _ = json_output;
        let output = serde_json::to_string_pretty(&value)?;
        println!("{}", output);
        Ok(())
    }
}

fn fetch_package_modules(
    graphql: &GraphQLClient,
    package_id: &str,
) -> Result<Vec<(String, Vec<u8>)>> {
    let pkg = graphql
        .fetch_package(package_id)
        .with_context(|| format!("fetch package {}", package_id))?;
    decode_graphql_modules(package_id, &pkg.modules)
}

fn parse_object_inputs(raw: &Option<String>) -> Result<Vec<ObjectInputSpec>> {
    match raw {
        Some(raw) if !raw.trim().is_empty() => {
            serde_json::from_str(raw).context("invalid --object-inputs JSON")
        }
        Some(raw) => {
            if raw.trim() == "[]" {
                Ok(Vec::new())
            } else {
                serde_json::from_str(raw).context("invalid --object-inputs JSON")
            }
        }
        None => Ok(Vec::new()),
    }
}

fn parse_pure_inputs(raw: &Option<String>) -> Result<Vec<String>> {
    match raw {
        Some(raw) if !raw.trim().is_empty() => {
            serde_json::from_str(raw).context("invalid --pure-inputs JSON")
        }
        Some(raw) => {
            if raw.trim() == "[]" {
                Ok(Vec::new())
            } else {
                serde_json::from_str(raw).context("invalid --pure-inputs JSON")
            }
        }
        None => Ok(Vec::new()),
    }
}

fn parse_child_objects(raw: &Option<String>) -> Result<HashMap<String, Vec<ChildInputSpec>>> {
    match raw {
        Some(raw) if !raw.trim().is_empty() => {
            serde_json::from_str(raw).context("invalid --child-objects JSON")
        }
        _ => Ok(HashMap::new()),
    }
}

fn parse_package_bytecodes(raw: &Option<String>) -> Result<HashMap<String, Vec<Vec<u8>>>> {
    let value: HashMap<String, Vec<String>> = match raw {
        Some(raw) if !raw.trim().is_empty() => {
            let PackageBytecodeMap(map) =
                serde_json::from_str(raw).context("invalid --package-bytecodes JSON")?;
            map
        }
        _ => HashMap::new(),
    };

    let mut out = HashMap::new();
    for (pkg_id, encoded_modules) in value {
        let mut decoded = Vec::new();
        for encoded in encoded_modules {
            decoded.push(
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .with_context(|| format!("decode package bytecode for {}", pkg_id))?,
            );
        }
        out.insert(pkg_id, decoded);
    }
    Ok(out)
}

fn extract_type_args_package_ids(raw_types: &[String]) -> BTreeSet<AccountAddress> {
    let mut out = BTreeSet::new();
    for ty in raw_types {
        for id in sui_sandbox_core::utilities::extract_package_ids_from_type(ty) {
            if let Ok(addr) = AccountAddress::from_hex_literal(&id) {
                out.insert(addr);
            }
        }
    }
    out
}

fn parse_address(addr: &str) -> Result<AccountAddress> {
    AccountAddress::from_hex_literal(addr).context("invalid address")
}

fn build_child_fetcher(
    child_objects: &HashMap<String, Vec<ChildInputSpec>>,
) -> Result<sui_sandbox_core::sandbox_runtime::ChildFetcherFn> {
    let mut child_map: HashMap<(AccountAddress, AccountAddress), (TypeTag, Vec<u8>)> =
        HashMap::new();
    for (parent_id_str, children) in child_objects {
        let parent_id = parse_address(parent_id_str)?;
        for child in children {
            let child_id = parse_address(&child.child_id)?;
            let type_tag = parse_type_tag(&child.type_tag)
                .with_context(|| format!("invalid child object type tag {}", child.type_tag))?;
            let bcs = base64::engine::general_purpose::STANDARD
                .decode(&child.bcs_bytes)
                .with_context(|| {
                    format!(
                        "decode child object {} bcs for parent {}",
                        child_id, parent_id
                    )
                })?;
            child_map.insert((parent_id, child_id), (type_tag, bcs));
        }
    }

    let fetcher: sui_sandbox_core::sandbox_runtime::ChildFetcherFn =
        Box::new(move |parent, child| child_map.get(&(parent, child)).cloned());
    Ok(fetcher)
}

fn parse_module_names(modules: &[(String, Vec<u8>)]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (path_name, bytes) in modules {
        let name = if let Ok(module) = CompiledModule::deserialize_with_defaults(bytes) {
            module.self_id().name().to_string()
        } else {
            path_name.clone()
        };
        out.push((name, bytes.clone()));
    }
    out
}

fn run(cmd: &CallViewFunctionCmd) -> Result<serde_json::Value> {
    let object_inputs = parse_object_inputs(&cmd.object_inputs)?;
    let pure_inputs = parse_pure_inputs(&cmd.pure_inputs)?;
    let child_inputs = parse_child_objects(&cmd.child_objects)?;
    let package_bytecodes = parse_package_bytecodes(&cmd.package_bytecodes)?;

    let target_addr = parse_address(&cmd.package_id)?;

    let mut resolver = LocalModuleResolver::with_sui_framework()?;
    let mut loaded = BTreeSet::new();
    let mut fetch_queue = VecDeque::new();
    let mut missing = HashSet::new();

    if !is_framework_address(&target_addr) {
        fetch_queue.push_back(target_addr);
    }

    for (package_id, raw_modules) in &package_bytecodes {
        let package_addr = parse_address(package_id)?;
        if is_framework_address(&package_addr) {
            continue;
        }

        let package_sources: Vec<(String, Vec<u8>)> = raw_modules
            .iter()
            .enumerate()
            .map(|(idx, bytes)| (format!("module_{idx}"), bytes.clone()))
            .collect();
        let named_modules = parse_module_names(&package_sources);
        let dep_addrs = extract_module_dependency_ids(&named_modules);
        resolver.load_package_at(named_modules, package_addr)?;
        loaded.insert(package_addr);

        for dep_addr in dep_addrs {
            if !loaded.contains(&dep_addr) {
                fetch_queue.push_back(dep_addr);
            }
        }
    }

    for addr in extract_type_args_package_ids(&cmd.type_args) {
        if !loaded.contains(&addr) {
            fetch_queue.push_back(addr);
        }
    }
    for object_input in &object_inputs {
        let type_tag = parse_type_tag(&object_input.type_tag)
            .with_context(|| format!("invalid object input type tag {}", object_input.type_tag))?;
        for pkg_id in
            sui_sandbox_core::utilities::extract_package_ids_from_type(&object_input.type_tag)
        {
            if let Ok(addr) = AccountAddress::from_hex_literal(&pkg_id) {
                if !loaded.contains(&addr) {
                    fetch_queue.push_back(addr);
                }
            }
        }
        let _ = type_tag; // used only for compile-time type checking above
    }

    for child_children in child_inputs.values() {
        for child in child_children {
            for pkg_id in
                sui_sandbox_core::utilities::extract_package_ids_from_type(&child.type_tag)
            {
                if let Ok(addr) = AccountAddress::from_hex_literal(&pkg_id) {
                    if !loaded.contains(&addr) {
                        fetch_queue.push_back(addr);
                    }
                }
            }
        }
    }

    if cmd.fetch_deps && !fetch_queue.is_empty() {
        let graphql = GraphQLClient::new("https://fullnode.mainnet.sui.io:443");
        let mut visited = loaded.clone();
        let mut rounds = 0usize;
        while let Some(package_id) = fetch_queue.pop_front() {
            if !visited.insert(package_id) || is_framework_address(&package_id) {
                continue;
            }
            rounds += 1;
            if rounds > 8 {
                eprintln!(
                    "Dependency resolution reached max depth (8), skipping remaining packages"
                );
                break;
            }

            let package_hex = package_id.to_hex_literal();
            let modules = fetch_package_modules(&graphql, &package_hex)?;
            if modules.is_empty() {
                missing.insert(package_id);
                continue;
            }

            let module_names = parse_module_names(&modules);
            resolver.load_package_at(module_names.clone(), package_id)?;
            for dep_addr in extract_module_dependency_ids(&module_names) {
                if !visited.contains(&dep_addr) {
                    fetch_queue.push_back(dep_addr);
                }
            }
        }

        if !missing.is_empty() && std::env::var("SUI_SANDBOX_DEBUG_JSON").is_err() {
            eprintln!(
                "Warning: failed to fetch {} dependency package(s)",
                missing.len()
            );
        }
    }

    let config = SimulationConfig::default();
    let mut vm = VMHarness::with_config(&resolver, false, config)?;

    if !child_inputs.is_empty() {
        vm.set_child_fetcher(build_child_fetcher(&child_inputs)?);
    }

    let mut executor = PTBExecutor::new(&mut vm);
    let mut input_indices = Vec::new();

    for object_input in object_inputs {
        let addr = parse_address(&object_input.object_id)
            .with_context(|| format!("invalid object ID {}", object_input.object_id))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(object_input.bcs_bytes)
            .context("decode object input bcs")?;
        let type_tag = parse_type_tag(&object_input.type_tag)
            .with_context(|| format!("invalid type tag {}", object_input.type_tag))?;

        let idx = if object_input.is_shared {
            executor.add_object_input(ObjectInput::Shared {
                id: addr,
                bytes,
                type_tag: Some(type_tag),
                version: None,
                mutable: object_input.mutable,
            })?
        } else {
            executor.add_object_input(ObjectInput::ImmRef {
                id: addr,
                bytes,
                type_tag: Some(type_tag),
                version: None,
            })?
        };
        input_indices.push(idx);
    }

    for pure_b64 in pure_inputs {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(pure_b64)
            .context("decode pure input bcs")?;
        let idx = executor.add_pure_input(bytes).context("add pure input")?;
        input_indices.push(idx);
    }

    let mut type_args = Vec::new();
    for type_arg in &cmd.type_args {
        type_args.push(
            parse_type_tag(type_arg).with_context(|| format!("invalid type arg {}", type_arg))?,
        );
    }

    let args: Vec<Argument> = (0..input_indices.len() as u16)
        .map(Argument::Input)
        .collect();
    let command = vec![Command::MoveCall {
        package: target_addr,
        module: Identifier::new(cmd.module.as_str()).context("invalid module name")?,
        function: Identifier::new(cmd.function.as_str()).context("invalid function name")?,
        type_args,
        args,
    }];

    let effects = executor.execute_commands(&command)?;

    let return_values: Vec<Vec<String>> = effects
        .return_values
        .iter()
        .map(|command_values| {
            command_values
                .iter()
                .map(|rv| base64::engine::general_purpose::STANDARD.encode(rv))
                .collect()
        })
        .collect();

    let return_type_tags: Vec<Vec<Option<String>>> = effects
        .return_type_tags
        .iter()
        .map(|command_types| {
            command_types
                .iter()
                .map(|type_tag| type_tag.as_ref().map(|tag| tag.to_canonical_string(true)))
                .collect()
        })
        .collect();

    Ok(serde_json::json!({
        "success": effects.success,
        "error": effects.error,
        "return_values": return_values,
        "return_type_tags": return_type_tags,
        "gas_used": effects.gas_used,
    }))
}
