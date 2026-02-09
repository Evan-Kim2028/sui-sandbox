use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

use super::{
    AnalyzeDynamicMode, AnalyzeObjectsCmd, AnalyzeObjectsDynamicSettings,
    AnalyzeObjectsProfileFile, AnalyzeObjectsProfileInfo, AnalyzeSemanticMode,
};

fn dynamic_settings_for_semantic(mode: AnalyzeSemanticMode) -> AnalyzeObjectsDynamicSettings {
    match mode {
        AnalyzeSemanticMode::Broad => AnalyzeObjectsDynamicSettings {
            mode: AnalyzeDynamicMode::Broad,
            lookback: 24,
            include_wrapper_apis: true,
            field_container_heuristic: true,
            use_uid_owner_flow: false,
            use_ref_param_owner_fallback: true,
        },
        AnalyzeSemanticMode::Strict => AnalyzeObjectsDynamicSettings {
            mode: AnalyzeDynamicMode::Strict,
            lookback: 12,
            include_wrapper_apis: false,
            field_container_heuristic: false,
            use_uid_owner_flow: true,
            use_ref_param_owner_fallback: false,
        },
        AnalyzeSemanticMode::Hybrid => AnalyzeObjectsDynamicSettings {
            mode: AnalyzeDynamicMode::Hybrid,
            lookback: 12,
            include_wrapper_apis: true,
            field_container_heuristic: false,
            use_uid_owner_flow: true,
            use_ref_param_owner_fallback: true,
        },
    }
}

fn builtin_objects_profile(name: &str) -> Option<AnalyzeObjectsProfileInfo> {
    let semantic_mode = match name.to_ascii_lowercase().as_str() {
        "broad" => AnalyzeSemanticMode::Broad,
        "strict" => AnalyzeSemanticMode::Strict,
        "hybrid" => AnalyzeSemanticMode::Hybrid,
        _ => return None,
    };
    Some(AnalyzeObjectsProfileInfo {
        name: semantic_mode.as_str().to_string(),
        source: "builtin".to_string(),
        path: None,
        semantic_mode,
        dynamic: dynamic_settings_for_semantic(semantic_mode),
    })
}

fn apply_semantic_defaults(
    profile: &mut AnalyzeObjectsProfileInfo,
    semantic_mode: AnalyzeSemanticMode,
) {
    profile.semantic_mode = semantic_mode;
    profile.dynamic = dynamic_settings_for_semantic(semantic_mode);
}

fn apply_profile_overrides(
    profile: &mut AnalyzeObjectsProfileInfo,
    file: &AnalyzeObjectsProfileFile,
) {
    if let Some(mode) = file.semantic_mode {
        apply_semantic_defaults(profile, mode);
    }
    if let Some(mode) = file.dynamic.mode {
        profile.dynamic.mode = mode;
    }
    if let Some(lookback) = file.dynamic.lookback {
        profile.dynamic.lookback = lookback;
    }
    if let Some(v) = file.dynamic.include_wrapper_apis {
        profile.dynamic.include_wrapper_apis = v;
    }
    if let Some(v) = file.dynamic.field_container_heuristic {
        profile.dynamic.field_container_heuristic = v;
    }
    if let Some(v) = file.dynamic.use_uid_owner_flow {
        profile.dynamic.use_uid_owner_flow = v;
    }
    if let Some(v) = file.dynamic.use_ref_param_owner_fallback {
        profile.dynamic.use_ref_param_owner_fallback = v;
    }
}

fn repo_profile_dir() -> Result<PathBuf> {
    Ok(std::env::current_dir()
        .context("resolve current directory for profile lookup")?
        .join(".sui-sandbox")
        .join("analyze")
        .join("profiles"))
}

fn global_profile_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("sui-sandbox").join("analyze").join("profiles"))
}

fn profile_file_name(name: &str) -> String {
    if name.ends_with(".yaml") || name.ends_with(".yml") {
        name.to_string()
    } else {
        format!("{name}.yaml")
    }
}

fn looks_like_profile_path(reference: &str) -> bool {
    reference.contains('/')
        || reference.contains('\\')
        || reference.ends_with(".yaml")
        || reference.ends_with(".yml")
        || reference.starts_with('.')
}

