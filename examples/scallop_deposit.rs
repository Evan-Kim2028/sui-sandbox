//! Scallop Lending Deposit Replay Example (No Cache)
//!
//! Demonstrates replaying a historical Scallop deposit transaction locally.
//! This example fetches all data fresh via gRPC - no cache required.
//!
//! Run with: cargo run --example scallop_deposit
//!
//! ## Overview
//!
//! This example replays a Scallop lending protocol deposit transaction using:
//! - gRPC for transaction data and historical object versions
//! - On-demand child fetching for dynamic fields
//! - Automatic package dependency resolution via linkage tables
//!
//! ## Required Setup
//!
//! Set `SURFLUX_API_KEY` in your `.env` file:
//! ```
//! SURFLUX_API_KEY=your-api-key-here
//! ```
//!
//! ## Key Techniques
//!
//! 1. **Pure gRPC Fetching**: All data fetched fresh via Surflux gRPC
//! 2. **Historical Object Versions**: Uses `unchanged_loaded_runtime_objects` for exact versions
//! 3. **Package Linkage Tables**: Follows upgrade chains to get correct package versions
//! 4. **Address Aliasing**: Maps storage IDs to bytecode addresses for upgraded packages

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use sui_move_interface_extractor::benchmark::object_runtime::ChildFetcherFn;
use sui_move_interface_extractor::benchmark::resolver::LocalModuleResolver;
use sui_move_interface_extractor::benchmark::tx_replay::{
    grpc_to_fetched_transaction, CachedTransaction,
};
use sui_move_interface_extractor::benchmark::vm::{SimulationConfig, VMHarness};
use sui_move_interface_extractor::grpc::{GrpcClient, GrpcInput};

/// Scallop lending deposit transaction
const TX_DIGEST: &str = "JwCJUP4DEXRJna37UEXGJfLS1qMd1TUqdmvhpfyhNmU";

