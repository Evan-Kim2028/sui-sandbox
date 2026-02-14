//! Package root and closure validation helpers.
//!
//! These helpers standardize how callers infer required package roots from
//! explicit package IDs plus referenced type information, and how they validate
//! that a fetched bytecode set has a complete transitive module dependency
//! closure.

use anyhow::{Context, Result};
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::BTreeSet;

use crate::resolver::LocalModuleResolver;

use super::{
    extract_package_ids_from_type, extract_package_ids_from_type_tag, is_framework_package,
};

fn include_non_framework(roots: &mut BTreeSet<AccountAddress>, address: AccountAddress) {
    if !is_framework_package(&address.to_hex_literal()) {
        roots.insert(address);
    }
}

/// Collect required package roots from explicit package IDs plus type strings.
///
/// This is useful for protocol adapters that know a root package set but also
/// need to include packages referenced only through type parameters.
pub fn collect_required_package_roots_from_type_strings(
    explicit_package_ids: &[AccountAddress],
    type_strings: &[String],
) -> Result<BTreeSet<AccountAddress>> {
    let mut roots = BTreeSet::new();
    for address in explicit_package_ids {
        include_non_framework(&mut roots, *address);
    }
    for ty in type_strings {
        for pkg_id in extract_package_ids_from_type(ty) {
            let address = AccountAddress::from_hex_literal(&pkg_id).with_context(|| {
                format!("invalid package id extracted from type `{ty}`: {pkg_id}")
            })?;
            include_non_framework(&mut roots, address);
        }
    }
    Ok(roots)
}

/// Collect required package roots from explicit package IDs plus concrete type tags.
pub fn collect_required_package_roots_from_type_tags(
    explicit_package_ids: &[AccountAddress],
    type_tags: &[TypeTag],
) -> BTreeSet<AccountAddress> {
    let mut roots = BTreeSet::new();
    for address in explicit_package_ids {
        include_non_framework(&mut roots, *address);
    }
    for tag in type_tags {
        for address in extract_package_ids_from_type_tag(tag) {
            include_non_framework(&mut roots, address);
        }
    }
    roots
}

/// Return unresolved transitive package dependencies after loading package modules.
///
/// `packages` is `(storage_address, named_module_bytes)` where module bytes are
/// the compiled `.mv` contents.
pub fn unresolved_package_dependencies_for_modules(
    packages: Vec<(AccountAddress, Vec<(String, Vec<u8>)>)>,
) -> Result<BTreeSet<AccountAddress>> {
    let mut resolver = LocalModuleResolver::with_sui_framework()?;
    for (package_id, modules) in packages {
        resolver
            .add_package_modules_at(modules, Some(package_id))
            .with_context(|| format!("load package modules for {}", package_id.to_hex_literal()))?;
    }
    Ok(resolver.get_missing_dependencies())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_type_referenced_packages_from_type_strings() {
        let explicit = vec![AccountAddress::from_hex_literal(
            "0x97d9473771b01f77b0940c589484184b49f6444627ec121314fae6a6d36fb86b",
        )
        .expect("parse margin package")];
        let types = vec![
            "0x2::sui::SUI".to_string(),
            "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC"
                .to_string(),
        ];

        let roots =
            collect_required_package_roots_from_type_strings(&explicit, &types).expect("roots");

        assert!(roots.contains(&explicit[0]));
        assert!(roots.contains(
            &AccountAddress::from_hex_literal(
                "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7"
            )
            .expect("parse usdc package")
        ));
        assert!(!roots.contains(&AccountAddress::from_hex_literal("0x2").expect("parse framework")));
    }

    #[test]
    fn empty_module_set_has_no_unresolved_dependencies() {
        let unresolved =
            unresolved_package_dependencies_for_modules(Vec::new()).expect("closure validation");
        assert!(unresolved.is_empty());
    }
}
