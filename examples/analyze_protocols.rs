//! Protocol Package Analyzer
//!
//! Analyzes bytecode and struct layouts of popular Sui DeFi protocols
//! to identify version checking patterns and struct layouts.
//!
//! Run with: cargo run --example analyze_protocols
//!
//! ## What This Analyzes
//!
//! For each protocol, this tool examines:
//! 1. **Version Constants**: U64 constants in range 1-100 used in comparisons
//! 2. **Version Structs**: Structs with fields like `package_version` or `value`
//! 3. **Config Objects**: GlobalConfig, Market, Pool structs with version fields
//! 4. **Upgrade Patterns**: Linkage table entries showing upgrade chains

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use move_binary_format::file_format::{Bytecode, SignatureToken};
use move_binary_format::CompiledModule;
use sui_data_fetcher::grpc::GrpcClient;

/// Key mainnet packages to analyze
const PACKAGES_TO_ANALYZE: &[(&str, &str)] = &[
    (
        "Cetus AMM",
        "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb",
    ),
    (
        "Scallop Lending",
        "0xefe8b36d5b2e43728cc323298626b83177803521d195cfb11e15b910e892fddf",
    ),
    (
        "DeepBook v2",
        "0x000000000000000000000000000000000000000000000000000000000000dee9",
    ),
    (
        "Turbos Finance",
        "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1",
    ),
    (
        "Kriya DEX",
        "0xa0eba10b173538c8fecca1dff298e488402cc9ff374f8a12ca7758eebe830b66",
    ),
    (
        "Bluefin Exchange",
        "0x3492c874c1e3b3e2984e8c41b589e642d4d0a5d6459e5a9cfc2d52fd7c89c267",
    ),
];

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║              Sui DeFi Protocol Bytecode Analyzer                     ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝\n");

    let rt = tokio::runtime::Runtime::new()?;

    // Connect to gRPC
    let endpoint = std::env::var("SUI_GRPC_ENDPOINT")
        .or_else(|_| std::env::var("SURFLUX_GRPC_ENDPOINT"))
        .unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let api_key = std::env::var("SUI_GRPC_API_KEY")
        .or_else(|_| std::env::var("SURFLUX_API_KEY"))
        .ok();

    let grpc = Arc::new(rt.block_on(async { GrpcClient::with_api_key(&endpoint, api_key).await })?);
    println!("Connected to: {}\n", endpoint);

    for (name, address) in PACKAGES_TO_ANALYZE {
        println!("═══════════════════════════════════════════════════════════════════════");
        println!("  {} ({}...)", name, &address[..20]);
        println!("═══════════════════════════════════════════════════════════════════════");

        match rt.block_on(analyze_package(&grpc, address)) {
            Ok(analysis) => {
                print_analysis(&analysis);
            }
            Err(e) => {
                println!("  Error: {}\n", e);
            }
        }
    }

    Ok(())
}

#[derive(Default)]
#[allow(dead_code)]
struct PackageAnalysis {
    name: String,
    address: String,
    version: u64,
    module_count: usize,

    // Version detection
    version_constants: Vec<(String, u64)>, // (module_name, value)
    version_structs: Vec<VersionStructInfo>,

    // Linkage
    linkage_entries: Vec<(String, String, u64)>, // (original, upgraded, version)
    has_self_upgrade: bool,
    upgraded_address: Option<String>,
}

#[derive(Default, Clone)]
struct VersionStructInfo {
    module_name: String,
    struct_name: String,
    fields: Vec<(String, String)>, // (field_name, field_type)
    version_field_offset: Option<usize>,
    version_field_size: Option<usize>,
}

async fn analyze_package(grpc: &GrpcClient, address: &str) -> Result<PackageAnalysis> {
    let obj = grpc
        .get_object_at_version(address, None)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Package not found"))?;

    let mut analysis = PackageAnalysis {
        address: address.to_string(),
        version: obj.version,
        ..Default::default()
    };

    // Check linkage for self-upgrades
    if let Some(linkage) = &obj.package_linkage {
        for l in linkage {
            let orig = normalize(&l.original_id);
            let upgraded = normalize(&l.upgraded_id);

            if orig != upgraded {
                analysis
                    .linkage_entries
                    .push((orig.clone(), upgraded.clone(), l.upgraded_version));

                // Check for self-upgrade
                if orig == normalize(address) {
                    analysis.has_self_upgrade = true;
                    analysis.upgraded_address = Some(upgraded.clone());
                }
            }
        }
    }

    // If there's a self-upgrade, fetch the upgraded package
    let modules_to_analyze = if let Some(upgraded_addr) = &analysis.upgraded_address {
        let upgraded_obj = grpc
            .get_object_at_version(upgraded_addr, None)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Upgraded package not found"))?;
        analysis.version = upgraded_obj.version;
        upgraded_obj.package_modules
    } else {
        obj.package_modules
    };

    // Analyze modules
    if let Some(modules) = modules_to_analyze {
        analysis.module_count = modules.len();

        for (module_name, bytecode) in modules {
            if let Ok(module) = CompiledModule::deserialize_with_defaults(&bytecode) {
                analyze_module(&module, &module_name, &mut analysis);
            }
        }
    }

    Ok(analysis)
}

