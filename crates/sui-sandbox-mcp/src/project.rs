use anyhow::{anyhow, Result};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use sui_sandbox_core::package_builder::FrameworkCache;

use crate::paths::default_paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub persisted: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_package: Option<String>,
    #[serde(default)]
    pub packages: Vec<ProjectPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectPackage {
    pub package_id: String,
    pub deployed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProjectRegistry {
    version: u32,
    projects: Vec<ProjectInfo>,
}

pub struct ProjectManager {
    root: PathBuf,
    registry_path: PathBuf,
    projects: Mutex<HashMap<String, ProjectInfo>>,
}

impl ProjectManager {
    pub fn new(root: Option<PathBuf>) -> Result<Self> {
        let root = root.unwrap_or_else(|| default_paths().projects_dir());
        fs::create_dir_all(&root)?;
        let registry_path = root.join("projects.json");
        let projects = load_registry(&registry_path)?;
        Ok(Self {
            root,
            registry_path,
            projects: Mutex::new(projects),
        })
    }

    pub fn list_projects(&self) -> Vec<ProjectInfo> {
        self.projects.lock().values().cloned().collect()
    }

    pub fn get_project(&self, project_id: &str) -> Option<ProjectInfo> {
        self.projects.lock().get(project_id).cloned()
    }

    pub fn project_path(&self, project_id: &str) -> Result<PathBuf> {
        let info = self
            .projects
            .lock()
            .get(project_id)
            .cloned()
            .ok_or_else(|| anyhow!("Unknown project_id: {}", project_id))?;
        Ok(PathBuf::from(info.path))
    }

    pub fn create_project(
        &self,
        name: &str,
        initial_module: Option<&str>,
        dependencies: Vec<String>,
        persist: bool,
    ) -> Result<(ProjectInfo, Vec<String>)> {
        validate_project_name(name)?;

        let id = Uuid::new_v4().to_string();
        let base_dir = if persist {
            self.root.join("persisted")
        } else {
            self.root.join("_scratch")
        };
        fs::create_dir_all(&base_dir)?;

        let dir_name = format!("{}-{}", name, &id[..8]);
        let project_path = base_dir.join(dir_name);
        fs::create_dir_all(project_path.join("sources"))?;

        let mut created_files = Vec::new();

        let move_toml = render_move_toml(name, &dependencies)?;
        let move_toml_path = project_path.join("Move.toml");
        fs::write(&move_toml_path, move_toml)?;
        created_files.push("Move.toml".to_string());

        let module_name = name;
        let source_path = project_path
            .join("sources")
            .join(format!("{}.move", module_name));
        let source = initial_module
            .map(|s| s.to_string())
            .unwrap_or_else(|| default_module_template(name, module_name));
        fs::write(&source_path, source)?;
        created_files.push(format!("sources/{}.move", module_name));

        let now = Utc::now().to_rfc3339();
        let info = ProjectInfo {
            id: id.clone(),
            name: name.to_string(),
            path: project_path.to_string_lossy().to_string(),
            persisted: persist,
            created_at: now.clone(),
            updated_at: now,
            dependencies,
            active_package: None,
            packages: Vec::new(),
        };

        self.projects.lock().insert(id.clone(), info.clone());
        self.save_registry()?;

        Ok((info, created_files))
    }

    pub fn update_project(&self, info: &ProjectInfo) -> Result<()> {
        self.projects.lock().insert(info.id.clone(), info.clone());
        self.save_registry()
    }

    pub fn register_package(&self, project_id: &str, package_id: &str) -> Result<ProjectInfo> {
        let mut guard = self.projects.lock();
        let info = guard
            .get(project_id)
            .cloned()
            .ok_or_else(|| anyhow!("Unknown project_id: {}", project_id))?;
        let mut updated = info.clone();
        let now = Utc::now().to_rfc3339();
        updated.updated_at = now.clone();
        updated.active_package = Some(package_id.to_string());
        updated.packages.push(ProjectPackage {
            package_id: package_id.to_string(),
            deployed_at: now,
            notes: None,
        });
        guard.insert(project_id.to_string(), updated.clone());
        drop(guard);
        self.save_registry()?;
        Ok(updated)
    }

    pub fn set_active_package(&self, project_id: &str, package_id: &str) -> Result<ProjectInfo> {
        let mut guard = self.projects.lock();
        let info = guard
            .get(project_id)
            .cloned()
            .ok_or_else(|| anyhow!("Unknown project_id: {}", project_id))?;
        let mut updated = info.clone();
        updated.active_package = Some(package_id.to_string());
        updated.updated_at = Utc::now().to_rfc3339();
        guard.insert(project_id.to_string(), updated.clone());
        drop(guard);
        self.save_registry()?;
        Ok(updated)
    }

    pub fn touch(&self, project_id: &str) -> Result<()> {
        let mut guard = self.projects.lock();
        if let Some(info) = guard.get_mut(project_id) {
            info.updated_at = Utc::now().to_rfc3339();
        }
        drop(guard);
        self.save_registry()
    }

    fn save_registry(&self) -> Result<()> {
        let projects: Vec<ProjectInfo> = self.projects.lock().values().cloned().collect();
        let registry = ProjectRegistry {
            version: 1,
            projects,
        };
        let json = serde_json::to_string_pretty(&registry)?;
        fs::write(&self.registry_path, json)?;
        Ok(())
    }
}

fn load_registry(path: &Path) -> Result<HashMap<String, ProjectInfo>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let data = fs::read_to_string(path)?;
    let registry: ProjectRegistry = serde_json::from_str(&data)?;
    let map = registry
        .projects
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect();
    Ok(map)
}

