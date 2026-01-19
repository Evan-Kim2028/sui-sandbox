use sui_move_interface_extractor::grpc::GrpcClient;

#[tokio::test]
async fn test_grpc_historical_object() {
    let client = GrpcClient::archive()
        .await
        .expect("Failed to connect to archive");

    // Get CURRENT version of the read-only child object
    let current = client
        .get_object_at_version(
            "0x71e6517795bc3ba53416a0c3d89a4bfe8c239a7608e23af049f113c0f97abb0e",
            None, // Latest version
        )
        .await
        .expect("Failed to fetch current object");

    match current {
        Some(c) => {
            println!("Current version of child object:");
            println!("  ID: {}", c.object_id);
            println!("  Version: {}", c.version);
            println!("  Type: {:?}", c.type_string);

            // Now try to fetch at this version explicitly
            let at_version = client
                .get_object_at_version(
                    "0x71e6517795bc3ba53416a0c3d89a4bfe8c239a7608e23af049f113c0f97abb0e",
                    Some(c.version),
                )
                .await
                .expect("Failed to fetch at version");

            println!("\nFetched at version {}:", c.version);
            if let Some(v) = at_version {
                println!("  Found! BCS len: {:?}", v.bcs.as_ref().map(|b| b.len()));
            } else {
                println!("  Not found even at current version!");
            }

            // Try a few versions back to see what's available
            println!("\nChecking version availability:");
            for delta in [0, 1, 10, 100, 1000] {
                let test_version = c.version - delta;
                let result = client
                    .get_object_at_version(
                        "0x71e6517795bc3ba53416a0c3d89a4bfe8c239a7608e23af049f113c0f97abb0e",
                        Some(test_version),
                    )
                    .await;

                match result {
                    Ok(Some(_)) => println!("  v{}: Found", test_version),
                    Ok(None) => println!("  v{}: Not found (None)", test_version),
                    Err(_) => println!("  v{}: Error/Not found", test_version),
                }
            }
        }
        None => println!("Child object doesn't exist at all!"),
    }
}
