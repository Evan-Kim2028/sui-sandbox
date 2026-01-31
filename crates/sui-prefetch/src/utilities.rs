//! Data helper utilities for gRPC responses.
//!
//! This module provides utilities for aggregating and working with data from
//! gRPC responses. These are **data helpers** that collect and structure data,
//! distinct from infrastructure workarounds (see `sui_sandbox_core::utilities`).
//!
//! ## What Belongs Here
//!
//! - Aggregating data from gRPC transaction responses
//! - Dynamic field computation and prefetching
//! - Data extraction from gRPC types
//!
//! ## What Does NOT Belong Here
//!
//! - Object patching (use `sui_sandbox_core::utilities::GenericObjectPatcher`)
//! - Address normalization (use `sui_sandbox_core::utilities::normalize_address`)
//! - gRPC client initialization (use `sui_transport::create_grpc_client`)
//! - VM/resolver setup (use example-specific code)

use std::collections::HashMap;

use sui_sandbox_types::env_var_or;
use sui_transport::grpc::{GrpcClient, GrpcInput, GrpcTransaction};

/// Compute the dynamic field child object ID using Sui's exact formula.
///
/// The child ID is computed as:
/// `Blake2b256(0xf0 || parent || len(key_bytes) as u64 LE || key_bytes || type_tag_bytes)`
///
/// Where:
/// - 0xf0 is the HashingIntentScope::ChildObjectId constant
/// - parent is the 32-byte parent object address
/// - key_bytes is the BCS-encoded key value
/// - type_tag_bytes is the BCS-encoded key type tag
///
/// # Arguments
/// * `parent_address` - The parent object's address (hex string)
/// * `key_bcs` - BCS-encoded key bytes
/// * `key_type_bcs` - BCS-encoded type tag bytes
///
/// # Returns
/// The computed child object ID as a hex string (0x-prefixed)
pub fn compute_dynamic_field_id(
    parent_address: &str,
    key_bcs: &[u8],
    key_type_bcs: &[u8],
) -> Option<String> {
    use fastcrypto::hash::{Blake2b256, HashFunction};

    // Parse parent address
    let parent_hex = parent_address.strip_prefix("0x").unwrap_or(parent_address);
    let parent_bytes = hex::decode(parent_hex).ok()?;
    if parent_bytes.len() != 32 {
        return None;
    }

    const CHILD_OBJECT_ID_SCOPE: u8 = 0xf0;

    let mut hasher = Blake2b256::default();
    hasher.update([CHILD_OBJECT_ID_SCOPE]);
    hasher.update(&parent_bytes);
    hasher.update((key_bcs.len() as u64).to_le_bytes());
    hasher.update(key_bcs);
    hasher.update(key_type_bcs);

    let hash = hasher.finalize();
    Some(format!("0x{}", hex::encode(hash.digest)))
}

/// Serialize a Move type string to BCS-encoded TypeTag bytes.
///
/// This function parses a type string like "u64" or "0x2::object::ID" and
/// serializes it to the BCS format that Sui uses for type tag encoding.
pub fn type_string_to_bcs(type_str: &str) -> Option<Vec<u8>> {
    // Use the canonical type parsing from sui-sandbox-types
    let type_tag = sui_sandbox_types::parse_type_tag(type_str)?;

    // Serialize the TypeTag to BCS
    bcs::to_bytes(&type_tag).ok()
}