fn validate_project_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(anyhow!("Project name cannot be empty"));
    };
    if !first.is_ascii_lowercase() {
        return Err(anyhow!(
            "Project name must start with a lowercase letter: {}",
            name
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(anyhow!(
            "Project name must contain only lowercase letters, digits, or underscores: {}",
            name
        ));
    }
    Ok(())
}

fn render_move_toml(name: &str, dependencies: &[String]) -> Result<String> {
    let mut lines = Vec::new();
    lines.push("[package]".to_string());
    lines.push(format!("name = \"{}\"", name));
    lines.push("edition = \"2024.beta\"".to_string());
    lines.push("version = \"0.0.1\"".to_string());
    lines.push(String::new());

    lines.push("[dependencies]".to_string());
    if dependencies
        .iter()
        .any(|dep| dep.eq_ignore_ascii_case("sui"))
    {
        let framework = resolve_sui_framework_path()?;
        let path = framework.to_string_lossy().replace('\\', "/");
        lines.push(format!("Sui = {{ local = \"{}\" }}", path));
    }
    lines.push(String::new());

    lines.push("[addresses]".to_string());
    lines.push(format!("{} = \"0x0\"", name));
    lines.push(String::new());

    Ok(lines.join("\n"))
}

fn resolve_sui_framework_path() -> Result<PathBuf> {
    if let Some(path) = find_sui_framework_path() {
        return Ok(path);
    }

    let cache = FrameworkCache::new()?;
    cache.ensure_cached()?;
    Ok(cache.sui_framework_path())
}

fn find_sui_framework_path() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("SUI_FRAMEWORK_PATH") {
        let path = PathBuf::from(env_path);
        if path.exists() {
            return Some(path);
        }
    }

    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("sui-official/crates/sui-framework/packages/sui-framework");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn default_module_template(package_name: &str, module_name: &str) -> String {
    format!(
        "module {pkg}::{mod_name} {{\n    public fun add(a: u64, b: u64): u64 {{\n        a + b\n    }}\n}}\n",
        pkg = package_name,
        mod_name = module_name
    )
}
