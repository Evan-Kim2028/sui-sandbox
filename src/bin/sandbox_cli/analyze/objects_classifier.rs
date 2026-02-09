use move_binary_format::file_format::{
    Bytecode, SignatureToken, StructFieldInformation, Visibility,
};
use move_binary_format::internals::ModuleIndex;
use move_binary_format::CompiledModule;
use std::collections::{BTreeMap, BTreeSet};

use super::{AnalyzeObjectsDynamicSettings, ObjectTypeStats};

fn account_address_to_hex(addr: &move_core_types::account_address::AccountAddress) -> String {
    format!("0x{}", addr.to_hex())
}

fn datatype_handle_to_type_tag(
    module: &CompiledModule,
    datatype_idx: move_binary_format::file_format::DatatypeHandleIndex,
) -> String {
    let datatype = module.datatype_handle_at(datatype_idx);
    let module_handle = module.module_handle_at(datatype.module);
    let addr = account_address_to_hex(module.address_identifier_at(module_handle.address));
    let module_name = module.identifier_at(module_handle.name).to_string();
    let struct_name = module.identifier_at(datatype.name).to_string();
    format!("{addr}::{module_name}::{struct_name}")
}

fn object_type_from_sig_token(module: &CompiledModule, token: &SignatureToken) -> Option<String> {
    match token {
        SignatureToken::Datatype(idx) => Some(datatype_handle_to_type_tag(module, *idx)),
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, _) = &**inst;
            Some(datatype_handle_to_type_tag(module, *idx))
        }
        SignatureToken::Reference(inner) | SignatureToken::MutableReference(inner) => {
            object_type_from_sig_token(module, inner)
        }
        _ => None,
    }
}

fn is_receiving_wrapper(module: &CompiledModule, token: &SignatureToken) -> Option<String> {
    let SignatureToken::DatatypeInstantiation(inst) = token else {
        return None;
    };
    let (datatype_idx, type_args) = &**inst;
    if type_args.len() != 1 {
        return None;
    }
    let datatype = module.datatype_handle_at(*datatype_idx);
    let module_handle = module.module_handle_at(datatype.module);
    let module_name = module.identifier_at(module_handle.name).to_string();
    let struct_name = module.identifier_at(datatype.name).to_string();
    if module_name == "transfer" && struct_name == "Receiving" {
        return object_type_from_sig_token(module, &type_args[0]);
    }
    None
}

fn mark_transfer_mode_for_type(
    mode: &str,
    type_tag: &str,
    object_stats: &mut BTreeMap<String, ObjectTypeStats>,
) {
    let stats = object_stats.entry(type_tag.to_string()).or_default();
    match mode {
        "owned" => stats.owned = true,
        "shared" => stats.shared = true,
        "immutable" => stats.immutable = true,
        "party" => stats.party = true,
        "receive" => stats.receive = true,
        _ => {}
    }
}

fn mark_dynamic_usage_for_type(
    type_tag: &str,
    object_stats: &mut BTreeMap<String, ObjectTypeStats>,
) {
    let stats = object_stats.entry(type_tag.to_string()).or_default();
    stats.dynamic_fields = true;
}

fn sig_token_contains_dynamic_container(
    module: &CompiledModule,
    token: &SignatureToken,
    include_wrapper_apis: bool,
) -> bool {
    match token {
        SignatureToken::Datatype(idx) => {
            let datatype = module.datatype_handle_at(*idx);
            let module_handle = module.module_handle_at(datatype.module);
            let module_name = module.identifier_at(module_handle.name).to_string();
            let struct_name = module.identifier_at(datatype.name).to_string();
            let direct_dynamic = matches!(
                module_name.as_str(),
                "dynamic_field" | "dynamic_object_field"
            );
            if direct_dynamic {
                return true;
            }
            if include_wrapper_apis
                && matches!(
                    module_name.as_str(),
                    "table" | "bag" | "object_table" | "object_bag" | "linked_table" | "table_vec"
                )
            {
                return true;
            }
            include_wrapper_apis
                && matches!(
                    struct_name.as_str(),
                    "Table" | "Bag" | "ObjectTable" | "ObjectBag" | "LinkedTable" | "TableVec"
                )
        }
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, type_args) = &**inst;
            sig_token_contains_dynamic_container(
                module,
                &SignatureToken::Datatype(*idx),
                include_wrapper_apis,
            ) || type_args
                .iter()
                .any(|arg| sig_token_contains_dynamic_container(module, arg, include_wrapper_apis))
        }
        SignatureToken::Reference(inner)
        | SignatureToken::MutableReference(inner)
        | SignatureToken::Vector(inner) => {
            sig_token_contains_dynamic_container(module, inner, include_wrapper_apis)
        }
        _ => false,
    }
}