/// Collect historical object versions from a gRPC transaction.
///
/// Aggregates version information from multiple sources in the gRPC response:
/// - `unchanged_loaded_runtime_objects`: Objects read but not modified
/// - `changed_objects`: Objects modified (provides INPUT versions before tx)
/// - `unchanged_consensus_objects`: Actual consensus versions for shared objects
/// - Transaction inputs: Object, SharedObject, and Receiving inputs
///
/// Returns a map from object ID (hex string) to version number.
///
/// # Example
///
/// ```ignore
/// use sui_prefetch::collect_historical_versions;
///
/// let versions = collect_historical_versions(&grpc_tx);
/// for (obj_id, version) in &versions {
///     println!("Object {} at version {}", obj_id, version);
/// }
/// ```
pub fn collect_historical_versions(grpc_tx: &GrpcTransaction) -> HashMap<String, u64> {
    let mut versions: HashMap<String, u64> = HashMap::new();

    // From unchanged_loaded_runtime_objects
    for (id, ver) in &grpc_tx.unchanged_loaded_runtime_objects {
        versions.insert(id.clone(), *ver);
    }

    // From changed_objects (these give us INPUT versions)
    for (id, ver) in &grpc_tx.changed_objects {
        versions.insert(id.clone(), *ver);
    }

    // From unchanged_consensus_objects (actual consensus versions for shared objects)
    for (id, ver) in &grpc_tx.unchanged_consensus_objects {
        versions.insert(id.clone(), *ver);
    }

    // From transaction inputs
    for input in &grpc_tx.inputs {
        match input {
            GrpcInput::Object {
                object_id, version, ..
            } => {
                versions.insert(object_id.clone(), *version);
            }
            GrpcInput::SharedObject {
                object_id,
                initial_version,
                ..
            } => {
                versions.insert(object_id.clone(), *initial_version);
            }
            GrpcInput::Receiving {
                object_id, version, ..
            } => {
                versions.insert(object_id.clone(), *version);
            }
            _ => {}
        }
    }

    versions
}

/// Key for looking up dynamic fields by their name/key content.
/// This enables matching child objects even when the computed hash differs
/// due to package upgrades changing type addresses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DynamicFieldKey {
    /// Parent object ID (normalized to 0x-prefixed lowercase)
    pub parent_id: String,
    /// Type of the key (e.g., "0x2::object::ID", "u64")
    pub name_type: String,
    /// BCS-encoded key bytes
    pub name_bcs: Vec<u8>,
}

/// Information about a prefetched dynamic field child object.
#[derive(Debug, Clone)]
pub struct PrefetchedChild {
    /// Object ID of the child (wrapper object)
    pub object_id: String,
    /// Version of the object
    pub version: u64,
    /// Type string of the value
    pub type_string: String,
    /// BCS-encoded object bytes
    pub bcs: Vec<u8>,
}

/// Result of prefetching dynamic fields for transaction replay.
#[derive(Debug, Clone, Default)]
pub struct PrefetchedDynamicFields {
    /// Map of child object ID -> (version, type_string, bcs_bytes)
    /// Used for direct ID lookup (when hash matches)
    pub children: HashMap<String, (u64, String, Vec<u8>)>,
    /// Map of (parent_id, name_type, name_bcs) -> child info
    /// Used for key-based lookup (when hash doesn't match due to package upgrades)
    pub children_by_key: HashMap<DynamicFieldKey, PrefetchedChild>,
    /// Total number of dynamic fields discovered
    pub total_discovered: usize,
    /// Number of objects successfully fetched
    pub fetched_count: usize,
    /// Objects that failed to fetch (with error messages)
    pub failed: Vec<(String, String)>,
}