fn main() -> anyhow::Result<()> {
    // Load environment from .env file (SURFLUX_API_KEY required)
    // Searches for .env in current directory, then walks up parent directories
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║         Scallop Deposit Replay - Pure gRPC (No Cache)                ║");
    println!("║                                                                      ║");
    println!("║  Demonstrates fetching all historical state via gRPC without cache.  ║");
    println!("║  Requires SURFLUX_API_KEY in .env file.                              ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let result = replay_via_grpc_no_cache(TX_DIGEST)?;

    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                         VALIDATION SUMMARY                           ║");
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║ Scallop Deposit         | local: {:7} | expected: SUCCESS ║",
        if result { "SUCCESS" } else { "FAILURE" }
    );
    println!("╠══════════════════════════════════════════════════════════════════════╣");
    if result {
        println!("║ ✓ TRANSACTION MATCHES EXPECTED OUTCOME                              ║");
    } else {
        println!("║ ✗ TRANSACTION DID NOT MATCH EXPECTED OUTCOME                        ║");
        println!("║                                                                      ║");
        println!("║ Note: Version-locked protocols may fail verify_version checks if    ║");
        println!("║ the protocol was upgraded after the original transaction.           ║");
    }
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Replay a transaction using ONLY gRPC for data fetching (no cache).
fn replay_via_grpc_no_cache(tx_digest: &str) -> Result<bool> {
    let rt = tokio::runtime::Runtime::new()?;

    // =========================================================================
    // Step 1: Connect to Surflux gRPC
    // =========================================================================
    println!("Step 1: Connecting to Surflux gRPC...");

    let api_key = std::env::var("SURFLUX_API_KEY")
        .map_err(|_| anyhow!("SURFLUX_API_KEY not set in environment. Add it to .env file."))?;

    let grpc = rt.block_on(async {
        GrpcClient::with_api_key("https://grpc.surflux.dev:443", Some(api_key)).await
    })?;
    println!("   ✓ Connected to Surflux gRPC");

    // =========================================================================
    // Step 2: Fetch transaction via gRPC
    // =========================================================================
    println!("\nStep 2: Fetching transaction via gRPC...");

    let grpc_tx = rt
        .block_on(async { grpc.get_transaction(tx_digest).await })?
        .ok_or_else(|| anyhow!("Transaction not found: {}", tx_digest))?;

    println!("   Digest: {}", grpc_tx.digest);
    println!("   Sender: {}", grpc_tx.sender);
    println!("   Commands: {}", grpc_tx.commands.len());
    println!("   Inputs: {}", grpc_tx.inputs.len());
    println!("   Status: {:?}", grpc_tx.status);

    let tx_timestamp_ms = grpc_tx.timestamp_ms.unwrap_or(1700000000000);

    // =========================================================================
    // Step 3: Collect all historical object versions from gRPC effects
    // =========================================================================
    println!("\nStep 3: Collecting historical object versions...");

    let mut historical_versions: HashMap<String, u64> = HashMap::new();

    println!(
        "   unchanged_loaded_runtime_objects: {}",
        grpc_tx.unchanged_loaded_runtime_objects.len()
    );
    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    println!("   changed_objects: {}", grpc_tx.changed_objects.len());
    for (id, ver) in &grpc_tx.changed_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    println!(
        "   unchanged_consensus_objects: {}",
        grpc_tx.unchanged_consensus_objects.len()
    );
    for (id, ver) in &grpc_tx.unchanged_consensus_objects {
        historical_versions.insert(id.clone(), *ver);
    }

    for input in &grpc_tx.inputs {
        match input {
            GrpcInput::Object {
                object_id, version, ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*version);
            }
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*initial_version);
            }
            GrpcInput::Receiving {
                object_id, version, ..
            } => {
                historical_versions
                    .entry(object_id.clone())
                    .or_insert(*version);
            }
            GrpcInput::Pure { .. } => {}
        }
    }

    println!("   Total unique objects: {}", historical_versions.len());

    // =========================================================================
    // Step 4: Fetch all objects at historical versions via gRPC
    // =========================================================================
    println!("\nStep 4: Fetching objects at historical versions via gRPC...");

    let mut objects: HashMap<String, String> = HashMap::new();
    let mut object_types: HashMap<String, String> = HashMap::new();
    let mut packages: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut package_ids_to_fetch: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for cmd in &grpc_tx.commands {
        if let sui_move_interface_extractor::grpc::GrpcCommand::MoveCall { package, .. } = cmd {
            package_ids_to_fetch.insert(package.clone());
        }
    }
    println!(
        "   Packages referenced in commands: {}",
        package_ids_to_fetch.len()
    );

    let mut fetched_count = 0;
    let mut failed_count = 0;

    for (obj_id, version) in &historical_versions {
        let result =
            rt.block_on(async { grpc.get_object_at_version(obj_id, Some(*version)).await });

        match result {
            Ok(Some(obj)) => {
                if let Some(bcs) = &obj.bcs {
                    let bcs_b64 = base64::engine::general_purpose::STANDARD.encode(bcs);
                    objects.insert(obj_id.clone(), bcs_b64);
                    if let Some(type_str) = &obj.type_string {
                        object_types.insert(obj_id.clone(), type_str.clone());
                        for pkg_id in extract_package_ids_from_type(type_str) {
                            package_ids_to_fetch.insert(pkg_id);
                        }
                    }
                    fetched_count += 1;

                    if let Some(modules) = &obj.package_modules {
                        let modules_b64: Vec<(String, String)> = modules
                            .iter()
                            .map(|(name, bytes)| {
                                (
                                    name.clone(),
                                    base64::engine::general_purpose::STANDARD.encode(bytes),
                                )
                            })
                            .collect();
                        packages.insert(obj_id.clone(), modules_b64);
                        package_ids_to_fetch.remove(obj_id);
                    }
                }
            }
            Ok(None) => {
                failed_count += 1;
            }
            Err(_) => {
                failed_count += 1;
            }
        }
    }

    println!(
        "   ✓ Fetched {} objects ({} failed)",
        fetched_count, failed_count
    );

    // =========================================================================
    // Step 5: Fetch packages with transitive dependencies
    // =========================================================================
    println!("\nStep 5: Fetching packages with transitive dependencies...");

    let mut fetched_packages: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut packages_to_fetch = package_ids_to_fetch.clone();
    let max_depth = 10;

    let mut linkage_upgrades: HashMap<String, String> = HashMap::new();

    for depth in 0..max_depth {
        if packages_to_fetch.is_empty() {
            break;
        }

        println!(
            "   Depth {}: fetching {} packages...",
            depth,
            packages_to_fetch.len()
        );
        let mut new_deps: std::collections::HashSet<String> = std::collections::HashSet::new();

        for pkg_id in packages_to_fetch.iter() {
            let pkg_id_normalized = normalize_address(pkg_id);
            if fetched_packages.contains(&pkg_id_normalized) {
                continue;
            }

            let version = historical_versions.get(pkg_id).copied();
            let result = rt.block_on(async { grpc.get_object_at_version(pkg_id, version).await });

            match result {
                Ok(Some(obj)) => {
                    if let Some(modules) = &obj.package_modules {
                        let modules_b64: Vec<(String, String)> = modules
                            .iter()
                            .map(|(name, bytes)| {
                                (
                                    name.clone(),
                                    base64::engine::general_purpose::STANDARD.encode(bytes),
                                )
                            })
                            .collect();
                        println!(
                            "      ✓ {} v{} ({} modules)",
                            &pkg_id[..20.min(pkg_id.len())],
                            obj.version,
                            modules.len()
                        );

                        if let Some(linkage) = &obj.package_linkage {
                            for l in linkage {
                                let is_framework = matches!(
                                    l.original_id.as_str(),
                                    "0x0000000000000000000000000000000000000000000000000000000000000001"
                                        | "0x0000000000000000000000000000000000000000000000000000000000000002"
                                        | "0x0000000000000000000000000000000000000000000000000000000000000003"
                                        | "0x1"
                                        | "0x2"
                                        | "0x3"
                                );
                                if is_framework {
                                    continue;
                                }

                                let orig_normalized = normalize_address(&l.original_id);
                                let upgraded_normalized = normalize_address(&l.upgraded_id);
                                if orig_normalized != upgraded_normalized {
                                    linkage_upgrades.insert(
                                        orig_normalized.clone(),
                                        upgraded_normalized.clone(),
                                    );

                                    if !fetched_packages.contains(&upgraded_normalized)
                                        && !packages.contains_key(&upgraded_normalized)
                                    {
                                        new_deps.insert(upgraded_normalized.clone());
                                    }
                                }
                            }
                        }

                        for (_name, bytecode_b64) in &modules_b64 {
                            if let Ok(bytecode) =
                                base64::engine::general_purpose::STANDARD.decode(bytecode_b64)
                            {
                                let deps = extract_dependencies_from_bytecode(&bytecode);
                                for dep in deps {
                                    let dep_normalized = normalize_address(&dep);
                                    let actual_dep = linkage_upgrades
                                        .get(&dep_normalized)
                                        .cloned()
                                        .unwrap_or(dep_normalized);
                                    if !fetched_packages.contains(&actual_dep)
                                        && !packages.contains_key(&actual_dep)
                                    {
                                        new_deps.insert(actual_dep);
                                    }
                                }
                            }
                        }

                        let pkg_id_normalized = normalize_address(pkg_id);
                        packages.insert(pkg_id_normalized.clone(), modules_b64);
                        fetched_packages.insert(pkg_id_normalized);
                    }
                }
                Ok(None) => {
                    fetched_packages.insert(pkg_id_normalized.clone());
                }
                Err(_) => {
                    fetched_packages.insert(pkg_id_normalized.clone());
                }
            }
        }

        packages_to_fetch = new_deps;
    }

    if !linkage_upgrades.is_empty() {
        println!("   Linkage upgrades: {} mappings", linkage_upgrades.len());
    }

    println!("   Total packages: {}", packages.len());

    // =========================================================================
    // Step 6: Build transaction structure
    // =========================================================================
    println!("\nStep 6: Building transaction structure...");

    let fetched_tx = grpc_to_fetched_transaction(&grpc_tx)?;
    let mut cached = CachedTransaction::new(fetched_tx);

    for (pkg_id, modules) in packages {
        cached.packages.insert(pkg_id, modules);
    }

    cached.objects = objects;
    cached.object_types = object_types;
    cached.object_versions = historical_versions.clone();

    println!("   ✓ Built CachedTransaction");
    println!("      Packages: {}", cached.packages.len());
    println!("      Objects: {}", cached.objects.len());

    // =========================================================================
    // Step 7: Build module resolver
    // =========================================================================
    println!("\nStep 7: Building module resolver...");

    let mut resolver = LocalModuleResolver::new();

    let mut module_load_count = 0;
    let mut alias_count = 0;
    let mut skipped_count = 0;

    for (pkg_id, modules) in &cached.packages {
        let pkg_id_normalized = normalize_address(pkg_id);
        if let Some(upgraded_id) = linkage_upgrades.get(&pkg_id_normalized) {
            if cached.packages.contains_key(upgraded_id) {
                skipped_count += 1;
                continue;
            }
        }

        let target_addr = AccountAddress::from_hex_literal(pkg_id).ok();

        let decoded_modules: Vec<(String, Vec<u8>)> = modules
            .iter()
            .filter_map(|(name, b64)| {
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .ok()
                    .map(|bytes| (name.clone(), bytes))
            })
            .collect();

        match resolver.add_package_modules_at(decoded_modules, target_addr) {
            Ok((count, source_addr)) => {
                module_load_count += count;
                if let (Some(target), Some(source)) = (target_addr, source_addr) {
                    if target != source {
                        alias_count += 1;
                    }
                }
            }
            Err(e) => {
                println!(
                    "   ! Failed to load package {}: {}",
                    &pkg_id[..16.min(pkg_id.len())],
                    e
                );
            }
        }
    }
    println!(
        "   ✓ Loaded {} user modules ({} packages with aliases, {} skipped)",
        module_load_count, alias_count, skipped_count
    );

    match resolver.load_sui_framework() {
        Ok(n) => println!("   ✓ Loaded {} framework modules", n),
        Err(e) => println!("   ! Framework load failed: {}", e),
    }

    println!("   ✓ Resolver ready");

    // =========================================================================
    // Step 8: Create VM harness
    // =========================================================================
    println!("\nStep 8: Creating VM harness...");

    let sender_hex = grpc_tx.sender.strip_prefix("0x").unwrap_or(&grpc_tx.sender);
    let sender_address = AccountAddress::from_hex_literal(&format!("0x{:0>64}", sender_hex))?;
    println!("   Sender: 0x{}", hex::encode(sender_address.as_ref()));

    let config = SimulationConfig::default()
        .with_clock_base(tx_timestamp_ms)
        .with_sender_address(sender_address);

    let mut harness = VMHarness::with_config(&resolver, false, config)?;
    println!("   ✓ VM harness created");

    // =========================================================================
    // Step 9: Set up on-demand child fetcher
    // =========================================================================
    println!("\nStep 9: Setting up on-demand child fetcher...");

    let grpc_arc = Arc::new(grpc);
    let historical_arc = Arc::new(historical_versions.clone());
    let grpc_for_closure = grpc_arc.clone();
    let historical_for_closure = historical_arc.clone();

    let child_fetcher: ChildFetcherFn = Box::new(move |child_id: AccountAddress| {
        let child_id_str = child_id.to_hex_literal();

        let version = historical_for_closure.get(&child_id_str).copied();

        let rt = tokio::runtime::Runtime::new().ok()?;
        let result = rt.block_on(async {
            grpc_for_closure
                .get_object_at_version(&child_id_str, version)
                .await
        });

        if let Ok(Some(obj)) = result {
            if let (Some(type_str), Some(bcs)) = (&obj.type_string, &obj.bcs) {
                if let Some(type_tag) = parse_type_tag_simple(type_str) {
                    return Some((type_tag, bcs.clone()));
                }
            }
        }

        None
    });

    harness.set_child_fetcher(child_fetcher);
    println!("   ✓ Child fetcher configured");

    // =========================================================================
    // Step 10: Register input objects
    // =========================================================================
    println!("\nStep 10: Registering input objects...");

    let mut registered_count = 0;
    for (obj_id, version) in &historical_versions {
        if let Ok(addr) = AccountAddress::from_hex_literal(obj_id) {
            harness.add_sui_input_object(
                addr,
                *version,
                sui_types::object::Owner::Shared {
                    initial_shared_version: sui_types::base_types::SequenceNumber::from_u64(
                        *version,
                    ),
                },
            );
            registered_count += 1;
        }
    }
    println!("   ✓ Registered {} input objects", registered_count);

    // =========================================================================
    // Step 11: Execute transaction replay
    // =========================================================================
    println!("\nStep 11: Executing transaction replay...");

    let address_aliases =
        sui_move_interface_extractor::benchmark::tx_replay::build_address_aliases_for_test(&cached);
    if !address_aliases.is_empty() {
        println!("   Address aliases for replay: {}", address_aliases.len());
    }

    harness.set_address_aliases(address_aliases.clone());

    let result = cached.transaction.replay_with_objects_and_aliases(
        &mut harness,
        &cached.objects,
        &address_aliases,
    )?;

    println!(
        "\n  Local execution: {}",
        if result.local_success {
            "SUCCESS"
        } else {
            "FAILURE"
        }
    );

    if !result.local_success {
        if let Some(err) = &result.local_error {
            println!("  Error: {}", err);
        }
    }

    Ok(result.local_success)
}

