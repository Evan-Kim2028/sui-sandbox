use base64::Engine;
use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use sui_prefetch::compute_dynamic_field_id;
#[cfg(feature = "mm2")]
use sui_sandbox_core::mm2::{TypeModel, TypeSynthesizer};
use sui_sandbox_core::types::{format_type_tag, parse_type_tag};
use sui_sandbox_core::utilities::rewrite_type_tag;
use sui_state_fetcher::{
    fetch_child_object as fetch_child_object_shared,
    fetch_object_via_grpc as fetch_object_via_grpc_shared, HistoricalStateProvider,
};
use sui_transport::graphql::GraphQLClient;

fn b64_matches_bytes(encoded: &str, expected: &[u8]) -> bool {
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) {
        return decoded == expected;
    }
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded) {
        return decoded == expected;
    }
    false
}

fn is_type_name_tag(tag: &TypeTag) -> bool {
    let TypeTag::Struct(s) = tag else {
        return false;
    };
    let Ok(std_addr) = AccountAddress::from_hex_literal("0x1") else {
        return false;
    };
    s.address == std_addr && s.module.as_str() == "type_name" && s.name.as_str() == "TypeName"
}

#[derive(Debug, Clone)]
pub(super) struct MissEntry {
    pub(super) count: u32,
    pub(super) last: std::time::Instant,
}

pub(super) struct ChildFetchOptions<'a> {
    pub(super) provider: &'a HistoricalStateProvider,
    pub(super) checkpoint: Option<u64>,
    pub(super) max_version: u64,
    pub(super) strict_checkpoint: bool,
    pub(super) aliases: &'a HashMap<AccountAddress, AccountAddress>,
    pub(super) child_id_aliases:
        &'a Arc<parking_lot::Mutex<HashMap<AccountAddress, AccountAddress>>>,
    pub(super) miss_cache: Option<&'a Arc<parking_lot::Mutex<HashMap<String, MissEntry>>>>,
    pub(super) debug_df: bool,
    pub(super) debug_df_full: bool,
    pub(super) self_heal_dynamic_fields: bool,
    pub(super) synth_modules: Option<Arc<Vec<CompiledModule>>>,
    pub(super) log_self_heal: bool,
}

