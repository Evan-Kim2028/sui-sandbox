#![allow(clippy::large_enum_variant)]
#![allow(clippy::doc_overindented_list_items)]
//! gRPC Client for Sui Network
//!
//! Provides real-time streaming access to Sui blockchain data via gRPC.
//! Use for checkpoint subscriptions and high-performance data fetching.
//!
//! ## Capabilities
//!
//! - **Streaming subscriptions** - Subscribe to new checkpoints as they're finalized
//! - **Batch fetching** - Efficiently fetch multiple objects/transactions at once
//! - **Full PTB data** - Complete transaction inputs, commands, effects with type arguments
//!
//! ## Endpoints
//!
//! - Mainnet: `https://mainnet.sui.io:443`
//! - Testnet: `https://testnet.sui.io:443`
//!
//! ## Usage
//!
//! ```ignore
//! use sui_move_interface_extractor::grpc::GrpcClient;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let client = GrpcClient::mainnet().await?;
//!
//!     // Subscribe to checkpoints (streaming)
//!     let mut stream = client.subscribe_checkpoints().await?;
//!     while let Some(checkpoint) = stream.next().await {
//!         println!("Checkpoint {}: {} transactions",
//!             checkpoint.sequence_number,
//!             checkpoint.transactions.len());
//!     }
//!
//!     Ok(())
//! }
//! ```

// Generated proto modules
// The sui.rpc.v2 module references google.rpc via `super::super::super::super::google::rpc`
// so we need the google module at the crate root level

pub mod generated {
    pub mod sui_rpc_v2 {
        include!("generated/sui.rpc.v2.rs");
    }
}

// google.rpc needs to be accessible from the path the generated code expects
pub mod google {
    pub mod rpc {
        include!("generated/google.rpc.rs");
    }
}

mod client;

pub use client::*;
