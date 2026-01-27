//! Walrus Archival Client
//!
//! Fetches historical checkpoint data from Walrus decentralized storage.
//!
//! # Architecture
//!
//! Walrus stores Sui checkpoints in compressed blobs with the following flow:
//! 1. Query caching server for checkpoint metadata (blob_id, offset, length)
//! 2. Fetch checkpoint data from Walrus aggregator via byte-range request
//! 3. Decode BCS-encoded CheckpointData
//!
//! # Example
//!
//! ```ignore
//! use sui_transport::walrus::WalrusClient;
//!
//! let client = WalrusClient::mainnet();
//! let checkpoint = client.get_checkpoint(12345).await?;
//! ```

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::str::FromStr;
use sui_types::full_checkpoint_content::CheckpointData;
use sui_types::base_types::{ObjectID, SequenceNumber, SuiAddress, MoveObjectType};
use sui_types::object::{Object, Owner, MoveObject};
use sui_types::digests::TransactionDigest;
use base64::Engine;
use std::collections::HashMap;
use sui_storage::blob::Blob;

/// Walrus archival client for fetching historical checkpoint data.
#[derive(Clone, Debug)]
pub struct WalrusClient {
    /// Base URL for the caching server (metadata queries)
    caching_url: String,
    /// Base URL for the Walrus aggregator (blob data)
    aggregator_url: String,
    /// HTTP client for requests
    http_client: ureq::Agent,
}

/// Response from /v1/app_checkpoint endpoint
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckpointInfoResponse {
    pub checkpoint_number: u64,
    pub blob_id: String,
    pub object_id: String,
    pub index: usize,
    pub offset: u64,
    pub length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
}

impl WalrusClient {
    /// Create a client for Sui mainnet archival.
    pub fn mainnet() -> Self {
        Self {
            caching_url: "https://walrus-sui-archival.mainnet.walrus.space".to_string(),
            aggregator_url: "https://aggregator.walrus-mainnet.walrus.space".to_string(),
            http_client: ureq::Agent::new(),
        }
    }

    /// Create a client for Sui testnet archival.
    pub fn testnet() -> Self {
        Self {
            caching_url: "https://walrus-sui-archival.testnet.walrus.space".to_string(),
            aggregator_url: "https://aggregator.walrus-testnet.walrus.space".to_string(),
            http_client: ureq::Agent::new(),
        }
    }

    /// Create a custom client with specific endpoints.
    pub fn new(caching_url: String, aggregator_url: String) -> Self {
        Self {
            caching_url,
            aggregator_url,
            http_client: ureq::Agent::new(),
        }
    }

    /// Get the latest archived checkpoint number.
    ///
    /// Queries the homepage API to find the most recent checkpoint in Walrus.
    pub fn get_latest_checkpoint(&self) -> Result<u64> {
        let url = format!("{}/v1/app_info_for_homepage", self.caching_url);

        let response: serde_json::Value = self
            .http_client
            .get(&url)
            .call()
            .map_err(|e| anyhow!("Failed to fetch homepage info: {}", e))?
            .into_json()
            .map_err(|e| anyhow!("Failed to parse homepage response: {}", e))?;

        let latest = response
            .get("latest_checkpoint")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("latest_checkpoint not found in response"))?;

