#![allow(unused_imports)]
//! Shared test utilities for integration tests.
//!
//! This module provides common functionality used across test files to avoid
//! code duplication and ensure consistent test patterns.
//!
//! # Modules
//!
//! - `fixtures`: Fixture loading utilities (resolver, modules)
//! - `mocks`: Mock object creation helpers (coins, objects)
//! - `helpers`: Common test helper functions
//! - `assertions`: Custom assertion macros and utilities
//! - `network`: Network-dependent test utilities
//! - `setup`: High-level test setup helpers

pub mod assertions;
pub mod fixtures;
pub mod helpers;
pub mod mocks;
pub mod network;
pub mod setup;

// Re-export commonly used items for convenience
pub use fixtures::{empty_resolver, framework_resolver, load_fixture_resolver};
pub use helpers::{find_module_by_name, find_test_module, format_module_path, make_module_id};
pub use mocks::{create_mock_coin, get_coin_balance};
pub use network::get_grpc_endpoint;
pub use setup::{fixture_with_module_details, fixture_with_test_module, harness_with_fixture};

// Re-export assertion helpers for better test error messages
pub use assertions::{
    assert_bytes_eq, assert_err, assert_error_contains, assert_error_contains_any, assert_in_range,
    assert_not_empty, assert_ok,
};
