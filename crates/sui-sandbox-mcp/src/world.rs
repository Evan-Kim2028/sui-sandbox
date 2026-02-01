//! World management for sui-sandbox.
//!
//! A **World** is the unified container for all sandbox development. It ties together:
//! - Source code (Move packages)
//! - Deployed state (packages, objects)
//! - History (git commits, deployments, snapshots)
//! - Configuration (network, sender, etc.)
//!
//! Think of a World as an entire DeFi ecosystem you're building/simulating,
//! not just a single package.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

use crate::paths::default_paths;
use crate::transaction_history::{
    HistoryConfig, HistorySummary, SearchCriteria, TransactionHistory, TransactionRecord,
};

// ============================================================================
// Session Tracking
// ============================================================================

/// Session information persisted across MCP restarts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// ID of the currently active world (if any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_world: Option<String>,
    /// Timestamp of last activity
    pub last_activity: DateTime<Utc>,
    /// Optional window state for IDE integration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_state: Option<WindowState>,
    /// Version for future compatibility
    #[serde(default = "session_version")]
    pub version: u32,
}

fn session_version() -> u32 {
    1
}

impl Default for Session {
    fn default() -> Self {
        Self {
            active_world: None,
            last_activity: Utc::now(),
            window_state: None,
            version: 1,
        }
    }
}

/// Window state for IDE integration (optional)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    /// Last file being edited
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_file: Option<String>,
    /// Cursor position (line, column)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_position: Option<(u32, u32)>,
}

/// Manages session persistence
pub struct SessionManager {
    session_path: PathBuf,
    session: Mutex<Session>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(base_dir: &Path) -> Result<Self> {
        let session_path = base_dir.join("session.json");
        let session = Self::load_session(&session_path)?;

        Ok(Self {
            session_path,
            session: Mutex::new(session),
        })
    }

    /// Load session from disk, or create default
    fn load_session(path: &Path) -> Result<Session> {
        if path.exists() {
            let data = fs::read_to_string(path)?;
            match serde_json::from_str(&data) {
                Ok(session) => Ok(session),
                Err(e) => {
                    // Log warning but don't fail - just use default
                    eprintln!("Warning: failed to parse session.json: {}", e);
                    Ok(Session::default())
                }
            }
        } else {
            Ok(Session::default())
        }
    }

    /// Save session to disk
    pub fn save(&self) -> Result<()> {
        let session = self.session.lock();
        let json = serde_json::to_string_pretty(&*session)?;
        fs::write(&self.session_path, json)?;
        Ok(())
    }

    /// Get the active world ID from the session
    pub fn active_world(&self) -> Option<String> {
        self.session.lock().active_world.clone()
    }

    /// Set the active world
    pub fn set_active_world(&self, world_id: Option<String>) -> Result<()> {
        {
            let mut session = self.session.lock();
            session.active_world = world_id;
            session.last_activity = Utc::now();
        }
        self.save()
    }

    /// Update last activity timestamp
    pub fn touch(&self) -> Result<()> {
        {
            let mut session = self.session.lock();
            session.last_activity = Utc::now();
        }
        self.save()
    }

    /// Get the full session state
    pub fn get_session(&self) -> Session {
        self.session.lock().clone()
    }

    /// Update window state
    pub fn set_window_state(&self, state: Option<WindowState>) -> Result<()> {
        {
            let mut session = self.session.lock();
            session.window_state = state;
            session.last_activity = Utc::now();
        }
        self.save()
    }
}

// ============================================================================
// Git Helper Functions
// ============================================================================

/// Initialize a git repository in the given directory
fn git_init(path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git init failed: {}", stderr));
    }
    Ok(())
}

/// Create a .gitignore file for a world
fn create_gitignore(path: &Path) -> Result<()> {
    let gitignore = r#"# Sui Sandbox World
# Ignore state and build artifacts

# State files (these are large and can be recreated)
state/
*.state

# Build output
build/

# Lock files
Move.lock

# OS files
.DS_Store
Thumbs.db
"#;
    fs::write(path.join(".gitignore"), gitignore)?;
    Ok(())
}

/// Stage files and create a commit
fn git_commit(path: &Path, message: &str) -> Result<Option<String>> {
    // Stage all changes
    let add_output = Command::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .output()?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        return Err(anyhow!("git add failed: {}", stderr));
    }

    // Check if there are changes to commit
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()?;

    let status = String::from_utf8_lossy(&status_output.stdout);
    if status.trim().is_empty() {
        // Nothing to commit
        return Ok(None);
    }

    // Create commit
    let commit_output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(path)
        .output()?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        // Check if it's just "nothing to commit"
        if stderr.contains("nothing to commit") {
            return Ok(None);
        }
        return Err(anyhow!("git commit failed: {}", stderr));
    }

    // Get the commit hash
    let hash_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()?;

    let hash = String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string();

    Ok(Some(hash))
}

/// Create a git tag
fn git_tag(path: &Path, tag_name: &str, message: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["tag", "-a", tag_name, "-m", message])
        .current_dir(path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git tag failed: {}", stderr));
    }
    Ok(())
}

/// Get the current commit hash
fn git_current_hash(path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()?;

    if !output.status.success() {
        return Ok(None);
    }

    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if hash.is_empty() {
        Ok(None)
    } else {
        Ok(Some(hash))
    }
}

