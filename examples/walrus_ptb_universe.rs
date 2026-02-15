//! Checkpoint-source PTB universe example entrypoint.
//!
//! The reusable engine now lives in `sui_sandbox_core::ptb_universe`.

use anyhow::Result;

fn main() -> Result<()> {
    sui_sandbox_core::ptb_universe::run_cli()
}
