//! Cache-related sandbox handlers.
//!
//! Handles load_cached_objects, load_cached_object, list_cached_objects,
//! is_framework_cached, and ensure_framework_cached operations.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;
use crate::benchmark::types::format_type_tag;
use std::collections::HashMap;

/// Load cached objects from a transaction replay.
pub fn execute_load_cached_objects(
    env: &mut SimulationEnvironment,
    objects: &HashMap<String, String>,
    object_types: &HashMap<String, String>,
    shared_object_ids: &[String],
    verbose: bool,
) -> SandboxResponse {
    use base64::Engine;

    if verbose {
        eprintln!("Loading {} cached objects", objects.len());
    }

    let shared_set: std::collections::HashSet<&str> =
        shared_object_ids.iter().map(|s| s.as_str()).collect();
    let mut loaded = 0;
    let mut failed = Vec::new();

    for (object_id, b64_bytes) in objects {
        let is_shared = shared_set.contains(object_id.as_str());
        let object_type = object_types.get(object_id).map(|s| s.as_str());

        match base64::engine::general_purpose::STANDARD.decode(b64_bytes) {
            Ok(bcs_bytes) => {
                match env.load_cached_object_with_type(object_id, bcs_bytes, object_type, is_shared)
                {
                    Ok(_) => {
                        loaded += 1;
                        if verbose {
                            eprintln!("  Loaded object {} (shared={})", object_id, is_shared);
                        }
                    }
                    Err(e) => {
                        failed.push(serde_json::json!({
                            "object_id": object_id,
                            "error": e.to_string(),
                        }));
                    }
                }
            }
            Err(e) => {
                failed.push(serde_json::json!({
                    "object_id": object_id,
                    "error": format!("Base64 decode error: {}", e),
                }));
            }
        }
    }

    SandboxResponse::success_with_data(serde_json::json!({
        "loaded": loaded,
        "failed": failed.len(),
        "failures": failed,
    }))
}

/// Load a single cached object.
pub fn execute_load_cached_object(
    env: &mut SimulationEnvironment,
    object_id: &str,
    bcs_bytes_b64: &str,
    object_type: Option<&str>,
    is_shared: bool,
    verbose: bool,
) -> SandboxResponse {
    use base64::Engine;

    if verbose {
        eprintln!(
            "Loading cached object: {} (shared={})",
            object_id, is_shared
        );
    }

    let bcs_bytes = match base64::engine::general_purpose::STANDARD.decode(bcs_bytes_b64) {
        Ok(bytes) => bytes,
        Err(e) => {
            return SandboxResponse::error_with_category(
                format!("Base64 decode error: {}", e),
                "DecodeError",
            );
        }
    };

    match env.load_cached_object_with_type(object_id, bcs_bytes, object_type, is_shared) {
        Ok(id) => SandboxResponse::success_with_data(serde_json::json!({
            "object_id": id.to_hex_literal(),
            "is_shared": is_shared,
            "type": object_type,
        })),
        Err(e) => SandboxResponse::error_with_category(
            format!("Failed to load object: {}", e),
            "ObjectLoadError",
        ),
    }
}

/// List all loaded cached objects with their types.
pub fn execute_list_cached_objects(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing cached objects");
    }

    let objects: Vec<serde_json::Value> = env
        .list_objects()
        .iter()
        .map(|obj| {
            serde_json::json!({
                "object_id": obj.id.to_hex_literal(),
                "type": format_type_tag(&obj.type_tag),
                "is_shared": obj.is_shared,
                "is_immutable": obj.is_immutable,
                "version": obj.version,
                "bytes_len": obj.bcs_bytes.len(),
            })
        })
        .collect();

    SandboxResponse::success_with_data(serde_json::json!({
        "objects": objects,
        "count": objects.len(),
    }))
}

/// Check if Sui framework is cached locally.
pub fn execute_is_framework_cached(verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Checking if Sui framework is cached");
    }

    use crate::benchmark::package_builder::FrameworkCache;
    match FrameworkCache::new() {
        Ok(cache) => {
            let is_cached = cache.is_cached();
            SandboxResponse::success_with_data(serde_json::json!({
                "is_cached": is_cached,
                "path": cache.sui_framework_path().display().to_string(),
            }))
        }
        Err(e) => SandboxResponse::error(format!("Failed to check framework cache: {}", e)),
    }
}

/// Download and cache Sui framework (if not already cached).
pub fn execute_ensure_framework_cached(verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Ensuring Sui framework is cached");
    }

    use crate::benchmark::package_builder::FrameworkCache;
    match FrameworkCache::new() {
        Ok(cache) => {
            if cache.is_cached() {
                SandboxResponse::success_with_data(serde_json::json!({
                    "status": "already_cached",
                    "path": cache.sui_framework_path().display().to_string(),
                }))
            } else {
                match cache.ensure_cached() {
                    Ok(_) => SandboxResponse::success_with_data(serde_json::json!({
                        "status": "downloaded",
                        "path": cache.sui_framework_path().display().to_string(),
                    })),
                    Err(e) => {
                        SandboxResponse::error(format!("Failed to download framework: {}", e))
                    }
                }
            }
        }
        Err(e) => SandboxResponse::error(format!("Failed to initialize framework cache: {}", e)),
    }
}
