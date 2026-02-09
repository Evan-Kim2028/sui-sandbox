//! Snapshot lifecycle commands.

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use super::network::sandbox_home;
use super::SandboxState;
use sui_sandbox_core::simulation::PersistentState;

#[derive(Parser, Debug)]
pub struct SnapshotCmd {
    #[command(subcommand)]
    command: SnapshotSubcommand,
}

#[derive(Subcommand, Debug)]
enum SnapshotSubcommand {
    /// Save a named snapshot of current session state
    Save {
        /// Snapshot name
        name: String,
        /// Optional snapshot description
        #[arg(long)]
        description: Option<String>,
    },
    /// Load a previously saved snapshot by name
    Load {
        /// Snapshot name
        name: String,
    },
    /// List available snapshots
    List,
    /// Delete a snapshot by name
    Delete {
        /// Snapshot name
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotFile {
    schema_version: u32,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    created_at: String,
    state: PersistentState,
}

#[derive(Debug, Serialize)]
struct SnapshotListItem {
    name: String,
    path: String,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

impl SnapshotCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        state_file: &Path,
        json_output: bool,
    ) -> Result<()> {
        match &self.command {
            SnapshotSubcommand::Save { name, description } => {
                save_snapshot(state, name, description.clone(), json_output)
            }
            SnapshotSubcommand::Load { name } => {
                load_snapshot(state, state_file, name, json_output)
            }
            SnapshotSubcommand::List => list_snapshots(json_output),
            SnapshotSubcommand::Delete { name } => delete_snapshot(name, json_output),
        }
    }
}

fn save_snapshot(
    state: &mut SandboxState,
    name: &str,
    description: Option<String>,
    json_output: bool,
) -> Result<()> {
    let path = snapshot_path(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create snapshot directory {}", parent.display()))?;
    }

    let snapshot = SnapshotFile {
        schema_version: 1,
        name: name.to_string(),
        description,
        created_at: chrono::Utc::now().to_rfc3339(),
        state: state.snapshot_state(),
    };

    let data = serde_json::to_string_pretty(&snapshot)?;
    fs::write(&path, data)
        .with_context(|| format!("Failed to write snapshot {}", path.display()))?;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "success": true,
                "name": name,
                "path": path.display().to_string(),
            }))?
        );
    } else {
        println!("Saved snapshot '{}' at {}", name, path.display());
    }

    Ok(())
}

fn load_snapshot(
    state: &mut SandboxState,
    state_file: &Path,
    name: &str,
    json_output: bool,
) -> Result<()> {
    let path = snapshot_path(name);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read snapshot {}", path.display()))?;
    let snapshot: SnapshotFile = serde_json::from_str(&raw)
        .with_context(|| format!("Invalid snapshot format in {}", path.display()))?;

    if snapshot.schema_version != 1 {
        return Err(anyhow!(
            "Unsupported snapshot schema version {}",
            snapshot.schema_version
        ));
    }

    state.replace_persistent_state(snapshot.state)?;
    state.mark_dirty();

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "success": true,
                "name": name,
                "state_file": state_file.display().to_string(),
            }))?
        );
    } else {
        println!("Loaded snapshot '{}'", name);
    }

    Ok(())
}

fn list_snapshots(json_output: bool) -> Result<()> {
    let dir = snapshot_dir();
    if !dir.exists() {
        if json_output {
            println!("[]");
        } else {
            println!("No snapshots found");
        }
        return Ok(());
    }

    let mut items: Vec<SnapshotListItem> = Vec::new();

    for entry in fs::read_dir(&dir)
        .with_context(|| format!("Failed to list snapshots in {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = match fs::read_to_string(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let snapshot: SnapshotFile = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        items.push(SnapshotListItem {
            name: snapshot.name,
            path: path.display().to_string(),
            created_at: snapshot.created_at,
            description: snapshot.description,
        });
    }

    items.sort_by(|a, b| a.name.cmp(&b.name));

    if json_output {
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else if items.is_empty() {
        println!("No snapshots found");
    } else {
        println!("Snapshots:");
        for item in items {
            if let Some(desc) = item.description.as_ref() {
                println!("  - {} ({})", item.name, desc);
            } else {
                println!("  - {}", item.name);
            }
            println!("    created: {}", item.created_at);
            println!("    path: {}", item.path);
        }
    }

    Ok(())
}

fn delete_snapshot(name: &str, json_output: bool) -> Result<()> {
    let path = snapshot_path(name);
    if !path.exists() {
        return Err(anyhow!("Snapshot '{}' not found", name));
    }

    fs::remove_file(&path)
        .with_context(|| format!("Failed to delete snapshot {}", path.display()))?;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "success": true,
                "name": name,
            }))?
        );
    } else {
        println!("Deleted snapshot '{}'", name);
    }

    Ok(())
}

fn snapshot_dir() -> PathBuf {
    sandbox_home().join("snapshots")
}

fn snapshot_path(name: &str) -> PathBuf {
    snapshot_dir().join(format!("{}.json", sanitize_snapshot_name(name)))
}

fn sanitize_snapshot_name(name: &str) -> String {
    let filtered: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if filtered.is_empty() {
        "snapshot".to_string()
    } else {
        filtered
    }
}
