//! Well-known Sui framework types and addresses.
//!
//! These are validated once at initialization time, eliminating runtime panics
//! from repeated identifier parsing throughout the codebase.
//!
//! # Usage
//!
//! ```ignore
//! use sui_sandbox_core::well_known::{addr, ident, types};
//! use move_core_types::language_storage::TypeTag;
//!
//! // Use static addresses
//! let sui_framework = *addr::SUI_FRAMEWORK;
//! assert_eq!(sui_framework.to_hex_literal(), "0x2");
//!
//! // Use static identifiers
//! let coin_module = ident::COIN.clone();
//! assert_eq!(coin_module.as_str(), "coin");
//!
//! // Use type constructors
//! let sui_coin_type = types::sui_coin();
//! assert!(matches!(sui_coin_type, TypeTag::Struct(_)));
//! ```

use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::{StructTag, TypeTag};
use std::sync::LazyLock;

// Re-export framework constants from sui-sandbox-types
pub use sui_sandbox_types::framework::{
    CLOCK_OBJECT_ID, DENY_LIST_OBJECT_ID, MOVE_STDLIB, RANDOM_OBJECT_ID, SUI_FRAMEWORK, SUI_SYSTEM,
};

/// Well-known addresses in the Sui ecosystem.
///
/// NOTE: Prefer using the constants directly from `sui_sandbox_types::framework`
/// (e.g., `MOVE_STDLIB`, `SUI_FRAMEWORK`). This module provides `LazyLock` versions
/// for backwards compatibility where a `&'static AccountAddress` is needed.
pub mod addr {
    use super::*;

    /// Move stdlib address (0x1)
    pub static MOVE_STDLIB: LazyLock<AccountAddress> =
        LazyLock::new(|| sui_sandbox_types::framework::MOVE_STDLIB);

    /// Sui framework address (0x2)
    pub static SUI_FRAMEWORK: LazyLock<AccountAddress> =
        LazyLock::new(|| sui_sandbox_types::framework::SUI_FRAMEWORK);

    /// Sui system address (0x3)
    pub static SUI_SYSTEM: LazyLock<AccountAddress> =
        LazyLock::new(|| sui_sandbox_types::framework::SUI_SYSTEM);

    /// Clock object address (0x6)
    pub static CLOCK: LazyLock<AccountAddress> =
        LazyLock::new(|| sui_sandbox_types::framework::CLOCK_OBJECT_ID);

    /// Random object address (0x8)
    pub static RANDOM: LazyLock<AccountAddress> =
        LazyLock::new(|| sui_sandbox_types::framework::RANDOM_OBJECT_ID);

    /// Deny list object address (0x403)
    pub static DENY_LIST: LazyLock<AccountAddress> =
        LazyLock::new(|| sui_sandbox_types::framework::DENY_LIST_OBJECT_ID);
}

/// Well-known module and type identifiers.
pub mod ident {
    use super::*;

    // Module names
    pub static COIN: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("coin").expect("'coin' is a valid identifier"));

    pub static SUI: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("sui").expect("'sui' is a valid identifier"));

    pub static BALANCE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("balance").expect("'balance' is a valid identifier"));

    pub static OBJECT: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("object").expect("'object' is a valid identifier"));

    pub static TRANSFER: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("transfer").expect("'transfer' is a valid identifier"));

    pub static TX_CONTEXT: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::new("tx_context").expect("'tx_context' is a valid identifier")
    });

    pub static CLOCK: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("clock").expect("'clock' is a valid identifier"));

    pub static RANDOM: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("random").expect("'random' is a valid identifier"));

    pub static TABLE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("table").expect("'table' is a valid identifier"));

    pub static BAG: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("bag").expect("'bag' is a valid identifier"));

    pub static DYNAMIC_FIELD: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::new("dynamic_field").expect("'dynamic_field' is a valid identifier")
    });

    pub static OPTION: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("option").expect("'option' is a valid identifier"));

    pub static STRING: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("string").expect("'string' is a valid identifier"));

    pub static ASCII: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("ascii").expect("'ascii' is a valid identifier"));

    pub static VECTOR: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("vector").expect("'vector' is a valid identifier"));

    pub static PACKAGE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("package").expect("'package' is a valid identifier"));

    // Type/struct names (capitalized)
    pub static SUI_TYPE_NAME: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("SUI").expect("'SUI' is a valid identifier"));

    pub static COIN_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Coin").expect("'Coin' is a valid identifier"));

    pub static BALANCE_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Balance").expect("'Balance' is a valid identifier"));

    pub static UID: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("UID").expect("'UID' is a valid identifier"));

    pub static ID: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("ID").expect("'ID' is a valid identifier"));

    pub static TX_CONTEXT_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("TxContext").expect("'TxContext' is a valid identifier"));

    pub static CLOCK_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Clock").expect("'Clock' is a valid identifier"));

    pub static RANDOM_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Random").expect("'Random' is a valid identifier"));

    pub static TABLE_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Table").expect("'Table' is a valid identifier"));

    pub static BAG_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Bag").expect("'Bag' is a valid identifier"));

    pub static OPTION_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("Option").expect("'Option' is a valid identifier"));

    pub static STRING_TYPE: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::new("String").expect("'String' is a valid identifier"));

    pub static UPGRADE_CAP_TYPE: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::new("UpgradeCap").expect("'UpgradeCap' is a valid identifier")
    });

    pub static UPGRADE_RECEIPT_TYPE: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::new("UpgradeReceipt").expect("'UpgradeReceipt' is a valid identifier")
    });
}

