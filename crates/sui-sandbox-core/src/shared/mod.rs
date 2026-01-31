//! Shared utilities for CLI and MCP integration.
//!
//! This module provides shared abstractions that can be used by both the CLI
//! and MCP server to ensure feature parity and reduce code duplication.
//!
//! # Modules
//!
//! - [`parsing`] - Shared argument and value parsing utilities
//! - [`response`] - Unified tool response types
//! - [`identifiers`] - Safe identifier creation utilities
//! - [`tools`] - Shared tool registry and trait definitions
//! - [`encoding`] - Base64 encoding/decoding utilities
//! - [`address`] - Address parsing and formatting utilities

pub mod address;
pub mod encoding;
pub mod identifiers;
pub mod parsing;
pub mod response;
pub mod tools;

pub use address::{
    format_address, format_address_short, parse_address, parse_address_or_zero, try_parse_address,
};
pub use encoding::{decode_b64, decode_b64_no_pad_opt, decode_b64_opt, encode_b64};
pub use identifiers::*;
pub use parsing::*;
pub use response::*;
pub use tools::{Tool, ToolContext, ToolInput, ToolRegistry};