/// Get git log entries
fn git_log(path: &Path, limit: usize) -> Result<Vec<GitLogEntry>> {
    let output = Command::new("git")
        .args([
            "log",
            &format!("-{}", limit),
            "--pretty=format:%H|%s|%an|%aI",
        ])
        .current_dir(path)
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() >= 4 {
                Some(GitLogEntry {
                    hash: parts[0].to_string(),
                    message: parts[1].to_string(),
                    author: parts[2].to_string(),
                    date: parts[3].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(entries)
}

/// Get git status
fn git_status(path: &Path) -> Result<GitStatus> {
    // Get current branch
    let branch_output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(path)
        .output()?;

    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    // Get status
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()?;

    let status_str = String::from_utf8_lossy(&status_output.stdout);
    let uncommitted_changes = !status_str.trim().is_empty();

    // Get last commit
    let log_output = Command::new("git")
        .args(["log", "-1", "--pretty=format:%H %s"])
        .current_dir(path)
        .output()?;

    let last_commit = if log_output.status.success() {
        let s = String::from_utf8_lossy(&log_output.stdout).to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    } else {
        None
    };

    Ok(GitStatus {
        branch: if branch.is_empty() {
            "main".to_string()
        } else {
            branch
        },
        uncommitted_changes,
        last_commit,
    })
}

/// Check if a path is a git repository
fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Git log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLogEntry {
    pub hash: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

/// Git status summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: String,
    pub uncommitted_changes: bool,
    pub last_commit: Option<String>,
}

/// Network target for a world
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    #[default]
    Local,
    Mainnet,
    Testnet,
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Network::Local => write!(f, "local"),
            Network::Mainnet => write!(f, "mainnet"),
            Network::Testnet => write!(f, "testnet"),
        }
    }
}

/// Configuration for a world
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldConfig {
    /// Target network for fetching/forking
    pub network: Network,
    /// Default sender address for transactions
    pub default_sender: String,
    /// Automatically commit on successful build
    #[serde(default)]
    pub auto_commit: bool,
    /// Automatically snapshot on deploy
    #[serde(default = "default_true")]
    pub auto_snapshot: bool,
}

fn default_true() -> bool {
    true
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            network: Network::Local,
            default_sender: "0x0".to_string(),
            auto_commit: false,
            auto_snapshot: true,
        }
    }
}

/// Record of a package deployment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    /// On-chain package address
    pub package_id: String,
    /// Git commit hash at deploy time (if git enabled)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
    /// When the deployment occurred
    pub deployed_at: DateTime<Utc>,
    /// State snapshot name created for this deployment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    /// Module names in this deployment
    #[serde(default)]
    pub modules: Vec<String>,
    /// Optional notes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Information about a state snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Snapshot name (user-provided or auto-generated)
    pub name: String,
    /// When the snapshot was created
    pub created_at: DateTime<Utc>,
    /// Optional description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Git commit hash at snapshot time
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
    /// File path to the snapshot state
    pub state_file: String,
}

/// Fork manifest - tracks what was forked into this world
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForkManifest {
    /// Packages forked from mainnet/testnet
    #[serde(default)]
    pub packages: Vec<ForkedPackage>,
    /// Objects forked from mainnet/testnet
    #[serde(default)]
    pub objects: Vec<ForkedObject>,
    /// If forked from a transaction, the digest
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_transaction: Option<String>,
    /// If forked from a checkpoint, the sequence number
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_checkpoint: Option<u64>,
}

/// A package forked into this world
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkedPackage {
    /// Original package ID on source network
    pub original_id: String,
    /// Version that was forked
    pub version: u64,
    /// Local alias for this package
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Network it was forked from
    pub source_network: Network,
    /// When it was forked
    pub forked_at: DateTime<Utc>,
}

/// An object forked into this world
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkedObject {
    /// Original object ID
    pub object_id: String,
    /// Version that was forked
    pub version: u64,
    /// Object type
    pub object_type: String,
    /// Network it was forked from
    pub source_network: Network,
    /// When it was forked
    pub forked_at: DateTime<Utc>,
}

/// A World - the unified container for sandbox development
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct World {
    /// Unique identifier
    pub id: String,
    /// Human-readable name (lowercase_underscore)
    pub name: String,
    /// Filesystem path to world directory
    pub path: String,
    /// When the world was created
    pub created_at: DateTime<Utc>,
    /// When the world was last modified
    pub updated_at: DateTime<Utc>,
    /// Optional description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// World configuration
    pub config: WorldConfig,
    /// Deployment history
    #[serde(default)]
    pub deployments: Vec<Deployment>,
    /// Available snapshots
    #[serde(default)]
    pub snapshots: Vec<SnapshotInfo>,
    /// Fork manifest (if this world was forked from mainnet state)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_manifest: Option<ForkManifest>,
    /// Currently active snapshot (if restored)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_snapshot: Option<String>,
}

impl World {
    /// Get the path to the sources directory
    pub fn sources_dir(&self) -> PathBuf {
        PathBuf::from(&self.path).join("sources")
    }

    /// Get the path to the state directory
    pub fn state_dir(&self) -> PathBuf {
        PathBuf::from(&self.path).join("state")
    }

    /// Get the path to the snapshots directory
    pub fn snapshots_dir(&self) -> PathBuf {
        PathBuf::from(&self.path).join("snapshots")
    }

    /// Get the path to the current state file
    pub fn current_state_path(&self) -> PathBuf {
        self.state_dir().join("current.state")
    }

    /// Get the path to Move.toml
    pub fn move_toml_path(&self) -> PathBuf {
        PathBuf::from(&self.path).join("Move.toml")
    }

    /// Get the latest deployment, if any
    pub fn latest_deployment(&self) -> Option<&Deployment> {
        self.deployments.last()
    }