fn is_sui_framework_address(module_addr: &str) -> bool {
    let addr_hex = module_addr.strip_prefix("0x").unwrap_or(module_addr);
    let addr_trimmed = addr_hex.trim_start_matches('0');
    addr_trimmed == "2" || (addr_trimmed.is_empty() && addr_hex == "0")
}

pub(super) fn is_dynamic_field_api_function(
    module_addr: &str,
    module_name: &str,
    _function_name: &str,
    include_wrapper_apis: bool,
) -> bool {
    if !is_sui_framework_address(module_addr) {
        return false;
    }
    matches!(module_name, "dynamic_field" | "dynamic_object_field")
        || (include_wrapper_apis
            && matches!(
                module_name,
                "table" | "bag" | "object_table" | "object_bag" | "linked_table" | "table_vec"
            ))
}

fn is_uid_datatype(
    module: &CompiledModule,
    datatype_idx: move_binary_format::file_format::DatatypeHandleIndex,
) -> bool {
    let datatype = module.datatype_handle_at(datatype_idx);
    let module_handle = module.module_handle_at(datatype.module);
    let module_addr = account_address_to_hex(module.address_identifier_at(module_handle.address));
    let module_name = module.identifier_at(module_handle.name).to_string();
    let struct_name = module.identifier_at(datatype.name).to_string();
    is_sui_framework_address(&module_addr) && module_name == "object" && struct_name == "UID"
}

fn is_uid_signature_token(module: &CompiledModule, token: &SignatureToken) -> bool {
    match token {
        SignatureToken::Datatype(idx) => is_uid_datatype(module, *idx),
        SignatureToken::DatatypeInstantiation(inst) => {
            let (idx, _) = &**inst;
            is_uid_datatype(module, *idx)
        }
        SignatureToken::Reference(inner) | SignatureToken::MutableReference(inner) => {
            is_uid_signature_token(module, inner)
        }
        _ => false,
    }
}

fn uid_owner_type_from_field_handle(
    module: &CompiledModule,
    field_handle_idx: move_binary_format::file_format::FieldHandleIndex,
    key_struct_by_def_idx: &BTreeMap<usize, String>,
) -> Option<String> {
    let field_handle = module.field_handle_at(field_handle_idx);
    let owner_idx = field_handle.owner.into_index();
    let owner_type = key_struct_by_def_idx.get(&owner_idx)?.clone();
    let owner_def = module.struct_def_at(field_handle.owner);
    let field = owner_def.field(field_handle.field as usize)?;
    if is_uid_signature_token(module, &field.signature.0) {
        Some(owner_type)
    } else {
        None
    }
}