// ═══════════════════════════════════════════════════════════════════════════════
// HELPER FUNCTIONS
//
// These helper functions are self-contained within this example to demonstrate
// the complete workflow without external dependencies. They handle:
//
// 1. Type parsing - Converting Sui type strings to Move TypeTags
// 2. Address normalization - Ensuring consistent 66-char address format
// 3. Dependency extraction - Parsing bytecode to find package dependencies
// ═══════════════════════════════════════════════════════════════════════════════

/// Parse a Sui type string into a Move TypeTag.
fn parse_type_tag_simple(type_str: &str) -> Option<TypeTag> {
    match type_str {
        "u8" => return Some(TypeTag::U8),
        "u64" => return Some(TypeTag::U64),
        "u128" => return Some(TypeTag::U128),
        "u256" => return Some(TypeTag::U256),
        "bool" => return Some(TypeTag::Bool),
        "address" => return Some(TypeTag::Address),
        _ => {}
    }

    if type_str.starts_with("vector<") && type_str.ends_with('>') {
        let inner = &type_str[7..type_str.len() - 1];
        return parse_type_tag_simple(inner).map(|t| TypeTag::Vector(Box::new(t)));
    }

    let (base_type, type_params_str) = if let Some(idx) = type_str.find('<') {
        (
            &type_str[..idx],
            Some(&type_str[idx + 1..type_str.len() - 1]),
        )
    } else {
        (type_str, None)
    };

    let parts: Vec<&str> = base_type.split("::").collect();
    if parts.len() != 3 {
        return None;
    }

    let address = AccountAddress::from_hex_literal(parts[0]).ok()?;
    let module = Identifier::new(parts[1]).ok()?;
    let name = Identifier::new(parts[2]).ok()?;

    let type_params = type_params_str
        .map(|s| {
            split_type_params(s)
                .iter()
                .filter_map(|t| parse_type_tag_simple(t.trim()))
                .collect()
        })
        .unwrap_or_default();

    Some(TypeTag::Struct(Box::new(StructTag {
        address,
        module,
        name,
        type_params,
    })))
}