    /// Get deployment count
    pub fn deployment_count(&self) -> usize {
        self.deployments.len()
    }
}

/// Summary information about a world (for listing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldSummary {
    pub id: String,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub deployment_count: usize,
    pub snapshot_count: usize,
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl From<&World> for WorldSummary {
    fn from(world: &World) -> Self {
        Self {
            id: world.id.clone(),
            name: world.name.clone(),
            updated_at: world.updated_at,
            deployment_count: world.deployments.len(),
            snapshot_count: world.snapshots.len(),
            is_active: false, // Set by WorldManager
            description: world.description.clone(),
        }
    }
}

/// Registry of all worlds
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WorldRegistry {
    version: u32,
    worlds: Vec<World>,
}

/// Manager for world lifecycle operations
pub struct WorldManager {
    /// Root directory for worlds (~/.sui-sandbox/worlds)
    root: PathBuf,
    /// Path to registry file
    registry_path: PathBuf,
    /// In-memory world cache
    worlds: Mutex<HashMap<String, World>>,
    /// Currently active world ID
    active_world_id: Mutex<Option<String>>,
    /// Session manager for persistence
    session: SessionManager,
    /// Transaction history for the active world
    active_history: Mutex<Option<TransactionHistory>>,
}

impl WorldManager {
    /// Create a new WorldManager with automatic session resume
    pub fn new(root: Option<PathBuf>) -> Result<Self> {
        let base_dir = default_paths().base_dir();
        let root = root.unwrap_or_else(|| base_dir.join("worlds"));
        fs::create_dir_all(&root)?;
        let registry_path = root.join("registry.json");
        let worlds = load_registry(&registry_path)?;
        let session = SessionManager::new(&base_dir)?;

        // Auto-resume: restore active world from session
        let active_world_id = session.active_world().and_then(|id| {
            if worlds.contains_key(&id) {
                Some(id)
            } else {
                // World no longer exists, clear from session
                let _ = session.set_active_world(None);
                None
            }
        });

        // Open transaction history for active world if any
        let active_history = if let Some(ref id) = active_world_id {
            if let Some(world) = worlds.get(id) {
                TransactionHistory::open(Path::new(&world.path)).ok()
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            root,
            registry_path,
            worlds: Mutex::new(worlds),
            active_world_id: Mutex::new(active_world_id.clone()),
            session,
            active_history: Mutex::new(active_history),
        })
    }

    /// Create a new world with optional template
    pub fn create(
        &self,
        name: &str,
        description: Option<String>,
        config: Option<WorldConfig>,
    ) -> Result<World> {
        self.create_with_template(name, description, config, WorldTemplate::Blank)
    }

    /// Create a new world with a specific template
    pub fn create_with_template(
        &self,
        name: &str,
        description: Option<String>,
        config: Option<WorldConfig>,
        template: WorldTemplate,
    ) -> Result<World> {
        validate_world_name(name)?;

        // Check for name collision
        {
            let worlds = self.worlds.lock();
            if worlds.values().any(|w| w.name == name) {
                return Err(anyhow!("World with name '{}' already exists", name));
            }
        }

        let id = Uuid::new_v4().to_string();
        let dir_name = format!("{}-{}", name, &id[..8]);
        let world_path = self.root.join(&dir_name);

        // Create directory structure
        fs::create_dir_all(world_path.join("sources"))?;
        fs::create_dir_all(world_path.join("state"))?;
        fs::create_dir_all(world_path.join("snapshots"))?;

        // Generate Move.toml
        let move_toml = generate_move_toml(name)?;
        fs::write(world_path.join("Move.toml"), move_toml)?;

        // Generate template files
        let template_files = generate_template_files(name, template);
        for (filename, content) in template_files {
            fs::write(world_path.join("sources").join(&filename), content)?;
        }

        let now = Utc::now();
        let world = World {
            id: id.clone(),
            name: name.to_string(),
            path: world_path.to_string_lossy().to_string(),
            created_at: now,
            updated_at: now,
            description,
            config: config.unwrap_or_default(),
            deployments: Vec::new(),
            snapshots: Vec::new(),
            fork_manifest: None,
            active_snapshot: None,
        };

        // Save world.json in world directory
        let world_json = serde_json::to_string_pretty(&world)?;
        fs::write(world_path.join("world.json"), world_json)?;

        // Initialize git repository
        git_init(&world_path)?;
        create_gitignore(&world_path)?;

        // Initial commit
        let _ = git_commit(
            &world_path,
            &format!("Initial commit: create world '{}'", name),
        );

        // Add to registry
        self.worlds.lock().insert(id.clone(), world.clone());
        self.save_registry()?;

        Ok(world)
    }

    /// Open an existing world by name or ID
    pub fn open(&self, name_or_id: &str) -> Result<World> {
        // Find the world ID and path first (separate scope to avoid borrow issues)
        let (world_id, world_path) = {
            let worlds = self.worlds.lock();

            // Try exact ID match first
            if let Some(world) = worlds.get(name_or_id) {
                Some((world.id.clone(), world.path.clone()))
            // Try name match
            } else if let Some(world) = worlds.values().find(|w| w.name == name_or_id) {
                Some((world.id.clone(), world.path.clone()))
            // Try partial ID match
            } else {
                worlds
                    .values()
                    .find(|w| w.id.starts_with(name_or_id))
                    .map(|world| (world.id.clone(), world.path.clone()))
            }
        }
        .ok_or_else(|| anyhow!("World not found: {}", name_or_id))?;

        // Set as active
        {
            let mut active = self.active_world_id.lock();
            *active = Some(world_id.clone());
        }

        // Open transaction history for this world
        {
            let mut history = self.active_history.lock();
            *history = TransactionHistory::open(Path::new(&world_path)).ok();
        }

        // Persist to session
        let _ = self.session.set_active_world(Some(world_id.clone()));

        // Return the world
        let worlds = self.worlds.lock();
        Ok(worlds.get(&world_id).cloned().unwrap())
    }