/// Resolve a dynamic field's key type via GraphQL lookup.
///
/// Given a parent object and key bytes, queries the GraphQL API to find the matching
/// dynamic field and returns its key TypeTag. Results are cached to avoid redundant lookups.
/// Also detects and records child ID aliases when the computed dynamic field object ID
/// differs from the on-chain actual ID (due to package upgrades changing type hashes).
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_key_type_via_graphql(
    gql: &GraphQLClient,
    parent: AccountAddress,
    key_bytes: &[u8],
    checkpoint: Option<u64>,
    strict_checkpoint: bool,
    aliases: &HashMap<AccountAddress, AccountAddress>,
    child_id_aliases: &parking_lot::Mutex<HashMap<AccountAddress, AccountAddress>>,
    cache: &Mutex<HashMap<String, TypeTag>>,
) -> Option<TypeTag> {
    let parent_hex = parent.to_hex_literal();
    let key_b64 = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let cache_key = format!("{}:{}", parent_hex, key_b64);
    if let Ok(cache_guard) = cache.lock() {
        if let Some(tag) = cache_guard.get(&cache_key) {
            return Some(tag.clone());
        }
    }
    let enum_limit = std::env::var("SUI_DF_ENUM_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1000);
    let field = match checkpoint {
        Some(cp) => gql
            .find_dynamic_field_by_bcs(&parent_hex, key_bytes, Some(cp), enum_limit)
            .or_else(|err| {
                if strict_checkpoint {
                    Err(err)
                } else {
                    gql.find_dynamic_field_by_bcs(&parent_hex, key_bytes, None, enum_limit)
                }
            }),
        None => gql.find_dynamic_field_by_bcs(&parent_hex, key_bytes, None, enum_limit),
    };
    let df = match field {
        Ok(Some(df)) => df,
        _ => return None,
    };
    let tag = match parse_type_tag(&df.name_type) {
        Ok(tag) => tag,
        _ => return None,
    };
    // Check if the on-chain object ID differs from the computed one (upgrade aliasing)
    if let Some(object_id) = df.object_id.as_deref() {
        let mut candidate_tags = vec![tag.clone()];
        let rewritten = rewrite_type_tag(tag.clone(), aliases);
        if rewritten != tag {
            candidate_tags.push(rewritten);
        }
        for candidate in candidate_tags {
            if let Ok(type_bcs) = bcs::to_bytes(&candidate) {
                if let Some(computed_hex) =
                    compute_dynamic_field_id(&parent_hex, key_bytes, &type_bcs)
                {
                    if let (Ok(computed_id), Ok(actual_id)) = (
                        AccountAddress::from_hex_literal(&computed_hex),
                        AccountAddress::from_hex_literal(object_id),
                    ) {
                        if computed_id != actual_id {
                            child_id_aliases.lock().insert(computed_id, actual_id);
                        }
                    }
                }
            }
        }
    }
    if let Ok(mut cache_guard) = cache.lock() {
        cache_guard.insert(cache_key, tag.clone());
    }
    Some(tag)
}

pub(super) fn fetch_child_object_by_key(
    options: &ChildFetchOptions<'_>,
    parent_id: AccountAddress,
    child_id: AccountAddress,
    key_type: &TypeTag,
    key_bytes: &[u8],
) -> Option<(TypeTag, Vec<u8>)> {
    let provider = options.provider;
    let checkpoint = options.checkpoint;
    let max_version = options.max_version;
    let strict_checkpoint = options.strict_checkpoint && checkpoint.is_some();
    let allow_latest = !strict_checkpoint;
    let aliases = options.aliases;
    let child_id_aliases = options.child_id_aliases;
    let miss_cache = options.miss_cache;
    let debug_df = options.debug_df;
    let debug_df_full = options.debug_df_full;
    let self_heal_dynamic_fields = options.self_heal_dynamic_fields;
    #[cfg(feature = "mm2")]
    let synth_modules = options.synth_modules.as_ref();
    #[cfg(feature = "mm2")]
    let log_self_heal = options.log_self_heal;
    #[cfg(not(feature = "mm2"))]
    let _ = (options.synth_modules.as_ref(), options.log_self_heal);

    let try_synthesize = |value_type: &str,
                          object_id: Option<&str>,
                          source: &str|
     -> Option<(TypeTag, Vec<u8>)> {
        if !self_heal_dynamic_fields {
            return None;
        }
        #[cfg(feature = "mm2")]
        {
            let modules = synth_modules?;
            let parsed = parse_type_tag(value_type).ok()?;
            let rewritten = rewrite_type_tag(parsed, aliases);
            let synth_type = format_type_tag(&rewritten);
            let type_model = match TypeModel::from_modules(modules.as_ref().clone()) {
                Ok(model) => model,
                Err(err) => {
                    if log_self_heal {
                        eprintln!("[df_self_heal] type model build failed: {}", err);
                    }
                    return None;
                }
            };
            let mut synthesizer = TypeSynthesizer::new(&type_model);
            let mut result = synthesizer.synthesize_with_fallback(&synth_type);
            let mut synth_id = child_id;
            if let Some(obj_id) = object_id.and_then(|s| AccountAddress::from_hex_literal(s).ok()) {
                if obj_id != child_id {
                    let mut map = child_id_aliases.lock();
                    map.insert(child_id, obj_id);
                }
                synth_id = obj_id;
                if result.bytes.len() >= 32 {
                    result.bytes[..32].copy_from_slice(synth_id.as_ref());
                }
            }
            if log_self_heal {
                eprintln!(
                    "[df_self_heal] synthesized source={} child={} type={} stub={} ({})",
                    source,
                    synth_id.to_hex_literal(),
                    synth_type,
                    result.is_stub,
                    result.description
                );
            }
            Some((rewritten, result.bytes))
        }
        #[cfg(not(feature = "mm2"))]
        {
            let _ = (value_type, object_id, source);
            None
        }
    };

    if let Some(obj) = provider.cache().get_object_latest(&child_id) {
        if obj.version <= max_version {
            if let Some(type_str) = obj.type_tag {
                if let Ok(tag) = parse_type_tag(&type_str) {
                    if debug_df {
                        eprintln!(
                            "[df_fetch] cache hit child={} type={}",
                            child_id.to_hex_literal(),
                            type_str
                        );
                    }
                    return Some((tag, obj.bcs_bytes));
                }
            }
        }
    }

    let gql = provider.graphql();
    let child_hex = child_id.to_hex_literal();
    let record_alias = |child_id: AccountAddress, object_id: &str| {
        if let Ok(actual) = AccountAddress::from_hex_literal(object_id) {
            if actual != child_id {
                let mut map = child_id_aliases.lock();
                map.insert(child_id, actual);
            }
        }
    };

    if let Some(cp) = checkpoint {
        if let Ok(obj) = gql.fetch_object_at_checkpoint(&child_hex, cp) {
            if obj.version <= max_version {
                if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&bcs_b64) {
                        if let Ok(tag) = parse_type_tag(&type_str) {
                            if debug_df {
                                eprintln!(
                                    "[df_fetch] checkpoint object child={} type={}",
                                    child_hex, type_str
                                );
                            }
                            return Some((tag, bytes));
                        }
                    }
                }
            }
        }
    }

    let parent_hex = parent_id.to_hex_literal();
    let miss_key = miss_cache.map(|_| {
        let key_type_str = format_type_tag(key_type);
        let key_b64 = base64::engine::general_purpose::STANDARD.encode(key_bytes);
        format!("{}:{}:{}:{}", parent_hex, child_hex, key_type_str, key_b64)
    });
    if let (Some(cache), Some(key)) = (miss_cache, miss_key.as_ref()) {
        if let Some(entry) = cache.lock().get(key).cloned() {
            let backoff_ms = std::env::var("SUI_DF_MISS_BACKOFF_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(250);
            let exp = entry.count.saturating_sub(1).min(3);
            let delay = backoff_ms.saturating_mul(1u64 << exp);
            if entry.last.elapsed().as_millis() < delay as u128 {
                if debug_df {
                    eprintln!(
                        "[df_fetch] cached miss/backoff parent={} child={} key_len={} delay_ms={}",
                        parent_hex,
                        child_hex,
                        key_bytes.len(),
                        delay
                    );
                }
                return None;
            }
        }
    }

    let mut reverse_aliases: HashMap<AccountAddress, AccountAddress> = HashMap::new();
    let mut reverse_aliases_all: HashMap<AccountAddress, Vec<AccountAddress>> = HashMap::new();
    if !aliases.is_empty() {
        for (storage, runtime) in aliases {
            reverse_aliases.insert(*runtime, *storage);
            reverse_aliases_all
                .entry(*runtime)
                .or_default()
                .push(*storage);
        }
    }
    let mut name_types = Vec::with_capacity(2);
    name_types.push(format_type_tag(key_type));
    if !aliases.is_empty() {
        let rewritten = rewrite_type_tag(key_type.clone(), aliases);
        let alt = format_type_tag(&rewritten);
        if alt != name_types[0] {
            name_types.push(alt);
        }
        let reverse = rewrite_type_tag(key_type.clone(), &reverse_aliases);
        let reverse_str = format_type_tag(&reverse);
        if !name_types.contains(&reverse_str) {
            name_types.push(reverse_str);
        }
        if let TypeTag::Struct(s) = key_type {
            if let Some(storages) = reverse_aliases_all.get(&s.address) {
                for storage in storages {
                    if *storage == s.address {
                        continue;
                    }
                    let mut reverse_map = HashMap::new();
                    reverse_map.insert(s.address, *storage);
                    let alt_tag = rewrite_type_tag(key_type.clone(), &reverse_map);
                    let alt_str = format_type_tag(&alt_tag);
                    if !name_types.contains(&alt_str) {
                        name_types.push(alt_str);
                    }
                }
            }
        }
    }
    let has_vector_u8 = name_types.iter().any(|t| t == "vector<u8>");
    let has_string = name_types.iter().any(|t| {
        t == "0x1::string::String"
            || t == "0x0000000000000000000000000000000000000000000000000000000000000001::string::String"
    });
    if has_vector_u8 && !has_string {
        name_types.push("0x1::string::String".to_string());
        name_types.push(
            "0x0000000000000000000000000000000000000000000000000000000000000001::string::String"
                .to_string(),
        );
    } else if has_string && !has_vector_u8 {
        name_types.push("vector<u8>".to_string());
    }

    let mut key_variants: Vec<Vec<u8>> = Vec::new();
    let mut key_variants_seen: HashSet<Vec<u8>> = HashSet::new();
    let mut push_key_variant = |bytes: Vec<u8>| {
        if key_variants_seen.insert(bytes.clone()) {
            key_variants.push(bytes);
        }
    };
    push_key_variant(key_bytes.to_vec());

    let mut type_name_variants: Vec<String> = Vec::new();
    let mut type_name_seen: HashSet<String> = HashSet::new();
    if is_type_name_tag(key_type) {
        if let Ok(raw_bytes) = bcs::from_bytes::<Vec<u8>>(key_bytes) {
            if let Ok(name_str) = String::from_utf8(raw_bytes) {
                if type_name_seen.insert(name_str.clone()) {
                    type_name_variants.push(name_str.clone());
                }
                if let Ok(parsed) = parse_type_tag(&name_str) {
                    let mut tag_variants = Vec::new();
                    tag_variants.push(parsed.clone());
                    let rewritten = rewrite_type_tag(parsed.clone(), aliases);
                    if rewritten != parsed {
                        tag_variants.push(rewritten);
                    }
                    if !reverse_aliases.is_empty() {
                        let reversed = rewrite_type_tag(parsed.clone(), &reverse_aliases);
                        if reversed != parsed {
                            tag_variants.push(reversed.clone());
                        }
                        if let TypeTag::Struct(s) = &parsed {
                            if let Some(storages) = reverse_aliases_all.get(&s.address) {
                                for storage in storages {
                                    if *storage == s.address {
                                        continue;
                                    }
                                    let mut reverse_map = HashMap::new();
                                    reverse_map.insert(s.address, *storage);
                                    let alt = rewrite_type_tag(parsed.clone(), &reverse_map);
                                    tag_variants.push(alt);
                                }
                            }
                        }
                    }
                    for tag in tag_variants {
                        let rendered = format_type_tag(&tag);
                        if type_name_seen.insert(rendered.clone()) {
                            type_name_variants.push(rendered);
                        }
                    }
                }
                for rendered in &type_name_variants {
                    if let Ok(bcs_bytes) = bcs::to_bytes(&rendered.as_bytes().to_vec()) {
                        push_key_variant(bcs_bytes);
                    }
                }
            }
        }
    }

    // If we can derive an alternate child ID from known name types, prefer cached hits.
    {
        let mut seen = std::collections::HashSet::new();
        for name_type in &name_types {
            let Ok(tag) = parse_type_tag(name_type) else {
                continue;
            };
            let Ok(type_bcs) = bcs::to_bytes(&tag) else {
                continue;
            };
            for key_variant in &key_variants {
                let Some(computed_hex) =
                    compute_dynamic_field_id(&parent_hex, key_variant, &type_bcs)
                else {
                    continue;
                };
                let Ok(computed_id) = AccountAddress::from_hex_literal(&computed_hex) else {
                    continue;
                };
                if !seen.insert(computed_id) {
                    continue;
                }
                if let Some(obj) = provider.cache().get_object_latest(&computed_id) {
                    if obj.version <= max_version {
                        if let Some(type_str) = obj.type_tag {
                            if let Ok(tag) = parse_type_tag(&type_str) {
                                if computed_id != child_id {
                                    let mut map = child_id_aliases.lock();
                                    map.insert(child_id, computed_id);
                                }
                                if debug_df {
                                    eprintln!(
                                        "[df_fetch] cache alias hit child={} alias={} type={}",
                                        child_hex,
                                        computed_id.to_hex_literal(),
                                        type_str
                                    );
                                }
                                return Some((tag, obj.bcs_bytes));
                            }
                        }
                    }
                }
                if self_heal_dynamic_fields {
                    if let Some((tag, bytes, _)) =
                        fetch_child_object_shared(provider, computed_id, checkpoint, max_version)
                    {
                        if computed_id != child_id {
                            let mut map = child_id_aliases.lock();
                            map.insert(child_id, computed_id);
                        }
                        if debug_df {
                            eprintln!(
                                "[df_fetch] fetched alias child={} alias={} type={}",
                                child_hex,
                                computed_id.to_hex_literal(),
                                format_type_tag(&tag)
                            );
                        }
                        return Some((tag, bytes));
                    }
                }
            }
        }
    }

    if debug_df && !type_name_variants.is_empty() {
        let preview = if debug_df_full {
            type_name_variants.join(" | ")
        } else {
            type_name_variants
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        };
        eprintln!(
            "[df_fetch] type_name variants parent={} child={} count={} [{}]",
            parent_hex,
            child_hex,
            type_name_variants.len(),
            preview
        );
    }

    for (variant_idx, key_variant) in key_variants.iter().enumerate() {
        for name_type in &name_types {
            let df = if let Some(cp) = checkpoint {
                match gql.fetch_dynamic_field_by_name_at_checkpoint(
                    &parent_hex,
                    name_type,
                    key_variant,
                    cp,
                ) {
                    Ok(Some(df)) => Ok(Some(df)),
                    Ok(None) => {
                        if allow_latest {
                            gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_variant)
                        } else {
                            Ok(None)
                        }
                    }
                    Err(err) => {
                        if allow_latest {
                            gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_variant)
                        } else {
                            Err(err)
                        }
                    }
                }
            } else {
                gql.fetch_dynamic_field_by_name(&parent_hex, name_type, key_variant)
            };
            if let Ok(Some(df)) = df {
                if let Some(version) = df.version {
                    if version > max_version {
                        continue;
                    }
                }
                if let Some(object_id) = df.object_id.as_deref() {
                    record_alias(child_id, object_id);
                    if let Some(version) = df.version {
                        if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        if debug_df {
                                            eprintln!(
                                                "[df_fetch] by_name object versioned child={} version={}",
                                                object_id, version
                                            );
                                        }
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                        if let Some((tag, bytes, _)) =
                            fetch_object_via_grpc_shared(provider, object_id, Some(version))
                        {
                            return Some((tag, bytes));
                        }
                    }
                    if let Some(cp) = checkpoint {
                        if let Ok(obj) = gql.fetch_object_at_checkpoint(object_id, cp) {
                            if obj.version <= max_version {
                                if let (Some(type_str), Some(bcs_b64)) =
                                    (obj.type_string, obj.bcs_base64)
                                {
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                    {
                                        if let Ok(tag) = parse_type_tag(&type_str) {
                                            if debug_df {
                                                eprintln!(
                                                    "[df_fetch] by_name object checkpoint child={} type={}",
                                                    object_id, type_str
                                                );
                                            }
                                            return Some((tag, bytes));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if allow_latest {
                        if let Ok(obj) = gql.fetch_object(object_id) {
                            if obj.version <= max_version {
                                if let (Some(type_str), Some(bcs_b64)) =
                                    (obj.type_string, obj.bcs_base64)
                                {
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                    {
                                        if let Ok(tag) = parse_type_tag(&type_str) {
                                            if debug_df {
                                                eprintln!(
                                                    "[df_fetch] by_name object child={} type={}",
                                                    object_id, type_str
                                                );
                                            }
                                            return Some((tag, bytes));
                                        }
                                    }
                                }
                            }
                        }
                        if let Some((tag, bytes, version)) =
                            fetch_object_via_grpc_shared(provider, object_id, None)
                        {
                            if version <= max_version {
                                return Some((tag, bytes));
                            }
                        }
                    }
                }
                if let (Some(value_type), Some(value_bcs)) = (&df.value_type, &df.value_bcs) {
                    if let Ok(bytes) =
                        base64::engine::general_purpose::STANDARD.decode(value_bcs.as_bytes())
                    {
                        if let Ok(tag) = parse_type_tag(value_type.as_str()) {
                            if debug_df {
                                if key_variants.len() > 1 {
                                    eprintln!(
                                        "[df_fetch] by_name hit parent={} name_type={} child={} value_type={} key_variant={}",
                                        parent_hex, name_type, child_hex, value_type, variant_idx
                                    );
                                } else {
                                    eprintln!(
                                        "[df_fetch] by_name hit parent={} name_type={} child={} value_type={}",
                                        parent_hex, name_type, child_hex, value_type
                                    );
                                }
                            }
                            return Some((tag, bytes));
                        }
                    }
                }
                if let Some(value_type) = df.value_type.as_deref() {
                    if let Some(synth) =
                        try_synthesize(value_type, df.object_id.as_deref(), "by_name")
                    {
                        return Some(synth);
                    }
                }
            } else if debug_df {
                eprintln!(
                    "[df_fetch] by_name miss parent={} name_type={} child={}",
                    parent_hex, name_type, child_hex
                );
            }
        }
    }

    let enum_limit = std::env::var("SUI_DF_ENUM_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1000);
    let key_b64s: Vec<String> = key_variants
        .iter()
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .collect();
    for name_type in &name_types {
        let fields = match checkpoint {
            Some(cp) => {
                if allow_latest {
                    gql.fetch_dynamic_fields_at_checkpoint(&parent_hex, enum_limit, cp)
                        .or_else(|_| gql.fetch_dynamic_fields(&parent_hex, enum_limit))
                } else {
                    gql.fetch_dynamic_fields_at_checkpoint(&parent_hex, enum_limit, cp)
                }
            }
            None => gql.fetch_dynamic_fields(&parent_hex, enum_limit),
        };
        let Ok(fields) = fields else {
            if debug_df {
                eprintln!(
                    "[df_fetch] enumerate failed parent={} name_type={}",
                    parent_hex, name_type
                );
            }
            continue;
        };
        let fields = if allow_latest && fields.is_empty() && checkpoint.is_some() {
            match gql.fetch_dynamic_fields(&parent_hex, enum_limit) {
                Ok(latest) if !latest.is_empty() => {
                    if debug_df {
                        eprintln!(
                            "[df_fetch] enumerate fallback latest parent={} name_type={} fields={}",
                            parent_hex,
                            name_type,
                            latest.len()
                        );
                    }
                    latest
                }
                _ => fields,
            }
        } else {
            fields
        };
        if debug_df {
            eprintln!(
                "[df_fetch] enumerate parent={} name_type={} fields={}",
                parent_hex,
                name_type,
                fields.len()
            );
            let key_preview = if debug_df_full {
                key_b64s.join("|")
            } else {
                key_b64s
                    .first()
                    .and_then(|b| b.get(0..16))
                    .unwrap_or("<none>")
                    .to_string()
            };
            eprintln!(
                "[df_fetch] key_b64 parent={} name_type={} key_b64={}",
                parent_hex, name_type, key_preview
            );
            for (idx, df) in fields.iter().take(5).enumerate() {
                let bcs_preview = df
                    .name_bcs
                    .as_deref()
                    .and_then(|s| s.get(0..16))
                    .unwrap_or("<none>");
                eprintln!(
                    "[df_fetch] enumerate field parent={} idx={} name_type={} name_bcs_prefix={}",
                    parent_hex, idx, df.name_type, bcs_preview
                );
                if debug_df_full {
                    let full = df.name_bcs.as_deref().unwrap_or("<none>");
                    eprintln!(
                        "[df_fetch] enumerate field full parent={} idx={} name_bcs_full={}",
                        parent_hex, idx, full
                    );
                }
            }
        }
        let mut fallback: Option<sui_transport::graphql::DynamicFieldInfo> = None;
        let mut fallback_count = 0usize;
        let mut fallback_missing_bcs: Option<sui_transport::graphql::DynamicFieldInfo> = None;
        let mut fallback_missing_bcs_count = 0usize;
        for df in &fields {
            let name_bcs = match df.name_bcs.as_deref() {
                Some(bcs) => bcs,
                None => {
                    if self_heal_dynamic_fields {
                        fallback_missing_bcs_count += 1;
                        if fallback_missing_bcs.is_none() {
                            fallback_missing_bcs = Some(df.clone());
                        }
                    }
                    continue;
                }
            };
            let mut matched = false;
            for (idx, key_b64) in key_b64s.iter().enumerate() {
                if name_bcs == key_b64.as_str() || b64_matches_bytes(name_bcs, &key_variants[idx]) {
                    matched = true;
                    break;
                }
            }
            if !matched {
                continue;
            }
            if df.name_type != *name_type {
                fallback_count += 1;
                if fallback.is_none() {
                    fallback = Some(df.clone());
                }
                continue;
            }
            if let Some(version) = df.version {
                if version > max_version {
                    continue;
                }
            }
            if let Some(object_id) = &df.object_id {
                record_alias(child_id, object_id);
                if let Some(version) = df.version {
                    if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                        if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                            if let Ok(bytes) =
                                base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                            {
                                if let Ok(tag) = parse_type_tag(&type_str) {
                                    if debug_df {
                                        eprintln!(
                                            "[df_fetch] enum object versioned child={} version={}",
                                            object_id, version
                                        );
                                    }
                                    return Some((tag, bytes));
                                }
                            }
                        }
                    }
                }
                if let Some(cp) = checkpoint {
                    if let Ok(obj) = gql.fetch_object_at_checkpoint(object_id, cp) {
                        if obj.version <= max_version {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        if debug_df {
                                            eprintln!(
                                                "[df_fetch] enum object checkpoint child={} type={}",
                                                object_id, type_str
                                            );
                                        }
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Ok(obj) = gql.fetch_object(object_id) {
                    if obj.version <= max_version {
                        if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                            if let Ok(bytes) =
                                base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                            {
                                if let Ok(tag) = parse_type_tag(&type_str) {
                                    if debug_df {
                                        eprintln!(
                                            "[df_fetch] enum object child={} type={}",
                                            object_id, type_str
                                        );
                                    }
                                    return Some((tag, bytes));
                                }
                            }
                        }
                    }
                }
                if let Some((tag, bytes, version)) =
                    fetch_object_via_grpc_shared(provider, object_id, None)
                {
                    if version <= max_version {
                        return Some((tag, bytes));
                    }
                }
            }
            if let (Some(value_type), Some(value_bcs)) = (&df.value_type, &df.value_bcs) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(value_bcs) {
                    if let Ok(tag) = parse_type_tag(value_type) {
                        if debug_df {
                            eprintln!(
                                "[df_fetch] enum hit parent={} name_type={} child={} value_type={}",
                                parent_hex, name_type, child_hex, value_type
                            );
                        }
                        return Some((tag, bytes));
                    }
                }
            }
            if let Some(value_type) = df.value_type.as_deref() {
                if let Some(synth) =
                    try_synthesize(value_type, df.object_id.as_deref(), "enumerate")
                {
                    return Some(synth);
                }
            }
        }
        if self_heal_dynamic_fields && fallback_count == 1 {
            if let Some(df) = fallback {
                if debug_df {
                    eprintln!(
                        "[df_fetch] enum fallback parent={} requested={} found={} child={}",
                        parent_hex, name_type, df.name_type, child_hex
                    );
                }
                if let Some(version) = df.version {
                    if version > max_version {
                        continue;
                    }
                }
                if let Some(object_id) = df.object_id.as_deref() {
                    record_alias(child_id, object_id);
                    if let Some(version) = df.version {
                        if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                        if let Some((tag, bytes, _)) =
                            fetch_object_via_grpc_shared(provider, object_id, Some(version))
                        {
                            return Some((tag, bytes));
                        }
                    }
                    if let Some(cp) = checkpoint {
                        if let Ok(obj) = gql.fetch_object_at_checkpoint(object_id, cp) {
                            if obj.version <= max_version {
                                if let (Some(type_str), Some(bcs_b64)) =
                                    (obj.type_string, obj.bcs_base64)
                                {
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                    {
                                        if let Ok(tag) = parse_type_tag(&type_str) {
                                            return Some((tag, bytes));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Ok(obj) = gql.fetch_object(object_id) {
                        if obj.version <= max_version {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                    }
                    if let Some((tag, bytes, version)) =
                        fetch_object_via_grpc_shared(provider, object_id, None)
                    {
                        if version <= max_version {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let (Some(value_type), Some(value_bcs)) = (&df.value_type, &df.value_bcs) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(value_bcs) {
                        if let Ok(tag) = parse_type_tag(value_type) {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let Some(value_type) = df.value_type.as_deref() {
                    if let Some(synth) =
                        try_synthesize(value_type, df.object_id.as_deref(), "fallback")
                    {
                        return Some(synth);
                    }
                }
            }
        }
        if self_heal_dynamic_fields && fallback_count == 0 && fallback_missing_bcs_count == 1 {
            if let Some(df) = fallback_missing_bcs {
                if debug_df {
                    eprintln!(
                        "[df_fetch] enum fallback missing name_bcs parent={} name_type={} child={}",
                        parent_hex, name_type, child_hex
                    );
                }
                if let Some(version) = df.version {
                    if version > max_version {
                        continue;
                    }
                }
                if let Some(object_id) = df.object_id.as_deref() {
                    record_alias(child_id, object_id);
                    if let Some(version) = df.version {
                        if let Ok(obj) = gql.fetch_object_at_version(object_id, version) {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                        if let Some((tag, bytes, _)) =
                            fetch_object_via_grpc_shared(provider, object_id, Some(version))
                        {
                            return Some((tag, bytes));
                        }
                    }
                    if let Some(cp) = checkpoint {
                        if let Ok(obj) = gql.fetch_object_at_checkpoint(object_id, cp) {
                            if obj.version <= max_version {
                                if let (Some(type_str), Some(bcs_b64)) =
                                    (obj.type_string, obj.bcs_base64)
                                {
                                    if let Ok(bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                    {
                                        if let Ok(tag) = parse_type_tag(&type_str) {
                                            return Some((tag, bytes));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Ok(obj) = gql.fetch_object(object_id) {
                        if obj.version <= max_version {
                            if let (Some(type_str), Some(bcs_b64)) =
                                (obj.type_string, obj.bcs_base64)
                            {
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(&bcs_b64)
                                {
                                    if let Ok(tag) = parse_type_tag(&type_str) {
                                        return Some((tag, bytes));
                                    }
                                }
                            }
                        }
                    }
                    if let Some((tag, bytes, version)) =
                        fetch_object_via_grpc_shared(provider, object_id, None)
                    {
                        if version <= max_version {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let (Some(value_type), Some(value_bcs)) = (&df.value_type, &df.value_bcs) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(value_bcs) {
                        if let Ok(tag) = parse_type_tag(value_type) {
                            return Some((tag, bytes));
                        }
                    }
                }
                if let Some(value_type) = df.value_type.as_deref() {
                    if let Some(synth) =
                        try_synthesize(value_type, df.object_id.as_deref(), "fallback_missing_bcs")
                    {
                        return Some(synth);
                    }
                }
            }
        }
    }

    if let Ok(obj) = gql.fetch_object(&child_hex) {
        if obj.version <= max_version {
            if let (Some(type_str), Some(bcs_b64)) = (obj.type_string, obj.bcs_base64) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&bcs_b64) {
                    if let Ok(tag) = parse_type_tag(&type_str) {
                        if debug_df {
                            eprintln!(
                                "[df_fetch] fallback object child={} type={}",
                                child_hex, type_str
                            );
                        }
                        return Some((tag, bytes));
                    }
                }
            }
        }
    }

    if let Some((tag, bytes, version)) = fetch_object_via_grpc_shared(provider, &child_hex, None) {
        if version <= max_version {
            if debug_df {
                eprintln!(
                    "[df_fetch] fallback grpc child={} version={}",
                    child_hex, version
                );
            }
            return Some((tag, bytes));
        }
    }

    if debug_df {
        eprintln!(
            "[df_fetch] miss parent={} child={} key_len={}",
            parent_hex,
            child_hex,
            key_bytes.len()
        );
    }
    if let (Some(cache), Some(key)) = (miss_cache, miss_key) {
        let mut map = cache.lock();
        let entry = map.entry(key).or_insert_with(|| MissEntry {
            count: 0,
            last: std::time::Instant::now(),
        });
        entry.count = entry.count.saturating_add(1);
        entry.last = std::time::Instant::now();
    }
    None
}