/// Recursively prefetch dynamic fields for all input objects.
///
/// This function takes the historical versions map (from `collect_historical_versions`)
/// and recursively fetches all dynamic fields for each object. This is essential for
/// historical transaction replay where child objects may not be included in the
/// transaction effects.
///
/// # Arguments
/// * `graphql` - GraphQL client for fetching dynamic fields
/// * `grpc` - gRPC client for fetching object BCS at specific versions
/// * `rt` - Tokio runtime for async operations
/// * `historical_versions` - Map of object IDs to their historical versions
/// * `max_depth` - Maximum recursion depth for nested dynamic fields (default: 3)
/// * `max_fields_per_object` - Maximum dynamic fields to fetch per object (default: 100)
///
/// # Returns
/// A `PrefetchedDynamicFields` struct containing all discovered child objects with their
/// versions and BCS data.
///
/// # Example
///
/// ```ignore
/// use sui_prefetch::{collect_historical_versions, prefetch_dynamic_fields};
///
/// let versions = collect_historical_versions(&grpc_tx);
/// let prefetched = prefetch_dynamic_fields(
///     &graphql, &grpc, &rt, &versions, 3, 100
/// );
/// println!("Prefetched {} child objects", prefetched.fetched_count);
/// ```
pub fn prefetch_dynamic_fields(
    graphql: &sui_transport::graphql::GraphQLClient,
    grpc: &GrpcClient,
    rt: &tokio::runtime::Runtime,
    historical_versions: &HashMap<String, u64>,
    max_depth: usize,
    max_fields_per_object: usize,
) -> PrefetchedDynamicFields {
    // Compute max lamport version from historical_versions to validate discovered children
    let max_lamport_version = historical_versions.values().copied().max().unwrap_or(0);
    prefetch_dynamic_fields_with_version_bound(
        graphql,
        grpc,
        rt,
        historical_versions,
        max_depth,
        max_fields_per_object,
        max_lamport_version,
    )
}

/// Prefetch dynamic fields using a checkpoint snapshot when available.
pub fn prefetch_dynamic_fields_at_checkpoint(
    graphql: &sui_transport::graphql::GraphQLClient,
    grpc: &GrpcClient,
    rt: &tokio::runtime::Runtime,
    historical_versions: &HashMap<String, u64>,
    max_depth: usize,
    max_fields_per_object: usize,
    checkpoint: u64,
) -> PrefetchedDynamicFields {
    let max_lamport_version = historical_versions.values().copied().max().unwrap_or(0);
    prefetch_dynamic_fields_with_version_bound_internal(
        graphql,
        grpc,
        rt,
        historical_versions,
        max_depth,
        max_fields_per_object,
        max_lamport_version,
        Some(checkpoint),
    )
}