    /// Close the currently active world
    pub fn close(&self) -> Result<Option<World>> {
        let mut active = self.active_world_id.lock();
        if let Some(id) = active.take() {
            let worlds = self.worlds.lock();
            let world = worlds.get(&id).cloned();
            drop(worlds);
            drop(active);

            // Close transaction history
            {
                let mut history = self.active_history.lock();
                *history = None;
            }

            // Clear from session
            let _ = self.session.set_active_world(None);
            return Ok(world);
        }
        Ok(None)
    }

    /// Get the currently active world
    pub fn active(&self) -> Option<World> {
        let active = self.active_world_id.lock();
        if let Some(id) = active.as_ref() {
            let worlds = self.worlds.lock();
            return worlds.get(id).cloned();
        }
        None
    }

    /// Get the active world ID
    pub fn active_id(&self) -> Option<String> {
        self.active_world_id.lock().clone()
    }

    /// Set the active world by ID
    pub fn set_active(&self, world_id: &str) -> Result<()> {
        let worlds = self.worlds.lock();
        if !worlds.contains_key(world_id) {
            return Err(anyhow!("World not found: {}", world_id));
        }
        drop(worlds);

        let mut active = self.active_world_id.lock();
        *active = Some(world_id.to_string());
        drop(active);

        // Persist to session
        self.session.set_active_world(Some(world_id.to_string()))?;
        Ok(())
    }

    /// List all worlds
    pub fn list(&self) -> Vec<WorldSummary> {
        let worlds = self.worlds.lock();
        let active_id = self.active_world_id.lock().clone();

        worlds
            .values()
            .map(|w| {
                let mut summary = WorldSummary::from(w);
                summary.is_active = active_id.as_ref() == Some(&w.id);
                summary
            })
            .collect()
    }

    /// Get a world by ID
    pub fn get(&self, world_id: &str) -> Option<World> {
        self.worlds.lock().get(world_id).cloned()
    }

    /// Update a world
    pub fn update(&self, world: &World) -> Result<()> {
        let mut updated = world.clone();
        updated.updated_at = Utc::now();

        // Save world.json
        let world_json = serde_json::to_string_pretty(&updated)?;
        fs::write(PathBuf::from(&world.path).join("world.json"), world_json)?;

        // Update registry
        self.worlds.lock().insert(updated.id.clone(), updated);
        self.save_registry()?;

        Ok(())
    }

    /// Delete a world
    pub fn delete(&self, name_or_id: &str, force: bool) -> Result<()> {
        let world = self.open(name_or_id)?;

        // Check if it's the active world
        {
            let active = self.active_world_id.lock();
            if active.as_ref() == Some(&world.id) && !force {
                return Err(anyhow!(
                    "Cannot delete active world. Close it first or use force=true"
                ));
            }
        }

        // Remove from active if it was active
        {
            let mut active = self.active_world_id.lock();
            if active.as_ref() == Some(&world.id) {
                *active = None;
            }
        }

        // Remove directory
        if PathBuf::from(&world.path).exists() {
            fs::remove_dir_all(&world.path)?;
        }

        // Remove from registry
        self.worlds.lock().remove(&world.id);
        self.save_registry()?;

        Ok(())
    }

    /// Record a deployment for a world
    pub fn record_deployment(
        &self,
        world_id: &str,
        package_id: &str,
        modules: Vec<String>,
        notes: Option<String>,
    ) -> Result<Deployment> {
        let mut worlds = self.worlds.lock();
        let world = worlds
            .get_mut(world_id)
            .ok_or_else(|| anyhow!("World not found: {}", world_id))?;

        let world_path = PathBuf::from(&world.path);
        let deploy_number = world.deployments.len() + 1;

        // Get current git commit hash if available
        let commit_hash = git_current_hash(&world_path).ok().flatten();

        // Create git tag for deployment
        let tag_name = format!("deploy-v{}", deploy_number);
        let tag_message = format!(
            "Deploy v{}: package {} ({} modules)",
            deploy_number,
            package_id,
            modules.len()
        );
        let _ = git_tag(&world_path, &tag_name, &tag_message);

        let deployment = Deployment {
            package_id: package_id.to_string(),
            commit_hash,
            deployed_at: Utc::now(),
            snapshot: None,
            modules,
            notes,
        };

        world.deployments.push(deployment.clone());
        world.updated_at = Utc::now();

        // Save world.json
        let world_json = serde_json::to_string_pretty(&*world)?;
        fs::write(world_path.join("world.json"), &world_json)?;

        drop(worlds);
        self.save_registry()?;

        Ok(deployment)
    }

