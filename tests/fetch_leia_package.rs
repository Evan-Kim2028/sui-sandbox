// This test file is temporarily disabled due to API changes.
#![cfg(feature = "legacy_tests")]
//! Fetch the LEIA token package and add it to the transaction cache.
//!
//! This is a one-time script to complete the Cetus swap case study.

use sui_move_interface_extractor::benchmark::tx_replay::{CachedTransaction, TransactionFetcher};

const LEIA_PACKAGE_ID: &str = "0xb55d9fa9168c5f5f642f90b0330a47ccba9ef8e20a3207c1163d3d15c5c8663e";
const TX_DIGEST: &str = "7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp";

#[test]
fn fetch_and_cache_leia_package() {
    println!("=== Fetching LEIA Package ===\n");

    // Load the existing transaction cache
    let cache_dir = std::path::PathBuf::from(".tx-cache");
    let cache_file = cache_dir.join(format!("{}.json", TX_DIGEST));

    if !cache_file.exists() {
        println!("ERROR: Transaction cache file not found: {:?}", cache_file);
        println!("Run the transaction cache first.");
        return;
    }

    // Load cache
    let cache_data = std::fs::read_to_string(&cache_file).expect("Failed to read cache file");
    let mut cache: CachedTransaction =
        serde_json::from_str(&cache_data).expect("Failed to parse cache");

    println!("Loaded cache with {} packages", cache.packages.len());

    // Check if LEIA is already cached
    let leia_normalized = LEIA_PACKAGE_ID.to_lowercase();
    if cache.packages.contains_key(&leia_normalized) {
        println!("LEIA package already in cache!");
        return;
    }

    // Create client to fetch the package
    let endpoint = "https://fullnode.mainnet.sui.io:443";
    let client = TransactionFetcher::new(endpoint);

    println!("Fetching LEIA package from mainnet...");

    match client.fetch_package_modules(LEIA_PACKAGE_ID) {
        Ok(modules) => {
            println!("  Fetched {} modules:", modules.len());
            for (name, bytes) in &modules {
                println!("    - {} ({} bytes)", name, bytes.len());
            }

            // Add to cache
            cache.add_package(LEIA_PACKAGE_ID.to_string(), modules);
            println!("\nAdded LEIA package to cache");

            // Save cache
            let updated = serde_json::to_string_pretty(&cache).expect("Failed to serialize cache");
            std::fs::write(&cache_file, updated).expect("Failed to write cache");
            println!("Saved updated cache to {:?}", cache_file);
        }
        Err(e) => {
            println!("ERROR: Failed to fetch LEIA package: {}", e);
        }
    }
}
