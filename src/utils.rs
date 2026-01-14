use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use serde_json::Value;
use std::fs::{self, File};
use std::future::Future;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::{GitHeadMetadata, GitMetadata};
use sha2::Digest;

// =============================================================================
// Address Utilities
// =============================================================================
// Canonical address formatting functions. Use these instead of module-specific
// implementations for consistent address representation across the codebase.

/// Parse an address string (short or long form) into an AccountAddress.
///
/// Accepts formats: "0x2", "0x0000...0002", "2"
pub fn parse_address(addr: &str) -> Result<AccountAddress> {
    let s = addr.trim();
    let hex_str = s.strip_prefix("0x").unwrap_or(s);

    if hex_str.is_empty() {
        return Err(anyhow!("empty address"));
    }

    if !hex_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid hex address: {}", addr));
    }

    // Pad to 64 hex chars (32 bytes)
    let padded = format!("{:0>64}", hex_str);
    if padded.len() > 64 {
        return Err(anyhow!("address too long: {}", addr));
    }

    AccountAddress::from_hex_literal(&format!("0x{}", padded))
        .map_err(|e| anyhow!("invalid address '{}': {:?}", addr, e))
}

/// Format an address to short form (0x2 instead of 0x0000...0002).
///
/// This is the preferred format for display to users and in API responses.
pub fn format_address_short(addr: &AccountAddress) -> String {
    let hex = addr.to_hex_literal();
    // Strip leading zeros after 0x prefix
    if hex.starts_with("0x") {
        let without_prefix = &hex[2..];
        let trimmed = without_prefix.trim_start_matches('0');
        if trimmed.is_empty() {
            "0x0".to_string()
        } else {
            format!("0x{}", trimmed)
        }
    } else {
        hex
    }
}

/// Format an address to full 64-character form (0x0000...0002).
///
/// This is the canonical form for storage and comparison.
pub fn format_address_full(addr: &AccountAddress) -> String {
    format!("0x{}", hex::encode(addr.as_ref()))
}

/// Check if an address is a framework address (0x1, 0x2, 0x3).
pub fn is_framework_address(addr: &AccountAddress) -> bool {
    let bytes = addr.as_ref();
    // Check if all bytes except the last are zero
    bytes[..31].iter().all(|&b| b == 0) && bytes[31] <= 3 && bytes[31] >= 1
}

#[derive(Debug, Clone, Copy)]
pub struct BytesInfo {
    pub len: usize,
    pub sha256: [u8; 32],
}

pub fn sha256_32(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..]);
    out
}

pub fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn bytes_info(bytes: &[u8]) -> BytesInfo {
    BytesInfo {
        len: bytes.len(),
        sha256: sha256_32(bytes),
    }
}

pub fn bytes_info_sha256_hex(info: BytesInfo) -> String {
    bytes_to_hex(&info.sha256)
}

pub fn should_retry_error(error: &anyhow::Error) -> bool {
    let s = format!("{:#}", error);
    s.contains("Request rejected `429`")
        || s.contains("429")
        || s.to_ascii_lowercase().contains("too many")
        || s.to_ascii_lowercase().contains("timed out")
        || s.to_ascii_lowercase().contains("timeout")
        || s.to_ascii_lowercase().contains("connection")
        || s.to_ascii_lowercase().contains("transport")
}

pub async fn with_retries<T, F, Fut>(
    retries: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
    mut f: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0usize;
    let mut backoff = initial_backoff;

    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= retries || !should_retry_error(&e) {
                    return Err(e);
                }
                attempt += 1;
                tokio::time::sleep(backoff).await;
                backoff = std::cmp::min(backoff * 2, max_backoff);
            }
        }
    }
}

pub fn canonicalize_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let old_map = std::mem::take(map);
            let mut entries: Vec<(String, Value)> = old_map.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));

            for (_, v) in entries.iter_mut() {
                canonicalize_json_value(v);
            }

            for (k, v) in entries {
                map.insert(k, v);
            }
        }
        Value::Array(values) => {
            for v in values.iter_mut() {
                canonicalize_json_value(v);
            }
        }
        _ => {}
    }
}

pub fn bytes_to_hex_prefixed(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

pub fn git_metadata_for_path(path: &Path) -> Option<GitMetadata> {
    let root = find_git_root(path)?;
    let head = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(o) } else { None })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())?;

    let head_commit_time = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["log", "-1", "--format=%cI"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(o) } else { None })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    Some(GitMetadata {
        git_root: root.display().to_string(),
        head,
        head_commit_time,
    })
}

