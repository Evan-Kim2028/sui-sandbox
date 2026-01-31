use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SandboxPaths {
    base: PathBuf,
}

impl SandboxPaths {
    pub fn from_base(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    pub fn base_dir(&self) -> PathBuf {
        self.base.clone()
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.base.join("cache")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.base.join("projects")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.base.join("logs").join("mcp")
    }
}

pub fn default_paths() -> SandboxPaths {
    let base = std::env::var("SUI_SANDBOX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".sui-sandbox")
        });
    SandboxPaths::from_base(base)
}