    /// Create a snapshot
    pub fn create_snapshot(
        &self,
        world_id: &str,
        name: &str,
        description: Option<String>,
        state_data: &[u8],
    ) -> Result<SnapshotInfo> {
        let mut worlds = self.worlds.lock();
        let world = worlds
            .get_mut(world_id)
            .ok_or_else(|| anyhow!("World not found: {}", world_id))?;

        // Check for name collision
        if world.snapshots.iter().any(|s| s.name == name) {
            return Err(anyhow!("Snapshot '{}' already exists", name));
        }

        let snapshots_dir = PathBuf::from(&world.path).join("snapshots");
        fs::create_dir_all(&snapshots_dir)?;

        let state_file = format!("{}.state", name);
        let state_path = snapshots_dir.join(&state_file);
        fs::write(&state_path, state_data)?;

        let snapshot = SnapshotInfo {
            name: name.to_string(),
            created_at: Utc::now(),
            description,
            commit_hash: None, // TODO: Get from git
            state_file,
        };

        world.snapshots.push(snapshot.clone());
        world.updated_at = Utc::now();

        // Save world.json
        let world_json = serde_json::to_string_pretty(&*world)?;
        fs::write(PathBuf::from(&world.path).join("world.json"), &world_json)?;

        drop(worlds);
        self.save_registry()?;

        Ok(snapshot)
    }

    /// List snapshots for a world
    pub fn list_snapshots(&self, world_id: &str) -> Result<Vec<SnapshotInfo>> {
        let worlds = self.worlds.lock();
        let world = worlds
            .get(world_id)
            .ok_or_else(|| anyhow!("World not found: {}", world_id))?;
        Ok(world.snapshots.clone())
    }

    /// Get snapshot data
    pub fn get_snapshot_data(&self, world_id: &str, snapshot_name: &str) -> Result<Vec<u8>> {
        let worlds = self.worlds.lock();
        let world = worlds
            .get(world_id)
            .ok_or_else(|| anyhow!("World not found: {}", world_id))?;

        let snapshot = world
            .snapshots
            .iter()
            .find(|s| s.name == snapshot_name)
            .ok_or_else(|| anyhow!("Snapshot not found: {}", snapshot_name))?;

        let state_path = PathBuf::from(&world.path)
            .join("snapshots")
            .join(&snapshot.state_file);

        fs::read(&state_path).map_err(|e| anyhow!("Failed to read snapshot: {}", e))
    }

    /// Get the world directory path
    pub fn world_path(&self, world_id: &str) -> Result<PathBuf> {
        let worlds = self.worlds.lock();
        let world = worlds
            .get(world_id)
            .ok_or_else(|| anyhow!("World not found: {}", world_id))?;
        Ok(PathBuf::from(&world.path))
    }

    /// Create a git commit in the world
    pub fn git_commit(&self, world_id: &str, message: &str) -> Result<Option<String>> {
        let world_path = self.world_path(world_id)?;

        if !is_git_repo(&world_path) {
            return Err(anyhow!("World is not a git repository"));
        }

        git_commit(&world_path, message)
    }

    /// Get git status for a world
    pub fn git_status(&self, world_id: &str) -> Result<GitStatus> {
        let world_path = self.world_path(world_id)?;

        if !is_git_repo(&world_path) {
            return Err(anyhow!("World is not a git repository"));
        }

        git_status(&world_path)
    }

    /// Get git log for a world
    pub fn git_log(&self, world_id: &str, limit: usize) -> Result<Vec<GitLogEntry>> {
        let world_path = self.world_path(world_id)?;

        if !is_git_repo(&world_path) {
            return Ok(Vec::new());
        }

        git_log(&world_path, limit)
    }

    /// Check if a world has git initialized
    pub fn has_git(&self, world_id: &str) -> bool {
        if let Ok(path) = self.world_path(world_id) {
            is_git_repo(&path)
        } else {
            false
        }
    }

    fn save_registry(&self) -> Result<()> {
        let worlds: Vec<World> = self.worlds.lock().values().cloned().collect();
        let registry = WorldRegistry { version: 1, worlds };
        let json = serde_json::to_string_pretty(&registry)?;
        fs::write(&self.registry_path, json)?;
        Ok(())
    }

    // ========================================================================
    // State Recovery Methods
    // ========================================================================

    /// Save simulation state to the active world's state directory
    /// Uses write-ahead log pattern for crash safety
    pub fn save_world_state(&self, world_id: &str, state_data: &[u8]) -> Result<()> {
        let world_path = self.world_path(world_id)?;
        let state_dir = world_path.join("state");
        fs::create_dir_all(&state_dir)?;

        let current_path = state_dir.join("current.state");
        let recovery_path = state_dir.join("recovery.state");

        // Write-ahead: write to recovery file first
        fs::write(&recovery_path, state_data)?;

        // Atomic rename (on most filesystems)
        if current_path.exists() {
            let backup_path = state_dir.join("previous.state");
            let _ = fs::rename(&current_path, &backup_path);
        }
        fs::rename(&recovery_path, &current_path)?;

        // Update last activity
        let _ = self.session.touch();

        Ok(())
    }

