//! Debug the deserialization of Partner object

use base64::Engine;

#[test]
fn test_debug_partner_bcs() {
    // Partner object bytes from cache
    let b64 = "Y5teQz2jFznoAM0IXzVuZMriIpZtDxsRvZ3HazIv9YsSYWdncmVnYXRvci1kZWZhdWx0AQAAAAAAAAA4jbVmAAAAADhXUKIAAAAAQx3VYmU1ZmRqDVCuzXMsrCBHedb+CJIBT/aQY8fPMciFAAAAAAAAAA==";
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();

    println!("Total bytes: {}", bytes.len());
    println!("\nHex dump:");
    for (i, chunk) in bytes.chunks(32).enumerate() {
        print!("  {:04x}: ", i * 32);
        for b in chunk {
            print!("{:02x}", b);
        }
        println!();
    }

    println!("\n=== BCS Structure Analysis ===");

    // First 32 bytes should be the UID.id.bytes (object ID)
    let obj_id = &bytes[0..32];
    println!("Object ID (UID.id.bytes): 0x{}", hex::encode(obj_id));

    // After the 32-byte ID, there's a string (partner name)
    let name_len = bytes[32] as usize;
    let name_start = 33;
    let name_end = name_start + name_len;
    let name = String::from_utf8_lossy(&bytes[name_start..name_end]);
    println!("Name length: {}", name_len);
    println!("Name: {}", name);

    // After name, there are more fields
    let remaining = &bytes[name_end..];
    println!("\nRemaining bytes after name ({}):", remaining.len());
    for (i, chunk) in remaining.chunks(8).enumerate() {
        print!("  {:04x}: ", name_end + i * 8);
        for b in chunk {
            print!("{:02x} ", b);
        }
        // Try to interpret as u64
        if chunk.len() == 8 {
            let val = u64::from_le_bytes(chunk.try_into().unwrap());
            print!(" = {}", val);
        }
        println!();
    }

    println!("\n=== Expected Sui Partner struct ===");
    println!("struct Partner {{");
    println!("    id: UID,        // 32 bytes (UID wraps ID which wraps address)");
    println!("    name: String,   // ULEB128 length + bytes");
    println!("    ref_fee_rate: u64,");
    println!("    start_time: u64,");
    println!("    end_time: u64,");
    println!("    receiver: address, // 32 bytes");
    println!("    balances: Table,   // complex type");
    println!("}}");
}

#[test]
fn test_debug_global_config_bcs() {
    // GlobalConfig object bytes from cache
    let cache_file = ".tx-cache/7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp.json";
    let data = std::fs::read_to_string(cache_file).expect("read cache");
    let cache: serde_json::Value = serde_json::from_str(&data).expect("parse");

    let global_config_b64 = cache["objects"]
        ["0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f"]
        .as_str()
        .expect("no global config");

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(global_config_b64)
        .unwrap();

    println!("GlobalConfig total bytes: {}", bytes.len());
    println!("\nHex dump:");
    for (i, chunk) in bytes.chunks(32).enumerate() {
        print!("  {:04x}: ", i * 32);
        for b in chunk {
            print!("{:02x}", b);
        }
        println!();
    }

    // First 32 bytes = UID
    let obj_id = &bytes[0..32];
    println!("\nObject ID: 0x{}", hex::encode(obj_id));
}

#[test]
fn test_debug_pool_bcs() {
    // Pool object bytes from cache
    let cache_file = ".tx-cache/7aQ29xk764ELpHjxxTyMUcHdvyoNzUcnBdwT7emhPNrp.json";
    let data = std::fs::read_to_string(cache_file).expect("read cache");
    let cache: serde_json::Value = serde_json::from_str(&data).expect("parse");

    let pool_b64 = cache["objects"]
        ["0x8b7a1b6e8f853a1f0f99099731de7d7d17e90e445e28935f212b67268f8fe772"]
        .as_str()
        .expect("no pool");

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(pool_b64)
        .unwrap();

    println!("Pool total bytes: {}", bytes.len());
    println!("\nFirst 64 bytes hex dump:");
    for (i, chunk) in bytes.chunks(32).take(2).enumerate() {
        print!("  {:04x}: ", i * 32);
        for b in chunk {
            print!("{:02x}", b);
        }
        println!();
    }

    // First 32 bytes = UID
    let obj_id = &bytes[0..32];
    println!("\nPool Object ID: 0x{}", hex::encode(obj_id));
}
