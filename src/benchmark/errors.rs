//! Error codes and diagnostic messages for type inhabitation evaluation.
//!
//! This module centralizes error definitions to ensure consistent messaging
//! across the codebase and avoid relying on external documentation.

/// Error code used by native functions that cannot be simulated.
///
/// When a function calls an unsupported native (crypto verification, randomness, zklogin),
/// the native aborts with this error code. The runner detects this and provides a
/// user-friendly error message.
pub const E_NOT_SUPPORTED: u64 = 1000;

/// Categories of native functions and their support status.
///
/// This is the source of truth for what natives are supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCategory {
    /// Real implementations from move-stdlib-natives
    RealImpl,
    /// Safe mocks that return placeholder values
    SafeMock,
    /// Full support via ObjectRuntime VM extension
    VmExtension,
    /// Aborts with E_NOT_SUPPORTED - cannot be simulated
    Unsupported,
}

/// Information about a native function module.
pub struct NativeModuleInfo {
    pub module: &'static str,
    pub category: NativeCategory,
    pub description: &'static str,
}

/// Get information about native function support.
///
/// Returns a list of all native modules and their support status.
pub fn native_support_info() -> Vec<NativeModuleInfo> {
    vec![
        // Category A: Real implementations
        NativeModuleInfo {
            module: "0x1::vector",
            category: NativeCategory::RealImpl,
            description: "Vector operations (empty, length, borrow, push, pop, etc.)",
        },
        NativeModuleInfo {
            module: "0x1::bcs",
            category: NativeCategory::RealImpl,
            description: "BCS serialization",
        },
        NativeModuleInfo {
            module: "0x1::hash",
            category: NativeCategory::RealImpl,
            description: "SHA2-256, SHA3-256 hashing",
        },
        NativeModuleInfo {
            module: "0x1::string",
            category: NativeCategory::RealImpl,
            description: "UTF-8 string operations",
        },
        NativeModuleInfo {
            module: "0x1::type_name",
            category: NativeCategory::RealImpl,
            description: "Type name reflection",
        },
        NativeModuleInfo {
            module: "0x1::debug",
            category: NativeCategory::RealImpl,
            description: "Debug printing (no-op in production)",
        },
        NativeModuleInfo {
            module: "0x1::signer",
            category: NativeCategory::RealImpl,
            description: "Signer address extraction",
        },
        
        // Category B: Safe mocks
        NativeModuleInfo {
            module: "0x2::tx_context",
            category: NativeCategory::SafeMock,
            description: "Transaction context (sender, epoch, etc.)",
        },
        NativeModuleInfo {
            module: "0x2::object",
            category: NativeCategory::SafeMock,
            description: "Object ID operations",
        },
        NativeModuleInfo {
            module: "0x2::transfer",
            category: NativeCategory::SafeMock,
            description: "Object transfers (no-op, ownership not tracked)",
        },
        NativeModuleInfo {
            module: "0x2::event",
            category: NativeCategory::SafeMock,
            description: "Event emission (no-op)",
        },
        NativeModuleInfo {
            module: "0x2::types",
            category: NativeCategory::SafeMock,
            description: "OTW type checking (real implementation)",
        },
        
        // Category: VM Extension (full support)
        NativeModuleInfo {
            module: "0x2::dynamic_field",
            category: NativeCategory::VmExtension,
            description: "Dynamic field operations (full support via ObjectRuntime)",
        },
        
        // Category C: Unsupported (abort with E_NOT_SUPPORTED)
        NativeModuleInfo {
            module: "0x2::bls12381",
            category: NativeCategory::Unsupported,
            description: "BLS12-381 signature verification",
        },
        NativeModuleInfo {
            module: "0x2::ecdsa_k1",
            category: NativeCategory::Unsupported,
            description: "ECDSA secp256k1 signature verification",
        },
        NativeModuleInfo {
            module: "0x2::ecdsa_r1",
            category: NativeCategory::Unsupported,
            description: "ECDSA secp256r1 signature verification",
        },
        NativeModuleInfo {
            module: "0x2::ed25519",
            category: NativeCategory::Unsupported,
            description: "Ed25519 signature verification",
        },
        NativeModuleInfo {
            module: "0x2::groth16",
            category: NativeCategory::Unsupported,
            description: "Groth16 ZK proof verification",
        },
        NativeModuleInfo {
            module: "0x2::poseidon",
            category: NativeCategory::Unsupported,
            description: "Poseidon hash for ZK circuits",
        },
        NativeModuleInfo {
            module: "0x2::zklogin",
            category: NativeCategory::Unsupported,
            description: "zkLogin verification",
        },
        NativeModuleInfo {
            module: "0x2::random",
            category: NativeCategory::Unsupported,
            description: "On-chain randomness",
        },
        NativeModuleInfo {
            module: "0x2::config",
            category: NativeCategory::Unsupported,
            description: "System configuration",
        },
        NativeModuleInfo {
            module: "0x3::nitro_attestation",
            category: NativeCategory::Unsupported,
            description: "AWS Nitro attestation verification",
        },
    ]
}

/// Format an error message for unsupported native function.
///
/// This is used by the runner when it detects error code 1000.
pub fn unsupported_native_error_message() -> String {
    let unsupported: Vec<_> = native_support_info()
        .into_iter()
        .filter(|n| n.category == NativeCategory::Unsupported)
        .map(|n| n.module)
        .collect();
    
    format!(
        "execution failed: unsupported native function (error {}). \
         This function uses a native that cannot be simulated. \
         Unsupported modules: {}",
        E_NOT_SUPPORTED,
        unsupported.join(", ")
    )
}

/// Check if an error message indicates an unsupported native function.
pub fn is_unsupported_native_error(error: &str) -> bool {
    error.contains(&E_NOT_SUPPORTED.to_string()) && error.contains("MoveAbort")
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_native_support_info_has_all_categories() {
        let info = native_support_info();
        assert!(info.iter().any(|n| n.category == NativeCategory::RealImpl));
        assert!(info.iter().any(|n| n.category == NativeCategory::SafeMock));
        assert!(info.iter().any(|n| n.category == NativeCategory::VmExtension));
        assert!(info.iter().any(|n| n.category == NativeCategory::Unsupported));
    }
    
    #[test]
    fn test_is_unsupported_native_error() {
        assert!(is_unsupported_native_error("VMError: MoveAbort(1000)"));
        assert!(is_unsupported_native_error("execution failed: MoveAbort with code 1000"));
        assert!(!is_unsupported_native_error("VMError: MoveAbort(42)"));
        assert!(!is_unsupported_native_error("some other error"));
    }
}
