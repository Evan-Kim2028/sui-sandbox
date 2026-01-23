use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::normalization::signature_token_to_json;
use crate::types::{
    BytecodeFieldJson, BytecodeFunctionJson, BytecodeFunctionTypeParamJson, BytecodeModuleJson,
    BytecodePackageInterfaceJson, BytecodeStructJson, BytecodeStructRefJson,
    BytecodeStructTypeParamJson, LocalBytecodeCounts, LocalBytesCheck, ModuleBytesMismatch,
    SanityCounts,
};
use crate::utils::{
    bytes_info, bytes_info_sha256_hex, bytes_to_hex_prefixed, canonicalize_json_value, BytesInfo,
};
use move_binary_format::file_format::{
    Ability, AbilitySet, CompiledModule, StructFieldInformation, Visibility,
};

pub fn module_self_address_hex(module: &CompiledModule) -> String {
    bytes_to_hex_prefixed(module.self_id().address().as_ref())
}

pub fn ability_set_to_strings(set: &AbilitySet) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if set.has_ability(Ability::Copy) {
        out.push("copy".to_string());
    }
    if set.has_ability(Ability::Drop) {
        out.push("drop".to_string());
    }
    if set.has_ability(Ability::Store) {
        out.push("store".to_string());
    }
    if set.has_ability(Ability::Key) {
        out.push("key".to_string());
    }
    out
}

pub fn visibility_to_string(v: Visibility) -> String {
    match v {
        Visibility::Public => "public".to_string(),
        Visibility::Friend => "friend".to_string(),
        Visibility::Private => "private".to_string(),
    }
}

pub fn build_bytecode_module_json(module: &CompiledModule) -> Result<BytecodeModuleJson> {
    let mut structs: BTreeMap<String, BytecodeStructJson> = BTreeMap::new();
    let mut functions: BTreeMap<String, BytecodeFunctionJson> = BTreeMap::new();

    for def in module.struct_defs() {
        let handle = module.datatype_handle_at(def.struct_handle);
        let name = module.identifier_at(handle.name).to_string();

        let type_params: Vec<BytecodeStructTypeParamJson> = handle
            .type_parameters
            .iter()
            .map(|tp| BytecodeStructTypeParamJson {
                constraints: ability_set_to_strings(&tp.constraints),
                is_phantom: tp.is_phantom,
            })
            .collect();

        let abilities = ability_set_to_strings(&handle.abilities);

        let mut fields: Vec<BytecodeFieldJson> = Vec::new();
        let mut is_native = false;
        match &def.field_information {
            StructFieldInformation::Declared(field_defs) => {
                for f in field_defs {
                    let field_name = module.identifier_at(f.name).to_string();
                    let field_ty = signature_token_to_json(module, &f.signature.0);
                    fields.push(BytecodeFieldJson {
                        name: field_name,
                        r#type: field_ty,
                    });
                }
            }
            StructFieldInformation::Native => {
                is_native = true;
            }
        }

        structs.insert(
            name,
            BytecodeStructJson {
                abilities,
                type_params,
                is_native,
                fields,
            },
        );
    }

    for def in module.function_defs() {
        let handle = module.function_handle_at(def.function);
        let name = module.identifier_at(handle.name).to_string();

        let params_sig = module.signature_at(handle.parameters);
        let returns_sig = module.signature_at(handle.return_);
        let params: Vec<Value> = params_sig
            .0
            .iter()
            .map(|t| signature_token_to_json(module, t))
            .collect();
        let returns: Vec<Value> = returns_sig
            .0
            .iter()
            .map(|t| signature_token_to_json(module, t))
            .collect();

        let type_params: Vec<BytecodeFunctionTypeParamJson> = handle
            .type_parameters
            .iter()
            .map(|c| BytecodeFunctionTypeParamJson {
                constraints: ability_set_to_strings(c),
            })
            .collect();

        let mut acquires: Vec<BytecodeStructRefJson> = Vec::new();
        for idx in def.acquires_global_resources.iter() {
            let sdef = module.struct_def_at(*idx);
            let sh = module.datatype_handle_at(sdef.struct_handle);
            acquires.push(BytecodeStructRefJson {
                address: module_self_address_hex(module),
                module: compiled_module_name(module),
                name: module.identifier_at(sh.name).to_string(),
            });
        }
        acquires.sort_by(|a, b| a.name.cmp(&b.name));

        functions.insert(
            name,
            BytecodeFunctionJson {
                visibility: visibility_to_string(def.visibility),
                is_entry: def.is_entry,
                is_native: def.code.is_none(),
                type_params,
                params,
                returns,
                acquires,
            },
        );
    }

    Ok(BytecodeModuleJson {
        address: module_self_address_hex(module),
        structs,
        functions,
    })
}