        Ok(latest)
    }

    /// Get checkpoint metadata (blob location) from the caching server.
    ///
    /// This returns the blob_id, offset, and length needed to fetch the checkpoint data.
    pub fn get_checkpoint_metadata(&self, checkpoint: u64) -> Result<CheckpointInfoResponse> {
        let url = format!(
            "{}/v1/app_checkpoint?checkpoint={}",
            self.caching_url, checkpoint
        );

        let response: CheckpointInfoResponse = self
            .http_client
            .get(&url)
            .call()
            .map_err(|e| anyhow!("Failed to fetch checkpoint metadata: {}", e))?
            .into_json()
            .map_err(|e| anyhow!("Failed to parse checkpoint metadata: {}", e))?;

        Ok(response)
    }

    /// Fetch raw checkpoint bytes from Walrus aggregator.
    ///
    /// Uses byte-range request to efficiently fetch only the checkpoint data.
    pub fn fetch_checkpoint_bytes(&self, blob_id: &str, offset: u64, length: u64) -> Result<Vec<u8>> {
        let url = format!(
            "{}/v1/blobs/{}/byte-range?start={}&length={}",
            self.aggregator_url, blob_id, offset, length
        );

        let response = self
            .http_client
            .get(&url)
            .call()
            .map_err(|e| anyhow!("Failed to fetch from Walrus aggregator: {}", e))?;

        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| anyhow!("Failed to read response body: {}", e))?;

        Ok(bytes)
    }

    /// Get full checkpoint data from Walrus.
    ///
    /// This is the main entry point for fetching checkpoint data:
    /// 1. Query metadata to get blob_id, offset, length
    /// 2. Fetch raw bytes from Walrus aggregator
    /// 3. Decode BCS-encoded CheckpointData
    pub fn get_checkpoint(&self, checkpoint: u64) -> Result<CheckpointData> {
        // Step 1: Get metadata
        let metadata = self.get_checkpoint_metadata(checkpoint)?;

        // Step 2: Fetch raw bytes
        let bcs_bytes = self.fetch_checkpoint_bytes(
            &metadata.blob_id,
            metadata.offset,
            metadata.length,
        )?;

        // Step 3: Decode (Walrus aggregator returns a Sui `Blob` wrapper: [encoding_byte || bcs_payload])
        let checkpoint_data: CheckpointData = Blob::from_bytes::<CheckpointData>(&bcs_bytes)
            .map_err(|e| anyhow!("Failed to decode checkpoint data: {}", e))?;

        Ok(checkpoint_data)
    }

    /// Fetch checkpoint data via BCS and serialize to JSON locally.
    ///
    /// This is typically faster and transfers less data than using `show_content=true`
    /// because the server-side JSON encoding can be large.
    pub fn get_checkpoint_json(&self, checkpoint: u64) -> Result<serde_json::Value> {
        let data = self.get_checkpoint(checkpoint)?;
        serde_json::to_value(&data).map_err(|e| anyhow!("Failed to serialize checkpoint data: {e}"))
    }

    /// Get checkpoint data with full content via the caching server.
    ///
    /// This uses the caching server's endpoint which returns the checkpoint
    /// content as JSON with base64-encoded byte arrays.
    ///
    /// Note: This is less efficient than get_checkpoint() for programmatic use
    /// but useful for debugging/inspection.
    pub fn get_checkpoint_with_content(&self, checkpoint: u64) -> Result<serde_json::Value> {
        let url = format!(
            "{}/v1/app_checkpoint?checkpoint={}&show_content=true",
            self.caching_url, checkpoint
        );

        let response: CheckpointInfoResponse = self
            .http_client
            .get(&url)
            .call()
            .map_err(|e| anyhow!("Failed to fetch checkpoint with content: {}", e))?
            .into_json()
            .map_err(|e| anyhow!("Failed to parse response: {}", e))?;

        response
            .content
            .ok_or_else(|| anyhow!("No content in response"))
    }

    /// Fetch many checkpoints more efficiently by batching byte-range downloads per blob.
    ///
    /// How it works:
    /// - For each checkpoint, query `/v1/app_checkpoint` to obtain (blob_id, offset, length)
    /// - Group checkpoints by blob_id
    /// - Within each blob, merge adjacent ranges into chunks (bounded by `max_chunk_bytes`)
    /// - Download each merged range once, then slice out each checkpoint's byte segment and BCS-decode it
    ///
    /// This reduces the number of aggregator requests dramatically when replaying long sequential ranges.
    pub fn get_checkpoints_batched(
        &self,
        checkpoints: &[u64],
        max_chunk_bytes: u64,
    ) -> Result<Vec<(u64, CheckpointData)>> {
        if checkpoints.is_empty() {
            return Ok(vec![]);
        }
        let max_chunk_bytes = max_chunk_bytes.max(1024 * 1024); // at least 1 MiB

        // Step 1: per-checkpoint metadata (still required by current API surface)
        let mut by_blob: HashMap<String, Vec<CheckpointSegment>> = HashMap::new();
        for &cp in checkpoints {
            let meta = self.get_checkpoint_metadata(cp)?;
            by_blob
                .entry(meta.blob_id.clone())
                .or_default()
                .push(CheckpointSegment {
                    checkpoint: cp,
                    offset: meta.offset,
                    length: meta.length,
                });
        }

        // Step 2: for each blob, merge segments into fetch ranges and slice
        let mut out: Vec<(u64, CheckpointData)> = Vec::with_capacity(checkpoints.len());
        for (blob_id, mut segs) in by_blob {
            segs.sort_by_key(|s| s.offset);
            let chunks = merge_segments_into_chunks(&segs, max_chunk_bytes);
            for chunk in chunks {
                let bytes = self.fetch_checkpoint_bytes(&blob_id, chunk.start, chunk.length)?;
                for seg in chunk.segments {
                    let rel = (seg.offset - chunk.start) as usize;
                    let len = seg.length as usize;
                    if rel + len > bytes.len() {
                        return Err(anyhow!(
                            "batched blob slice out of bounds (blob_id={}, checkpoint={}, rel={}, len={}, bytes={})",
                            blob_id,
                            seg.checkpoint,
                            rel,
                            len,
                            bytes.len()
                        ));
                    }
                    let slice = &bytes[rel..rel + len];
                    let cp_data: CheckpointData = Blob::from_bytes::<CheckpointData>(slice)
                        .map_err(|e| anyhow!("Failed to decode checkpoint {}: {}", seg.checkpoint, e))?;
                    out.push((seg.checkpoint, cp_data));
                }
            }
        }

        // Preserve input order if the caller provided ordered checkpoints.
        let mut by_cp: HashMap<u64, CheckpointData> = HashMap::new();
        for (cp, data) in out {
            by_cp.insert(cp, data);
        }
        let mut ordered = Vec::with_capacity(checkpoints.len());
        for &cp in checkpoints {
            let data = by_cp
                .remove(&cp)
                .ok_or_else(|| anyhow!("missing decoded checkpoint {}", cp))?;
            ordered.push((cp, data));
        }
        Ok(ordered)
    }

    /// Batched variant that returns JSON (serialized locally from BCS).
    pub fn get_checkpoints_json_batched(
        &self,
        checkpoints: &[u64],
        max_chunk_bytes: u64,
    ) -> Result<Vec<(u64, serde_json::Value)>> {
        let decoded = self.get_checkpoints_batched(checkpoints, max_chunk_bytes)?;
        decoded
            .into_iter()
            .map(|(cp, data)| {
                let v =
                    serde_json::to_value(&data).map_err(|e| anyhow!("serialize checkpoint {}: {e}", cp))?;
                Ok((cp, v))
            })
            .collect()
    }

    /// List available checkpoint blobs.
    ///
    /// Returns metadata about all archived checkpoint blobs including
    /// checkpoint ranges and blob IDs.
    pub fn list_blobs(&self, limit: Option<usize>) -> Result<Vec<BlobInfo>> {
        let url = if let Some(limit) = limit {
            format!("{}/v1/app_blobs?limit={}", self.caching_url, limit)
        } else {
            format!("{}/v1/app_blobs", self.caching_url)
        };

        let response: BlobListResponse = self
            .http_client
            .get(&url)
            .call()
            .map_err(|e| anyhow!("Failed to list blobs: {}", e))?
            .into_json()
            .map_err(|e| anyhow!("Failed to parse blobs response: {}", e))?;

        Ok(response.blobs)
    }

    /// Find which blob contains a specific checkpoint.
    pub fn find_blob_for_checkpoint(&self, checkpoint: u64) -> Result<Option<BlobInfo>> {
        // Fetch blobs until we find one containing the checkpoint
        // In a production system, this should use binary search or direct DB query
        let blobs = self.list_blobs(Some(100))?;

        Ok(blobs
            .into_iter()
            .find(|b| checkpoint >= b.start_checkpoint && checkpoint <= b.end_checkpoint))
    }

    /// Deserialize input objects from Walrus JSON into sui_types::object::Object.
    ///
    /// Takes the input_objects array from the JSON checkpoint data and converts
    /// the BCS-encoded object state into proper Object instances.
    ///
    /// Note: The input_objects array doesn't include explicit object IDs at the top level.
    /// Object IDs are embedded in the BCS contents (first field of the Move struct is the UID).
    /// We extract them from the BCS data.
    pub fn deserialize_input_objects(
        &self,
        input_objects: &[serde_json::Value],
    ) -> Result<Vec<Object>> {
        let mut objects = Vec::new();

        for obj_json in input_objects {
            let data = obj_json
                .get("data")
                .ok_or_else(|| anyhow!("Missing data field"))?;

            if let Some(move_obj) = data.get("Move") {
                // Extract BCS-encoded contents first (we'll extract object ID from it)
                let contents_b64 = move_obj
                    .get("contents")
                    .and_then(|c| c.as_str())
                    .ok_or_else(|| anyhow!("Missing contents"))?;
                let bcs_bytes = base64::engine::general_purpose::STANDARD.decode(contents_b64)?;

                // Extract object ID from BCS contents
                // In Move, all objects start with a UID struct which contains the ID (32 bytes)
                if bcs_bytes.len() < 32 {
                    return Err(anyhow!("BCS contents too short to contain object ID"));
                }
                let _object_id = ObjectID::from_bytes(&bcs_bytes[0..32])?;

                // Extract version
                let version = move_obj
                    .get("version")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("Missing version"))?;

                // Extract owner
                let owner = self.parse_owner(
                    obj_json.get("owner").ok_or_else(|| anyhow!("Missing owner"))?
                )?;

                // Extract type
                let type_json = move_obj
                    .get("type_")
                    .ok_or_else(|| anyhow!("Missing type_"))?;
                let type_tag = self.parse_type_tag(type_json)?;

                // Convert TypeTag to MoveObjectType
                let move_object_type = match type_tag {
                    move_core_types::language_storage::TypeTag::Struct(ref struct_tag) => {
                        MoveObjectType::from(struct_tag.as_ref().clone())
                    }
                    _ => return Err(anyhow!("Expected struct type, got {:?}", type_tag)),
                };

                // Create MoveObject
                let has_public_transfer = move_obj
                    .get("has_public_transfer")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let move_object = unsafe {
                    MoveObject::new_from_execution(
                        move_object_type,
                        has_public_transfer,
                        SequenceNumber::from_u64(version),
                        bcs_bytes,
                        &sui_protocol_config::ProtocolConfig::get_for_max_version_UNSAFE(),
                        true, // is_mutable - assume mutable for historical replay
                    )?
                };

                // Extract previous transaction digest
                let prev_tx = obj_json
                    .get("previous_transaction")
                    .and_then(|t| t.as_str())
                    .and_then(|s| TransactionDigest::from_str(s).ok())
                    .ok_or_else(|| anyhow!("Missing previous_transaction"))?;

                // Create Object
                let object = Object::new_move(move_object, owner, prev_tx);

                objects.push(object);
            }
        }

        Ok(objects)
    }

    /// Parse owner information from JSON.
    fn parse_owner(&self, owner_json: &serde_json::Value) -> Result<Owner> {
        if let Some(shared) = owner_json.get("Shared") {
            let initial_shared_version = shared
                .get("initial_shared_version")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| anyhow!("Missing initial_shared_version"))?;
            Ok(Owner::Shared {
                initial_shared_version: SequenceNumber::from_u64(initial_shared_version),
            })
        } else if let Some(addr) = owner_json.get("AddressOwner").and_then(|a| a.as_str()) {
            Ok(Owner::AddressOwner(SuiAddress::from_str(addr)?))
        } else if owner_json.get("Immutable").is_some() {
            Ok(Owner::Immutable)
        } else if let Some(obj_id) = owner_json.get("ObjectOwner").and_then(|o| o.as_str()) {
            Ok(Owner::ObjectOwner(SuiAddress::from_str(obj_id)?))
        } else {
            Err(anyhow!("Unknown owner type: {:?}", owner_json))
        }
    }

    /// Parse type tag from JSON.
    fn parse_type_tag(
        &self,
        type_json: &serde_json::Value,
    ) -> Result<move_core_types::language_storage::TypeTag> {
        use move_core_types::account_address::AccountAddress;
        use move_core_types::identifier::Identifier;
        use move_core_types::language_storage::{StructTag, TypeTag};

        // Handle string shortcuts like "GasCoin" and primitives
        if let Some(type_str) = type_json.as_str() {
            return match type_str {
                "GasCoin" => {
                    // GasCoin is 0x2::coin::Coin<0x2::sui::SUI>
                    let sui_type = TypeTag::Struct(Box::new(StructTag {
                        address: AccountAddress::from_hex_literal("0x2")?,
                        module: Identifier::new("sui")?,
                        name: Identifier::new("SUI")?,
                        type_params: vec![],
                    }));
                    Ok(TypeTag::Struct(Box::new(StructTag {
                        address: AccountAddress::from_hex_literal("0x2")?,
                        module: Identifier::new("coin")?,
                        name: Identifier::new("Coin")?,
                        type_params: vec![sui_type],
                    })))
                }
                "u64" => Ok(TypeTag::U64),
                "u8" => Ok(TypeTag::U8),
                "u16" => Ok(TypeTag::U16),
                "u32" => Ok(TypeTag::U32),
                "u128" => Ok(TypeTag::U128),
                "u256" => Ok(TypeTag::U256),
                "bool" => Ok(TypeTag::Bool),
                "address" => Ok(TypeTag::Address),
                _ => Err(anyhow!("Unknown type string: {}", type_str)),
            };
        }

        // Handle "Coin" wrapper format (for custom coins)
        if let Some(coin_json) = type_json.get("Coin") {
            if let Some(struct_json) = coin_json.get("struct") {
                // Parse the inner coin type
                let inner_type = self.parse_type_tag(&serde_json::json!({ "struct": struct_json }))?;

                // Return 0x2::coin::Coin<InnerType>
                return Ok(TypeTag::Struct(Box::new(StructTag {
                    address: AccountAddress::from_hex_literal("0x2")?,
                    module: Identifier::new("coin")?,
                    name: Identifier::new("Coin")?,
                    type_params: vec![inner_type],
                })));
            }
        }

        // Handle "Other" wrapper (for top-level types)
        let struct_json = if let Some(other) = type_json.get("Other") {
            other
        } else if type_json.get("struct").is_some() {
            // Handle nested "struct" key (for type parameters)
            type_json.get("struct").unwrap()
        } else {
            return Err(anyhow!("Unsupported type tag format: {:?}", type_json));
        };

        let address = struct_json
            .get("address")
            .and_then(|a| a.as_str())
            .ok_or_else(|| anyhow!("Missing address in type"))?;
        let module = struct_json
            .get("module")
            .and_then(|m| m.as_str())
            .ok_or_else(|| anyhow!("Missing module in type"))?;
        let name = struct_json
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| anyhow!("Missing name in type"))?;

        // Parse type_args recursively if present
        let type_params = if let Some(type_args) = struct_json.get("type_args").and_then(|t| t.as_array()) {
            type_args
                .iter()
                .map(|arg| self.parse_type_tag(arg))
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![]
        };

        // Add 0x prefix if missing for AccountAddress parsing
        let address_with_prefix = if address.starts_with("0x") {
            address.to_string()
        } else {
            format!("0x{}", address)
        };

        let struct_tag = StructTag {
            address: AccountAddress::from_hex_literal(&address_with_prefix)?,
            module: Identifier::new(module)?,
            name: Identifier::new(name)?,
            type_params,
        };

        Ok(TypeTag::Struct(Box::new(struct_tag)))
    }

    /// Extract package IDs from a PTB transaction.
    ///
    /// Parses the transaction JSON to find all MoveCall commands and extracts
    /// the package IDs that need to be fetched for execution.
    pub fn extract_package_ids(&self, tx_json: &serde_json::Value) -> Result<Vec<ObjectID>> {
        let mut package_ids = Vec::new();

        let ptb = tx_json
            .get("transaction")
            .and_then(|t| t.get("data"))
            .and_then(|d| d.get(0))
            .and_then(|d| d.get("intent_message"))
            .and_then(|i| i.get("value"))
            .and_then(|v| v.get("V1"))
            .and_then(|v1| v1.get("kind"))
            .and_then(|k| k.get("ProgrammableTransaction"))
            .ok_or_else(|| anyhow!("Not a PTB"))?;

        if let Some(commands) = ptb.get("commands").and_then(|c| c.as_array()) {
            for cmd in commands {
                if let Some(move_call) = cmd.get("MoveCall") {
                    if let Some(package) = move_call.get("package").and_then(|p| p.as_str()) {
                        let pkg_id = ObjectID::from_hex_literal(package)?;
                        if !package_ids.contains(&pkg_id) {
                            package_ids.push(pkg_id);
                        }
                    }
                }
            }
        }

        Ok(package_ids)
    }
}

