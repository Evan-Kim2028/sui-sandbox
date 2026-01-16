//! Clock-related sandbox handlers.
//!
//! Handles get_clock and set_clock operations for timestamp management.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;

/// Get the current Clock timestamp.
pub fn execute_get_clock(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Getting Clock timestamp");
    }

    let timestamp_ms = env.get_clock_timestamp_ms();

    // Convert to human-readable datetime string
    let datetime_str = {
        let seconds = (timestamp_ms / 1000) as i64;
        let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
        // Simple ISO 8601 format approximation
        let days_since_epoch = seconds / 86400;
        let remaining_seconds = seconds % 86400;
        let hours = remaining_seconds / 3600;
        let minutes = (remaining_seconds % 3600) / 60;
        let secs = remaining_seconds % 60;

        // Approximate year calculation (not accounting for leap years perfectly)
        let year = 1970 + (days_since_epoch / 365);

        format!(
            "~{}-??-?? {:02}:{:02}:{:02}.{:03} UTC (approx)",
            year,
            hours,
            minutes,
            secs,
            nanos / 1_000_000
        )
    };

    SandboxResponse::success_with_data(serde_json::json!({
        "clock_object_id": crate::benchmark::simulation::CLOCK_OBJECT_ID,
        "timestamp_ms": timestamp_ms,
        "datetime_approx": datetime_str,
    }))
}

/// Advance the Clock to a new timestamp.
pub fn execute_set_clock(
    env: &mut SimulationEnvironment,
    timestamp_ms: u64,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Setting Clock timestamp to {} ms", timestamp_ms);
    }

    match env.advance_clock(timestamp_ms) {
        Ok(()) => SandboxResponse::success_with_data(serde_json::json!({
            "clock_object_id": crate::benchmark::simulation::CLOCK_OBJECT_ID,
            "timestamp_ms": timestamp_ms,
            "message": format!("Clock advanced to {} ms", timestamp_ms),
        })),
        Err(e) => SandboxResponse::error(format!("Failed to set clock: {}", e)),
    }
}