/// Well-known type tags and type constructors.
pub mod types {
    use super::*;

    /// The SUI coin type: `0x2::sui::SUI`
    pub static SUI_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::SUI.clone(),
            name: ident::SUI_TYPE_NAME.clone(),
            type_params: vec![],
        }))
    });

    /// Create a `0x2::coin::Coin<T>` type tag.
    pub fn coin_of(inner: TypeTag) -> TypeTag {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::COIN.clone(),
            name: ident::COIN_TYPE.clone(),
            type_params: vec![inner],
        }))
    }

    /// Create a `Coin<0x2::sui::SUI>` type tag.
    pub fn sui_coin() -> TypeTag {
        coin_of(SUI_TYPE.clone())
    }

    /// Create a `0x2::balance::Balance<T>` type tag.
    pub fn balance_of(inner: TypeTag) -> TypeTag {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::BALANCE.clone(),
            name: ident::BALANCE_TYPE.clone(),
            type_params: vec![inner],
        }))
    }

    /// The UID type: `0x2::object::UID`
    pub static UID_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::OBJECT.clone(),
            name: ident::UID.clone(),
            type_params: vec![],
        }))
    });

    /// The ID type: `0x2::object::ID`
    pub static ID_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::OBJECT.clone(),
            name: ident::ID.clone(),
            type_params: vec![],
        }))
    });

    /// The TxContext type: `0x2::tx_context::TxContext`
    pub static TX_CONTEXT_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::TX_CONTEXT.clone(),
            name: ident::TX_CONTEXT_TYPE.clone(),
            type_params: vec![],
        }))
    });

    /// The Clock type: `0x2::clock::Clock`
    pub static CLOCK_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::CLOCK.clone(),
            name: ident::CLOCK_TYPE.clone(),
            type_params: vec![],
        }))
    });

    /// The Random type: `0x2::random::Random`
    pub static RANDOM_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::RANDOM.clone(),
            name: ident::RANDOM_TYPE.clone(),
            type_params: vec![],
        }))
    });

    /// Create a `0x2::table::Table<K, V>` type tag.
    pub fn table_of(key: TypeTag, value: TypeTag) -> TypeTag {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::TABLE.clone(),
            name: ident::TABLE_TYPE.clone(),
            type_params: vec![key, value],
        }))
    }

    /// Create a `0x2::bag::Bag` type tag.
    pub static BAG_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::BAG.clone(),
            name: ident::BAG_TYPE.clone(),
            type_params: vec![],
        }))
    });

    /// Create a `0x1::option::Option<T>` type tag.
    pub fn option_of(inner: TypeTag) -> TypeTag {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::MOVE_STDLIB,
            module: ident::OPTION.clone(),
            name: ident::OPTION_TYPE.clone(),
            type_params: vec![inner],
        }))
    }

    /// Create a `0x1::string::String` type tag.
    pub static UTF8_STRING_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::MOVE_STDLIB,
            module: ident::STRING.clone(),
            name: ident::STRING_TYPE.clone(),
            type_params: vec![],
        }))
    });

    /// Create a `0x1::ascii::String` type tag.
    pub static ASCII_STRING_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::MOVE_STDLIB,
            module: ident::ASCII.clone(),
            name: ident::STRING_TYPE.clone(),
            type_params: vec![],
        }))
    });

    /// The UpgradeCap type: `0x2::package::UpgradeCap`
    pub static UPGRADE_CAP_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::PACKAGE.clone(),
            name: ident::UPGRADE_CAP_TYPE.clone(),
            type_params: vec![],
        }))
    });

    /// The UpgradeReceipt type: `0x2::package::UpgradeReceipt`
    pub static UPGRADE_RECEIPT_TYPE: LazyLock<TypeTag> = LazyLock::new(|| {
        TypeTag::Struct(Box::new(StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::PACKAGE.clone(),
            name: ident::UPGRADE_RECEIPT_TYPE.clone(),
            type_params: vec![],
        }))
    });
}