fn analyze_module(module: &CompiledModule, module_name: &str, analysis: &mut PackageAnalysis) {
    // Find version constants (U64 values 1-100 used in comparisons)
    let comparison_constants = find_comparison_constants(module);

    for const_idx in comparison_constants {
        if const_idx >= module.constant_pool().len() {
            continue;
        }

        let constant = &module.constant_pool()[const_idx];
        if constant.type_ == SignatureToken::U64 && constant.data.len() == 8 {
            let value = u64::from_le_bytes(constant.data[..8].try_into().unwrap());
            if (1..=100).contains(&value) {
                analysis
                    .version_constants
                    .push((module_name.to_string(), value));
            }
        }
    }

    // Find version-related structs using datatype_handles (Sui's naming)
    for struct_def in &module.struct_defs {
        let struct_handle = &module.datatype_handles[struct_def.struct_handle.0 as usize];
        let struct_name = module.identifier_at(struct_handle.name).to_string();

        // Look for structs that likely contain version fields
        let is_version_struct = struct_name.contains("Version")
            || struct_name.contains("Config")
            || struct_name.contains("Global")
            || struct_name.contains("Pool")
            || struct_name.contains("Market");

        if !is_version_struct {
            continue;
        }

        if let move_binary_format::file_format::StructFieldInformation::Declared(fields) =
            &struct_def.field_information
        {
            let mut struct_info = VersionStructInfo {
                module_name: module_name.to_string(),
                struct_name: struct_name.clone(),
                ..Default::default()
            };

            let mut offset = 0usize;
            for (_idx, field_def) in fields.iter().enumerate() {
                let field_name = module.identifier_at(field_def.name).to_string();
                let field_type = format_signature_token(&field_def.signature.0, module);

                // Check if this is a version field
                let is_version_field = field_name == "package_version"
                    || field_name == "value"
                    || field_name == "version";

                if is_version_field {
                    struct_info.version_field_offset = Some(offset);
                    struct_info.version_field_size = get_field_size(&field_def.signature.0);
                }

                struct_info.fields.push((field_name, field_type.clone()));

                // Estimate offset for next field
                if let Some(size) = get_field_size(&field_def.signature.0) {
                    offset += size;
                }
            }

            if !struct_info.fields.is_empty() {
                analysis.version_structs.push(struct_info);
            }
        }
    }
}

fn find_comparison_constants(module: &CompiledModule) -> HashSet<usize> {
    let mut comparison_constants = HashSet::new();

    for func_def in &module.function_defs {
        if let Some(code) = &func_def.code {
            for (i, instr) in code.code.iter().enumerate() {
                if let Bytecode::LdConst(const_idx) = instr {
                    // Check if next few instructions include a comparison
                    let has_comparison = code.code.iter().skip(i + 1).take(3).any(|next| {
                        matches!(
                            next,
                            Bytecode::Eq
                                | Bytecode::Neq
                                | Bytecode::Lt
                                | Bytecode::Le
                                | Bytecode::Gt
                                | Bytecode::Ge
                        )
                    });

                    if has_comparison {
                        comparison_constants.insert(const_idx.0 as usize);
                    }
                }
            }
        }
    }

    comparison_constants
}