    /// Load simulation state from the active world
    pub fn load_world_state(&self, world_id: &str) -> Result<Option<Vec<u8>>> {
        let world_path = self.world_path(world_id)?;
        let state_dir = world_path.join("state");
        let current_path = state_dir.join("current.state");
        let recovery_path = state_dir.join("recovery.state");

        // Crash recovery: if recovery file exists but current doesn't,
        // it means we crashed during write - complete the write
        if recovery_path.exists() && !current_path.exists() {
            fs::rename(&recovery_path, &current_path)?;
        }

        if current_path.exists() {
            let data = fs::read(&current_path)?;
            if data.is_empty() {
                return Ok(None);
            }
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }

    /// Check if world has saved state
    pub fn has_saved_state(&self, world_id: &str) -> bool {
        if let Ok(path) = self.world_path(world_id) {
            path.join("state").join("current.state").exists()
        } else {
            false
        }
    }

    /// Get session manager reference
    pub fn session(&self) -> &SessionManager {
        &self.session
    }

    /// Get current session info
    pub fn get_session(&self) -> Session {
        self.session.get_session()
    }

    /// Perform cleanup on shutdown - saves session state
    pub fn shutdown(&self) -> Result<()> {
        self.session.save()
    }

    // ========================================================================
    // Transaction History Methods
    // ========================================================================

    /// Record a transaction in the active world's history
    pub fn record_transaction(&self, record: TransactionRecord) -> Result<()> {
        let mut history = self.active_history.lock();
        if let Some(ref mut h) = *history {
            h.record(record)?;
        }
        Ok(())
    }

    /// Get transaction history summary for the active world
    pub fn transaction_summary(&self) -> Option<HistorySummary> {
        let history = self.active_history.lock();
        history.as_ref().map(|h| h.summary())
    }

    /// Get recent transactions from the active world
    pub fn recent_transactions(&self, limit: usize) -> Vec<crate::transaction_history::IndexEntry> {
        let history = self.active_history.lock();
        history
            .as_ref()
            .map(|h| h.recent(limit))
            .unwrap_or_default()
    }

    /// Get a specific transaction by ID
    pub fn get_transaction(&self, tx_id: &str) -> Result<Option<TransactionRecord>> {
        let history = self.active_history.lock();
        match history.as_ref() {
            Some(h) => h.get_transaction(tx_id),
            None => Ok(None),
        }
    }

    /// Get a transaction by sequence number
    pub fn get_transaction_by_sequence(&self, sequence: u64) -> Result<Option<TransactionRecord>> {
        let history = self.active_history.lock();
        match history.as_ref() {
            Some(h) => h.get_by_sequence(sequence),
            None => Ok(None),
        }
    }

    /// List transactions with pagination
    pub fn list_transactions(&self, offset: u64, limit: usize) -> Result<Vec<TransactionRecord>> {
        let history = self.active_history.lock();
        match history.as_ref() {
            Some(h) => h.list(offset, limit),
            None => Ok(Vec::new()),
        }
    }

    /// Search transactions by criteria
    pub fn search_transactions(&self, criteria: &SearchCriteria) -> Result<Vec<TransactionRecord>> {
        let history = self.active_history.lock();
        match history.as_ref() {
            Some(h) => h.search(criteria),
            None => Ok(Vec::new()),
        }
    }

    /// Check if transaction history is enabled for the active world
    pub fn is_history_enabled(&self) -> bool {
        let history = self.active_history.lock();
        history.as_ref().map(|h| h.is_enabled()).unwrap_or(false)
    }

    /// Enable or disable transaction history for the active world
    pub fn set_history_enabled(&self, enabled: bool) -> Result<()> {
        let mut history = self.active_history.lock();
        if let Some(ref mut h) = *history {
            h.set_enabled(enabled)?;
        }
        Ok(())
    }

    /// Clear transaction history for the active world
    pub fn clear_transaction_history(&self) -> Result<()> {
        let mut history = self.active_history.lock();
        if let Some(ref mut h) = *history {
            h.clear()?;
        }
        Ok(())
    }

    /// Export transaction history to a file
    pub fn export_transaction_history(&self, output_path: &Path) -> Result<u64> {
        let history = self.active_history.lock();
        match history.as_ref() {
            Some(h) => h.export(output_path),
            None => Ok(0),
        }
    }

    /// Get history configuration for the active world
    pub fn history_config(&self) -> Option<HistoryConfig> {
        let history = self.active_history.lock();
        history.as_ref().map(|h| HistoryConfig {
            enabled: h.is_enabled(),
            ..Default::default()
        })
    }
}

fn load_registry(path: &Path) -> Result<HashMap<String, World>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let data = fs::read_to_string(path)?;
    let registry: WorldRegistry = serde_json::from_str(&data)?;
    let map = registry
        .worlds
        .into_iter()
        .map(|w| (w.id.clone(), w))
        .collect();
    Ok(map)
}

fn validate_world_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow!("World name cannot be empty"));
    }

    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(anyhow!(
            "World name must start with a lowercase letter: {}",
            name
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(anyhow!(
            "World name must contain only lowercase letters, digits, or underscores: {}",
            name
        ));
    }

    Ok(())
}

fn generate_move_toml(name: &str) -> Result<String> {
    use sui_sandbox_core::package_builder::FrameworkCache;

    let cache = FrameworkCache::new()?;
    cache.ensure_cached()?;
    let framework_path = cache.sui_framework_path();
    let path_str = framework_path.to_string_lossy().replace('\\', "/");

    Ok(format!(
        r#"[package]
name = "{name}"
edition = "2024.beta"
version = "0.0.1"

[dependencies]
Sui = {{ local = "{path}" }}

[addresses]
{name} = "0x0"
"#,
        name = name,
        path = path_str
    ))
}

/// Available world templates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorldTemplate {
    /// Minimal starter with just an example struct
    #[default]
    Blank,
    /// Fungible token (Coin) template
    Token,
    /// NFT collection template
    Nft,
    /// DeFi AMM/swap template
    Defi,
}

impl WorldTemplate {
    /// Get all available templates
    pub fn all() -> Vec<&'static str> {
        vec!["blank", "token", "nft", "defi"]
    }

    /// Parse from string name
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "blank" | "" => Some(Self::Blank),
            "token" | "coin" => Some(Self::Token),
            "nft" | "collection" => Some(Self::Nft),
            "defi" | "amm" | "swap" => Some(Self::Defi),
            _ => None,
        }
    }

    /// Get template description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Blank => "Minimal starter with an example struct",
            Self::Token => "Fungible token (Coin) with treasury cap",
            Self::Nft => "NFT collection with minting",
            Self::Defi => "AMM liquidity pool with swap functionality",
        }
    }
}

