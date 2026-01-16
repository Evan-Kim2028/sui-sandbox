//! Package building utilities for Move source compilation.
//!
//! This module provides tools to:
//! 1. Scaffold new Move packages (generate Move.toml, directory structure)
//! 2. Compile Move source to bytecode using sui-move-build
//! 3. Handle compilation errors for LLM iteration
//!
//! These capabilities enable an LLM to write Move source code and
//! compile it to bytecode for deployment in the sandbox.
//!
//! ## Framework Caching
//!
//! The Sui framework is cached locally to avoid repeated downloads:
//! - Default cache: `~/.sui-framework-cache/mainnet-v1.62.1`
//! - Set `SUI_FRAMEWORK_PATH` env var to use a custom location
//! - First compilation will download if not cached

use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use sui_move_build::BuildConfig;

/// Sui framework version we're targeting
pub const FRAMEWORK_VERSION: &str = "mainnet-v1.62.1";

/// Default cache directory for framework
pub const DEFAULT_CACHE_DIR: &str = ".sui-framework-cache";

/// Framework cache manager
pub struct FrameworkCache {
    cache_dir: PathBuf,
}

impl FrameworkCache {
    /// Create a new framework cache with default location (~/.sui-framework-cache)
    pub fn new() -> Result<Self> {
        let cache_dir = if let Ok(path) = std::env::var("SUI_FRAMEWORK_PATH") {
            PathBuf::from(path)
        } else {
            dirs::home_dir()
                .ok_or_else(|| anyhow!("Could not determine home directory"))?
                .join(DEFAULT_CACHE_DIR)
                .join(FRAMEWORK_VERSION)
        };

        Ok(Self { cache_dir })
    }

    /// Create with a specific cache directory
    pub fn with_cache_dir(cache_dir: impl AsRef<Path>) -> Self {
        Self {
            cache_dir: cache_dir.as_ref().to_path_buf(),
        }
    }

    /// Check if framework is cached
    pub fn is_cached(&self) -> bool {
        self.sui_framework_path().join("Move.toml").exists()
    }

    /// Get path to sui-framework package
    pub fn sui_framework_path(&self) -> PathBuf {
        self.cache_dir.join("sui-framework")
    }

    /// Get path to move-stdlib package
    pub fn move_stdlib_path(&self) -> PathBuf {
        self.cache_dir.join("move-stdlib")
    }

    /// Ensure framework is downloaded and cached
    pub fn ensure_cached(&self) -> Result<()> {
        if self.is_cached() {
            return Ok(());
        }

        fs::create_dir_all(&self.cache_dir)?;

        // Clone the specific tag from Sui repo (sparse checkout for just framework)
        let git_url = "https://github.com/MystenLabs/sui.git";
        let temp_clone = self.cache_dir.join("_temp_clone");

        // Clean up any previous failed attempt
        let _ = fs::remove_dir_all(&temp_clone);

        // Clone with depth 1 for speed
        let status = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--branch",
                FRAMEWORK_VERSION,
                "--filter=blob:none",
                "--sparse",
                git_url,
                temp_clone
                    .to_str()
                    .ok_or_else(|| anyhow!("Temp path contains invalid UTF-8"))?,
            ])
            .status()?;

        if !status.success() {
            return Err(anyhow!("Failed to clone Sui repository"));
        }

        // Sparse checkout just the framework packages
        let status = Command::new("git")
            .current_dir(&temp_clone)
            .args([
                "sparse-checkout",
                "set",
                "crates/sui-framework/packages/sui-framework",
                "crates/sui-framework/packages/move-stdlib",
            ])
            .status()?;

        if !status.success() {
            let _ = fs::remove_dir_all(&temp_clone);
            return Err(anyhow!("Failed to sparse checkout framework"));
        }

        // Copy framework packages to cache
        let src_framework = temp_clone.join("crates/sui-framework/packages/sui-framework");
        let src_stdlib = temp_clone.join("crates/sui-framework/packages/move-stdlib");

        if src_framework.exists() {
            copy_dir_recursive(&src_framework, &self.sui_framework_path())?;
        }
        if src_stdlib.exists() {
            copy_dir_recursive(&src_stdlib, &self.move_stdlib_path())?;
        }

        // Clean up temp clone
        let _ = fs::remove_dir_all(&temp_clone);

        // Verify
        if !self.is_cached() {
            return Err(anyhow!("Framework download completed but files not found"));
        }

        Ok(())
    }

    /// Get the cache directory
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

