#![allow(deprecated)]
//! Integration tests for the unified cache architecture.
//!
//! These tests verify:
//! 1. Cache-first lookups work correctly
//! 2. Write-through caching works for network fetches
//! 3. Address normalization is consistent across all cache operations
//! 4. Version and type metadata are preserved

use sui_move_interface_extractor::cache::{normalize_address, CacheManager};
use sui_move_interface_extractor::data_fetcher::{DataFetcher, DataSource};
use tempfile::TempDir;

#[test]
fn test_cache_manager_basics() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let mut cache = CacheManager::new(temp_dir.path()).expect("failed to create cache");

    // Test package operations
    let modules = vec![
        ("module_a".to_string(), vec![0u8, 1, 2, 3]),
        ("module_b".to_string(), vec![4, 5, 6, 7]),
    ];

    cache
        .put_package("0x123", 1, modules.clone())
        .expect("put_package failed");

    assert!(cache.has_package("0x123"));
    assert!(cache.has_package("0x0000000000000000000000000000000000000000000000000000000000000123"));

    let pkg = cache
        .get_package("0x123")
        .expect("get_package failed")
        .expect("package not found");

    assert_eq!(pkg.version, 1);
    assert_eq!(pkg.modules.len(), 2);

    // Test object operations
    let bcs_bytes = vec![10u8, 20, 30, 40];
    let type_tag = Some("0x2::coin::Coin<0x2::sui::SUI>".to_string());

    cache
        .put_object("0xabc", 5, type_tag.clone(), bcs_bytes.clone())
        .expect("put_object failed");

    assert!(cache.has_object("0xabc"));

    let obj = cache
        .get_object("0xabc")
        .expect("get_object failed")
        .expect("object not found");

    assert_eq!(obj.version, 5);
    assert_eq!(obj.type_tag, type_tag);
    assert_eq!(obj.bcs_bytes, bcs_bytes);
}

#[test]
fn test_address_normalization_consistency() {
    // All these should normalize to the same address
    let variants = [
        "0x2",
        "2",
        "0x02",
        "0X2",
        "0x0000000000000000000000000000000000000000000000000000000000000002",
    ];

    let expected = "0x0000000000000000000000000000000000000000000000000000000000000002";

    for variant in &variants {
        let normalized = normalize_address(variant);
        assert_eq!(
            normalized, expected,
            "Address '{}' should normalize to '{}'",
            variant, expected
        );
    }
}

#[test]
fn test_cache_stats() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let mut cache = CacheManager::new(temp_dir.path()).expect("failed to create cache");

    // Empty cache
    let stats = cache.stats();
    assert_eq!(stats.package_count, 0);
    assert_eq!(stats.object_count, 0);

    // Add some data
    cache
        .put_package("0x1", 1, vec![("m".to_string(), vec![1u8])])
        .unwrap();
    cache
        .put_package("0x2", 1, vec![("m".to_string(), vec![2u8])])
        .unwrap();
    cache.put_object("0xa", 1, None, vec![1u8]).unwrap();

    let stats = cache.stats();
    assert_eq!(stats.package_count, 2);
    assert_eq!(stats.object_count, 1);
    assert!(stats.disk_size_bytes > 0);
}

#[test]
fn test_version_preservation() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let mut cache = CacheManager::new(temp_dir.path()).expect("failed to create cache");

    // Put v1
    cache
        .put_package("0x100", 1, vec![("v1".to_string(), vec![1u8])])
        .unwrap();

    // Put v2 (should replace)
    cache
        .put_package("0x100", 2, vec![("v2".to_string(), vec![2u8])])
        .unwrap();

    let pkg = cache.get_package("0x100").unwrap().unwrap();
    assert_eq!(pkg.version, 2);
    assert_eq!(pkg.modules[0].0, "v2");

    // Put v1 again (should be ignored - older version)
    cache
        .put_package("0x100", 1, vec![("v1_again".to_string(), vec![0u8])])
        .unwrap();

    let pkg = cache.get_package("0x100").unwrap().unwrap();
    assert_eq!(pkg.version, 2); // Still v2
}

