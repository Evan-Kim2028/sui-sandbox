//! Linkage table handling for package upgrades.
//!
//! Each Sui package has a linkage table that maps original package IDs (runtime IDs)
//! to their upgraded storage IDs. This module provides utilities to extract and
//! work with linkage information.

use std::collections::HashMap;

use sui_transport::grpc::GrpcObject;

use crate::address::normalize_address;

/// Extract linkage map from a GrpcObject (original_id -> upgraded_id).
///
/// The linkage table tells us where dependencies have been upgraded to.
/// For example, if package A depends on package B, and B was upgraded,
/// A's linkage table will map B's original_id to B's new storage_id.
pub fn extract_linkage_map(obj: &GrpcObject) -> HashMap<String, String> {
    obj.package_linkage
        .as_ref()
        .map(|linkage| {
            linkage
                .iter()
                .map(|l| {
                    (
                        normalize_address(&l.original_id),
                        normalize_address(&l.upgraded_id),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Extract linkage map with versions from a GrpcObject.
///
/// Returns HashMap<original_id, (upgraded_id, upgraded_version)>.
/// The version is the version number of the upgraded package.
pub fn extract_linkage_with_versions(obj: &GrpcObject) -> HashMap<String, (String, u64)> {
    obj.package_linkage
        .as_ref()
        .map(|linkage| {
            linkage
                .iter()
                .map(|l| {
                    (
                        normalize_address(&l.original_id),
                        (normalize_address(&l.upgraded_id), l.upgraded_version),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sui_transport::grpc::{GrpcLinkage, GrpcOwner};

    #[test]
    fn test_extract_linkage_map_empty() {
        let obj = GrpcObject {
            object_id: "0x1".to_string(),
            version: 1,
            digest: String::new(),
            type_string: None,
            owner: GrpcOwner::Unknown,
            bcs: None,
            bcs_full: None,
            package_modules: None,
            package_linkage: None,
            package_original_id: None,
        };

        let linkage = extract_linkage_map(&obj);
        assert!(linkage.is_empty());
    }

    #[test]
    fn test_extract_linkage_map() {
        let obj = GrpcObject {
            object_id: "0x1".to_string(),
            version: 1,
            digest: String::new(),
            type_string: None,
            owner: GrpcOwner::Unknown,
            bcs: None,
            bcs_full: None,
            package_modules: None,
            package_linkage: Some(vec![GrpcLinkage {
                original_id: "0xabc".to_string(),
                upgraded_id: "0xdef".to_string(),
                upgraded_version: 5,
            }]),
            package_original_id: None,
        };

        let linkage = extract_linkage_map(&obj);
        assert_eq!(linkage.len(), 1);
        let orig = normalize_address("0xabc");
        let upgraded = normalize_address("0xdef");
        assert_eq!(linkage.get(&orig), Some(&upgraded));
    }
}