// Note: Default is intentionally not implemented for FrameworkCache
// because FrameworkCache::new() can fail (e.g., if home directory is unavailable).
// Use FrameworkCache::new()? instead.

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Configuration for scaffolding a new Move package
#[derive(Debug, Clone)]
pub struct PackageConfig {
    /// Package name (used in Move.toml)
    pub name: String,
    /// Named addresses to declare (e.g., "my_package" -> None means "to be assigned")
    pub addresses: Vec<(String, Option<String>)>,
    /// Whether to include Sui framework dependency
    pub include_sui_framework: bool,
    /// Edition (defaults to "2024.beta")
    pub edition: Option<String>,
}

impl Default for PackageConfig {
    fn default() -> Self {
        Self {
            name: "my_package".to_string(),
            addresses: vec![("my_package".to_string(), None)],
            include_sui_framework: true,
            edition: Some("2024.beta".to_string()),
        }
    }
}

/// Result of compiling a Move package
#[derive(Debug)]
pub struct CompilationResult {
    /// Whether compilation succeeded
    pub success: bool,
    /// Compiled bytecode modules (module_name -> bytes)
    pub modules: Vec<(String, Vec<u8>)>,
    /// Compilation errors/warnings as human-readable text
    pub diagnostics: String,
    /// Package digest if compilation succeeded
    pub digest: Option<String>,
}

/// Builder for creating and compiling Move packages
pub struct PackageBuilder {
    /// Base directory for package operations
    work_dir: PathBuf,
    /// Framework cache for local dependencies
    framework_cache: Option<FrameworkCache>,
}

impl PackageBuilder {
    /// Create a new package builder with a working directory
    pub fn new(work_dir: impl AsRef<Path>) -> Result<Self> {
        let work_dir = work_dir.as_ref().to_path_buf();
        fs::create_dir_all(&work_dir)?;
        Ok(Self {
            work_dir,
            framework_cache: None,
        })
    }

    /// Create a new package builder with a temporary directory
    pub fn new_temp() -> Result<Self> {
        let temp_dir = std::env::temp_dir().join(format!("sui-pkg-{}", std::process::id()));
        fs::create_dir_all(&temp_dir)?;
        Ok(Self {
            work_dir: temp_dir,
            framework_cache: None,
        })
    }

    /// Create a new package builder with framework caching enabled
    pub fn with_framework_cache(work_dir: impl AsRef<Path>) -> Result<Self> {
        let work_dir = work_dir.as_ref().to_path_buf();
        fs::create_dir_all(&work_dir)?;
        let framework_cache = FrameworkCache::new()?;
        Ok(Self {
            work_dir,
            framework_cache: Some(framework_cache),
        })
    }

    /// Enable framework caching on an existing builder
    pub fn enable_framework_cache(&mut self) -> Result<()> {
        self.framework_cache = Some(FrameworkCache::new()?);
        Ok(())
    }

    /// Check if framework is cached (returns false if caching not enabled)
    pub fn is_framework_cached(&self) -> bool {
        self.framework_cache
            .as_ref()
            .map(|c| c.is_cached())
            .unwrap_or(false)
    }

    /// Ensure framework is downloaded (no-op if caching not enabled)
    pub fn ensure_framework(&self) -> Result<()> {
        if let Some(cache) = &self.framework_cache {
            cache.ensure_cached()?;
        }
        Ok(())
    }

    /// Scaffold a new Move package with the given configuration
    pub fn scaffold(&self, config: &PackageConfig) -> Result<PathBuf> {
        let package_dir = self.work_dir.join(&config.name);
        fs::create_dir_all(&package_dir)?;

        // Create sources directory
        let sources_dir = package_dir.join("sources");
        fs::create_dir_all(&sources_dir)?;

        // Generate Move.toml
        let move_toml = self.generate_move_toml(config)?;
        fs::write(package_dir.join("Move.toml"), move_toml)?;

        Ok(package_dir)
    }

