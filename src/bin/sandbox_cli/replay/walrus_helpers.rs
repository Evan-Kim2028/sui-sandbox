use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;

use sui_transport::graphql::GraphQLClient;

type ObjectBcsCache = parking_lot::Mutex<HashMap<String, (String, Vec<u8>, u64)>>;

/// Parse a checkpoint specification string into a list of checkpoint numbers.
///
/// Supported formats:
///   - Single: "239615926"
///   - Range:  "239615920..239615926" (inclusive)
///   - List:   "239615920,239615923,239615926"
pub(super) fn parse_checkpoint_spec(spec: &str) -> Result<Vec<u64>> {
    if spec.contains("..") {
        let parts: Vec<&str> = spec.split("..").collect();
        if parts.len() != 2 {
            return Err(anyhow!(
                "Invalid range format '{}', expected START..END",
                spec
            ));
        }
        let start: u64 = parts[0]
            .trim()
            .parse()
            .with_context(|| format!("Invalid range start '{}'", parts[0]))?;
        let end: u64 = parts[1]
            .trim()
            .parse()
            .with_context(|| format!("Invalid range end '{}'", parts[1]))?;
        if end < start {
            return Err(anyhow!("Range end {} < start {}", end, start));
        }
        if end - start > 100 {
            return Err(anyhow!(
                "Checkpoint range too large ({} checkpoints, max 100)",
                end - start + 1
            ));
        }
        Ok((start..=end).collect())
    } else if spec.contains(',') {
        spec.split(',')
            .map(|s| {
                s.trim()
                    .parse::<u64>()
                    .with_context(|| format!("Invalid checkpoint number '{}'", s.trim()))
            })
            .collect()
    } else {
        let cp: u64 = spec
            .trim()
            .parse()
            .with_context(|| format!("Invalid checkpoint number '{}'", spec))?;
        Ok(vec![cp])
    }
}

/// Resolve the checkpoint number via GraphQL, fetch that checkpoint from Walrus,
/// and extract the object. This avoids scanning and works even for pruned nodes
/// because it only uses GraphQL for lightweight tx->checkpoint index lookups and
/// Walrus for the actual archival data.
pub(super) fn fetch_via_prev_tx(
    gql: &GraphQLClient,
    cache: &ObjectBcsCache,
    id_hex: &str,
    prev_tx_digest: &str,
) -> Option<(String, Vec<u8>, u64)> {
    // Step 1: Get the checkpoint number from the transaction digest.
    let tx_meta = gql.fetch_transaction_meta(prev_tx_digest).ok()?;
    let cp = tx_meta.checkpoint?;

    // Step 2: Fetch that checkpoint from Walrus.
    let walrus = sui_transport::walrus::WalrusClient::mainnet();
    let cp_data = match walrus.get_checkpoint(cp) {
        Ok(d) => d,
        Err(_) => return None,
    };

    // Step 3: Cache all move objects from this checkpoint and return the target.
    for tx in &cp_data.transactions {
        for obj in tx.input_objects.iter().chain(tx.output_objects.iter()) {
            let oid = format!("0x{}", hex::encode(obj.id().into_bytes()));
            if let Some((ts, bcs, ver, _shared)) = sui_transport::walrus::extract_object_bcs(obj) {
                cache.lock().insert(oid, (ts, bcs, ver));
            }
        }
    }

    cache.lock().get(id_hex).cloned()
}