pub fn build_bytecode_interface_value_from_compiled_modules(
    package_id: &str,
    compiled_modules: &[CompiledModule],
) -> Result<(Vec<String>, Value)> {
    let mut module_map: BTreeMap<String, BytecodeModuleJson> = BTreeMap::new();
    for module in compiled_modules {
        let name = compiled_module_name(module);
        module_map.insert(name, build_bytecode_module_json(module)?);
    }

    let module_names: Vec<String> = module_map.keys().cloned().collect();
    let mut modules_value =
        serde_json::to_value(&module_map).context("serialize bytecode modules")?;
    canonicalize_json_value(&mut modules_value);

    let interface = BytecodePackageInterfaceJson {
        schema_version: 1,
        package_id: package_id.to_string(),
        module_names: module_names.clone(),
        modules: modules_value,
    };

    let mut interface_value =
        serde_json::to_value(interface).context("build bytecode interface JSON")?;
    canonicalize_json_value(&mut interface_value);
    Ok((module_names, interface_value))
}

pub fn ability_set_has_key(set: &AbilitySet) -> bool {
    set.has_ability(Ability::Key)
}

pub fn analyze_compiled_module(module: &CompiledModule) -> LocalBytecodeCounts {
    let mut structs = 0usize;
    let mut key_structs = 0usize;

    let mut functions_total = 0usize;
    let mut functions_public = 0usize;
    let mut functions_friend = 0usize;
    let mut functions_private = 0usize;
    let mut functions_native = 0usize;

    let mut entry_functions = 0usize;
    let mut public_entry_functions = 0usize;
    let mut friend_entry_functions = 0usize;
    let mut private_entry_functions = 0usize;

    structs += module.struct_defs().len();

    for def in module.struct_defs() {
        let handle = module.datatype_handle_at(def.struct_handle);
        if ability_set_has_key(&handle.abilities) {
            key_structs += 1;
        }
    }

    functions_total += module.function_defs().len();
    for def in module.function_defs() {
        if def.code.is_none() {
            functions_native += 1;
        }

        match def.visibility {
            Visibility::Public => functions_public += 1,
            Visibility::Friend => functions_friend += 1,
            Visibility::Private => functions_private += 1,
        }

        if def.is_entry {
            entry_functions += 1;
            match def.visibility {
                Visibility::Public => public_entry_functions += 1,
                Visibility::Friend => friend_entry_functions += 1,
                Visibility::Private => private_entry_functions += 1,
            }
        }
    }

    LocalBytecodeCounts {
        modules: 1,
        structs,
        functions_total,
        functions_public,
        functions_friend,
        functions_private,
        functions_native,
        entry_functions,
        public_entry_functions,
        friend_entry_functions,
        private_entry_functions,
        key_structs,
    }
}

pub fn compiled_module_name(module: &CompiledModule) -> String {
    module.self_id().name().to_string()
}

pub fn read_package_id_from_metadata(package_dir: &Path) -> Result<String> {
    let metadata_path = package_dir.join("metadata.json");
    let metadata_text = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read {}", metadata_path.display()))?;
    let metadata: Value = serde_json::from_str(&metadata_text)
        .with_context(|| format!("parse {}", metadata_path.display()))?;
    Ok(metadata
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("metadata.json missing 'id'"))?
        .to_string())
}