/// Split type parameters respecting nested angle brackets.
fn split_type_params(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }

    if start < s.len() {
        result.push(&s[start..]);
    }

    result
}

/// Extract package IDs from a type string.
fn extract_package_ids_from_type(type_str: &str) -> Vec<String> {
    use std::collections::HashSet;
    let mut package_ids = HashSet::new();

    let framework_prefixes = [
        "0x1::",
        "0x2::",
        "0x3::",
        "0x0000000000000000000000000000000000000000000000000000000000000001::",
        "0x0000000000000000000000000000000000000000000000000000000000000002::",
        "0x0000000000000000000000000000000000000000000000000000000000000003::",
    ];

    let mut i = 0;
    let chars: Vec<char> = type_str.chars().collect();

    while i < chars.len() {
        if i + 2 < chars.len() && chars[i] == '0' && chars[i + 1] == 'x' {
            let start = i;
            i += 2;
            while i < chars.len() && (chars[i].is_ascii_hexdigit()) {
                i += 1;
            }
            if i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] == ':' {
                let pkg_id: String = chars[start..i].iter().collect();
                let full_prefix = format!("{}::", pkg_id);
                if !framework_prefixes.iter().any(|p| full_prefix == *p) {
                    package_ids.insert(pkg_id);
                }
            }
        } else {
            i += 1;
        }
    }

    package_ids.into_iter().collect()
}

/// Normalize a Sui address to a consistent 66-character format.
fn normalize_address(addr: &str) -> String {
    let addr = addr.strip_prefix("0x").unwrap_or(addr);
    format!("0x{:0>64}", addr)
}

/// Extract package addresses that a module depends on from its bytecode.
fn extract_dependencies_from_bytecode(bytecode: &[u8]) -> Vec<String> {
    use std::collections::BTreeSet;

    let framework_addrs: BTreeSet<AccountAddress> = [
        AccountAddress::from_hex_literal("0x1").unwrap(),
        AccountAddress::from_hex_literal("0x2").unwrap(),
        AccountAddress::from_hex_literal("0x3").unwrap(),
    ]
    .into_iter()
    .collect();

    let mut deps = Vec::new();

    if let Ok(module) = CompiledModule::deserialize_with_defaults(bytecode) {
        for handle in &module.module_handles {
            let addr = *module.address_identifier_at(handle.address);
            if !framework_addrs.contains(&addr) {
                deps.push(addr.to_hex_literal());
            }
        }
    }

    deps
}
