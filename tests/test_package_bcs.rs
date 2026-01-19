use sui_move_interface_extractor::grpc::GrpcClient;

/// Test fetching package BCS to understand its structure
#[tokio::test]
async fn test_package_bcs_structure() {
    let client = GrpcClient::archive().await.expect("Failed to connect");
    println!("Connected to: {}", client.endpoint());

    // Upgraded CLMM package that failed to deserialize
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";

    println!("\n=== Fetching CLMM Package ===");
    let obj = client
        .get_object(upgraded_clmm)
        .await
        .expect("Failed to fetch");

    match obj {
        Some(o) => {
            println!("Object ID: {}", o.object_id);
            println!("Version: {}", o.version);
            println!("Type: {:?}", o.type_string);
            println!("Owner: {:?}", o.owner);

            if let Some(ref bcs) = o.bcs {
                println!("\nBCS data: {} bytes", bcs.len());
                println!(
                    "First 64 bytes (hex): {}",
                    hex::encode(&bcs[..64.min(bcs.len())])
                );
                println!("First 64 bytes (raw): {:?}", &bcs[..64.min(bcs.len())]);

                // Try to understand the structure
                // Packages should have a different BCS format than Move objects

                // First byte might be an enum discriminant
                if !bcs.is_empty() {
                    println!("\nFirst byte: 0x{:02x} ({})", bcs[0], bcs[0]);
                }

                // Try parsing as BTreeMap<String, Vec<u8>>
                match bcs::from_bytes::<std::collections::BTreeMap<String, Vec<u8>>>(bcs) {
                    Ok(map) => {
                        println!("\n✓ Parses as BTreeMap<String, Vec<u8>>");
                        println!("  {} modules:", map.len());
                        for (name, bytes) in &map {
                            println!("    - {}: {} bytes", name, bytes.len());
                        }
                    }
                    Err(e) => {
                        println!("\n✗ Failed to parse as BTreeMap<String, Vec<u8>>: {}", e);
                    }
                }
            } else {
                println!("\nNo BCS data!");
            }

            if let Some(ref bcs_full) = o.bcs_full {
                println!("\nFull BCS data: {} bytes", bcs_full.len());
                println!(
                    "First 64 bytes (hex): {}",
                    hex::encode(&bcs_full[..64.min(bcs_full.len())])
                );

                // Try parsing bcs_full instead
                match bcs::from_bytes::<std::collections::BTreeMap<String, Vec<u8>>>(bcs_full) {
                    Ok(map) => {
                        println!("\n✓ bcs_full parses as BTreeMap<String, Vec<u8>>");
                        println!("  {} modules:", map.len());
                        for (name, bytes) in &map {
                            println!("    - {}: {} bytes", name, bytes.len());
                        }
                    }
                    Err(e) => {
                        println!(
                            "\n✗ bcs_full failed to parse as BTreeMap<String, Vec<u8>>: {}",
                            e
                        );
                    }
                }
            }
        }
        None => {
            println!("Package not found!");
        }
    }

    // Also test a simpler package to compare
    println!("\n\n=== Fetching skip_list Package (simpler) ===");
    let skip_list_pkg = "0xbe21a06129308e0495431d12286127897aff07a8ade3970495a4404d97f9eaaa";

    let obj2 = client
        .get_object(skip_list_pkg)
        .await
        .expect("Failed to fetch");

    match obj2 {
        Some(o) => {
            println!("Object ID: {}", o.object_id);
            println!("Type: {:?}", o.type_string);

            if let Some(ref bcs) = o.bcs {
                println!("\nBCS data: {} bytes", bcs.len());
                println!(
                    "First 64 bytes (hex): {}",
                    hex::encode(&bcs[..64.min(bcs.len())])
                );

                match bcs::from_bytes::<std::collections::BTreeMap<String, Vec<u8>>>(bcs) {
                    Ok(map) => {
                        println!("\n✓ Parses as BTreeMap<String, Vec<u8>>");
                        println!("  {} modules:", map.len());
                        for (name, bytes) in &map {
                            println!("    - {}: {} bytes", name, bytes.len());
                        }
                    }
                    Err(e) => {
                        println!("\n✗ Failed to parse as BTreeMap<String, Vec<u8>>: {}", e);
                    }
                }
            }
        }
        None => {
            println!("Package not found!");
        }
    }
}

/// Test with different field masks for packages
#[tokio::test]
async fn test_package_field_masks() {
    let client = GrpcClient::archive().await.expect("Failed to connect");

    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";

    // The default get_object uses specific field masks
    // Let's see what raw gRPC returns
    println!("Testing raw gRPC fetch for package...");

    use sui_move_interface_extractor::grpc::generated::sui_rpc_v2::{
        self as proto, ledger_service_client::LedgerServiceClient,
    };

    // We need to access the channel directly - let's just print what we have
    let obj = client
        .get_object(upgraded_clmm)
        .await
        .expect("Failed to fetch");

    if let Some(o) = obj {
        println!("bcs field present: {}", o.bcs.is_some());
        println!("bcs_full field present: {}", o.bcs_full.is_some());

        if let Some(ref bcs) = o.bcs {
            println!("bcs length: {}", bcs.len());
        }
        if let Some(ref bcs_full) = o.bcs_full {
            println!("bcs_full length: {}", bcs_full.len());
        }
    }
}