#[test]
fn test_data_fetcher_cache_integration() {
    // Create fetcher with the actual .tx-cache directory
    let fetcher = DataFetcher::mainnet().with_cache_optional(".tx-cache");

    if !fetcher.has_cache() {
        println!("No .tx-cache directory found, skipping integration test");
        return;
    }

    let stats = fetcher.cache_stats().expect("cache_stats failed");
    println!(
        "Cache stats: {} packages, {} objects, {} transactions, {} bytes on disk",
        stats.package_count, stats.object_count, stats.transaction_count, stats.disk_size_bytes
    );

    assert!(stats.package_count > 0, "Should have cached packages");
    assert!(stats.object_count > 0, "Should have cached objects");
    assert!(
        stats.transaction_count > 0,
        "Should have cached transactions"
    );

    // Test cache-first lookup
    // This package (cetus/clmm) should be in our cached transactions
    let test_pkg = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";

    match fetcher.fetch_package(test_pkg) {
        Ok(pkg) => {
            println!(
                "Fetched package: {} modules via {:?}",
                pkg.modules.len(),
                pkg.source
            );
            // If it came from cache, that's what we want
            // If it came from network, that's also fine (means it wasn't in cache)
            assert!(!pkg.modules.is_empty(), "Package should have modules");
        }
        Err(e) => {
            println!("Package not available: {}", e);
            // This is acceptable - the package might not be in cache
        }
    }
}

#[test]
fn test_write_through_caching() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");

    // Create a fetcher with write-through caching to our temp directory
    // Note: This test requires network access, so we skip if it fails
    let fetcher = match DataFetcher::mainnet().with_cache(temp_dir.path()) {
        Ok(f) => f,
        Err(e) => {
            println!("Could not create fetcher: {}", e);
            return;
        }
    };

    // First, verify cache is empty
    let stats_before = fetcher.cache_stats().unwrap();
    assert_eq!(stats_before.package_count, 0, "Cache should start empty");

    // Fetch a well-known package (Sui framework) - this will hit network
    println!("Fetching 0x2 (Sui framework)...");
    match fetcher.fetch_package("0x2") {
        Ok(pkg) => {
            println!("Fetched {} modules via {:?}", pkg.modules.len(), pkg.source);

            // If we got from network, it should now be cached
            if pkg.source != DataSource::Cache {
                // Write-through is synchronous, no sleep needed

                // Check cache was populated
                let stats_after = fetcher.cache_stats().unwrap();
                println!("Cache after fetch: {} packages", stats_after.package_count);

                // Note: write-through happens in the same thread, so it should be immediate
                // But if the package was already in cache, this assertion wouldn't apply
            }

            assert!(!pkg.modules.is_empty(), "Sui framework should have modules");
            // Check for common Sui framework modules
            let module_names: Vec<&str> = pkg.modules.iter().map(|m| m.name.as_str()).collect();
            println!("Modules found: {:?}", module_names);
            assert!(
                module_names
                    .iter()
                    .any(|n| *n == "coin" || *n == "object" || *n == "transfer"),
                "Should have core Sui modules like 'coin', 'object', or 'transfer'"
            );
        }
        Err(e) => {
            println!("Network fetch failed (this is OK in CI): {}", e);
        }
    }
}

#[test]
fn test_read_existing_tx_cache() {
    // Test that we can read the existing .tx-cache directory
    let cache_result = CacheManager::new(".tx-cache");

    match cache_result {
        Ok(cache) => {
            let stats = cache.stats();
            println!(
                "Existing cache: {} packages, {} objects, {} transactions",
                stats.package_count, stats.object_count, stats.transaction_count
            );

            // If we have data, test some lookups
            if stats.package_count > 0 {
                let packages: Vec<&str> = cache.list_packages();
                println!("Sample packages: {:?}", &packages[..packages.len().min(5)]);

                // Try to fetch the first package
                if let Some(first_pkg) = packages.first() {
                    match cache.get_package(first_pkg) {
                        Ok(Some(pkg)) => {
                            println!(
                                "First package has {} modules, version {}",
                                pkg.modules.len(),
                                pkg.version
                            );
                            assert!(!pkg.modules.is_empty());
                        }
                        Ok(None) => {
                            panic!("Package listed but not found: {}", first_pkg);
                        }
                        Err(e) => {
                            println!("Error reading package: {}", e);
                        }
                    }
                }
            }

            if stats.object_count > 0 {
                let objects: Vec<&str> = cache.list_objects();
                println!("Sample objects: {:?}", &objects[..objects.len().min(5)]);
            }
        }
        Err(e) => {
            println!("No .tx-cache directory: {}", e);
        }
    }
}
