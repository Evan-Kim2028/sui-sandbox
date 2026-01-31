use std::path::PathBuf;

use sui_sandbox_mcp::paths::SandboxPaths;

#[test]
fn derives_paths_from_base() {
    let base = PathBuf::from("/tmp/sui-sandbox-paths");
    let paths = SandboxPaths::from_base(base.clone());

    assert_eq!(paths.base_dir(), base);
    assert_eq!(
        paths.cache_dir(),
        PathBuf::from("/tmp/sui-sandbox-paths/cache")
    );
    assert_eq!(
        paths.projects_dir(),
        PathBuf::from("/tmp/sui-sandbox-paths/projects")
    );
    assert_eq!(
        paths.logs_dir(),
        PathBuf::from("/tmp/sui-sandbox-paths/logs/mcp")
    );
}