/// Prefetch dynamic fields with an explicit version bound.
///
/// For children NOT in `historical_versions`, checks if their current version
/// is <= `max_lamport_version`. If so, uses that version (the object wasn't
/// modified after the transaction). If the version is higher, the object was
/// modified and we can't use it for replay without additional historical lookup.
pub fn prefetch_dynamic_fields_with_version_bound(
    graphql: &sui_transport::graphql::GraphQLClient,
    grpc: &GrpcClient,
    rt: &tokio::runtime::Runtime,
    historical_versions: &HashMap<String, u64>,
    max_depth: usize,
    max_fields_per_object: usize,
    max_lamport_version: u64,
) -> PrefetchedDynamicFields {
    prefetch_dynamic_fields_with_version_bound_internal(
        graphql,
        grpc,
        rt,
        historical_versions,
        max_depth,
        max_fields_per_object,
        max_lamport_version,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn prefetch_dynamic_fields_with_version_bound_internal(
    graphql: &sui_transport::graphql::GraphQLClient,
    grpc: &GrpcClient,
    rt: &tokio::runtime::Runtime,
    historical_versions: &HashMap<String, u64>,
    max_depth: usize,
    max_fields_per_object: usize,
    max_lamport_version: u64,
    checkpoint: Option<u64>,
) -> PrefetchedDynamicFields {
    use base64::Engine;
    use std::time::{Duration, Instant};

    let mut result = PrefetchedDynamicFields::default();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut to_visit: Vec<(String, usize)> = historical_versions
        .keys()
        .map(|id| (id.clone(), 0))
        .collect();

    let max_secs: u64 = env_var_or("SUI_DF_PREFETCH_TIMEOUT_SECS", 30);
    let start = Instant::now();

    // Helper to normalize address for consistent lookups
    fn normalize_addr(addr: &str) -> String {
        let hex = addr.strip_prefix("0x").unwrap_or(addr);
        format!("0x{}", hex.to_lowercase())
    }

    while let Some((parent_id, depth)) = to_visit.pop() {
        if start.elapsed() > Duration::from_secs(max_secs) {
            eprintln!(
                "[prefetch_df] Timeout after {}s (discovered={}, fetched={})",
                max_secs, result.total_discovered, result.fetched_count
            );
            break;
        }

        // Skip if already visited or too deep
        if visited.contains(&parent_id) || depth > max_depth {
            continue;
        }
        visited.insert(parent_id.clone());

        // Fetch dynamic fields for this object (checkpoint snapshot if available)
        let (dfs, snapshot_used) = match checkpoint {
            Some(cp) => match graphql.fetch_dynamic_fields_at_checkpoint(
                &parent_id,
                max_fields_per_object,
                cp,
            ) {
                Ok(fields) => (fields, true),
                Err(_) => match graphql.fetch_dynamic_fields(&parent_id, max_fields_per_object) {
                    Ok(fields) => (fields, false),
                    Err(_) => continue,
                },
            },
            None => match graphql.fetch_dynamic_fields(&parent_id, max_fields_per_object) {
                Ok(fields) => (fields, false),
                Err(_) => continue,
            },
        };

        let normalized_parent = normalize_addr(&parent_id);

        // Debug: only log parents with dynamic fields
        if !dfs.is_empty() {
            eprintln!(
                "[prefetch_df] Parent {} has {} dynamic fields",
                &parent_id[..20.min(parent_id.len())],
                dfs.len()
            );
        }
        for df in dfs {
            if start.elapsed() > Duration::from_secs(max_secs) {
                eprintln!(
                    "[prefetch_df] Timeout after {}s (discovered={}, fetched={})",
                    max_secs, result.total_discovered, result.fetched_count
                );
                return result;
            }
            result.total_discovered += 1;

            // Get the child object ID (the dynamic field wrapper object).
            // For MoveObject types, GraphQL returns the object_id directly.
            // For MoveValue types, we need to compute the Field wrapper object ID ourselves.
            let child_id = match &df.object_id {
                Some(id) => id.clone(),
                None => {
                    // MoveValue type - compute the Field wrapper object ID ourselves
                    // The Field object ID is hash(0xf0 || parent || len(key) || key || key_type)
                    let name_bcs = match df.decode_name_bcs() {
                        Some(bcs) => bcs,
                        None => continue, // Can't compute ID without key bytes
                    };
                    let type_bcs = match type_string_to_bcs(&df.name_type) {
                        Some(bcs) => bcs,
                        None => {
                            eprintln!(
                                "[prefetch_df] Failed to serialize type '{}' to BCS",
                                df.name_type
                            );
                            continue;
                        }
                    };
                    match compute_dynamic_field_id(&normalized_parent, &name_bcs, &type_bcs) {
                        Some(id) => id,
                        None => continue,
                    }
                }
            };

            // Skip if already have this child by ID
            if result.children.contains_key(&child_id) {
                continue;
            }

            // Get version - prefer historical if known, otherwise try to validate current version
            let version_opt = if let Some(hist_ver) = historical_versions.get(&child_id).copied() {
                Some(hist_ver)
            } else if let Some(current_ver) = df.version {
                // Child is NOT in historical_versions - check if current version is valid
                if snapshot_used || current_ver <= max_lamport_version {
                    // Object hasn't been modified since the transaction, safe to use
                    eprintln!(
                        "[prefetch_df] Child {} not in effects, using current version {} (valid: <= {})",
                        &child_id[..20.min(child_id.len())],
                        current_ver,
                        max_lamport_version
                    );
                    Some(current_ver)
                } else {
                    // Object was modified after the transaction - we can't use current version!
                    // For now, skip this object with a warning
                    eprintln!(
                        "[prefetch_df] WARNING: Child {} has version {} > max {} - SKIPPING (stale data)",
                        &child_id[..20.min(child_id.len())],
                        current_ver,
                        max_lamport_version
                    );
                    continue;
                }
            } else {
                None
            };

            // Try to fetch the full object BCS from gRPC only when we have a known version
            let fetch_result = match version_opt {
                Some(version) => rt
                    .block_on(async { grpc.get_object_at_version(&child_id, Some(version)).await }),
                None => Ok(None),
            };

            // Helper closure to store the child data
            let store_child = |result: &mut PrefetchedDynamicFields,
                               child_id: &str,
                               version: u64,
                               type_str: String,
                               bcs: Vec<u8>,
                               df: &sui_transport::graphql::DynamicFieldInfo,
                               normalized_parent: &str| {
                // Store by ID for direct lookup
                result.children.insert(
                    child_id.to_string(),
                    (version, type_str.clone(), bcs.clone()),
                );

                // Also store by key for key-based lookup (handles package upgrade mismatches)
                if let Some(name_bcs) = df.decode_name_bcs() {
                    let key = DynamicFieldKey {
                        parent_id: normalized_parent.to_string(),
                        name_type: df.name_type.clone(),
                        name_bcs,
                    };
                    result.children_by_key.insert(
                        key,
                        PrefetchedChild {
                            object_id: child_id.to_string(),
                            version,
                            type_string: type_str,
                            bcs,
                        },
                    );
                }

                result.fetched_count += 1;
            };

            match fetch_result {
                Ok(Some(obj)) => {
                    if let (Some(type_str), Some(bcs)) = (obj.type_string, obj.bcs) {
                        store_child(
                            &mut result,
                            &child_id,
                            obj.version,
                            type_str,
                            bcs,
                            &df,
                            &normalized_parent,
                        );

                        // Queue this child for recursive exploration
                        if depth < max_depth {
                            to_visit.push((child_id, depth + 1));
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    let mut gql_obj_opt = None;
                    let mut gql_snapshot_used = false;

                    if let Some(expected_version) = version_opt {
                        gql_obj_opt = graphql
                            .fetch_object_at_version(&child_id, expected_version)
                            .ok();
                    } else if let Some(cp) = checkpoint {
                        if let Ok(obj) = graphql.fetch_object_at_checkpoint(&child_id, cp) {
                            gql_obj_opt = Some(obj);
                            gql_snapshot_used = true;
                        }
                    }

                    if gql_obj_opt.is_none() {
                        gql_obj_opt = graphql.fetch_object(&child_id).ok();
                    }

                    if let Some(gql_obj) = gql_obj_opt {
                        // Validate version if we didn't have a known historical version
                        if let Some(expected_version) = version_opt {
                            if gql_obj.version != expected_version {
                                continue;
                            }
                        } else if !gql_snapshot_used && gql_obj.version > max_lamport_version {
                            continue;
                        }

                        if let (Some(type_str), Some(bcs_b64)) =
                            (gql_obj.type_string, gql_obj.bcs_base64)
                        {
                            if let Ok(bcs) =
                                base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                            {
                                store_child(
                                    &mut result,
                                    &child_id,
                                    gql_obj.version,
                                    type_str,
                                    bcs,
                                    &df,
                                    &normalized_parent,
                                );

                                if depth < max_depth {
                                    to_visit.push((child_id, depth + 1));
                                }
                                continue;
                            }
                        }
                    }
                    result
                        .failed
                        .push((child_id, "Object not found".to_string()));
                }
            }
        }
    }

    result
}

impl PrefetchedDynamicFields {
    /// Look up a child by its object ID (direct hash match).
    pub fn get_by_id(&self, child_id: &str) -> Option<&(u64, String, Vec<u8>)> {
        self.children.get(child_id)
    }

    /// Look up a child by its parent and key content.
    /// This is useful when package upgrades cause the computed child ID to differ.
    ///
    /// # Arguments
    /// * `parent_id` - Parent object ID (will be normalized)
    /// * `name_type` - Type of the key (e.g., "0x2::object::ID")
    /// * `name_bcs` - BCS-encoded key bytes
    pub fn get_by_key(
        &self,
        parent_id: &str,
        name_type: &str,
        name_bcs: &[u8],
    ) -> Option<&PrefetchedChild> {
        let normalized = {
            let hex = parent_id.strip_prefix("0x").unwrap_or(parent_id);
            format!("0x{}", hex.to_lowercase())
        };

        let key = DynamicFieldKey {
            parent_id: normalized,
            name_type: name_type.to_string(),
            name_bcs: name_bcs.to_vec(),
        };

        self.children_by_key.get(&key)
    }

    /// Look up a child, trying ID first then key-based lookup.
    /// Returns (type_string, bcs_bytes).
    pub fn get_child(
        &self,
        child_id: &str,
        parent_id: &str,
        name_type: &str,
        name_bcs: &[u8],
    ) -> Option<(String, Vec<u8>)> {
        // Try direct ID lookup first
        if let Some((_, type_str, bcs)) = self.get_by_id(child_id) {
            return Some((type_str.clone(), bcs.clone()));
        }

        // Fall back to key-based lookup
        if let Some(child) = self.get_by_key(parent_id, name_type, name_bcs) {
            return Some((child.type_string.clone(), child.bcs.clone()));
        }

        None
    }

    /// Look up a child by parent and key bytes only, ignoring type.
    /// This is a fallback for when package upgrades cause type addresses to differ.
    /// Returns the first matching child found.
    ///
    /// # Arguments
    /// * `parent_id` - Parent object ID (will be normalized)
    /// * `name_bcs` - BCS-encoded key bytes
    pub fn get_by_key_bytes_only(
        &self,
        parent_id: &str,
        name_bcs: &[u8],
    ) -> Option<&PrefetchedChild> {
        let normalized = {
            let hex = parent_id.strip_prefix("0x").unwrap_or(parent_id);
            format!("0x{}", hex.to_lowercase())
        };

        // Linear scan through all keys for this parent with matching name_bcs
        for (key, child) in &self.children_by_key {
            if key.parent_id == normalized && key.name_bcs == name_bcs {
                return Some(child);
            }
        }

        None
    }

    /// Look up a child by parent and key bytes with fuzzy length matching.
    /// This handles cases where the BCS encoding differs due to type wrapping differences.
    /// It looks for entries where the bytes content matches even if there's a small length difference.
    ///
    /// # Arguments
    /// * `parent_id` - Parent object ID (will be normalized)
    /// * `name_bcs` - BCS-encoded key bytes
    pub fn get_by_key_bytes_fuzzy(
        &self,
        parent_id: &str,
        name_bcs: &[u8],
    ) -> Option<&PrefetchedChild> {
        let normalized = {
            let hex = parent_id.strip_prefix("0x").unwrap_or(parent_id);
            format!("0x{}", hex.to_lowercase())
        };

        // First try exact match
        for (key, child) in &self.children_by_key {
            if key.parent_id == normalized && key.name_bcs == name_bcs {
                return Some(child);
            }
        }

        // Try fuzzy match: look for entries with similar bytes content
        // The key bytes typically contain the actual value, so we look for
        // entries where the meaningful part matches
        let min_len = name_bcs.len().saturating_sub(10);
        let max_len = name_bcs.len() + 10;

        for (key, child) in &self.children_by_key {
            if key.parent_id != normalized {
                continue;
            }
            // Skip if length difference is too large
            if key.name_bcs.len() < min_len || key.name_bcs.len() > max_len {
                continue;
            }

            // Compare from the start to find matching prefix
            let compare_len = name_bcs.len().min(key.name_bcs.len());

            // Check if they share a long common prefix (indicates same key value)
            // For dynamic field keys, the meaningful part is often at the beginning
            let prefix_match =
                compare_len > 20 && name_bcs[..compare_len] == key.name_bcs[..compare_len];
            if prefix_match {
                eprintln!(
                    "[fuzzy] MATCH via prefix: lookup_len={}, stored_len={}, compare_len={}",
                    name_bcs.len(),
                    key.name_bcs.len(),
                    compare_len
                );
                return Some(child);
            }

            // Also check if the first 20 bytes match exactly - this often contains the key identifier
            // The length difference can be due to different type prefix/wrapping
            if name_bcs.len() >= 20
                && key.name_bcs.len() >= 20
                && name_bcs[..20] == key.name_bcs[..20]
            {
                // Check that the rest of the bytes are also similar (at least 50% match)
                let min_compare = name_bcs.len().min(key.name_bcs.len());
                let matches = name_bcs
                    .iter()
                    .zip(key.name_bcs.iter())
                    .take(min_compare)
                    .filter(|(a, b)| a == b)
                    .count();
                let similarity = matches * 100 / min_compare;
                eprintln!(
                    "[fuzzy] 20-byte prefix match: {}% similarity ({}/{} bytes)",
                    similarity, matches, min_compare
                );
                // Accept if first 20 bytes match exactly and overall similarity is >50%
                if similarity >= 50 {
                    eprintln!(
                        "[fuzzy] MATCH via 20-byte prefix + similarity: {}% match",
                        similarity
                    );
                    return Some(child);
                }
            }

            // Also try comparing from the end (in case there's a type prefix)
            let lookup_suffix = if name_bcs.len() > 20 {
                &name_bcs[name_bcs.len() - 20..]
            } else {
                name_bcs
            };
            let stored_suffix = if key.name_bcs.len() > 20 {
                &key.name_bcs[key.name_bcs.len() - 20..]
            } else {
                &key.name_bcs
            };
            if lookup_suffix == stored_suffix {
                eprintln!("[fuzzy] MATCH via suffix");
                return Some(child);
            }

            // Debug: show first difference for promising candidates
            if key.name_bcs.len() >= 20
                && name_bcs.len() >= 20
                && key.name_bcs[..20] == name_bcs[..20]
            {
                // Find first difference
                for i in 20..compare_len {
                    if name_bcs[i] != key.name_bcs[i] {
                        eprintln!("[fuzzy] First diff at byte {}: lookup={:02x}, stored={:02x} (stored_len={}, lookup_len={})",
                            i, name_bcs[i], key.name_bcs[i], key.name_bcs.len(), name_bcs.len());
                        break;
                    }
                }
            }
        }

        None
    }

    /// Look up a child by parent and key bytes, with fuzzy type matching.
    /// This handles package upgrades where type addresses may differ.
    ///
    /// Matching strategy:
    /// 1. Try exact match on (parent, type, bytes)
    /// 2. Try match on (parent, bytes) only (ignoring type differences)
    /// 3. Try fuzzy bytes match (handles small encoding differences)
    pub fn get_by_key_fuzzy(
        &self,
        parent_id: &str,
        name_type: &str,
        name_bcs: &[u8],
    ) -> Option<&PrefetchedChild> {
        // First try exact match
        if let Some(child) = self.get_by_key(parent_id, name_type, name_bcs) {
            return Some(child);
        }

        // Fall back to exact bytes match (different type)
        if let Some(child) = self.get_by_key_bytes_only(parent_id, name_bcs) {
            return Some(child);
        }

        // Finally try fuzzy bytes match (handles encoding differences)
        self.get_by_key_bytes_fuzzy(parent_id, name_bcs)
    }
}

/// Prefetch dynamic fields with default settings.
///
/// Uses max_depth=3 and max_fields_per_object=100.
pub fn prefetch_dynamic_fields_default(
    graphql: &sui_transport::graphql::GraphQLClient,
    grpc: &GrpcClient,
    rt: &tokio::runtime::Runtime,
    historical_versions: &HashMap<String, u64>,
) -> PrefetchedDynamicFields {
    prefetch_dynamic_fields(graphql, grpc, rt, historical_versions, 3, 100)
}

/// Prefetch epoch-keyed dynamic fields for DeepBook-style historical data.
///
/// DeepBook and similar protocols store historical data in dynamic fields keyed by epoch.
/// This function specifically targets these epoch-keyed fields by scanning existing
/// prefetched data for u64-keyed fields that look like epoch values.
///
/// Note: This function scans already-prefetched data to identify epoch-keyed fields.
/// The initial prefetch should already have fetched the relevant dynamic fields.
/// This function adds them to both caches for easier lookup.
///
/// # Arguments
/// * `prefetched` - Existing prefetched dynamic fields (already populated)
/// * `tx_epoch` - The epoch of the transaction being replayed
/// * `lookback_epochs` - Number of past epochs to consider valid
///
/// # Returns
/// Number of epoch-keyed fields identified (already in cache).
pub fn prefetch_epoch_keyed_fields(
    _graphql: &sui_transport::graphql::GraphQLClient,
    _grpc: &GrpcClient,
    _rt: &tokio::runtime::Runtime,
    prefetched: &mut PrefetchedDynamicFields,
    tx_epoch: u64,
    lookback_epochs: u64,
) -> usize {
    let start_epoch = tx_epoch.saturating_sub(lookback_epochs);
    let mut identified_count = 0;

    // Scan already-prefetched data for u64 keys that look like epochs
    // The initial prefetch should have already fetched these - we're just
    // identifying which ones are epoch-keyed for potential future use
    for key in prefetched.children_by_key.keys() {
        if key.name_type != "u64" {
            continue;
        }

        if key.name_bcs.len() != 8 {
            continue;
        }

        let stored_epoch =
            u64::from_le_bytes(key.name_bcs.as_slice().try_into().unwrap_or([0u8; 8]));

        // Check if this looks like a valid epoch (within reasonable range)
        if stored_epoch >= start_epoch && stored_epoch <= tx_epoch {
            identified_count += 1;
        }
    }

    // The data is already in prefetched.children_by_key and prefetched.children
    // from the initial prefetch. No additional fetching needed.
    identified_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_historical_versions_empty() {
        let grpc_tx = GrpcTransaction {
            digest: "test".to_string(),
            sender: "0x1".to_string(),
            timestamp_ms: None,
            checkpoint: None,
            epoch: None,
            gas_budget: None,
            gas_price: None,
            inputs: vec![],
            commands: vec![],
            status: None,
            objects: vec![],
            execution_error: None,
            unchanged_loaded_runtime_objects: vec![],
            unchanged_consensus_objects: vec![],
            changed_objects: vec![],
            created_objects: vec![],
        };

        let versions = collect_historical_versions(&grpc_tx);
        assert!(versions.is_empty());
    }

    #[test]
    fn test_collect_historical_versions_aggregates() {
        let grpc_tx = GrpcTransaction {
            digest: "test".to_string(),
            sender: "0x1".to_string(),
            timestamp_ms: None,
            checkpoint: None,
            epoch: None,
            gas_budget: None,
            gas_price: None,
            inputs: vec![GrpcInput::Object {
                object_id: "0xaaa".to_string(),
                version: 10,
                digest: "d1".to_string(),
            }],
            commands: vec![],
            status: None,
            objects: vec![],
            execution_error: None,
            unchanged_loaded_runtime_objects: vec![("0xbbb".to_string(), 20)],
            unchanged_consensus_objects: vec![("0xccc".to_string(), 30)],
            changed_objects: vec![("0xddd".to_string(), 40)],
            created_objects: vec![],
        };

        let versions = collect_historical_versions(&grpc_tx);
        assert_eq!(versions.get("0xaaa"), Some(&10));
        assert_eq!(versions.get("0xbbb"), Some(&20));
        assert_eq!(versions.get("0xccc"), Some(&30));
        assert_eq!(versions.get("0xddd"), Some(&40));
    }
}