fn read_profile_file(path: &Path) -> Result<AnalyzeObjectsProfileFile> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read profile file {}", path.display()))?;
    serde_yaml::from_str(&raw).with_context(|| format!("parse profile file {}", path.display()))
}

fn resolve_named_profile_path(name: &str) -> Result<Option<(String, PathBuf)>> {
    let file_name = profile_file_name(name);
    let repo_candidate = repo_profile_dir()?.join(&file_name);
    if repo_candidate.exists() {
        return Ok(Some(("repo".to_string(), repo_candidate)));
    }
    if let Some(global_dir) = global_profile_dir() {
        let global_candidate = global_dir.join(&file_name);
        if global_candidate.exists() {
            return Ok(Some(("global".to_string(), global_candidate)));
        }
    }
    Ok(None)
}

fn resolve_profile_from_file_path(
    path: &Path,
    source: &str,
    depth: usize,
) -> Result<AnalyzeObjectsProfileInfo> {
    if depth > 8 {
        return Err(anyhow!("profile resolution exceeded max depth (8)"));
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("resolve current directory for profile file")?
            .join(path)
    };
    let file = read_profile_file(&path)?;
    let mut profile = if let Some(extends) = file.extends.as_deref() {
        resolve_profile_reference(extends, path.parent(), depth + 1)?
    } else {
        builtin_objects_profile("hybrid").expect("builtin hybrid must exist")
    };
    apply_profile_overrides(&mut profile, &file);
    profile.name = file.name.unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "custom".to_string())
    });
    profile.source = source.to_string();
    profile.path = Some(path.display().to_string());
    Ok(profile)
}

fn resolve_profile_reference(
    reference: &str,
    local_dir: Option<&Path>,
    depth: usize,
) -> Result<AnalyzeObjectsProfileInfo> {
    let ref_name = reference.trim();
    if let Some(profile) = builtin_objects_profile(ref_name) {
        return Ok(profile);
    }
    if looks_like_profile_path(ref_name) {
        let path = PathBuf::from(ref_name);
        let resolved = if path.is_absolute() {
            path
        } else if let Some(dir) = local_dir {
            dir.join(path)
        } else {
            std::env::current_dir()
                .context("resolve current directory for profile reference")?
                .join(path)
        };
        return resolve_profile_from_file_path(&resolved, "file", depth + 1);
    }
    if let Some((source, path)) = resolve_named_profile_path(ref_name)? {
        return resolve_profile_from_file_path(&path, &source, depth + 1);
    }
    Err(anyhow!(
        "profile `{}` not found (checked built-in, repo, and global profile dirs)",
        ref_name
    ))
}

pub(super) fn resolve_objects_profile(
    cmd: &AnalyzeObjectsCmd,
) -> Result<AnalyzeObjectsProfileInfo> {
    let mut profile = if let Some(path) = cmd.profile_file.as_ref() {
        resolve_profile_from_file_path(path, "file", 0)?
    } else if let Some(name) = cmd.profile.as_deref() {
        resolve_profile_reference(name, None, 0)?
    } else {
        builtin_objects_profile("hybrid").expect("builtin hybrid must exist")
    };

    if let Some(mode) = cmd.semantic_mode {
        apply_semantic_defaults(&mut profile, mode);
    }
    if let Some(lookback) = cmd.dynamic_lookback {
        if lookback == 0 {
            return Err(anyhow!("--dynamic-lookback must be >= 1"));
        }
        profile.dynamic.lookback = lookback;
    }
    Ok(profile)
}

pub(super) fn dynamic_confidence_label(profile: &AnalyzeObjectsProfileInfo) -> String {
    match profile.dynamic.mode {
        AnalyzeDynamicMode::Broad => {
            "medium-low (broad container/API heuristic; higher recall, more false positives)"
                .to_string()
        }
        AnalyzeDynamicMode::Strict => format!(
            "medium-high (UID-owner flow + dynamic API call sites, lookback={})",
            profile.dynamic.lookback
        ),
        AnalyzeDynamicMode::Hybrid => format!(
            "medium (strict UID-owner flow plus optional broad fallbacks, lookback={})",
            profile.dynamic.lookback
        ),
    }
}
