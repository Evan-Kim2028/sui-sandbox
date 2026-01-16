//! # Sandbox Execution Interface for LLM Integration
//!
//! **This is the canonical API for LLM agents to interact with the Move VM sandbox.**
//!
//! ## Design Philosophy
//!
//! 1. **Single Entry Point**: All LLM interactions go through [`SandboxRequest`] /
//!    [`execute_request`]. Use `{"action": "list_available_tools"}` to discover
//!    all available operations.
//!
//! 2. **Neutral and Unopinionated**: The API provides facts, not guidance. Error
//!    messages describe what happened without suggesting fixes. Tool descriptions
//!    explain what each tool does without recommending when to use it. This ensures
//!    unbiased evaluation of LLM reasoning capabilities.
//!
//! 3. **Stateful via SimulationEnvironment**: All operations share state through
//!    [`SimulationEnvironment`]. Loading a module makes it available for execution.
//!    Creating an object makes it available for PTBs. State persists across requests.
//!
//! 4. **JSON In, JSON Out**: Requests and responses are JSON for easy integration
//!    with any language. The CLI (`sandbox-exec --interactive`) reads JSON lines
//!    from stdin and writes JSON responses to stdout.
//!
//! ## Supported Operations
//!
//! - **Module operations**: load_module, compile_move, list_modules
//! - **Type introspection**: list_structs, get_struct_info, list_functions, get_function_info
//! - **Object management**: create_object, list_objects, inspect_object
//! - **Execution**: execute_ptb, call_function
//! - **Utilities**: encode_bcs, decode_bcs, validate_type, get_clock, set_clock
//!
//! See `list_available_tools` for the complete schema.

pub mod cli;
pub mod handlers;
pub mod types;

// Re-export types for backwards compatibility
pub use types::*;

// Re-export CLI entry point
pub use cli::run_sandbox_exec;

use crate::benchmark::simulation::SimulationEnvironment;

// Import all handlers
use handlers::{
    bytecode, cache, clock, coins, encoding, events, execution, introspection, mainnet, module,
    objects, state, utils,
};

