//! Shared protocol-adapter utilities.
//!
//! Keeps protocol-name parsing and package-id requirements consistent across
//! CLI and Python entrypoints.

use anyhow::{anyhow, Result};

use crate::checkpoint_discovery::normalize_package_id;

/// Supported protocol adapter families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolAdapter {
    Generic,
    Deepbook,
    Cetus,
    Suilend,
    Scallop,
}

impl ProtocolAdapter {
    pub const SUPPORTED: [Self; 5] = [
        Self::Generic,
        Self::Deepbook,
        Self::Cetus,
        Self::Suilend,
        Self::Scallop,
    ];

    pub fn parse(input: &str) -> Result<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "generic" => Ok(Self::Generic),
            "deepbook" => Ok(Self::Deepbook),
            "cetus" => Ok(Self::Cetus),
            "suilend" => Ok(Self::Suilend),
            "scallop" => Ok(Self::Scallop),
            other => Err(anyhow!(
                "invalid protocol '{}': expected one of {}",
                other,
                Self::SUPPORTED
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Deepbook => "deepbook",
            Self::Cetus => "cetus",
            Self::Suilend => "suilend",
            Self::Scallop => "scallop",
        }
    }
}

fn requires_package_id_error(protocol: ProtocolAdapter) -> anyhow::Error {
    anyhow!(
        "protocol `{}` requires --package-id (no built-in protocol package defaults)",
        protocol.as_str()
    )
}

/// Resolve required package id for protocol prepare/run flows.
///
/// `package_id` is required for all protocols, including `generic`.
pub fn resolve_required_package_id(
    protocol: ProtocolAdapter,
    package_id: Option<&str>,
) -> Result<String> {
    let raw = package_id.ok_or_else(|| requires_package_id_error(protocol))?;
    normalize_package_id(raw)
}

/// Resolve optional package filter for protocol discovery flows.
///
/// `generic` allows no package filter. Non-generic protocols require one.
pub fn resolve_discovery_package_filter(
    protocol: ProtocolAdapter,
    package_id: Option<&str>,
) -> Result<Option<String>> {
    if let Some(raw) = package_id {
        return normalize_package_id(raw).map(Some);
    }
    if protocol == ProtocolAdapter::Generic {
        return Ok(None);
    }
    Err(requires_package_id_error(protocol))
}

#[cfg(test)]
mod tests {
    use super::{resolve_discovery_package_filter, resolve_required_package_id, ProtocolAdapter};

    #[test]
    fn parses_known_protocols() {
        assert_eq!(
            ProtocolAdapter::parse("deepbook").expect("parse"),
            ProtocolAdapter::Deepbook
        );
        assert_eq!(
            ProtocolAdapter::parse("GENERIC").expect("parse"),
            ProtocolAdapter::Generic
        );
    }

    #[test]
    fn generic_discovery_allows_none() {
        let filter = resolve_discovery_package_filter(ProtocolAdapter::Generic, None)
            .expect("generic should allow missing package filter");
        assert!(filter.is_none());
    }

    #[test]
    fn non_generic_requires_package_id() {
        let err = resolve_required_package_id(ProtocolAdapter::Deepbook, None)
            .expect_err("should require package id");
        assert!(err.to_string().contains("requires --package-id"));
    }
}
