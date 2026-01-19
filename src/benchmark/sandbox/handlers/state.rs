//! State management sandbox handlers.
//!
//! Handles get_state, reset, get_lamport_clock, advance_lamport_clock, set_sender, get_sender,
//! save_state, load_state, and list_available_tools operations.

use crate::benchmark::sandbox::types::SandboxResponse;
use crate::benchmark::simulation::SimulationEnvironment;
use move_core_types::account_address::AccountAddress;
use std::path::Path;

/// Get current sandbox state (loaded modules, objects).
pub fn execute_get_state(env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Getting sandbox state");
    }

    let state = env.get_state_summary();

    SandboxResponse::success_with_data(serde_json::json!({
        "loaded_packages": state.loaded_packages,
        "loaded_modules": state.loaded_modules,
        "object_count": state.object_count,
        "sender": state.sender,
        "timestamp_ms": state.timestamp_ms,
    }))
}

/// Reset sandbox to initial state.
pub fn execute_reset(env: &mut SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Resetting sandbox");
    }

    match env.reset() {
        Ok(_) => SandboxResponse::success(),
        Err(e) => SandboxResponse::error(format!("Failed to reset: {}", e)),
    }
}

/// Get the current lamport clock value.
pub fn execute_get_lamport_clock(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Getting lamport clock value");
    }

    let clock = env.lamport_clock();
    SandboxResponse::success_with_data(serde_json::json!({
        "lamport_clock": clock,
        "description": "Current lamport clock value used for shared object versioning"
    }))
}

/// Manually advance the lamport clock.
pub fn execute_advance_lamport_clock(
    env: &mut SimulationEnvironment,
    verbose: bool,
) -> SandboxResponse {
    let previous = env.lamport_clock();
    let new_value = env.advance_lamport_clock();

    if verbose {
        eprintln!("Advanced lamport clock: {} -> {}", previous, new_value);
    }

    SandboxResponse::success_with_data(serde_json::json!({
        "previous_value": previous,
        "new_value": new_value
    }))
}

/// Set the transaction sender address.
pub fn execute_set_sender(
    env: &mut SimulationEnvironment,
    address: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Setting sender to: {}", address);
    }

    // Parse the address
    let addr = match AccountAddress::from_hex_literal(address) {
        Ok(a) => a,
        Err(e) => {
            return SandboxResponse::error(format!("Invalid address '{}': {}", address, e));
        }
    };

    env.set_sender(addr);

    SandboxResponse::success_with_data(serde_json::json!({
        "sender": addr.to_hex_literal()
    }))
}

/// Get the current transaction sender address.
pub fn execute_get_sender(env: &SimulationEnvironment, verbose: bool) -> SandboxResponse {
    let sender = env.sender();

    if verbose {
        eprintln!("Current sender: {}", sender.to_hex_literal());
    }

    SandboxResponse::success_with_data(serde_json::json!({
        "sender": sender.to_hex_literal()
    }))
}

/// Save the current sandbox state to a file.
pub fn execute_save_state(
    env: &SimulationEnvironment,
    path: &str,
    description: Option<&str>,
    tags: Option<&[String]>,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Saving state to: {}", path);
    }

    let path = Path::new(path);

    // Use save_state_with_metadata if we have metadata, otherwise use save_state
    let result = if description.is_some() || tags.is_some() {
        env.save_state_with_metadata(
            path,
            description.map(String::from),
            tags.map(|t| t.to_vec()).unwrap_or_default(),
        )
    } else {
        env.save_state(path)
    };

    match result {
        Ok(_) => {
            // Get some stats about what was saved
            let state = env.get_state_summary();
            SandboxResponse::success_with_data(serde_json::json!({
                "path": path.to_string_lossy(),
                "version": 4,  // Current state format version
                "objects_count": state.object_count,
                "modules_count": state.loaded_modules.len(),
            }))
        }
        Err(e) => SandboxResponse::error(format!("Failed to save state: {}", e)),
    }
}

