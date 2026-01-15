// Allow clippy lints that are intentional or impractical to fix
#![allow(clippy::result_large_err)] // Failure struct is intentionally rich for debugging
#![allow(clippy::type_complexity)] // Complex types are sometimes clearer than type aliases
#![allow(clippy::too_many_arguments)] // Some functions need many parameters

pub mod args;
pub mod benchmark;
pub mod bytecode;
pub mod comparator;
pub mod corpus;
pub mod move_stubs;
pub mod normalization;
pub mod rpc;
pub mod runner;
pub mod types;
pub mod utils;
