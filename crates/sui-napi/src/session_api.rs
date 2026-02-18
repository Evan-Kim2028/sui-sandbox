use super::*;

/// Run environment and endpoint preflight checks (CLI parity for `doctor`).
///
/// This function never raises on check failures; it always returns a structured report.
#[napi]
pub async fn doctor(
    rpc_url: Option<String>,
    state_file: Option<String>,
    timeout_secs: Option<u32>,
    include_toolchain_checks: Option<bool>,
) -> napi::Result<serde_json::Value> {
    let rpc = rpc_url
        .as_deref()
        .unwrap_or("https://archive.mainnet.sui.io:443");
    let cfg = CoreDoctorConfig {
        timeout_secs: timeout_secs.map(|v| v as u64).unwrap_or(20),
        rpc_url: rpc.to_string(),
        state_file: state_file
            .map(PathBuf::from)
            .unwrap_or_else(default_state_file_path),
        include_toolchain_checks: include_toolchain_checks.unwrap_or(false),
    };
    let report = core_run_doctor(&cfg).await.map_err(to_napi_err)?;
    serde_json::to_value(report).map_err(|e| to_napi_err(anyhow!(e)))
}

/// Return sandbox session status for a state file (CLI parity for `status`).
#[napi]
pub fn session_status(
    state_file: Option<String>,
    rpc_url: Option<String>,
) -> napi::Result<serde_json::Value> {
    let rpc = rpc_url
        .as_deref()
        .unwrap_or("https://archive.mainnet.sui.io:443");
    let state_path = state_file
        .map(PathBuf::from)
        .unwrap_or_else(default_state_file_path);
    let state = load_or_create_state(&state_path).map_err(to_napi_err)?;

    let mut package_modules: HashMap<String, HashSet<String>> = HashMap::new();
    for pkg in &state.packages {
        let addr = normalize_address_like_cli(&pkg.address);
        let entry = package_modules.entry(addr).or_default();
        for module in &pkg.modules {
            entry.insert(module.name.clone());
        }
    }
    for module in &state.modules {
        if let Some((addr, name)) = module.id.split_once("::") {
            let key = normalize_address_like_cli(addr);
            package_modules
                .entry(key)
                .or_default()
                .insert(name.to_string());
        }
    }

    let mut packages: Vec<serde_json::Value> = package_modules
        .into_iter()
        .map(|(address, modules)| {
            let mut modules: Vec<String> = modules.into_iter().collect();
            modules.sort();
            serde_json::json!({
                "address": address,
                "modules": modules,
            })
        })
        .collect();
    packages.sort_by(|a, b| {
        let aa = a
            .get("address")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let bb = b
            .get("address")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        aa.cmp(bb)
    });

    let last_sender = state
        .sender
        .trim()
        .strip_prefix("0x")
        .map(|_| state.sender.clone())
        .and_then(|sender| AccountAddress::from_hex_literal(&sender).ok())
        .and_then(|addr| {
            if addr == AccountAddress::ZERO {
                None
            } else {
                Some(addr.to_hex_literal())
            }
        });

    Ok(serde_json::json!({
        "packages_loaded": packages.len(),
        "packages": packages,
        "objects_loaded": state.objects.len(),
        "modules_loaded": state.modules.len()
            + state.packages.iter().map(|p| p.modules.len()).sum::<usize>(),
        "dynamic_fields_loaded": state.dynamic_fields.len(),
        "last_sender": last_sender,
        "rpc_url": rpc,
        "state_file": state_path.display().to_string(),
        "state_file_exists": state_path.exists(),
        "created_at": state.metadata.as_ref().and_then(|m| m.created_at.clone()),
        "modified_at": state.metadata.as_ref().and_then(|m| m.modified_at.clone()),
    }))
}

/// Reset sandbox session state to a clean baseline (CLI parity for `reset`).
#[napi]
pub fn session_reset(state_file: Option<String>) -> napi::Result<serde_json::Value> {
    let state_path = state_file
        .map(PathBuf::from)
        .unwrap_or_else(default_state_file_path);
    save_state(&state_path, &default_persistent_state()).map_err(to_napi_err)?;
    Ok(serde_json::json!({
        "success": true,
        "message": "Session reset",
        "state_file": state_path.display().to_string(),
    }))
}