    /// Generate Move.toml content from configuration
    fn generate_move_toml(&self, config: &PackageConfig) -> Result<String> {
        let mut toml = String::new();

        // [package] section
        toml.push_str("[package]\n");
        toml.push_str(&format!("name = \"{}\"\n", config.name));
        if let Some(edition) = &config.edition {
            toml.push_str(&format!("edition = \"{}\"\n", edition));
        }
        toml.push('\n');

        // [dependencies] section
        toml.push_str("[dependencies]\n");
        if config.include_sui_framework {
            // Use local path if framework is cached, otherwise fall back to git
            if let Some(cache) = &self.framework_cache {
                if cache.is_cached() {
                    let framework_path = cache.sui_framework_path();
                    toml.push_str(&format!(
                        "Sui = {{ local = \"{}\" }}\n",
                        framework_path.display()
                    ));
                } else {
                    // Framework not cached yet, use git (will be slow first time)
                    toml.push_str(&format!(
                        "Sui = {{ git = \"https://github.com/MystenLabs/sui.git\", subdir = \"crates/sui-framework/packages/sui-framework\", rev = \"{}\" }}\n",
                        FRAMEWORK_VERSION
                    ));
                }
            } else {
                // No caching enabled, use git
                toml.push_str(&format!(
                    "Sui = {{ git = \"https://github.com/MystenLabs/sui.git\", subdir = \"crates/sui-framework/packages/sui-framework\", rev = \"{}\" }}\n",
                    FRAMEWORK_VERSION
                ));
            }
        }
        toml.push('\n');

        // [addresses] section
        if !config.addresses.is_empty() {
            toml.push_str("[addresses]\n");
            for (name, addr) in &config.addresses {
                match addr {
                    Some(a) => toml.push_str(&format!("{} = \"{}\"\n", name, a)),
                    None => toml.push_str(&format!("{} = \"0x0\"\n", name)),
                }
            }
        }

        Ok(toml)
    }

    /// Write a Move source file to a package
    pub fn write_source(
        &self,
        package_dir: &Path,
        module_name: &str,
        source: &str,
    ) -> Result<PathBuf> {
        let sources_dir = package_dir.join("sources");
        fs::create_dir_all(&sources_dir)?;

        let file_path = sources_dir.join(format!("{}.move", module_name));
        fs::write(&file_path, source)?;

        Ok(file_path)
    }

    /// Compile a Move package to bytecode
    pub fn compile(&self, package_dir: &Path) -> Result<CompilationResult> {
        // Register Sui package hooks
        move_package::package_hooks::register_package_hooks(Box::new(
            sui_move_build::SuiPackageHooks,
        ));

        // Create build config
        let build_config = BuildConfig::new_for_testing();

        // Attempt to build
        match build_config.build(package_dir) {
            Ok(compiled) => {
                // Extract compiled modules
                let modules: Result<Vec<(String, Vec<u8>)>> = compiled
                    .package
                    .root_compiled_units
                    .iter()
                    .map(|unit| {
                        let name = unit.unit.name().to_string();
                        let mut bytes = Vec::new();
                        let module = &unit.unit.module;
                        module
                            .serialize_with_version(module.version, &mut bytes)
                            .map_err(|e| anyhow!("Failed to serialize module {}: {}", name, e))?;
                        Ok((name, bytes))
                    })
                    .collect();

                Ok(CompilationResult {
                    success: true,
                    modules: modules?,
                    diagnostics: String::new(),
                    digest: compiled.published_at.ok().map(|id| id.to_string()),
                })
            }
            Err(e) => {
                // Extract diagnostics from error
                let diagnostics = format!("{:#}", e);

                Ok(CompilationResult {
                    success: false,
                    modules: vec![],
                    diagnostics,
                    digest: None,
                })
            }
        }
    }

    /// Scaffold, write source, and compile in one step
    pub fn build_from_source(
        &self,
        package_name: &str,
        module_name: &str,
        source: &str,
    ) -> Result<CompilationResult> {
        let config = PackageConfig {
            name: package_name.to_string(),
            addresses: vec![(package_name.to_string(), None)],
            include_sui_framework: true,
            edition: Some("2024.beta".to_string()),
        };

        let package_dir = self.scaffold(&config)?;
        self.write_source(&package_dir, module_name, source)?;
        self.compile(&package_dir)
    }

    /// Get the working directory
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }
}

/// Generate a minimal Move module source template
pub fn generate_module_template(package_name: &str, module_name: &str) -> String {
    format!(
        r#"module {}::{} {{
    use sui::object::{{Self, UID}};
    use sui::tx_context::TxContext;

    // Add your types and functions here
}}
"#,
        package_name, module_name
    )
}

