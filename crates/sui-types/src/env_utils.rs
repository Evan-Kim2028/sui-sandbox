//! Environment variable parsing utilities.
//!
//! This module provides type-safe utilities for parsing environment variables
//! with default values, eliminating repeated boilerplate patterns like:
//!
//! ```ignore
//! std::env::var("VAR_NAME")
//!     .ok()
//!     .and_then(|v| v.parse::<u64>().ok())
//!     .unwrap_or(default_value)
//! ```
//!
//! # Example
//!
//! ```
//! use sui_sandbox_types::env_utils::{env_var, env_var_or};
//!
//! // Parse with default value
//! let timeout: u64 = env_var_or("TIMEOUT_MS", 5000);
//!
//! // Parse returning Option
//! let custom: Option<u64> = env_var("CUSTOM_VALUE");
//!
//! // Boolean environment variables
//! use sui_sandbox_types::env_utils::env_bool;
//! let debug_enabled = env_bool("DEBUG_MODE");
//! ```

use std::str::FromStr;

/// Parse an environment variable into a type that implements `FromStr`.
///
/// Returns `None` if the variable is not set or cannot be parsed.
///
/// # Example
///
/// ```
/// use sui_sandbox_types::env_utils::env_var;
///
/// let value: Option<u64> = env_var("MY_VAR");
/// ```
pub fn env_var<T: FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

/// Parse an environment variable with a default value.
///
/// Returns the default if the variable is not set or cannot be parsed.
///
/// # Example
///
/// ```
/// use sui_sandbox_types::env_utils::env_var_or;
///
/// let timeout: u64 = env_var_or("TIMEOUT_MS", 5000);
/// let retries: usize = env_var_or("MAX_RETRIES", 3);
/// ```
pub fn env_var_or<T: FromStr>(key: &str, default: T) -> T {
    env_var(key).unwrap_or(default)
}

/// Check if an environment variable is set to a truthy value.
///
/// Returns `true` if the variable is set to "1", "true", "yes", or "on" (case-insensitive).
/// Returns `false` otherwise.
///
/// # Example
///
/// ```
/// use sui_sandbox_types::env_utils::env_bool;
///
/// let debug = env_bool("DEBUG_MODE");
/// let verbose = env_bool("VERBOSE");
/// ```
pub fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Check if an environment variable is set to a truthy value, with a default.
///
/// # Example
///
/// ```
/// use sui_sandbox_types::env_utils::env_bool_or;
///
/// // Default to true if not set
/// let logging = env_bool_or("ENABLE_LOGGING", true);
/// ```
pub fn env_bool_or(key: &str, default: bool) -> bool {
    match std::env::var(key).ok() {
        Some(v) => matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        None => default,
    }
}

/// Get an environment variable as a string with a default value.
///
/// # Example
///
/// ```
/// use sui_sandbox_types::env_utils::env_string_or;
///
/// let endpoint = env_string_or("API_ENDPOINT", "https://api.example.com");
/// ```
pub fn env_string_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Parse a comma-separated environment variable into a vector.
///
/// Returns an empty vector if the variable is not set.
///
/// # Example
///
/// ```
/// use sui_sandbox_types::env_utils::env_list;
///
/// // If PACKAGES="0x1,0x2,0x3" then returns vec!["0x1", "0x2", "0x3"]
/// let packages: Vec<String> = env_list("PACKAGES");
/// ```
pub fn env_list(key: &str) -> Vec<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_var_parsing() {
        std::env::set_var("TEST_U64", "42");
        let val: Option<u64> = env_var("TEST_U64");
        assert_eq!(val, Some(42));

        let missing: Option<u64> = env_var("NONEXISTENT_VAR_12345");
        assert_eq!(missing, None);

        std::env::remove_var("TEST_U64");
    }

    #[test]
    fn test_env_var_or() {
        std::env::set_var("TEST_WITH_DEFAULT", "100");
        let val: u64 = env_var_or("TEST_WITH_DEFAULT", 50);
        assert_eq!(val, 100);

        let default_val: u64 = env_var_or("NONEXISTENT_VAR_12346", 50);
        assert_eq!(default_val, 50);

        std::env::remove_var("TEST_WITH_DEFAULT");
    }

    #[test]
    fn test_env_bool() {
        std::env::set_var("TEST_BOOL_TRUE", "true");
        std::env::set_var("TEST_BOOL_1", "1");
        std::env::set_var("TEST_BOOL_YES", "YES");
        std::env::set_var("TEST_BOOL_FALSE", "false");

        assert!(env_bool("TEST_BOOL_TRUE"));
        assert!(env_bool("TEST_BOOL_1"));
        assert!(env_bool("TEST_BOOL_YES"));
        assert!(!env_bool("TEST_BOOL_FALSE"));
        assert!(!env_bool("NONEXISTENT_VAR_12347"));

        std::env::remove_var("TEST_BOOL_TRUE");
        std::env::remove_var("TEST_BOOL_1");
        std::env::remove_var("TEST_BOOL_YES");
        std::env::remove_var("TEST_BOOL_FALSE");
    }

    #[test]
    fn test_env_string_or() {
        std::env::set_var("TEST_STRING", "hello");
        assert_eq!(env_string_or("TEST_STRING", "default"), "hello");
        assert_eq!(env_string_or("NONEXISTENT_VAR_12348", "default"), "default");
        std::env::remove_var("TEST_STRING");
    }

    #[test]
    fn test_env_list() {
        std::env::set_var("TEST_LIST", "a, b, c");
        let list = env_list("TEST_LIST");
        assert_eq!(list, vec!["a", "b", "c"]);

        let empty = env_list("NONEXISTENT_VAR_12349");
        assert!(empty.is_empty());

        std::env::remove_var("TEST_LIST");
    }
}