pub fn analyze_local_bytecode_package(
    package_dir: &Path,
) -> Result<(Vec<String>, LocalBytecodeCounts)> {
    let bytecode_dir = package_dir.join("bytecode_modules");
    let mut module_names: Vec<String> = Vec::new();
    let mut counts = LocalBytecodeCounts {
        modules: 0,
        structs: 0,
        functions_total: 0,
        functions_public: 0,
        functions_friend: 0,
        functions_private: 0,
        functions_native: 0,
        entry_functions: 0,
        public_entry_functions: 0,
        friend_entry_functions: 0,
        private_entry_functions: 0,
        key_structs: 0,
    };

    let mut entries: Vec<_> = fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read {}", bytecode_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", bytecode_dir.display()))?;
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mv") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let module = CompiledModule::deserialize_with_defaults(&bytes)
            .map_err(|e| anyhow!("deserialize {}: {}", path.display(), e))?;

        module_names.push(compiled_module_name(&module));
        let module_counts = analyze_compiled_module(&module);
        counts.modules += module_counts.modules;
        counts.structs += module_counts.structs;
        counts.functions_total += module_counts.functions_total;
        counts.functions_public += module_counts.functions_public;
        counts.functions_friend += module_counts.functions_friend;
        counts.functions_private += module_counts.functions_private;
        counts.functions_native += module_counts.functions_native;
        counts.entry_functions += module_counts.entry_functions;
        counts.public_entry_functions += module_counts.public_entry_functions;
        counts.friend_entry_functions += module_counts.friend_entry_functions;
        counts.private_entry_functions += module_counts.private_entry_functions;
        counts.key_structs += module_counts.key_structs;
    }

    Ok((module_names, counts))
}

pub fn list_local_module_names_only(package_dir: &Path) -> Result<Vec<String>> {
    let bytecode_dir = package_dir.join("bytecode_modules");
    let mut entries: Vec<_> = fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read {}", bytecode_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", bytecode_dir.display()))?;
    entries.sort_by_key(|e| e.path());

    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mv") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("bad filename {}", path.display()))?
            .to_string();
        names.push(name);
    }
    if names.is_empty() {
        return Err(anyhow!("no .mv files found in {}", bytecode_dir.display()));
    }
    Ok(names)
}

pub fn read_local_compiled_modules(package_dir: &Path) -> Result<Vec<CompiledModule>> {
    let bytecode_dir = package_dir.join("bytecode_modules");
    let mut entries: Vec<_> = fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read {}", bytecode_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", bytecode_dir.display()))?;
    entries.sort_by_key(|e| e.path());

    let mut modules: Vec<CompiledModule> = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mv") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let module = CompiledModule::deserialize_with_defaults(&bytes)
            .with_context(|| format!("deserialize {}", path.display()))?;
        modules.push(module);
    }
    if modules.is_empty() {
        return Err(anyhow!("no .mv files found in {}", bytecode_dir.display()));
    }
    Ok(modules)
}

pub fn read_local_bcs_module_names(package_dir: &Path) -> Result<Vec<String>> {
    let bcs_path = package_dir.join("bcs.json");
    let text =
        fs::read_to_string(&bcs_path).with_context(|| format!("read {}", bcs_path.display()))?;
    let v: Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", bcs_path.display()))?;
    let module_map = v
        .get("moduleMap")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("bcs.json missing moduleMap"))?;
    let mut names: Vec<String> = module_map.keys().cloned().collect();
    names.sort();
    Ok(names)
}

pub fn decode_module_map_entry_bytes(module: &str, v: &Value) -> Result<Vec<u8>> {
    use base64::Engine;
    match v {
        Value::String(s) => base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .with_context(|| format!("base64 decode moduleMap[{}]", module)),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for x in arr {
                let n = x
                    .as_u64()
                    .ok_or_else(|| anyhow!("moduleMap[{}] contains non-u64 byte", module))?;
                if n > 255 {
                    return Err(anyhow!(
                        "moduleMap[{}] contains out-of-range byte {}",
                        module,
                        n
                    ));
                }
                out.push(n as u8);
            }
            Ok(out)
        }
        _ => Err(anyhow!(
            "moduleMap[{}] unexpected JSON type (expected string/array)",
            module
        )),
    }
}

pub fn read_local_bcs_module_map_bytes_info(
    package_dir: &Path,
) -> Result<BTreeMap<String, BytesInfo>> {
    let bcs_path = package_dir.join("bcs.json");
    let text =
        fs::read_to_string(&bcs_path).with_context(|| format!("read {}", bcs_path.display()))?;
    let v: Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", bcs_path.display()))?;
    let module_map = v
        .get("moduleMap")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("bcs.json missing moduleMap"))?;

    let mut out = BTreeMap::<String, BytesInfo>::new();
    for (name, entry) in module_map {
        let bytes = decode_module_map_entry_bytes(name, entry)
            .with_context(|| format!("decode bcs.json moduleMap[{}]", name))?;
        out.insert(name.clone(), bytes_info(&bytes));
    }
    Ok(out)
}