/// Load a previously saved sandbox state from a file.
pub fn execute_load_state(
    env: &mut SimulationEnvironment,
    path: &str,
    verbose: bool,
) -> SandboxResponse {
    if verbose {
        eprintln!("Loading state from: {}", path);
    }

    let path = Path::new(path);

    match env.load_state(path) {
        Ok(_) => {
            let state = env.get_state_summary();
            SandboxResponse::success_with_data(serde_json::json!({
                "path": path.to_string_lossy(),
                "objects_count": state.object_count,
                "modules_count": state.loaded_modules.len(),
                "sender": state.sender,
            }))
        }
        Err(e) => SandboxResponse::error(format!("Failed to load state: {}", e)),
    }
}

/// Generate unified schema of all available sandbox tools.
/// This is the single source of truth for LLM tool discovery.
pub fn execute_list_available_tools(verbose: bool) -> SandboxResponse {
    if verbose {
        eprintln!("Generating tool discovery schema");
    }

    let tools = serde_json::json!({
        "version": "1.0",
        "description": "Sui Move VM Sandbox - All available tools for LLM agents",
        "categories": {
            "state_management": {
                "description": "Tools for managing sandbox state",
                "tools": [
                    {
                        "action": "reset",
                        "description": "Reset sandbox to initial state. Clears all loaded modules, objects, and coins.",
                        "params": {},
                        "example": {"action": "reset"}
                    },
                    {
                        "action": "get_state",
                        "description": "Get current sandbox state summary (loaded modules, objects, coins).",
                        "params": {},
                        "example": {"action": "get_state"}
                    },
                    {
                        "action": "set_sender",
                        "description": "Set the transaction sender address for TxContext.",
                        "params": {
                            "address": "string - sender address (hex, e.g., '0x123...')"
                        },
                        "example": {"action": "set_sender", "address": "0x123..."}
                    },
                    {
                        "action": "get_sender",
                        "description": "Get the current transaction sender address.",
                        "params": {},
                        "example": {"action": "get_sender"}
                    },
                    {
                        "action": "save_state",
                        "description": "Save the current sandbox state to a file for later restoration.",
                        "params": {
                            "path": "string - file path to save state",
                            "description": "string (optional) - description of the saved state",
                            "tags": "array (optional) - tags for categorizing the state"
                        },
                        "example": {"action": "save_state", "path": "/tmp/state.json"}
                    },
                    {
                        "action": "load_state",
                        "description": "Load a previously saved sandbox state from a file.",
                        "params": {
                            "path": "string - path to the state file"
                        },
                        "example": {"action": "load_state", "path": "/tmp/state.json"}
                    }
                ]
            },
            "module_operations": {
                "description": "Tools for loading and inspecting Move modules",
                "tools": [
                    {
                        "action": "load_module",
                        "description": "Load compiled Move module(s) from bytecode file(s).",
                        "params": {
                            "bytecode_path": "string - path to .mv file or directory containing .mv files",
                            "module_name": "string (optional) - filter to only load modules matching this name"
                        },
                        "example": {"action": "load_module", "bytecode_path": "/path/to/bytecode"}
                    },
                    {
                        "action": "compile_move",
                        "description": "Compile Move source code to bytecode and load it.",
                        "params": {
                            "package_name": "string - package name for address resolution",
                            "module_name": "string - module name",
                            "source": "string - Move source code"
                        }
                    },
                    {
                        "action": "list_modules",
                        "description": "List all loaded modules.",
                        "params": {}
                    }
                ]
            },
            "type_introspection": {
                "description": "Tools for inspecting Move types and functions",
                "tools": [
                    {
                        "action": "list_structs",
                        "description": "List all structs in a module.",
                        "params": {
                            "module_path": "string - module path (e.g., '0x2::coin')"
                        }
                    },
                    {
                        "action": "get_struct_info",
                        "description": "Get detailed struct type definition.",
                        "params": {
                            "type_path": "string - full type path (e.g., '0x2::coin::Coin')"
                        }
                    },
                    {
                        "action": "list_functions",
                        "description": "List all functions in a module.",
                        "params": {
                            "module_path": "string - module path"
                        }
                    },
                    {
                        "action": "get_function_info",
                        "description": "Get detailed function information.",
                        "params": {
                            "module_path": "string - module path",
                            "function_name": "string - function name"
                        }
                    },
                    {
                        "action": "find_constructors",
                        "description": "Find functions that construct a type.",
                        "params": {
                            "type_path": "string - type to find constructors for"
                        }
                    },
                    {
                        "action": "search_types",
                        "description": "Search for types matching a pattern.",
                        "params": {
                            "pattern": "string - pattern with * wildcard",
                            "ability_filter": "string (optional) - filter by ability (e.g., 'key', 'store')"
                        }
                    },
                    {
                        "action": "search_functions",
                        "description": "Search for functions matching a pattern.",
                        "params": {
                            "pattern": "string - pattern with * wildcard",
                            "entry_only": "bool (optional) - only return entry functions"
                        }
                    },
                    {
                        "action": "validate_type",
                        "description": "Validate a type string.",
                        "params": {
                            "type_str": "string - type string to validate"
                        }
                    }
                ]
            },
            "object_management": {
                "description": "Tools for creating and inspecting objects",
                "tools": [
                    {
                        "action": "create_object",
                        "description": "Create an object with specific field values.",
                        "params": {
                            "object_type": "string - full type string",
                            "fields": "object - field values as JSON",
                            "object_id": "string (optional) - specific object ID"
                        }
                    },
                    {
                        "action": "list_objects",
                        "description": "List all objects in the sandbox.",
                        "params": {}
                    },
                    {
                        "action": "inspect_object",
                        "description": "Inspect an object's current state.",
                        "params": {
                            "object_id": "string - object ID (hex)"
                        }
                    },
                    {
                        "action": "list_shared_objects",
                        "description": "List all shared objects and their lock status.",
                        "params": {}
                    }
                ]
            },
            "execution": {
                "description": "Tools for executing Move code",
                "tools": [
                    {
                        "action": "execute_ptb",
                        "description": "Execute a Programmable Transaction Block.",
                        "params": {
                            "inputs": "array - PTB inputs (pure values, objects, gas)",
                            "commands": "array - PTB commands to execute"
                        }
                    },
                    {
                        "action": "validate_ptb",
                        "description": "Validate a PTB without executing it.",
                        "params": {
                            "inputs": "array - PTB inputs",
                            "commands": "array - PTB commands"
                        }
                    },
                    {
                        "action": "call_function",
                        "description": "Call a specific Move function directly.",
                        "params": {
                            "package": "string - package address",
                            "module": "string - module name",
                            "function": "string - function name",
                            "type_args": "array - type arguments",
                            "args": "array - function arguments"
                        }
                    }
                ]
            },
            "encoding": {
                "description": "Tools for BCS encoding/decoding",
                "tools": [
                    {
                        "action": "encode_bcs",
                        "description": "Encode a value to BCS bytes.",
                        "params": {
                            "type_str": "string - Move type",
                            "value": "any - value to encode"
                        }
                    },
                    {
                        "action": "decode_bcs",
                        "description": "Decode BCS bytes to a value.",
                        "params": {
                            "type_str": "string - Move type",
                            "bytes_hex": "string - hex-encoded BCS bytes"
                        }
                    },
                    {
                        "action": "encode_vector",
                        "description": "Encode an array as a BCS vector.",
                        "params": {
                            "element_type": "string - element type",
                            "values": "array - values to encode"
                        }
                    }
                ]
            },
            "clock": {
                "description": "Tools for timestamp management",
                "tools": [
                    {
                        "action": "get_clock",
                        "description": "Get the current Clock timestamp.",
                        "params": {}
                    },
                    {
                        "action": "set_clock",
                        "description": "Advance the Clock to a new timestamp.",
                        "params": {
                            "timestamp_ms": "u64 - new timestamp in milliseconds"
                        }
                    }
                ]
            },
            "coins": {
                "description": "Tools for coin management",
                "tools": [
                    {
                        "action": "register_coin",
                        "description": "Register a custom coin with metadata.",
                        "params": {
                            "coin_type": "string - full coin type",
                            "decimals": "u8 - decimal places",
                            "symbol": "string - coin symbol",
                            "name": "string - coin name"
                        }
                    },
                    {
                        "action": "get_coin_metadata",
                        "description": "Get metadata for a registered coin.",
                        "params": {
                            "coin_type": "string - coin type to lookup"
                        }
                    },
                    {
                        "action": "list_coins",
                        "description": "List all registered coins.",
                        "params": {}
                    }
                ]
            },
            "utility": {
                "description": "General utility tools",
                "tools": [
                    {
                        "action": "generate_id",
                        "description": "Generate a fresh unique ID.",
                        "params": {}
                    },
                    {
                        "action": "parse_address",
                        "description": "Parse and validate an address string.",
                        "params": {
                            "address": "string - address to parse"
                        }
                    },
                    {
                        "action": "format_address",
                        "description": "Format an address to different representations.",
                        "params": {
                            "address": "string - address to format",
                            "format": "string (optional) - 'short', 'full', or 'no_prefix'"
                        }
                    },
                    {
                        "action": "compute_hash",
                        "description": "Compute a cryptographic hash.",
                        "params": {
                            "bytes_hex": "string - hex bytes to hash",
                            "algorithm": "string (optional) - 'sha256', 'sha3_256', or 'blake2b_256'"
                        }
                    },
                    {
                        "action": "convert_number",
                        "description": "Convert between numeric types.",
                        "params": {
                            "value": "string - numeric value",
                            "from_type": "string - source type",
                            "to_type": "string - target type"
                        }
                    }
                ]
            },
            "events": {
                "description": "Tools for event queries",
                "tools": [
                    {
                        "action": "list_events",
                        "description": "List all emitted events.",
                        "params": {}
                    },
                    {
                        "action": "get_events_by_type",
                        "description": "Get events filtered by type prefix.",
                        "params": {
                            "type_prefix": "string - type prefix to filter"
                        }
                    },
                    {
                        "action": "get_last_tx_events",
                        "description": "Get events from the last transaction.",
                        "params": {}
                    },
                    {
                        "action": "clear_events",
                        "description": "Clear all captured events.",
                        "params": {}
                    }
                ]
            },
            "cache": {
                "description": "Tools for cached object management",
                "tools": [
                    {
                        "action": "load_cached_objects",
                        "description": "Load multiple cached objects from base64 BCS.",
                        "params": {
                            "objects": "object - map of object_id to base64 bytes",
                            "object_types": "object (optional) - map of object_id to type string",
                            "shared_object_ids": "array (optional) - IDs of shared objects"
                        }
                    },
                    {
                        "action": "load_cached_object",
                        "description": "Load a single cached object.",
                        "params": {
                            "object_id": "string - object ID",
                            "bcs_bytes": "string - base64 BCS bytes",
                            "object_type": "string (optional) - type string",
                            "is_shared": "bool (optional) - whether object is shared"
                        }
                    },
                    {
                        "action": "list_cached_objects",
                        "description": "List all loaded cached objects.",
                        "params": {}
                    },
                    {
                        "action": "is_framework_cached",
                        "description": "Check if Sui framework is cached.",
                        "params": {}
                    },
                    {
                        "action": "ensure_framework_cached",
                        "description": "Download and cache Sui framework if needed.",
                        "params": {}
                    }
                ]
            },
            "bytecode": {
                "description": "Tools for bytecode analysis",
                "tools": [
                    {
                        "action": "disassemble_function",
                        "description": "Disassemble a function to bytecode.",
                        "params": {
                            "module_path": "string - module path",
                            "function_name": "string - function name"
                        }
                    },
                    {
                        "action": "disassemble_module",
                        "description": "Disassemble an entire module.",
                        "params": {
                            "module_path": "string - module path"
                        }
                    },
                    {
                        "action": "module_summary",
                        "description": "Get human-readable module summary.",
                        "params": {
                            "module_path": "string - module path"
                        }
                    },
                    {
                        "action": "get_module_dependencies",
                        "description": "Get module dependency graph.",
                        "params": {
                            "module_path": "string - module path"
                        }
                    }
                ]
            },
            "shared_objects": {
                "description": "Tools for shared object versioning",
                "tools": [
                    {
                        "action": "get_lamport_clock",
                        "description": "Get current lamport clock value.",
                        "params": {}
                    },
                    {
                        "action": "get_shared_object_info",
                        "description": "Get detailed shared object info.",
                        "params": {
                            "object_id": "string - object ID"
                        }
                    },
                    {
                        "action": "list_shared_locks",
                        "description": "List all shared object locks.",
                        "params": {}
                    },
                    {
                        "action": "advance_lamport_clock",
                        "description": "Manually advance lamport clock.",
                        "params": {}
                    }
                ]
            },
            "system": {
                "description": "System object information",
                "tools": [
                    {
                        "action": "get_system_object_info",
                        "description": "Get system object details.",
                        "params": {
                            "object_name": "string - 'clock', 'random', 'deny_list', or 'system_state'"
                        }
                    }
                ]
            },
            "error_handling": {
                "description": "Error parsing tools",
                "tools": [
                    {
                        "action": "parse_error",
                        "description": "Parse an error string for structured info.",
                        "params": {
                            "error": "string - error message to parse"
                        }
                    }
                ]
            },
            "mainnet_import": {
                "description": "Import real mainnet state into the sandbox for accurate simulation. Use sparingly - fetches data from Sui network.",
                "tools": [
                    {
                        "action": "import_package_from_mainnet",
                        "description": "Import a package from Sui mainnet. Fetches bytecode and deploys it locally at the same address.",
                        "params": {
                            "package_id": "string - package address on mainnet (e.g., '0xd22b24490e0bae52676651b4f56660a5ff8022a2576e0089f79b3c88d44e08f0')",
                            "network": "string (optional) - 'mainnet' or 'testnet', default: 'mainnet'"
                        },
                        "example": {"action": "import_package_from_mainnet", "package_id": "0x2"}
                    },
                    {
                        "action": "import_object_from_mainnet",
                        "description": "Import an object from Sui mainnet. Fetches current state (type, fields, ownership) and loads it at the same address.",
                        "params": {
                            "object_id": "string - object ID on mainnet",
                            "network": "string (optional) - 'mainnet' or 'testnet', default: 'mainnet'"
                        },
                        "example": {"action": "import_object_from_mainnet", "object_id": "0x6"}
                    },
                    {
                        "action": "import_objects_from_mainnet",
                        "description": "Import multiple objects from mainnet in a batch. More efficient than individual imports.",
                        "params": {
                            "object_ids": "array - list of object IDs to import",
                            "network": "string (optional) - 'mainnet' or 'testnet', default: 'mainnet'"
                        }
                    },
                    {
                        "action": "import_object_at_version",
                        "description": "Import an object at a specific historical version. Useful for replaying transactions.",
                        "params": {
                            "object_id": "string - object ID",
                            "version": "u64 - specific version to fetch",
                            "network": "string (optional) - 'mainnet' or 'testnet', default: 'mainnet'"
                        }
                    }
                ]
            }
        }
    });

    SandboxResponse::success_with_data(tools)
}
