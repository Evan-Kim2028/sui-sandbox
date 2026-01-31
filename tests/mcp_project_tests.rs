use std::fs;
use std::path::PathBuf;

use std::sync::{Mutex, OnceLock};
use tempfile::TempDir;

use sui_sandbox_mcp::paths::SandboxPaths;
use sui_sandbox_mcp::project::ProjectManager;

#[test]
fn creates_project_without_sui_dependency() {
    let temp = TempDir::new().expect("tempdir");
    let paths = SandboxPaths::from_base(temp.path().join("root"));
    let manager = ProjectManager::new(Some(paths.projects_dir())).expect("project manager");

    let (info, files) = manager
        .create_project("demo", None, Vec::new(), false)
        .expect("create project");

    assert!(files.contains(&"Move.toml".to_string()));
    assert!(files.iter().any(|f| f.ends_with("sources/demo.move")));

    let move_toml =
        fs::read_to_string(PathBuf::from(&info.path).join("Move.toml")).expect("read Move.toml");
    assert!(move_toml.contains("name = \"demo\""));
    assert!(!move_toml.contains("Sui = { local ="));
}

#[test]
fn creates_project_with_sui_dependency_and_tracks_packages() {
    let temp = TempDir::new().expect("tempdir");
    let paths = SandboxPaths::from_base(temp.path().join("root"));
    let manager = ProjectManager::new(Some(paths.projects_dir())).expect("project manager");

    let framework_dir = temp.path().join("framework");
    fs::create_dir_all(&framework_dir).expect("framework dir");
    let _guard = env_lock().lock().expect("env lock");
    let previous = std::env::var("SUI_FRAMEWORK_PATH").ok();
    std::env::set_var("SUI_FRAMEWORK_PATH", &framework_dir);

    let (info, _) = manager
        .create_project("with_sui", None, vec!["sui".to_string()], true)
        .expect("create project");

    let move_toml =
        fs::read_to_string(PathBuf::from(&info.path).join("Move.toml")).expect("read Move.toml");
    assert!(move_toml.contains("Sui = { local ="));

    let updated = manager
        .register_package(&info.id, "0x123")
        .expect("register package");
    assert_eq!(updated.active_package.as_deref(), Some("0x123"));
    assert_eq!(updated.packages.len(), 1);

    if let Some(value) = previous {
        std::env::set_var("SUI_FRAMEWORK_PATH", value);
    } else {
        std::env::remove_var("SUI_FRAMEWORK_PATH");
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
