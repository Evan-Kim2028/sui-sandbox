use sui_move_interface_extractor::grpc::GrpcClient;

/// Debug package BCS structure in detail
#[tokio::test]
async fn test_debug_package_structure() {
    let client = GrpcClient::archive().await.expect("Failed to connect");

    let upgraded_clmm = "0x75b2e9ecad34944b8d0c874e568c90db0cf9437f0d7392abfd4cb902972f3e40";

    let obj = client
        .get_object(upgraded_clmm)
        .await
        .expect("Failed to fetch")
        .expect("Package not found");

    let bcs = obj.bcs.expect("No BCS data");

    // Package BCS format: 0x01 || address (32) || version (8) || module_map
    const HEADER_SIZE: usize = 1 + 32 + 8;

    println!("Total BCS: {} bytes", bcs.len());
    println!("Header: {} bytes", HEADER_SIZE);

    let module_data = &bcs[HEADER_SIZE..];
    println!("Module section: {} bytes", module_data.len());

    // Manually parse the module map to understand its exact size
    let mut offset = 0;

    // Read count
    let (count, count_bytes) = read_uleb128(&module_data[offset..]).unwrap();
    offset += count_bytes;
    println!(
        "\nModule count: {} ({} bytes for ULEB128)",
        count, count_bytes
    );

    let mut total_module_bytes = count_bytes;

    for i in 0..count {
        let entry_start = offset;

        // Read string length
        let (str_len, str_bytes) =
            read_uleb128(&module_data[offset..]).expect("Failed to read string length");
        offset += str_bytes;

        // Read string
        let name = String::from_utf8_lossy(&module_data[offset..offset + str_len]);
        offset += str_len;

        // Read bytecode length
        let (bytecode_len, bytecode_bytes) =
            read_uleb128(&module_data[offset..]).expect("Failed to read bytecode length");
        offset += bytecode_bytes;

        // Skip bytecode
        offset += bytecode_len;

        let entry_size = offset - entry_start;
        total_module_bytes += entry_size;

        if i < 5 || i >= count - 2 {
            println!(
                "  Module {}: '{}' ({} bytes name, {} bytes bytecode, {} total entry)",
                i, name, str_len, bytecode_len, entry_size
            );
        } else if i == 5 {
            println!("  ...");
        }
    }

    println!("\nTotal module map size: {} bytes", total_module_bytes);
    println!("Module section size: {} bytes", module_data.len());
    println!(
        "Remaining after modules: {} bytes",
        module_data.len() - total_module_bytes
    );

    if module_data.len() > total_module_bytes {
        let remaining = &module_data[total_module_bytes..];
        println!(
            "\nRemaining data (first 64 bytes): {}",
            hex::encode(&remaining[..64.min(remaining.len())])
        );

        // The remaining data might be:
        // - type_origin_table: Vec<TypeOrigin>
        // - linkage_table: BTreeMap<ObjectID, UpgradeInfo>
        // See sui-types/src/move_package.rs

        // Try to understand the structure
        if let Some((type_origin_count, bytes)) = read_uleb128(remaining) {
            println!(
                "Possible type_origin_table count: {} ({} bytes)",
                type_origin_count, bytes
            );
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