#[derive(Debug, Clone)]
struct CheckpointSegment {
    checkpoint: u64,
    offset: u64,
    length: u64,
}

#[derive(Debug, Clone)]
struct BlobChunk {
    start: u64,
    length: u64,
    segments: Vec<CheckpointSegment>,
}

fn merge_segments_into_chunks(segs: &[CheckpointSegment], max_chunk_bytes: u64) -> Vec<BlobChunk> {
    let mut out: Vec<BlobChunk> = Vec::new();
    let mut current: Option<BlobChunk> = None;

    for seg in segs {
        let seg_end = seg.offset.saturating_add(seg.length);
        match current.as_mut() {
            None => {
                current = Some(BlobChunk {
                    start: seg.offset,
                    length: seg.length,
                    segments: vec![seg.clone()],
                });
            }
            Some(ch) => {
                let ch_end = ch.start.saturating_add(ch.length);
                let new_start = ch.start.min(seg.offset);
                let new_end = ch_end.max(seg_end);
                let new_len = new_end.saturating_sub(new_start);

                // If this segment would blow up the chunk, flush and start a new one.
                if new_len > max_chunk_bytes {
                    out.push(ch.clone());
                    *ch = BlobChunk {
                        start: seg.offset,
                        length: seg.length,
                        segments: vec![seg.clone()],
                    };
                    continue;
                }

                // Extend the chunk to cover this segment.
                ch.start = new_start;
                ch.length = new_len;
                ch.segments.push(seg.clone());
            }
        }
    }

    if let Some(ch) = current {
        out.push(ch);
    }

    out
}