pub fn read_local_mv_bytes_info_map(package_dir: &Path) -> Result<BTreeMap<String, BytesInfo>> {
    let bytecode_dir = package_dir.join("bytecode_modules");
    let mut entries: Vec<_> = fs::read_dir(&bytecode_dir)
        .with_context(|| format!("read {}", bytecode_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", bytecode_dir.display()))?;
    entries.sort_by_key(|e| e.path());

    let mut out = BTreeMap::<String, BytesInfo>::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mv") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("bad filename {}", path.display()))?
            .to_string();
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        out.insert(name, bytes_info(&bytes));
    }
    if out.is_empty() {
        return Err(anyhow!("no .mv files found in {}", bytecode_dir.display()));
    }
    Ok(out)
}

pub fn local_bytes_check_for_package(
    package_dir: &Path,
    max_mismatches: usize,
) -> Result<LocalBytesCheck> {
    let mv = read_local_mv_bytes_info_map(package_dir)?;
    let bcs = read_local_bcs_module_map_bytes_info(package_dir)?;

    let mut all = BTreeSet::<String>::new();
    all.extend(mv.keys().cloned());
    all.extend(bcs.keys().cloned());

    let mut missing_in_bcs = Vec::<String>::new();
    let mut missing_in_mv = Vec::<String>::new();
    let mut mismatches_sample = Vec::<ModuleBytesMismatch>::new();
    let mut mismatches_total = 0usize;
    let mut exact_match_modules = 0usize;

    for module in all {
        let mv_info = mv.get(&module).copied();
        let bcs_info = bcs.get(&module).copied();

        match (mv_info, bcs_info) {
            (None, Some(bcs_info)) => {
                mismatches_total += 1;
                missing_in_mv.push(module.clone());
                if mismatches_sample.len() < max_mismatches {
                    mismatches_sample.push(ModuleBytesMismatch {
                        module,
                        reason: "missing_in_mv".to_string(),
                        mv_len: None,
                        bcs_len: Some(bcs_info.len),
                        mv_sha256: None,
                        bcs_sha256: Some(bytes_info_sha256_hex(bcs_info)),
                    });
                }
            }
            (Some(mv_info), None) => {
                mismatches_total += 1;
                missing_in_bcs.push(module.clone());
                if mismatches_sample.len() < max_mismatches {
                    mismatches_sample.push(ModuleBytesMismatch {
                        module,
                        reason: "missing_in_bcs".to_string(),
                        mv_len: Some(mv_info.len),
                        bcs_len: None,
                        mv_sha256: Some(bytes_info_sha256_hex(mv_info)),
                        bcs_sha256: None,
                    });
                }
            }
            (Some(mv_info), Some(bcs_info)) => {
                if mv_info.len == bcs_info.len && mv_info.sha256 == bcs_info.sha256 {
                    exact_match_modules += 1;
                    continue;
                }

                mismatches_total += 1;
                let reason = if mv_info.len != bcs_info.len {
                    "len_mismatch"
                } else {
                    "sha256_mismatch"
                };
                if mismatches_sample.len() < max_mismatches {
                    mismatches_sample.push(ModuleBytesMismatch {
                        module,
                        reason: reason.to_string(),
                        mv_len: Some(mv_info.len),
                        bcs_len: Some(bcs_info.len),
                        mv_sha256: Some(bytes_info_sha256_hex(mv_info)),
                        bcs_sha256: Some(bytes_info_sha256_hex(bcs_info)),
                    });
                }
            }
            (None, None) => {}
        }
    }

    Ok(LocalBytesCheck {
        mv_modules: mv.len(),
        bcs_modules: bcs.len(),
        exact_match_modules,
        mismatches_total,
        missing_in_bcs,
        missing_in_mv,
        mismatches_sample,
    })
}

pub fn collect_corpus_package_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut package_dirs: Vec<PathBuf> = Vec::new();

    let mut prefixes: Vec<_> = fs::read_dir(root)
        .with_context(|| format!("read {}", root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("list {}", root.display()))?;
    prefixes.sort_by_key(|e| e.path());

    for prefix in prefixes {
        let prefix_path = prefix.path();
        if !prefix_path.is_dir() {
            continue;
        }

        let mut entries: Vec<_> = fs::read_dir(&prefix_path)
            .with_context(|| format!("read {}", prefix_path.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("list {}", prefix_path.display()))?;
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                package_dirs.push(path);
                continue;
            }
            if entry
                .file_type()
                .ok()
                .map(|t| t.is_symlink())
                .unwrap_or(false)
            {
                package_dirs.push(path);
            }
        }
    }

    Ok(package_dirs)
}