fn format_signature_token(token: &SignatureToken, module: &CompiledModule) -> String {
    match token {
        SignatureToken::Bool => "bool".to_string(),
        SignatureToken::U8 => "u8".to_string(),
        SignatureToken::U16 => "u16".to_string(),
        SignatureToken::U32 => "u32".to_string(),
        SignatureToken::U64 => "u64".to_string(),
        SignatureToken::U128 => "u128".to_string(),
        SignatureToken::U256 => "u256".to_string(),
        SignatureToken::Address => "address".to_string(),
        SignatureToken::Signer => "signer".to_string(),
        SignatureToken::Vector(inner) => {
            format!("vector<{}>", format_signature_token(inner, module))
        }
        SignatureToken::Datatype(idx) => {
            let handle = &module.datatype_handles[idx.0 as usize];
            module.identifier_at(handle.name).to_string()
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, args) = inst.as_ref();
            let handle = &module.datatype_handles[idx.0 as usize];
            let name = module.identifier_at(handle.name).to_string();
            let type_args: Vec<_> = args
                .iter()
                .map(|t| format_signature_token(t, module))
                .collect();
            format!("{}<{}>", name, type_args.join(", "))
        }
        SignatureToken::Reference(inner) => format!("&{}", format_signature_token(inner, module)),
        SignatureToken::MutableReference(inner) => {
            format!("&mut {}", format_signature_token(inner, module))
        }
        SignatureToken::TypeParameter(idx) => format!("T{}", idx),
    }
}

fn get_field_size(token: &SignatureToken) -> Option<usize> {
    match token {
        SignatureToken::Bool => Some(1),
        SignatureToken::U8 => Some(1),
        SignatureToken::U16 => Some(2),
        SignatureToken::U32 => Some(4),
        SignatureToken::U64 => Some(8),
        SignatureToken::U128 => Some(16),
        SignatureToken::U256 => Some(32),
        SignatureToken::Address => Some(32),
        SignatureToken::Datatype(_) => None, // Variable size (struct)
        SignatureToken::DatatypeInstantiation(_) => None,
        SignatureToken::Vector(_) => None, // Variable size
        _ => None,
    }
}

fn normalize(addr: &str) -> String {
    let clean = addr.strip_prefix("0x").unwrap_or(addr).to_lowercase();
    format!("0x{}", clean.trim_start_matches('0'))
}

fn print_analysis(analysis: &PackageAnalysis) {
    println!("  Package Version: {}", analysis.version);
    println!("  Module Count: {}", analysis.module_count);

    if analysis.has_self_upgrade {
        println!("\n  ⚠️  SELF-UPGRADE DETECTED");
        if let Some(upgraded) = &analysis.upgraded_address {
            println!(
                "     Upgraded storage: {}...",
                &upgraded[..20.min(upgraded.len())]
            );
        }
    }

    if !analysis.linkage_entries.is_empty() {
        println!("\n  Linkage Upgrades:");
        for (orig, upgraded, ver) in &analysis.linkage_entries {
            if orig == upgraded {
                continue;
            }
            println!(
                "     {}... -> {}... (v{})",
                &orig[..16.min(orig.len())],
                &upgraded[..16.min(upgraded.len())],
                ver
            );
        }
    }

    if !analysis.version_constants.is_empty() {
        println!("\n  Version Constants (used in comparisons):");
        let mut by_module: HashMap<String, Vec<u64>> = HashMap::new();
        for (module, value) in &analysis.version_constants {
            by_module.entry(module.clone()).or_default().push(*value);
        }
        for (module, values) in by_module {
            let unique: HashSet<_> = values.into_iter().collect();
            let mut sorted: Vec<_> = unique.into_iter().collect();
            sorted.sort();
            println!("     {}::{:?}", module, sorted);
        }
    }

    // Print key version structs
    let key_structs: Vec<_> = analysis
        .version_structs
        .iter()
        .filter(|s| {
            s.struct_name.contains("GlobalConfig")
                || s.struct_name.contains("Version")
                || s.struct_name == "Pool"
                || s.struct_name == "Market"
        })
        .collect();

    if !key_structs.is_empty() {
        println!("\n  Key Version-Related Structs:");
        for s in key_structs {
            println!("     {}::{}", s.module_name, s.struct_name);
            for (i, (field_name, field_type)) in s.fields.iter().enumerate() {
                let marker = if s.version_field_offset.is_some()
                    && (field_name == "package_version"
                        || field_name == "value"
                        || field_name == "version")
                {
                    " ← VERSION FIELD"
                } else {
                    ""
                };
                println!("        {}: {}: {}{}", i, field_name, field_type, marker);
            }
            if let (Some(offset), Some(size)) = (s.version_field_offset, s.version_field_size) {
                println!(
                    "        [Version at byte offset ~{}, size {}]",
                    offset, size
                );
            }
        }
    }

    println!();
}
