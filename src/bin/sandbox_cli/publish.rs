//! Publish command - compile and deploy Move packages locally

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use std::path::PathBuf;
use std::process::Command;

use super::output::{format_error, format_publish_result};
use super::SandboxState;
use sui_package_extractor::bytecode::read_local_compiled_module_bytes;

#[derive(Parser, Debug)]
pub struct PublishCmd {
    /// Path to Move package directory (with Move.toml)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Named address assignments (e.g., my_pkg=0x0)
    #[arg(long = "address", value_parser = parse_address_assignment)]
    pub addresses: Vec<(String, String)>,

    /// Skip compilation, use existing bytecode_modules/ directory
    #[arg(long)]
    pub bytecode_only: bool,

    /// Don't persist to session state
    #[arg(long)]
    pub dry_run: bool,

    /// Assign package to this address (default: auto-generated)
    #[arg(long)]
    pub assign_address: Option<String>,
}

fn parse_address_assignment(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid address assignment '{}', expected 'name=address'",
            s
        ));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

impl PublishCmd {
    pub async fn execute(
        &self,
        state: &mut SandboxState,
        json_output: bool,
        _verbose: bool,
    ) -> Result<()> {
        let result = self.execute_inner(state).await;

        match result {
            Ok((address, modules)) => {
                println!("{}", format_publish_result(address, &modules, json_output));
                Ok(())
            }
            Err(e) => {
                eprintln!("{}", format_error(&e, json_output));
                Err(e)
            }
        }
    }

    async fn execute_inner(
        &self,
        state: &mut SandboxState,
    ) -> Result<(AccountAddress, Vec<(String, Vec<u8>)>)> {
        let package_path = self.path.canonicalize().context("Invalid package path")?;

        // Determine bytecode directory
        let bytecode_dir = if self.bytecode_only {
            // Look for bytecode_modules directly or in build/<name>/bytecode_modules
            let direct = package_path.join("bytecode_modules");
            if direct.exists() {
                direct
            } else {
                // Try to find in build directory
                let build_dir = package_path.join("build");
                if build_dir.exists() {
                    find_bytecode_dir(&build_dir)?
                } else {
                    return Err(anyhow!(
                        "No bytecode_modules directory found. Run 'sui move build' first or remove --bytecode-only"
                    ));
                }
            }
        } else {
            // Compile using sui move build
            compile_package(&package_path, &self.addresses)?;

            // Find bytecode in build directory
            let build_dir = package_path.join("build");
            find_bytecode_dir(&build_dir)?
        };

        // Load bytecode modules
        let modules = read_local_compiled_module_bytes(&bytecode_dir)?;

        if modules.is_empty() {
            return Err(anyhow!("No modules found in {}", bytecode_dir.display()));
        }

        // Determine package address
        let package_address = if let Some(addr_str) = &self.assign_address {
            AccountAddress::from_hex_literal(addr_str).context("Invalid --assign-address value")?
        } else {
            // Extract from first module's self-address
            let first_module = CompiledModule::deserialize_with_defaults(&modules[0].1)
                .context("Failed to deserialize module")?;
            *first_module.self_id().address()
        };

        // Add to state (unless dry-run)
        if !self.dry_run {
            state.add_package(package_address, modules.clone());
        }

        Ok((package_address, modules))
    }
}

/// Compile a Move package using the sui CLI
fn compile_package(package_path: &PathBuf, addresses: &[(String, String)]) -> Result<()> {
    let mut cmd = Command::new("sui");
    cmd.args(["move", "build"]);
    cmd.arg("--path");
    cmd.arg(package_path);

    // Add address assignments
    for (name, addr) in addresses {
        cmd.arg("--named-address");
        cmd.arg(format!("{}={}", name, addr));
    }

    let output = cmd
        .output()
        .context("Failed to run 'sui move build'. Is the sui CLI installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow!("Compilation failed:\n{}\n{}", stdout, stderr));
    }

    Ok(())
}

/// Find the bytecode_modules directory in a build directory
fn find_bytecode_dir(build_dir: &PathBuf) -> Result<PathBuf> {
    // Look for any subdirectory with bytecode_modules
    for entry in std::fs::read_dir(build_dir).context("Failed to read build directory")? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let bytecode_dir = path.join("bytecode_modules");
            if bytecode_dir.exists() {
                return Ok(bytecode_dir);
            }
        }
    }

    Err(anyhow!(
        "No bytecode_modules directory found in {}",
        build_dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_parse_address_assignment() {
        let result = parse_address_assignment("my_pkg=0x123");
        assert!(result.is_ok());
        let (name, addr) = result.unwrap();
        assert_eq!(name, "my_pkg");
        assert_eq!(addr, "0x123");
    }

    #[test]
    fn test_parse_address_assignment_invalid() {
        let result = parse_address_assignment("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_bytecode_dir() {
        let dir = tempdir().unwrap();

        // Create build/my_package/bytecode_modules structure
        let bytecode_dir = dir.path().join("my_package").join("bytecode_modules");
        std::fs::create_dir_all(&bytecode_dir).unwrap();

        let found = find_bytecode_dir(&dir.path().to_path_buf()).unwrap();
        assert_eq!(found, bytecode_dir);
    }
}