pub fn git_head_metadata_for_path(path: &Path) -> Option<GitHeadMetadata> {
    let meta = git_metadata_for_path(path)?;
    Some(GitHeadMetadata {
        head: meta.head,
        head_commit_time: meta.head_commit_time,
    })
}

pub fn fnv1a64(seed: u64, s: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64 ^ seed;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn write_canonical_json(path: &Path, value: &Value) -> Result<()> {
    if path.as_os_str() == "-" {
        let stdout = io::stdout();
        let mut writer = BufWriter::new(stdout.lock());
        if let Err(e) = serde_json::to_writer_pretty(&mut writer, value) {
            if e.is_io() && e.io_error_kind() == Some(io::ErrorKind::BrokenPipe) {
                return Ok(());
            }
            return Err(e).context("serialize JSON");
        }
        writer.write_all(b"\n").ok();
        return Ok(());
    }

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value).context("serialize JSON")?;
    writer.write_all(b"\n").ok();
    Ok(())
}

pub fn check_stability(interface_value: &Value) -> Result<()> {
    let s1 = serde_json::to_string_pretty(interface_value).context("serialize JSON")?;
    let mut v2: Value = serde_json::from_str(&s1).context("parse JSON")?;
    canonicalize_json_value(&mut v2);
    let s2 = serde_json::to_string_pretty(&v2).context("serialize JSON")?;
    if s1 != s2 {
        return Err(anyhow::anyhow!(
            "canonical JSON is not stable under roundtrip"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_to_hex() {
        assert_eq!(bytes_to_hex(&[0x00, 0xff, 0x12]), "00ff12");
        assert_eq!(bytes_to_hex(&[]), "");
    }

    #[test]
    fn test_bytes_to_hex_prefixed() {
        assert_eq!(bytes_to_hex_prefixed(&[0x00, 0xff]), "0x00ff");
        assert_eq!(bytes_to_hex_prefixed(&[]), "0x");
    }

    #[test]
    fn test_fnv1a64() {
        // Known test vectors for FNV-1a 64-bit
        assert_eq!(fnv1a64(0, "foobar"), 0x85944171f73967e8);
        assert_eq!(fnv1a64(0, ""), 0xcbf29ce484222325);
    }

    #[test]
    fn test_canonicalize_json_value() {
        let mut v = serde_json::json!({
            "b": 2,
            "a": 1,
            "c": [
                {"y": 2, "x": 1},
                3
            ]
        });
        canonicalize_json_value(&mut v);

        let s = serde_json::to_string(&v).unwrap();
        // Maps should be sorted by key.
        // "a":1 comes before "b":2
        // Inside list, objects also sorted: "x":1 before "y":2
        assert_eq!(s, r#"{"a":1,"b":2,"c":[{"x":1,"y":2},3]}"#);
    }

    #[test]
    fn test_parse_address() {
        // Short form
        let addr = parse_address("0x2").unwrap();
        assert_eq!(format_address_short(&addr), "0x2");

        // Without prefix
        let addr = parse_address("2").unwrap();
        assert_eq!(format_address_short(&addr), "0x2");

        // Full form
        let full = "0x0000000000000000000000000000000000000000000000000000000000000002";
        let addr = parse_address(full).unwrap();
        assert_eq!(format_address_short(&addr), "0x2");
        assert_eq!(format_address_full(&addr), full);

        // Error cases
        assert!(parse_address("").is_err());
        assert!(parse_address("0x").is_err());
        assert!(parse_address("0xgg").is_err());
    }

    #[test]
    fn test_format_address_short() {
        let addr = AccountAddress::from_hex_literal(
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        ).unwrap();
        assert_eq!(format_address_short(&addr), "0x2");

        let addr = AccountAddress::from_hex_literal(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        ).unwrap();
        assert_eq!(format_address_short(&addr), "0x0");
    }

    #[test]
    fn test_is_framework_address() {
        let addr1 = parse_address("0x1").unwrap();
        let addr2 = parse_address("0x2").unwrap();
        let addr3 = parse_address("0x3").unwrap();
        let addr4 = parse_address("0x4").unwrap();
        let user = parse_address("0xabc123").unwrap();

        assert!(is_framework_address(&addr1));
        assert!(is_framework_address(&addr2));
        assert!(is_framework_address(&addr3));
        assert!(!is_framework_address(&addr4));
        assert!(!is_framework_address(&user));
    }
}