/// Generate a Move module with a simple struct
pub fn generate_struct_module(
    package_name: &str,
    module_name: &str,
    struct_name: &str,
    fields: &[(String, String)], // (field_name, field_type)
) -> String {
    let mut source = format!(
        r#"module {}::{} {{
    use sui::object::{{Self, UID}};
    use sui::tx_context::TxContext;
    use sui::transfer;

    public struct {} has key, store {{
        id: UID,
"#,
        package_name, module_name, struct_name
    );

    for (name, ty) in fields {
        source.push_str(&format!("        {}: {},\n", name, ty));
    }

    source.push_str("    }\n\n");

    // Add a constructor
    source.push_str(r#"    public fun new("#);

    let params: Vec<String> = fields
        .iter()
        .map(|(name, ty)| format!("{}: {}", name, ty))
        .collect();
    source.push_str(&params.join(", "));
    source.push_str(", ctx: &mut TxContext): ");
    source.push_str(struct_name);
    source.push_str(" {\n");
    source.push_str(&format!("        {} {{\n", struct_name));
    source.push_str("            id: object::new(ctx),\n");
    for (name, _) in fields {
        source.push_str(&format!("            {},\n", name));
    }
    source.push_str("        }\n");
    source.push_str("    }\n");
    source.push_str("}\n");

    source
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_move_toml() {
        let builder = PackageBuilder::new_temp().unwrap();
        let config = PackageConfig {
            name: "test_package".to_string(),
            addresses: vec![("test_package".to_string(), None)],
            include_sui_framework: true,
            edition: Some("2024.beta".to_string()),
        };

        let toml = builder.generate_move_toml(&config).unwrap();
        assert!(toml.contains("name = \"test_package\""));
        assert!(toml.contains("edition = \"2024.beta\""));
        assert!(toml.contains("[dependencies]"));
        assert!(toml.contains("Sui = {"));
        assert!(toml.contains("[addresses]"));
        assert!(toml.contains("test_package = \"0x0\""));

        println!("Generated Move.toml:\n{}", toml);
    }

    #[test]
    fn test_scaffold_package() {
        let builder = PackageBuilder::new_temp().unwrap();
        let config = PackageConfig::default();

        let package_dir = builder.scaffold(&config).unwrap();
        assert!(package_dir.exists());
        assert!(package_dir.join("Move.toml").exists());
        assert!(package_dir.join("sources").exists());

        println!("Scaffolded package at: {:?}", package_dir);
    }

    #[test]
    fn test_generate_module_template() {
        let template = generate_module_template("my_pkg", "my_module");
        assert!(template.contains("module my_pkg::my_module"));
        assert!(template.contains("use sui::object"));

        println!("Generated template:\n{}", template);
    }

    #[test]
    fn test_generate_struct_module() {
        let source = generate_struct_module(
            "my_pkg",
            "counter",
            "Counter",
            &[
                ("value".to_string(), "u64".to_string()),
                ("owner".to_string(), "address".to_string()),
            ],
        );

        assert!(source.contains("public struct Counter has key, store"));
        assert!(source.contains("value: u64"));
        assert!(source.contains("owner: address"));
        assert!(source.contains("public fun new("));

        println!("Generated struct module:\n{}", source);
    }

    #[test]
    fn test_framework_cache_detection() {
        let cache = FrameworkCache::new().unwrap();
        println!("Framework cache directory: {:?}", cache.cache_dir());
        println!("Framework cached: {}", cache.is_cached());
        println!("Sui framework path: {:?}", cache.sui_framework_path());
        println!("Move stdlib path: {:?}", cache.move_stdlib_path());
    }

    #[test]
    fn test_builder_with_framework_cache() {
        let temp_dir = std::env::temp_dir().join(format!("sui-pkg-test-{}", std::process::id()));
        let builder = PackageBuilder::with_framework_cache(&temp_dir).unwrap();

        let config = PackageConfig {
            name: "cached_test".to_string(),
            addresses: vec![("cached_test".to_string(), None)],
            include_sui_framework: true,
            edition: Some("2024.beta".to_string()),
        };

        let toml = builder.generate_move_toml(&config).unwrap();
        println!("Generated Move.toml with cache:\n{}", toml);

        // If framework is cached, should use local path
        if builder.is_framework_cached() {
            assert!(
                toml.contains("local = "),
                "Should use local path when cached"
            );
        } else {
            assert!(toml.contains("git = "), "Should use git when not cached");
        }

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // Note: Full compilation tests are expensive and require framework download
    // They should be run separately as integration tests
}
