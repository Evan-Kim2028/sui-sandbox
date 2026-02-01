use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);
static PROCESS_NONCE: OnceLock<[u8; 16]> = OnceLock::new();

fn nanos_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn process_nonce() -> [u8; 16] {
    let nanos = nanos_now();
    let pid = std::process::id() as u128;
    let seed = nanos ^ (pid << 64);
    seed.to_le_bytes()
}

pub(crate) fn generate_tx_hash() -> [u8; 32] {
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nonce = PROCESS_NONCE.get_or_init(process_nonce);
    let nanos = nanos_now();
    let mut tx_hash = [0u8; 32];
    tx_hash[0..16].copy_from_slice(nonce);
    tx_hash[16..24].copy_from_slice(&counter.to_le_bytes());
    tx_hash[24..32].copy_from_slice(&(nanos as u64).to_le_bytes());
    tx_hash
}