pub fn extract_sanity_counts(modules_value: &Value) -> SanityCounts {
    let mut structs = 0usize;
    let mut functions = 0usize;
    let mut key_structs = 0usize;

    let modules_obj = modules_value.as_object();
    let modules = modules_obj.map(|o| o.len()).unwrap_or(0);

    if let Some(modules_obj) = modules_obj {
        for (_module_name, module_def) in modules_obj {
            if let Some(structs_obj) = get_object(module_def, &["structs"]) {
                structs += structs_obj.len();
                for (_struct_name, struct_def) in structs_obj {
                    if struct_has_key(struct_def) {
                        key_structs += 1;
                    }
                }
            }

            if let Some(funcs_obj) = get_object(
                module_def,
                &["functions", "exposedFunctions", "exposed_functions"],
            ) {
                functions += funcs_obj.len();
            }
        }
    }

    SanityCounts {
        modules,
        structs,
        functions,
        key_structs,
    }
}

pub fn struct_has_key(struct_def: &Value) -> bool {
    let Some(abilities_value) = struct_def.get("abilities") else {
        return false;
    };

    iter_ability_strings(abilities_value)
        .map(|s| s.to_ascii_lowercase())
        .any(|s| s == "key")
}

pub fn iter_ability_strings<'a>(value: &'a Value) -> Box<dyn Iterator<Item = &'a str> + 'a> {
    if let Some(arr) = value.as_array() {
        return Box::new(arr.iter().filter_map(|v| v.as_str()));
    }
    if let Some(obj) = value.as_object() {
        if let Some(arr) = obj.get("abilities").and_then(Value::as_array) {
            return Box::new(arr.iter().filter_map(|v| v.as_str()));
        }
    }
    Box::new(std::iter::empty())
}

pub fn get_object<'a>(
    value: &'a Value,
    keys: &[&str],
) -> Option<&'a serde_json::Map<String, Value>> {
    for key in keys {
        if let Some(obj) = value.get(*key).and_then(Value::as_object) {
            return Some(obj);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_module_map_entry_bytes_base64_string() {
        let v = Value::String("AAEC".to_string());
        let bytes = decode_module_map_entry_bytes("m", &v).unwrap();
        assert_eq!(bytes, vec![0u8, 1u8, 2u8]);
    }

    #[test]
    fn test_decode_module_map_entry_bytes_array() {
        let v = serde_json::json!([0, 255, 1]);
        let bytes = decode_module_map_entry_bytes("m", &v).unwrap();
        assert_eq!(bytes, vec![0u8, 255u8, 1u8]);
    }

    #[test]
    fn test_decode_module_map_entry_bytes_array_rejects_out_of_range() {
        let v = serde_json::json!([256]);
        let err = decode_module_map_entry_bytes("m", &v).unwrap_err();
        assert!(format!("{:#}", err).contains("out-of-range"));
    }

    #[test]
    fn test_visibility_to_string() {
        assert_eq!(visibility_to_string(Visibility::Public), "public");
        assert_eq!(visibility_to_string(Visibility::Friend), "friend");
        assert_eq!(visibility_to_string(Visibility::Private), "private");
    }

    #[test]
    fn test_struct_has_key() {
        let v = serde_json::json!({
            "abilities": ["Store", "key", "copy"]
        });
        assert!(struct_has_key(&v));

        let v2 = serde_json::json!({
            "abilities": ["Store", "copy"]
        });
        assert!(!struct_has_key(&v2));

        // Test nested format often seen in RPC
        let v3 = serde_json::json!({
            "abilities": { "abilities": ["Key"] }
        });
        assert!(struct_has_key(&v3));
    }
}