fn nearest_uid_owner_before_call(
    module: &CompiledModule,
    code: &[Bytecode],
    call_index: usize,
    key_struct_by_def_idx: &BTreeMap<usize, String>,
    lookback: usize,
) -> Option<String> {
    let start = call_index.saturating_sub(lookback);
    for idx in (start..call_index).rev() {
        match &code[idx] {
            Bytecode::MutBorrowField(field_idx) | Bytecode::ImmBorrowField(field_idx) => {
                if let Some(owner_type) =
                    uid_owner_type_from_field_handle(module, *field_idx, key_struct_by_def_idx)
                {
                    return Some(owner_type);
                }
            }
            Bytecode::MutBorrowFieldGeneric(inst_idx)
            | Bytecode::ImmBorrowFieldGeneric(inst_idx) => {
                let inst = module.field_instantiation_at(*inst_idx);
                if let Some(owner_type) =
                    uid_owner_type_from_field_handle(module, inst.handle, key_struct_by_def_idx)
                {
                    return Some(owner_type);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn mode_from_transfer_function(
    module_addr: &str,
    module_name: &str,
    function_name: &str,
) -> Option<&'static str> {
    if module_name != "transfer" || !is_sui_framework_address(module_addr) {
        return None;
    }
    if function_name.contains("party_transfer") {
        return Some("party");
    }
    if function_name.contains("share_object") {
        return Some("shared");
    }
    if function_name.contains("freeze_object") {
        return Some("immutable");
    }
    if function_name == "receive" || function_name.ends_with("_receive") {
        return Some("receive");
    }
    if function_name == "transfer"
        || function_name == "public_transfer"
        || function_name.ends_with("_transfer")
    {
        return Some("owned");
    }
    None
}

pub(super) fn analyze_module_object_usage(
    module: &CompiledModule,
    object_stats: &mut BTreeMap<String, ObjectTypeStats>,
    settings: &AnalyzeObjectsDynamicSettings,
) {
    let mut key_struct_by_def_idx = BTreeMap::<usize, String>::new();

    for (idx, struct_def) in module.struct_defs().iter().enumerate() {
        let datatype = module.datatype_handle_at(struct_def.struct_handle);
        if !datatype
            .abilities
            .has_ability(move_binary_format::file_format::Ability::Key)
        {
            continue;
        }
        let has_store = datatype
            .abilities
            .has_ability(move_binary_format::file_format::Ability::Store);
        let type_tag = datatype_handle_to_type_tag(module, struct_def.struct_handle);
        key_struct_by_def_idx.insert(idx, type_tag.clone());
        let stats = object_stats.entry(type_tag).or_default();
        stats.key_struct = true;
        stats.has_store |= has_store;
        stats.occurrences += 1;
        if settings.field_container_heuristic
            && matches!(
                &struct_def.field_information,
                StructFieldInformation::Declared(_)
            )
        {
            if let StructFieldInformation::Declared(fields) = &struct_def.field_information {
                if fields.iter().any(|field| {
                    sig_token_contains_dynamic_container(
                        module,
                        &field.signature.0,
                        settings.include_wrapper_apis,
                    )
                }) {
                    stats.dynamic_fields = true;
                }
            }
        }
    }

    for function_def in module.function_defs() {
        let handle = module.function_handle_at(function_def.function);
        let function_name = module.identifier_at(handle.name).to_string();
        let is_entry_or_public =
            function_def.is_entry || matches!(function_def.visibility, Visibility::Public);

        let params = module.signature_at(handle.parameters);
        let mut dynamic_owner_candidate_types = BTreeSet::new();
        for param in &params.0 {
            if let Some(receiving_inner) = is_receiving_wrapper(module, param) {
                mark_transfer_mode_for_type("receive", &receiving_inner, object_stats);
                continue;
            }
            if settings.use_ref_param_owner_fallback
                && matches!(
                    param,
                    SignatureToken::Reference(_) | SignatureToken::MutableReference(_)
                )
            {
                if let Some(type_tag) = object_type_from_sig_token(module, param) {
                    dynamic_owner_candidate_types.insert(type_tag);
                }
            }
            if is_entry_or_public {
                if let Some(type_tag) = object_type_from_sig_token(module, param) {
                    let is_ref = matches!(
                        param,
                        SignatureToken::Reference(_) | SignatureToken::MutableReference(_)
                    );
                    if !is_ref {
                        mark_transfer_mode_for_type("owned", &type_tag, object_stats);
                    }
                }
            }
        }

        let Some(code) = &function_def.code else {
            continue;
        };
        for (pc, bytecode) in code.code.iter().enumerate() {
            match bytecode {
                Bytecode::Pack(def_idx) => {
                    if let Some(type_tag) = key_struct_by_def_idx.get(&def_idx.into_index()) {
                        let stats = object_stats.entry(type_tag.clone()).or_default();
                        stats.pack_count += 1;
                        if function_name == "init" {
                            stats.packed_in_init = true;
                        } else {
                            stats.packed_outside_init = true;
                        }
                    }
                }
                Bytecode::PackGeneric(inst_idx) => {
                    let inst = module.struct_instantiation_at(*inst_idx);
                    if let Some(type_tag) = key_struct_by_def_idx.get(&inst.def.into_index()) {
                        let stats = object_stats.entry(type_tag.clone()).or_default();
                        stats.pack_count += 1;
                        if function_name == "init" {
                            stats.packed_in_init = true;
                        } else {
                            stats.packed_outside_init = true;
                        }
                    }
                }
                Bytecode::Call(handle_idx) => {
                    let target_handle = module.function_handle_at(*handle_idx);
                    let module_handle = module.module_handle_at(target_handle.module);
                    let target_module_addr =
                        account_address_to_hex(module.address_identifier_at(module_handle.address));
                    let target_module_name = module.identifier_at(module_handle.name).to_string();
                    let target_function_name = module.identifier_at(target_handle.name).to_string();
                    let target_sig = module.signature_at(target_handle.parameters);
                    if is_dynamic_field_api_function(
                        &target_module_addr,
                        &target_module_name,
                        &target_function_name,
                        settings.include_wrapper_apis,
                    ) {
                        if settings.use_uid_owner_flow {
                            if let Some(owner_type) = nearest_uid_owner_before_call(
                                module,
                                &code.code,
                                pc,
                                &key_struct_by_def_idx,
                                settings.lookback,
                            ) {
                                mark_dynamic_usage_for_type(&owner_type, object_stats);
                            }
                        }
                        if settings.use_ref_param_owner_fallback {
                            for type_tag in &dynamic_owner_candidate_types {
                                mark_dynamic_usage_for_type(type_tag, object_stats);
                            }
                        }
                    }
                    if let Some(mode) = mode_from_transfer_function(
                        &target_module_addr,
                        &target_module_name,
                        &target_function_name,
                    ) {
                        for token in &target_sig.0 {
                            if let Some(type_tag) = object_type_from_sig_token(module, token) {
                                mark_transfer_mode_for_type(mode, &type_tag, object_stats);
                            }
                        }
                    }
                }
                Bytecode::CallGeneric(inst_idx) => {
                    let inst = module.function_instantiation_at(*inst_idx);
                    let target_handle = module.function_handle_at(inst.handle);
                    let module_handle = module.module_handle_at(target_handle.module);
                    let target_module_addr =
                        account_address_to_hex(module.address_identifier_at(module_handle.address));
                    let target_module_name = module.identifier_at(module_handle.name).to_string();
                    let target_function_name = module.identifier_at(target_handle.name).to_string();
                    let type_args = module.signature_at(inst.type_parameters);
                    if is_dynamic_field_api_function(
                        &target_module_addr,
                        &target_module_name,
                        &target_function_name,
                        settings.include_wrapper_apis,
                    ) {
                        if settings.use_uid_owner_flow {
                            if let Some(owner_type) = nearest_uid_owner_before_call(
                                module,
                                &code.code,
                                pc,
                                &key_struct_by_def_idx,
                                settings.lookback,
                            ) {
                                mark_dynamic_usage_for_type(&owner_type, object_stats);
                            }
                        }
                        if settings.use_ref_param_owner_fallback {
                            for type_tag in &dynamic_owner_candidate_types {
                                mark_dynamic_usage_for_type(type_tag, object_stats);
                            }
                        }
                    }
                    if let Some(mode) = mode_from_transfer_function(
                        &target_module_addr,
                        &target_module_name,
                        &target_function_name,
                    ) {
                        for token in &type_args.0 {
                            if let Some(type_tag) = object_type_from_sig_token(module, token) {
                                mark_transfer_mode_for_type(mode, &type_tag, object_stats);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
