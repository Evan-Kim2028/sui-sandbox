//! Sandbox Types
//!
//! Common types used across the sandbox simulation environment.

use serde::{Deserialize, Serialize};

/// A synthesized Move object with its BCS-encoded bytes.
/// Used for injecting pre-built objects into the simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesizedObject {
    pub object_id: String,
    pub type_path: String,
    /// BCS-encoded bytes
    pub bcs_bytes: Vec<u8>,
    pub is_shared: bool,
}