/// StructTag constructors for common patterns.
pub mod structs {
    use super::*;

    /// Create a StructTag for `0x2::coin::Coin<T>`.
    pub fn coin_struct(inner: TypeTag) -> StructTag {
        StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::COIN.clone(),
            name: ident::COIN_TYPE.clone(),
            type_params: vec![inner],
        }
    }

    /// Create a StructTag for `Coin<0x2::sui::SUI>`.
    pub fn sui_coin_struct() -> StructTag {
        coin_struct(types::SUI_TYPE.clone())
    }

    /// Create a StructTag for `0x2::balance::Balance<T>`.
    pub fn balance_struct(inner: TypeTag) -> StructTag {
        StructTag {
            address: *addr::SUI_FRAMEWORK,
            module: ident::BALANCE.clone(),
            name: ident::BALANCE_TYPE.clone(),
            type_params: vec![inner],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addresses_are_valid() {
        // Just accessing these should not panic
        let _ = *addr::MOVE_STDLIB;
        let _ = *addr::SUI_FRAMEWORK;
        let _ = *addr::SUI_SYSTEM;
        let _ = *addr::CLOCK;
        let _ = *addr::RANDOM;
        let _ = *addr::DENY_LIST;
    }

    #[test]
    fn test_sui_framework_is_0x2() {
        // to_hex_literal returns the short form
        assert_eq!(addr::SUI_FRAMEWORK.to_hex_literal(), "0x2");
    }

    #[test]
    fn test_identifiers_are_valid() {
        // Just accessing these should not panic
        let _ = ident::COIN.clone();
        let _ = ident::SUI.clone();
        let _ = ident::BALANCE.clone();
        let _ = ident::COIN_TYPE.clone();
        let _ = ident::UID.clone();
    }

    #[test]
    fn test_sui_type_is_correct() {
        if let TypeTag::Struct(s) = &*types::SUI_TYPE {
            assert_eq!(s.address, *addr::SUI_FRAMEWORK);
            assert_eq!(s.module, *ident::SUI);
            assert_eq!(s.name, *ident::SUI_TYPE_NAME);
            assert!(s.type_params.is_empty());
        } else {
            panic!("SUI_TYPE should be a struct");
        }
    }

    #[test]
    fn test_coin_of_creates_correct_type() {
        let coin = types::coin_of(types::SUI_TYPE.clone());
        if let TypeTag::Struct(s) = coin {
            assert_eq!(s.module, *ident::COIN);
            assert_eq!(s.name, *ident::COIN_TYPE);
            assert_eq!(s.type_params.len(), 1);
        } else {
            panic!("coin_of should create a struct type");
        }
    }

    #[test]
    fn test_sui_coin_type() {
        let sui_coin = types::sui_coin();
        if let TypeTag::Struct(s) = sui_coin {
            assert_eq!(s.name.as_str(), "Coin");
            assert_eq!(s.type_params.len(), 1);
            if let TypeTag::Struct(inner) = &s.type_params[0] {
                assert_eq!(inner.name.as_str(), "SUI");
            } else {
                panic!("Inner type should be SUI struct");
            }
        } else {
            panic!("sui_coin should create a struct type");
        }
    }

    #[test]
    fn test_table_of_creates_correct_type() {
        let table = types::table_of(TypeTag::U64, TypeTag::Bool);
        if let TypeTag::Struct(s) = table {
            assert_eq!(s.name, *ident::TABLE_TYPE);
            assert_eq!(s.type_params.len(), 2);
            assert!(matches!(s.type_params[0], TypeTag::U64));
            assert!(matches!(s.type_params[1], TypeTag::Bool));
        } else {
            panic!("table_of should create a struct type");
        }
    }

    #[test]
    fn test_struct_constructors() {
        let coin_struct = structs::sui_coin_struct();
        assert_eq!(coin_struct.name.as_str(), "Coin");
        assert_eq!(coin_struct.module.as_str(), "coin");
    }
}
