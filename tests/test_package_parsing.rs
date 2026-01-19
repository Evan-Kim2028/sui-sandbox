use sui_move_interface_extractor::benchmark::tx_replay::TransactionFetcher;
use sui_move_interface_extractor::grpc::GrpcClient;

/// Test fetching and parsing package modules
#[tokio::test]
async fn test_fetch_package_modules() {
    // Use TransactionFetcher which has the parse_package_modules logic
    let fetcher = TransactionFetcher::mainnet_with_archive();

    // Test the upgraded CLMM package
    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";

    println!("\n=== Fetching CLMM Package via TransactionFetcher ===");
    match fetcher.fetch_package_modules(upgraded_clmm) {
        Ok(modules) => {
            println!("✓ Successfully parsed {} modules:", modules.len());
            for (name, bytes) in &modules {
                println!("  - {}: {} bytes", name, bytes.len());
            }
        }
        Err(e) => {
            println!("✗ Failed to fetch/parse: {}", e);
        }
    }

    // Test a simpler package
    let skip_list_pkg = "0xbe21a06129308e0495431d12286127897aff07a8ade3970495a4404d97f9eaaa";

    println!("\n=== Fetching skip_list Package ===");
    match fetcher.fetch_package_modules(skip_list_pkg) {
        Ok(modules) => {
            println!("✓ Successfully parsed {} modules:", modules.len());
            for (name, bytes) in &modules {
                println!("  - {}: {} bytes", name, bytes.len());
            }
        }
        Err(e) => {
            println!("✗ Failed to fetch/parse: {}", e);
        }
    }

    // Test the original CLMM package
    let original_clmm = "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";

    println!("\n=== Fetching Original CLMM Package ===");
    match fetcher.fetch_package_modules(original_clmm) {
        Ok(modules) => {
            println!("✓ Successfully parsed {} modules:", modules.len());
            for (name, bytes) in &modules {
                println!("  - {}: {} bytes", name, bytes.len());
            }
        }
        Err(e) => {
            println!("✗ Failed to fetch/parse: {}", e);
        }
    }
}

/// Test raw package BCS parsing
#[tokio::test]
async fn test_raw_package_bcs_parsing() {
    let client = GrpcClient::archive().await.expect("Failed to connect");

    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";

    let obj = client
        .get_object(upgraded_clmm)
        .await
        .expect("Failed to fetch")
        .expect("Package not found");

    println!("Package type: {:?}", obj.type_string);
    println!("BCS length: {:?}", obj.bcs.as_ref().map(|b| b.len()));

    if let Some(bcs) = &obj.bcs {
        // Package BCS format: 0x01 || address (32) || version (8) || module_map
        const HEADER_SIZE: usize = 1 + 32 + 8;

        if bcs.len() > HEADER_SIZE {
            println!("\nHeader:");
            println!("  Variant: 0x{:02x}", bcs[0]);
            println!("  Address: 0x{}", hex::encode(&bcs[1..33]));
            println!(
                "  Version: {}",
                u64::from_le_bytes(bcs[33..41].try_into().unwrap())
            );

            // Try parsing the module map
            let module_data = &bcs[HEADER_SIZE..];
            println!("\nModule data: {} bytes", module_data.len());
            println!(
                "First 32 bytes: {}",
                hex::encode(&module_data[..32.min(module_data.len())])
            );

            match bcs::from_bytes::<std::collections::BTreeMap<String, Vec<u8>>>(module_data) {
                Ok(map) => {
                    println!("\n✓ Module map parsed successfully!");
                    println!("  {} modules:", map.len());
                    for (name, bytes) in &map {
                        println!("    - {}: {} bytes", name, bytes.len());
                    }
                }
                Err(e) => {
                    println!("\n✗ Failed to parse module map: {}", e);

                    // Debug: try to understand the format
                    // BTreeMap BCS starts with ULEB128 count
                    if let Some((count, bytes_read)) = read_uleb128(module_data) {
                        println!("  ULEB128 count: {} ({} bytes)", count, bytes_read);

                        // Then each entry is: string_len + string + vec_len + vec
                        let mut offset = bytes_read;
                        for i in 0..count.min(3) {
                            if offset >= module_data.len() {
                                break;
                            }
                            if let Some((str_len, str_bytes)) = read_uleb128(&module_data[offset..])
                            {
                                offset += str_bytes;
                                if offset + str_len <= module_data.len() {
                                    let name = String::from_utf8_lossy(
                                        &module_data[offset..offset + str_len],
                                    );
                                    println!("  Entry {}: name='{}' ({} bytes)", i, name, str_len);
                                    offset += str_len;

                                    if let Some((vec_len, vec_bytes)) =
                                        read_uleb128(&module_data[offset..])
                                    {
                                        println!("           bytecode={} bytes", vec_len);
                                        offset += vec_bytes + vec_len;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn read_uleb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut result: usize = 0;
    let mut shift = 0;
    let mut bytes_read = 0;

    for &byte in data.iter().take(5) {
        bytes_read += 1;
        result |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Some((result, bytes_read));
        }
        shift += 7;
    }
    None
}