/// Generate module files for a template
fn generate_template_files(name: &str, template: WorldTemplate) -> Vec<(String, String)> {
    match template {
        WorldTemplate::Blank => vec![(format!("{}.move", name), generate_blank_module(name))],
        WorldTemplate::Token => vec![(format!("{}.move", name), generate_token_module(name))],
        WorldTemplate::Nft => vec![(format!("{}.move", name), generate_nft_module(name))],
        WorldTemplate::Defi => vec![
            (
                "coin_a.move".to_string(),
                generate_coin_module(name, "coin_a", "COIN_A", "Coin A"),
            ),
            (
                "coin_b.move".to_string(),
                generate_coin_module(name, "coin_b", "COIN_B", "Coin B"),
            ),
            ("pool.move".to_string(), generate_pool_module(name)),
        ],
    }
}

fn generate_blank_module(name: &str) -> String {
    format!(
        r#"module {name}::{name} {{
    use sui::object::{{Self, UID}};
    use sui::tx_context::TxContext;

    /// Example struct with key ability for on-chain storage
    public struct Example has key {{
        id: UID,
        value: u64,
    }}

    /// Create a new Example object
    public fun new(value: u64, ctx: &mut TxContext): Example {{
        Example {{
            id: object::new(ctx),
            value,
        }}
    }}

    /// Get the value
    public fun value(self: &Example): u64 {{
        self.value
    }}
}}
"#,
        name = name
    )
}

fn generate_token_module(name: &str) -> String {
    format!(
        r#"module {name}::{name} {{
    use sui::coin::{{Self, TreasuryCap}};
    use sui::url::Url;

    /// One-time witness for the token
    public struct {upper_name} has drop {{}}

    /// Initialize the token with metadata
    fun init(witness: {upper_name}, ctx: &mut TxContext) {{
        let (treasury_cap, metadata) = coin::create_currency(
            witness,
            9, // decimals
            b"{upper_name}",
            b"{name} Token",
            b"A token created with sui-sandbox",
            option::none<Url>(),
            ctx
        );

        // Transfer treasury cap to sender (they can mint)
        transfer::public_transfer(treasury_cap, tx_context::sender(ctx));
        // Make metadata publicly accessible
        transfer::public_share_object(metadata);
    }}

    /// Mint new tokens (requires treasury cap)
    public fun mint(
        treasury: &mut TreasuryCap<{upper_name}>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext
    ) {{
        let coin = coin::mint(treasury, amount, ctx);
        transfer::public_transfer(coin, recipient);
    }}

    /// Burn tokens
    public fun burn(
        treasury: &mut TreasuryCap<{upper_name}>,
        coin: coin::Coin<{upper_name}>
    ) {{
        coin::burn(treasury, coin);
    }}
}}
"#,
        name = name,
        upper_name = name.to_uppercase()
    )
}

fn generate_nft_module(name: &str) -> String {
    format!(
        r#"module {name}::{name} {{
    use sui::object::{{Self, UID}};
    use sui::tx_context::TxContext;
    use std::string::String;
    use sui::url::{{Self, Url}};
    use sui::event;

    /// The NFT struct
    public struct NFT has key, store {{
        id: UID,
        name: String,
        description: String,
        image_url: Url,
        creator: address,
    }}

    /// Event emitted when NFT is minted
    public struct NFTMinted has copy, drop {{
        id: ID,
        creator: address,
        name: String,
    }}

    /// Mint a new NFT
    public fun mint(
        name: String,
        description: String,
        image_url: vector<u8>,
        ctx: &mut TxContext
    ): NFT {{
        let sender = tx_context::sender(ctx);
        let nft = NFT {{
            id: object::new(ctx),
            name,
            description,
            image_url: url::new_unsafe_from_bytes(image_url),
            creator: sender,
        }};

        event::emit(NFTMinted {{
            id: object::id(&nft),
            creator: sender,
            name: nft.name,
        }});

        nft
    }}

    /// Transfer NFT to another address
    public fun transfer(nft: NFT, recipient: address) {{
        transfer::public_transfer(nft, recipient);
    }}

    /// Burn/destroy an NFT
    public fun burn(nft: NFT) {{
        let NFT {{ id, name: _, description: _, image_url: _, creator: _ }} = nft;
        object::delete(id);
    }}

    // === Accessors ===

    public fun name(nft: &NFT): &String {{ &nft.name }}
    public fun description(nft: &NFT): &String {{ &nft.description }}
    public fun image_url(nft: &NFT): &Url {{ &nft.image_url }}
    public fun creator(nft: &NFT): address {{ nft.creator }}
}}
"#,
        name = name
    )
}

fn generate_coin_module(pkg: &str, mod_name: &str, witness: &str, display_name: &str) -> String {
    format!(
        r#"module {pkg}::{mod_name} {{
    use sui::coin::{{Self, TreasuryCap}};
    use sui::url::Url;

    /// One-time witness
    public struct {witness} has drop {{}}

    fun init(witness: {witness}, ctx: &mut TxContext) {{
        let (treasury_cap, metadata) = coin::create_currency(
            witness,
            9,
            b"{witness}",
            b"{display_name}",
            b"Test coin for AMM",
            option::none<Url>(),
            ctx
        );
        transfer::public_transfer(treasury_cap, tx_context::sender(ctx));
        transfer::public_share_object(metadata);
    }}

    public fun mint(
        treasury: &mut TreasuryCap<{witness}>,
        amount: u64,
        recipient: address,
        ctx: &mut TxContext
    ) {{
        transfer::public_transfer(coin::mint(treasury, amount, ctx), recipient);
    }}
}}
"#,
        pkg = pkg,
        mod_name = mod_name,
        witness = witness,
        display_name = display_name
    )
}