/// Remove sandbox session state file (CLI parity for `clean`).
#[napi]
pub fn session_clean(state_file: Option<String>) -> napi::Result<serde_json::Value> {
    let state_path = state_file
        .map(PathBuf::from)
        .unwrap_or_else(default_state_file_path);
    let removed = if state_path.exists() {
        std::fs::remove_file(&state_path)
            .with_context(|| format!("Failed to remove {}", state_path.display()))
            .map_err(to_napi_err)?;
        true
    } else {
        false
    };
    Ok(serde_json::json!({
        "success": true,
        "removed": removed,
        "state_file": state_path.display().to_string(),
    }))
}

/// Save a named snapshot of the current session state (CLI parity for `snapshot save`).
#[napi]
pub fn snapshot_save(
    name: String,
    description: Option<String>,
    state_file: Option<String>,
) -> napi::Result<serde_json::Value> {
    let state_path = state_file
        .map(PathBuf::from)
        .unwrap_or_else(default_state_file_path);
    let persisted = load_or_create_state(&state_path).map_err(to_napi_err)?;
    let path = snapshot_path(&name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create snapshot dir {}", parent.display()))
            .map_err(to_napi_err)?;
    }

    let snapshot = SnapshotFile {
        schema_version: 1,
        name: name.clone(),
        description,
        created_at: chrono::Utc::now().to_rfc3339(),
        state: persisted,
    };
    let raw = serde_json::to_string_pretty(&snapshot).map_err(|e| to_napi_err(anyhow!(e)))?;
    std::fs::write(&path, raw)
        .with_context(|| format!("Failed to write snapshot {}", path.display()))
        .map_err(to_napi_err)?;

    Ok(serde_json::json!({
        "success": true,
        "name": name,
        "path": path.display().to_string(),
        "state_file": state_path.display().to_string(),
    }))
}

/// Load a named snapshot into the session state file (CLI parity for `snapshot load`).
#[napi]
pub fn snapshot_load(
    name: String,
    state_file: Option<String>,
) -> napi::Result<serde_json::Value> {
    let state_path = state_file
        .map(PathBuf::from)
        .unwrap_or_else(default_state_file_path);
    let path = snapshot_path(&name);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read snapshot {}", path.display()))
        .map_err(to_napi_err)?;
    let snapshot: SnapshotFile = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid snapshot format in {}", path.display()))
        .map_err(to_napi_err)?;
    if snapshot.schema_version != 1 {
        return Err(to_napi_err(anyhow!(
            "Unsupported snapshot schema version {}",
            snapshot.schema_version
        )));
    }
    save_state(&state_path, &snapshot.state).map_err(to_napi_err)?;

    Ok(serde_json::json!({
        "success": true,
        "name": name,
        "state_file": state_path.display().to_string(),
    }))
}

/// List available snapshots (CLI parity for `snapshot list`).
#[napi]
pub fn snapshot_list() -> napi::Result<serde_json::Value> {
    let dir = default_snapshot_dir();
    if !dir.exists() {
        return Ok(serde_json::json!([]));
    }
    let mut items = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to list snapshots in {}", dir.display()))
        .map_err(to_napi_err)?
    {
        let entry = entry.map_err(|e| to_napi_err(anyhow!(e)))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let snapshot: SnapshotFile = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        items.push(serde_json::json!({
            "name": snapshot.name,
            "path": path.display().to_string(),
            "created_at": snapshot.created_at,
            "description": snapshot.description,
        }));
    }
    items.sort_by(|a, b| {
        let aa = a
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let bb = b
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        aa.cmp(bb)
    });
    Ok(serde_json::Value::Array(items))
}

/// Delete a snapshot by name (CLI parity for `snapshot delete`).
#[napi]
pub fn snapshot_delete(name: String) -> napi::Result<serde_json::Value> {
    let path = snapshot_path(&name);
    if !path.exists() {
        return Err(to_napi_err(anyhow!("Snapshot '{}' not found", name)));
    }
    std::fs::remove_file(&path)
        .with_context(|| format!("Failed to delete snapshot {}", path.display()))
        .map_err(to_napi_err)?;
    Ok(serde_json::json!({
        "success": true,
        "name": name,
    }))
}
