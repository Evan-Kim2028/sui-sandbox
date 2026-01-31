//! Safe identifier creation utilities.
//!
//! This module provides safe ways to create Move identifiers, eliminating
//! the repeated `Identifier::new(...).expect(...)` pattern throughout the codebase.
//!
//! # Example
//!
//! ```
//! use sui_sandbox_core::shared::identifiers::{module_id, safe_identifier};
//! use move_core_types::account_address::AccountAddress;
//!
//! // Safe identifier creation
//! let ident = safe_identifier("transfer").unwrap();
//!
//! // Create a module ID safely
//! let module = module_id(&AccountAddress::TWO, "coin").unwrap();
//! ```

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;

/// Create an identifier from a string, returning an error on invalid input.
///
/// Use this instead of `Identifier::new(...).expect(...)` or
/// `Identifier::new(...).ok()` patterns.
pub fn safe_identifier(name: &str) -> Result<Identifier> {
    Identifier::new(name).map_err(|e| anyhow!("Invalid identifier '{}': {}", name, e))
}

/// Create an identifier from a string, returning None on invalid input.
///
/// Use this when you need to silently skip invalid identifiers.
pub fn try_identifier(name: &str) -> Option<Identifier> {
    Identifier::new(name).ok()
}

/// Create a module ID from an address and module name.
pub fn module_id(address: &AccountAddress, module: &str) -> Result<ModuleId> {
    let ident = safe_identifier(module).context("Invalid module name")?;
    Ok(ModuleId::new(*address, ident))
}

/// Create a module ID, returning None on invalid input.
pub fn try_module_id(address: &AccountAddress, module: &str) -> Option<ModuleId> {
    let ident = try_identifier(module)?;
    Some(ModuleId::new(*address, ident))
}

/// Parse a target string like "0xPKG::module::function" into components.
///
/// Returns (package_address, module_name, function_name).
pub fn parse_move_target(target: &str) -> Result<(AccountAddress, String, String)> {
    let parts: Vec<&str> = target.split("::").collect();

    match parts.len() {
        3 => {
            let package = AccountAddress::from_hex_literal(parts[0])
                .context("Invalid package address in target")?;
            Ok((package, parts[1].to_string(), parts[2].to_string()))
        }
        _ => Err(anyhow!(
            "Invalid target format '{}'. Expected '0xPKG::module::function'",
            target
        )),
    }
}

/// Parse a target string, allowing short form "module::function" with a default package.
pub fn parse_move_target_with_default(
    target: &str,
    default_package: Option<AccountAddress>,
) -> Result<(AccountAddress, String, String)> {
    let parts: Vec<&str> = target.split("::").collect();

    match parts.len() {
        2 => {
            // module::function - use default package
            let package = default_package.ok_or_else(|| {
                anyhow!("No default package. Use full target: 0xPKG::module::func")
            })?;
            Ok((package, parts[0].to_string(), parts[1].to_string()))
        }
        3 => {
            // 0xPKG::module::function
            let package = AccountAddress::from_hex_literal(parts[0])
                .context("Invalid package address in target")?;
            Ok((package, parts[1].to_string(), parts[2].to_string()))
        }
        _ => Err(anyhow!(
            "Invalid target format '{}'. Expected 'module::func' or '0xPKG::module::func'",
            target
        )),
    }
}

/// Validate that a string is a valid Move identifier.
pub fn is_valid_identifier(name: &str) -> bool {
    Identifier::new(name).is_ok()
}

/// Common well-known module names as identifiers.
pub mod well_known {
    use super::*;
    use std::sync::LazyLock;

    // Module names
    pub static COIN: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("coin").unwrap());
    pub static SUI: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("sui").unwrap());
    pub static BALANCE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("balance").unwrap());
    pub static OBJECT: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("object").unwrap());
    pub static TRANSFER: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("transfer").unwrap());
    pub static TX_CONTEXT: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("tx_context").unwrap());
    pub static CLOCK: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("clock").unwrap());
    pub static RANDOM: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("random").unwrap());
    pub static TABLE: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("table").unwrap());
    pub static BAG: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("bag").unwrap());
    pub static DYNAMIC_FIELD: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("dynamic_field").unwrap());
    pub static OPTION: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("option").unwrap());
    pub static STRING: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("string").unwrap());
    pub static VECTOR: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("vector").unwrap());
    pub static PACKAGE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("package").unwrap());

    // Type/struct names (capitalized)
    pub static SUI_TYPE: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("SUI").unwrap());
    pub static COIN_TYPE: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("Coin").unwrap());
    pub static BALANCE_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Balance").unwrap());
    pub static UID: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("UID").unwrap());
    pub static ID: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("ID").unwrap());
    pub static TX_CONTEXT_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("TxContext").unwrap());
    pub static CLOCK_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Clock").unwrap());
    pub static RANDOM_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Random").unwrap());
    pub static TABLE_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Table").unwrap());
    pub static BAG_TYPE: LazyLock<Identifier> = LazyLock::new(|| Identifier::new("Bag").unwrap());
    pub static OPTION_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Option").unwrap());
    pub static STRING_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("String").unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_identifier_valid() {
        let ident = safe_identifier("transfer").unwrap();
        assert_eq!(ident.as_str(), "transfer");
    }

    #[test]
    fn test_safe_identifier_invalid() {
        let result = safe_identifier("123invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_try_identifier() {
        assert!(try_identifier("valid").is_some());
        assert!(try_identifier("123invalid").is_none());
    }

    #[test]
    fn test_module_id() {
        let addr = AccountAddress::from_hex_literal("0x2").unwrap();
        let module = module_id(&addr, "coin").unwrap();
        assert_eq!(module.name().as_str(), "coin");
    }

    #[test]
    fn test_parse_move_target() {
        let (pkg, module, func) = parse_move_target("0x2::coin::transfer").unwrap();
        assert_eq!(pkg, AccountAddress::from_hex_literal("0x2").unwrap());
        assert_eq!(module, "coin");
        assert_eq!(func, "transfer");
    }

    #[test]
    fn test_parse_move_target_with_default() {
        let default = AccountAddress::from_hex_literal("0x123").unwrap();

        // Short form
        let (pkg, module, func) =
            parse_move_target_with_default("mymodule::myfunc", Some(default)).unwrap();
        assert_eq!(pkg, default);
        assert_eq!(module, "mymodule");
        assert_eq!(func, "myfunc");

        // Full form (ignores default)
        let (pkg, module, func) =
            parse_move_target_with_default("0x2::coin::transfer", Some(default)).unwrap();
        assert_eq!(pkg, AccountAddress::from_hex_literal("0x2").unwrap());
        assert_eq!(module, "coin");
        assert_eq!(func, "transfer");
    }

    #[test]
    fn test_well_known_identifiers() {
        assert_eq!(well_known::COIN.as_str(), "coin");
        assert_eq!(well_known::SUI.as_str(), "sui");
        assert_eq!(well_known::COIN_TYPE.as_str(), "Coin");
    }
}