fn generate_pool_module(name: &str) -> String {
    format!(
        r#"module {name}::pool {{
    use sui::object::{{Self, UID}};
    use sui::balance::{{Self, Balance}};
    use sui::coin::{{Self, Coin}};
    use sui::tx_context::TxContext;
    use {name}::coin_a::COIN_A;
    use {name}::coin_b::COIN_B;

    /// The liquidity pool
    public struct Pool has key {{
        id: UID,
        reserve_a: Balance<COIN_A>,
        reserve_b: Balance<COIN_B>,
        lp_supply: u64,
    }}

    /// LP token representing pool share
    public struct LPToken has key, store {{
        id: UID,
        amount: u64,
    }}

    /// Create a new empty pool
    public fun create_pool(ctx: &mut TxContext): Pool {{
        Pool {{
            id: object::new(ctx),
            reserve_a: balance::zero(),
            reserve_b: balance::zero(),
            lp_supply: 0,
        }}
    }}

    /// Add liquidity to the pool
    public fun add_liquidity(
        pool: &mut Pool,
        coin_a: Coin<COIN_A>,
        coin_b: Coin<COIN_B>,
        ctx: &mut TxContext
    ): LPToken {{
        let amount_a = coin::value(&coin_a);
        let amount_b = coin::value(&coin_b);

        balance::join(&mut pool.reserve_a, coin::into_balance(coin_a));
        balance::join(&mut pool.reserve_b, coin::into_balance(coin_b));

        // Simple LP calculation (sqrt of product)
        let lp_amount = sqrt(amount_a * amount_b);
        pool.lp_supply = pool.lp_supply + lp_amount;

        LPToken {{
            id: object::new(ctx),
            amount: lp_amount,
        }}
    }}

    /// Swap coin A for coin B
    public fun swap_a_for_b(
        pool: &mut Pool,
        coin_in: Coin<COIN_A>,
        ctx: &mut TxContext
    ): Coin<COIN_B> {{
        let amount_in = coin::value(&coin_in);
        let reserve_a = balance::value(&pool.reserve_a);
        let reserve_b = balance::value(&pool.reserve_b);

        // Constant product: (x + dx)(y - dy) = xy
        // dy = y * dx / (x + dx)
        let amount_out = (reserve_b * amount_in) / (reserve_a + amount_in);

        balance::join(&mut pool.reserve_a, coin::into_balance(coin_in));
        coin::from_balance(balance::split(&mut pool.reserve_b, amount_out), ctx)
    }}

    /// Swap coin B for coin A
    public fun swap_b_for_a(
        pool: &mut Pool,
        coin_in: Coin<COIN_B>,
        ctx: &mut TxContext
    ): Coin<COIN_A> {{
        let amount_in = coin::value(&coin_in);
        let reserve_a = balance::value(&pool.reserve_a);
        let reserve_b = balance::value(&pool.reserve_b);

        let amount_out = (reserve_a * amount_in) / (reserve_b + amount_in);

        balance::join(&mut pool.reserve_b, coin::into_balance(coin_in));
        coin::from_balance(balance::split(&mut pool.reserve_a, amount_out), ctx)
    }}

    /// Get pool reserves
    public fun reserves(pool: &Pool): (u64, u64) {{
        (balance::value(&pool.reserve_a), balance::value(&pool.reserve_b))
    }}

    /// Integer square root helper
    fun sqrt(x: u64): u64 {{
        if (x == 0) return 0;
        let mut z = (x + 1) / 2;
        let mut y = x;
        while (z < y) {{
            y = z;
            z = (x / z + z) / 2;
        }};
        y
    }}
}}
"#,
        name = name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_validate_world_name() {
        assert!(validate_world_name("my_world").is_ok());
        assert!(validate_world_name("world123").is_ok());
        assert!(validate_world_name("a").is_ok());

        assert!(validate_world_name("").is_err());
        assert!(validate_world_name("MyWorld").is_err());
        assert!(validate_world_name("123world").is_err());
        assert!(validate_world_name("my-world").is_err());
    }

    #[test]
    fn test_world_config_default() {
        let config = WorldConfig::default();
        assert_eq!(config.network, Network::Local);
        assert_eq!(config.default_sender, "0x0");
        assert!(!config.auto_commit);
        assert!(config.auto_snapshot);
    }

    #[test]
    fn test_session_default() {
        let session = Session::default();
        assert!(session.active_world.is_none());
        assert!(session.window_state.is_none());
        assert_eq!(session.version, 1);
    }

    #[test]
    fn test_session_manager_persistence() {
        let temp = TempDir::new().unwrap();
        let session_mgr = SessionManager::new(temp.path()).unwrap();

        // Initially no active world
        assert!(session_mgr.active_world().is_none());

        // Set active world
        session_mgr
            .set_active_world(Some("test-world-id".to_string()))
            .unwrap();
        assert_eq!(
            session_mgr.active_world(),
            Some("test-world-id".to_string())
        );

        // Create new session manager - should load from disk
        let session_mgr2 = SessionManager::new(temp.path()).unwrap();
        assert_eq!(
            session_mgr2.active_world(),
            Some("test-world-id".to_string())
        );

        // Clear active world
        session_mgr2.set_active_world(None).unwrap();
        let session_mgr3 = SessionManager::new(temp.path()).unwrap();
        assert!(session_mgr3.active_world().is_none());
    }
}