/// Execute a sandbox request and return a response.
///
/// This is the main entry point for LLM agents to interact with the sandbox.
/// All operations are dispatched through this function based on the request action.
pub fn execute_request(
    env: &mut SimulationEnvironment,
    request: &SandboxRequest,
    verbose: bool,
) -> SandboxResponse {
    match request {
        // Module operations
        SandboxRequest::LoadModule {
            bytecode_path,
            module_name,
        } => module::execute_load_module(env, bytecode_path, module_name.as_deref(), verbose),
        SandboxRequest::CompileMove {
            package_name,
            module_name,
            source,
        } => module::execute_compile_move(env, package_name, module_name, source, verbose),
        SandboxRequest::ListModules => module::execute_list_modules(env, verbose),

        // Object operations
        SandboxRequest::CreateObject {
            object_type,
            fields,
            object_id,
        } => {
            objects::execute_create_object(env, object_type, fields, object_id.as_deref(), verbose)
        }
        SandboxRequest::InspectObject { object_id } => {
            objects::execute_inspect_object(env, object_id, verbose)
        }
        SandboxRequest::ListObjects => objects::execute_list_objects(env, verbose),
        SandboxRequest::ListSharedObjects => objects::execute_list_shared_objects(env, verbose),
        SandboxRequest::GetSharedObjectInfo { object_id } => {
            objects::execute_get_shared_object_info(env, object_id, verbose)
        }
        SandboxRequest::ListSharedLocks => objects::execute_list_shared_locks(env, verbose),
        SandboxRequest::CreateTestObject { type_tag, value } => {
            objects::execute_create_test_object(env, type_tag, value, verbose)
        }

        // Execution operations
        SandboxRequest::ExecutePtb { inputs, commands } => {
            execution::execute_ptb_command(env, inputs, commands, verbose)
        }
        SandboxRequest::ValidatePtb { inputs, commands } => {
            execution::execute_validate_ptb(env, inputs, commands, verbose)
        }
        SandboxRequest::CallFunction {
            package,
            module,
            function,
            type_args,
            args,
        } => execution::execute_call_function(
            env, package, module, function, type_args, args, verbose,
        ),
        SandboxRequest::InspectStruct {
            package,
            module,
            struct_name,
        } => introspection::execute_inspect_struct(
            env,
            package,
            module.as_deref(),
            struct_name.as_deref(),
            verbose,
        ),

        // Introspection operations
        SandboxRequest::ListFunctions { module_path } => {
            introspection::execute_list_functions(env, module_path, verbose)
        }
        SandboxRequest::ListStructs { module_path } => {
            introspection::execute_list_structs(env, module_path, verbose)
        }
        SandboxRequest::GetFunctionInfo {
            module_path,
            function_name,
        } => introspection::execute_get_function_info(env, module_path, function_name, verbose),
        SandboxRequest::FindConstructors { type_path } => {
            introspection::execute_find_constructors(env, type_path, verbose)
        }
        SandboxRequest::SearchTypes {
            pattern,
            ability_filter,
        } => introspection::execute_search_types(env, pattern, ability_filter.as_deref(), verbose),
        SandboxRequest::SearchFunctions {
            pattern,
            entry_only,
        } => introspection::execute_search_functions(env, pattern, *entry_only, verbose),
        SandboxRequest::GetStructInfo { type_path } => {
            introspection::execute_get_struct_info(env, type_path, verbose)
        }
        SandboxRequest::GetSystemObjectInfo { object_name } => {
            introspection::execute_get_system_object_info(object_name, verbose)
        }

        // Encoding operations
        SandboxRequest::EncodeBcs { type_str, value } => {
            encoding::execute_encode_bcs(type_str, value, verbose)
        }
        SandboxRequest::DecodeBcs {
            type_str,
            bytes_hex,
        } => encoding::execute_decode_bcs(type_str, bytes_hex, verbose),
        SandboxRequest::ValidateType { type_str } => {
            encoding::execute_validate_type(type_str, verbose)
        }

        // Clock operations
        SandboxRequest::GetClock => clock::execute_get_clock(env, verbose),
        SandboxRequest::SetClock { timestamp_ms } => {
            clock::execute_set_clock(env, *timestamp_ms, verbose)
        }

        // Coin operations
        SandboxRequest::RegisterCoin {
            coin_type,
            decimals,
            symbol,
            name,
        } => coins::execute_register_coin(env, coin_type, *decimals, symbol, name, verbose),
        SandboxRequest::GetCoinMetadata { coin_type } => {
            coins::execute_get_coin_metadata(env, coin_type, verbose)
        }
        SandboxRequest::ListCoins => coins::execute_list_coins(env, verbose),

        // Event operations
        SandboxRequest::ListEvents => events::execute_list_events(env, verbose),
        SandboxRequest::GetEventsByType { type_prefix } => {
            events::execute_get_events_by_type(env, type_prefix, verbose)
        }
        SandboxRequest::GetLastTxEvents => events::execute_get_last_tx_events(env, verbose),
        SandboxRequest::ClearEvents => events::execute_clear_events(env, verbose),

        // Cache operations
        SandboxRequest::LoadCachedObjects {
            objects,
            object_types,
            shared_object_ids,
        } => cache::execute_load_cached_objects(
            env,
            objects,
            object_types,
            shared_object_ids,
            verbose,
        ),
        SandboxRequest::LoadCachedObject {
            object_id,
            bcs_bytes,
            object_type,
            is_shared,
        } => cache::execute_load_cached_object(
            env,
            object_id,
            bcs_bytes,
            object_type.as_deref(),
            *is_shared,
            verbose,
        ),
        SandboxRequest::ListCachedObjects => cache::execute_list_cached_objects(env, verbose),
        SandboxRequest::IsFrameworkCached => cache::execute_is_framework_cached(verbose),
        SandboxRequest::EnsureFrameworkCached => cache::execute_ensure_framework_cached(verbose),

        // Bytecode operations
        SandboxRequest::DisassembleFunction {
            module_path,
            function_name,
        } => bytecode::execute_disassemble_function(env, module_path, function_name, verbose),
        SandboxRequest::GetModuleDependencies { module_path } => {
            bytecode::execute_get_module_dependencies(env, module_path, verbose)
        }
        SandboxRequest::DisassembleModule { module_path } => {
            bytecode::execute_disassemble_module(env, module_path, verbose)
        }
        SandboxRequest::ModuleSummary { module_path } => {
            bytecode::execute_module_summary(env, module_path, verbose)
        }

        // Utility operations
        SandboxRequest::GenerateId => utils::execute_generate_id(env, verbose),
        SandboxRequest::ParseAddress { address } => utils::execute_parse_address(address, verbose),
        SandboxRequest::FormatAddress { address, format } => {
            utils::execute_format_address(address, format.as_deref(), verbose)
        }
        SandboxRequest::ComputeHash {
            bytes_hex,
            algorithm,
        } => utils::execute_compute_hash(bytes_hex, algorithm.as_deref(), verbose),
        SandboxRequest::ConvertNumber {
            value,
            from_type,
            to_type,
        } => utils::execute_convert_number(value, from_type, to_type, verbose),
        SandboxRequest::EncodeVector {
            element_type,
            values,
        } => utils::execute_encode_vector(element_type, values, verbose),
        SandboxRequest::ParseError { error } => utils::execute_parse_error(error, verbose),

        // State operations
        SandboxRequest::GetState => state::execute_get_state(env, verbose),
        SandboxRequest::Reset => state::execute_reset(env, verbose),
        SandboxRequest::GetLamportClock => state::execute_get_lamport_clock(env, verbose),
        SandboxRequest::AdvanceLamportClock => state::execute_advance_lamport_clock(env, verbose),

        // Mainnet import operations
        SandboxRequest::ImportPackageFromMainnet {
            package_id,
            network,
        } => mainnet::execute_import_package_from_mainnet(
            env,
            package_id,
            network.as_deref(),
            verbose,
        ),
        SandboxRequest::ImportObjectFromMainnet { object_id, network } => {
            mainnet::execute_import_object_from_mainnet(env, object_id, network.as_deref(), verbose)
        }
        SandboxRequest::ImportObjectsFromMainnet {
            object_ids,
            network,
        } => mainnet::execute_import_objects_from_mainnet(
            env,
            object_ids,
            network.as_deref(),
            verbose,
        ),
        SandboxRequest::ImportObjectAtVersion {
            object_id,
            version,
            network,
        } => mainnet::execute_import_object_at_version(
            env,
            object_id,
            *version,
            network.as_deref(),
            verbose,
        ),

        // Meta operations
        SandboxRequest::ListAvailableTools => state::execute_list_available_tools(verbose),
    }
}
