//! Event-related sandbox handlers.
//!
//! Handles list_events, get_events_by_type, get_last_tx_events, and clear_events operations.

use crate::benchmark::sandbox::types::{EventResponse, SandboxResponse};
use crate::benchmark::simulation::SimulationEnvironment;

/// List all events emitted during this session.
pub fn execute_list_events(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Listing all events ({})", env.event_count());
    }

    let events: Vec<EventResponse> = env
        .get_all_events()
        .iter()
        .map(|e| EventResponse {
            event_type: e.type_tag.clone(),
            data_hex: hex::encode(&e.data),
            sequence: e.sequence,
        })
        .collect();

    let count = events.len();
    SandboxResponse::success_with_data(serde_json::json!({
        "events": events,
        "count": count
    }))
}

/// Get events filtered by type.
pub fn execute_get_events_by_type(
    env: &SimulationEnvironment,
    type_prefix: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Getting events by type prefix: {}", type_prefix);
    }

    let events: Vec<EventResponse> = env
        .get_events_by_type(type_prefix)
        .iter()
        .map(|e| EventResponse {
            event_type: e.type_tag.clone(),
            data_hex: hex::encode(&e.data),
            sequence: e.sequence,
        })
        .collect();

    let count = events.len();
    SandboxResponse::success_with_data(serde_json::json!({
        "events": events,
        "count": count,
        "type_prefix": type_prefix
    }))
}

/// Get events from the last PTB execution.
pub fn execute_get_last_tx_events(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Getting events from last transaction");
    }

    let events: Vec<EventResponse> = env
        .get_last_tx_events()
        .iter()
        .map(|e| EventResponse {
            event_type: e.type_tag.clone(),
            data_hex: hex::encode(&e.data),
            sequence: e.sequence,
        })
        .collect();

    let count = events.len();
    SandboxResponse::success_with_data(serde_json::json!({
        "events": events,
        "count": count
    }))
}

/// Clear all captured events.
pub fn execute_clear_events(env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Clearing all events");
    }

    let previous_count = env.event_count();
    env.clear_events();

    SandboxResponse::success_with_data(serde_json::json!({
        "cleared": true,
        "previous_count": previous_count
    }))
}