/// Metadata about a checkpoint blob.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BlobInfo {
    pub blob_id: String,
    pub object_id: String,
    pub start_checkpoint: u64,
    pub end_checkpoint: u64,
    pub end_of_epoch: bool,
    pub expiry_epoch: u32,
    pub is_shared_blob: bool,
    pub entries_count: usize,
    pub total_size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct BlobListResponse {
    blobs: Vec<BlobInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Requires network access
    fn test_get_latest_checkpoint() {
        let client = WalrusClient::mainnet();
        let latest = client.get_latest_checkpoint().unwrap();
        println!("Latest checkpoint: {}", latest);
        assert!(latest > 0);
    }

    #[test]
    #[ignore] // Requires network access
    fn test_list_blobs() {
        let client = WalrusClient::mainnet();
        let blobs = client.list_blobs(Some(5)).unwrap();
        println!("Found {} blobs", blobs.len());
        for blob in &blobs {
            println!(
                "  Blob: {} (checkpoints {}-{})",
                blob.blob_id, blob.start_checkpoint, blob.end_checkpoint
            );
        }
        assert!(!blobs.is_empty());
    }

    #[test]
    #[ignore] // Requires network access
    fn test_get_checkpoint() {
        let client = WalrusClient::mainnet();

        // Get latest checkpoint number first
        let latest = client.get_latest_checkpoint().unwrap();
        println!("Latest checkpoint: {}", latest);

        // Fetch the checkpoint
        let checkpoint = client.get_checkpoint(latest).unwrap();

        println!("Checkpoint {} data:", latest);
        println!("  Transactions: {}", checkpoint.transactions.len());
        println!("  Checkpoint sequence: {}", checkpoint.checkpoint_summary.sequence_number);

        assert_eq!(checkpoint.checkpoint_summary.sequence_number, latest);
    }
}
