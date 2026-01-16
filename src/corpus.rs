//! Corpus analysis for validating bytecode extraction across many packages.
//!
//! Analyzes a directory of local bytecode packages, optionally comparing against
//! RPC-normalized interfaces and BCS module bytes.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use sui_sdk::types::base_types::ObjectID;

use crate::args::{Args, MvrNetwork, RetryConfig};
use crate::bytecode::{
    analyze_local_bytecode_package, build_bytecode_interface_value_from_compiled_modules,
    collect_corpus_package_dirs, extract_sanity_counts, list_local_module_names_only,
    local_bytes_check_for_package, read_local_bcs_module_names, read_local_compiled_modules,
    read_package_id_from_metadata,
};
use crate::comparator::{
    compare_interface_rpc_vs_bytecode, module_set_diff, InterfaceCompareOptions,
};
use crate::rpc::build_interface_value_for_package;
use crate::types::{
    CorpusIndexRow, CorpusRow, CorpusStats, CorpusSummary, LocalBytecodeCounts, ModuleSetDiff,
    RunMetadata, SubmissionSummary,
};
use crate::utils::{
    fnv1a64, git_head_metadata_for_path, git_metadata_for_path, now_unix_seconds,
    write_canonical_json,
};

/// Create an error CorpusRow with the given message. All counts are zeroed.
fn error_corpus_row(package_id: String, package_dir: String, error: String) -> CorpusRow {
    CorpusRow {
        package_id,
        package_dir,
        local: LocalBytecodeCounts::default(),
        local_vs_bcs: ModuleSetDiff::default(),
        local_bytes_check: None,
        local_bytes_check_error: None,
        rpc: None,
        rpc_vs_local: None,
        interface_compare: None,
        interface_compare_sample: None,
        error: Some(error),
    }
}

pub async fn run_corpus(args: &Args, client: Arc<sui_sdk::SuiClient>) -> Result<()> {
    let Some(root) = args.bytecode_corpus_root.as_ref() else {
        return Err(anyhow!("corpus mode requires --bytecode-corpus-root"));
    };
    let Some(out_dir) = args.out_dir.as_ref() else {
        return Err(anyhow!("corpus mode requires --out-dir"));
    };

    if args.emit_bytecode_json.is_some() {
        return Err(anyhow!(
            "--emit-bytecode-json is only valid for single-package mode"
        ));
    }
    if args.compare_bytecode_rpc || args.emit_compare_report.is_some() {
        return Err(anyhow!(
            "--compare-bytecode-rpc/--emit-compare-report are only valid for single-package mode"
        ));
    }
    if args.corpus_interface_compare && !args.corpus_rpc_compare {
        return Err(anyhow!(
            "--corpus-interface-compare requires --corpus-rpc-compare"
        ));
    }
    if args.corpus_module_names_only && (args.corpus_rpc_compare || args.corpus_interface_compare) {
        return Err(anyhow!(
            "--corpus-module-names-only is not compatible with --corpus-rpc-compare/--corpus-interface-compare"
        ));
    }

    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let run_started_at = now_unix_seconds();
    let argv: Vec<String> = std::env::args().collect();
    let sui_packages_git = git_metadata_for_path(root);

    let report_path = out_dir.join("corpus_report.jsonl");
    let problems_path = out_dir.join("problems.jsonl");
    let summary_path = out_dir.join("corpus_summary.json");
    let run_metadata_path = out_dir.join("run_metadata.json");
    let submission_summary_path = args.emit_submission_summary.clone();

    let index_path = args
        .corpus_index_jsonl
        .clone()
        .unwrap_or_else(|| out_dir.join("index.jsonl"));

    let sample_ids_path = args
        .corpus_sample_ids_out
        .clone()
        .unwrap_or_else(|| out_dir.join("sample_ids.txt"));

    let package_dirs = collect_corpus_package_dirs(root)?;
    let mut seen = HashSet::<String>::new();
    let mut targets: Vec<(String, PathBuf)> = Vec::new();
    for dir in package_dirs {
        let id = match read_package_id_from_metadata(&dir) {
            Ok(id) => id,
            Err(_) => continue,
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        targets.push((id, dir));
    }
    targets.sort_by(|a, b| a.0.cmp(&b.0));

    {
        let file = File::create(&index_path)
            .with_context(|| format!("create {}", index_path.display()))?;
        let mut writer = BufWriter::new(file);
        for (package_id, package_dir) in &targets {
            let row = CorpusIndexRow {
                package_id: package_id.clone(),
                package_dir: package_dir.display().to_string(),
            };
            serde_json::to_writer(&mut writer, &row).context("write index JSONL")?;
            writer.write_all(b"\n").ok();
        }
    }

    if let Some(path) = args.corpus_ids_file.as_ref() {
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let mut wanted = HashSet::<String>::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            wanted.insert(line.to_string());
        }

        let before = targets.len();
        let found_set: HashSet<&str> = targets.iter().map(|(id, _)| id.as_str()).collect();
        let mut missing: Vec<String> = wanted
            .iter()
            .filter(|id| !found_set.contains(id.as_str()))
            .cloned()
            .collect();
        missing.sort();

        targets.retain(|(id, _)| wanted.contains(id));
        if targets.is_empty() {
            return Err(anyhow!(
                "--corpus-ids-file selected 0 packages (wanted={}, missing={})",
                wanted.len(),
                missing.len()
            ));
        }

        if !missing.is_empty() {
            eprintln!(
                "corpus ids filter: selected {}/{} ({} missing; first missing: {})",
                targets.len(),
                before,
                missing.len(),
                missing.first().cloned().unwrap_or_default()
            );
        } else {
            eprintln!("corpus ids filter: selected {}/{}", targets.len(), before);
        }
    }

    if let Some(n) = args.corpus_sample {
        let seed = args.corpus_seed;
        let mut scored: Vec<(u64, String, PathBuf)> = targets
            .into_iter()
            .map(|(id, dir)| (fnv1a64(seed, &id), id, dir))
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        scored.truncate(n);
        targets = scored.into_iter().map(|(_h, id, dir)| (id, dir)).collect();
        targets.sort_by(|a, b| a.0.cmp(&b.0));

        let mut ids_text = String::new();
        for (id, _) in &targets {
            ids_text.push_str(id);
            ids_text.push('\n');
        }
        fs::write(&sample_ids_path, ids_text)
            .with_context(|| format!("write {}", sample_ids_path.display()))?;
    } else if let Some(max) = args.max_packages {
        targets.truncate(max);
    }

    let retry = RetryConfig::from_args(args);
    let concurrency = args.concurrency.max(1);
    if args.corpus_rpc_compare && concurrency > 1 {
        eprintln!(
            "note: --corpus-rpc-compare may hit rate limits; consider --concurrency 1 (current={})",
            concurrency
        );
    }
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut join_set: tokio::task::JoinSet<Result<CorpusRow>> = tokio::task::JoinSet::new();

    for (package_id_str, package_dir) in targets {
        let client = Arc::clone(&client);
        let semaphore = Arc::clone(&semaphore);
        let retry_cfg = retry;
        let do_rpc = args.corpus_rpc_compare;
        let do_interface_compare = args.corpus_interface_compare;
        let module_names_only = args.corpus_module_names_only;
        let do_local_bytes_check = args.corpus_local_bytes_check;
        let local_bytes_max_mismatches = args.corpus_local_bytes_check_max_mismatches;
        let corpus_max_mismatches = args.corpus_interface_compare_max_mismatches;
        let corpus_include_values = args.corpus_interface_compare_include_values;
        let compare_opts = InterfaceCompareOptions {
            max_mismatches: corpus_max_mismatches,
            include_values: corpus_include_values,
        };

        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|e| anyhow!("semaphore closed: {}", e))?;

            let dir_str = package_dir.display().to_string();

            let (local_module_names, local_counts) = if module_names_only {
                match list_local_module_names_only(&package_dir) {
                    Ok(names) => {
                        let counts = LocalBytecodeCounts {
                            modules: names.len(),
                            ..Default::default()
                        };
                        (names, counts)
                    }
                    Err(e) => {
                        return Ok(error_corpus_row(
                            package_id_str,
                            dir_str,
                            format!("local module name scan failed: {:#}", e),
                        ));
                    }
                }
            } else {
                match analyze_local_bytecode_package(&package_dir) {
                    Ok(v) => v,
                    Err(e) => {
                        return Ok(error_corpus_row(
                            package_id_str,
                            dir_str,
                            format!("local analysis failed: {:#}", e),
                        ));
                    }
                }
            };

            let bcs_module_names = match read_local_bcs_module_names(&package_dir) {
                Ok(v) => v,
                Err(e) => {
                    return Ok(CorpusRow {
                        package_id: package_id_str,
                        package_dir: dir_str,
                        local: local_counts,
                        local_vs_bcs: ModuleSetDiff {
                            left_count: 0,
                            right_count: 0,
                            missing_in_right: vec![],
                            extra_in_right: vec![],
                        },
                        local_bytes_check: None,
                        local_bytes_check_error: None,
                        rpc: None,
                        rpc_vs_local: None,
                        interface_compare: None,
                        interface_compare_sample: None,
                        error: Some(format!("read bcs.json failed: {:#}", e)),
                    });
                }
            };

            let local_vs_bcs = module_set_diff(&local_module_names, &bcs_module_names);
            let (local_bytes_check, local_bytes_check_error) = if do_local_bytes_check {
                match local_bytes_check_for_package(&package_dir, local_bytes_max_mismatches) {
                    Ok(check) => (Some(check), None),
                    Err(e) => (None, Some(format!("{:#}", e))),
                }
            } else {
                (None, None)
            };

            if !do_rpc {
                return Ok(CorpusRow {
                    package_id: package_id_str,
                    package_dir: dir_str,
                    local: local_counts,
                    local_vs_bcs,
                    local_bytes_check,
                    local_bytes_check_error,
                    rpc: None,
                    rpc_vs_local: None,
                    interface_compare: None,
                    interface_compare_sample: None,
                    error: None,
                });
            }

            let package_oid = ObjectID::from_str(&package_id_str).map_err(|e| {
                anyhow!("invalid package id from metadata {}: {}", package_id_str, e)
            })?;

            let (rpc_module_names, interface_value) =
                build_interface_value_for_package(Arc::clone(&client), package_oid, retry_cfg)
                    .await?;
            let rpc_counts =
                extract_sanity_counts(interface_value.get("modules").unwrap_or(&Value::Null));
            let rpc_vs_local = module_set_diff(&rpc_module_names, &local_module_names);

            let (interface_compare, interface_compare_sample) = if do_interface_compare {
                match read_local_compiled_modules(&package_dir).and_then(|compiled| {
                    let (_names, bytecode_value) =
                        build_bytecode_interface_value_from_compiled_modules(
                            &package_id_str,
                            &compiled,
                        )?;
                    let (summary, mismatches) = compare_interface_rpc_vs_bytecode(
                        &package_id_str,
                        &interface_value,
                        &bytecode_value,
                        compare_opts,
                    );
                    Ok((summary, mismatches))
                }) {
                    Ok((summary, mismatches)) => {
                        let sample = if summary.mismatches_total == 0 {
                            None
                        } else {
                            Some(mismatches)
                        };
                        (Some(summary), sample)
                    }
                    Err(e) => {
                        return Ok(CorpusRow {
                            package_id: package_id_str,
                            package_dir: dir_str,
                            local: local_counts,
                            local_vs_bcs,
                            local_bytes_check,
                            local_bytes_check_error,
                            rpc: Some(rpc_counts),
                            rpc_vs_local: Some(rpc_vs_local),
                            interface_compare: None,
                            interface_compare_sample: None,
                            error: Some(format!("interface compare failed: {:#}", e)),
                        });
                    }
                }
            } else {
                (None, None)
            };

            Ok(CorpusRow {
                package_id: package_id_str,
                package_dir: dir_str,
                local: local_counts,
                local_vs_bcs,
                local_bytes_check,
                local_bytes_check_error,
                rpc: Some(rpc_counts),
                rpc_vs_local: Some(rpc_vs_local),
                interface_compare,
                interface_compare_sample,
                error: None,
            })
        });
    }

    let mut rows: Vec<CorpusRow> = Vec::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(Ok(row)) => rows.push(row),
            Ok(Err(e)) => rows.push(error_corpus_row(
                "<join_error>".to_string(),
                "<unknown>".to_string(),
                format!("{:#}", e),
            )),
            Err(e) => rows.push(error_corpus_row(
                "<panic>".to_string(),
                "<unknown>".to_string(),
                format!("join error: {}", e),
            )),
        }
    }

    rows.sort_by(|a, b| a.package_id.cmp(&b.package_id));

    let file =
        File::create(&report_path).with_context(|| format!("create {}", report_path.display()))?;
    let mut writer = BufWriter::new(file);

    let mut total = 0usize;
    let mut local_ok = 0usize;
    let mut bcs_module_match = 0usize;
    let mut local_bytes_ok = 0usize;
    let mut local_bytes_mismatch_packages = 0usize;
    let mut local_bytes_mismatches_total = 0usize;
    let mut rpc_ok = 0usize;
    let mut rpc_module_match = 0usize;
    let mut rpc_exposed_function_count_match = 0usize;
    let mut interface_ok = 0usize;
    let mut interface_mismatch_packages = 0usize;
    let mut interface_mismatches_total = 0usize;
    let mut problems = 0usize;

    for row in &rows {
        total += 1;
        if row.error.is_none() {
            local_ok += 1;
        }
        if row.local_vs_bcs.missing_in_right.is_empty()
            && row.local_vs_bcs.extra_in_right.is_empty()
        {
            bcs_module_match += 1;
        }
        if args.corpus_local_bytes_check {
            match row.local_bytes_check.as_ref() {
                Some(check) => {
                    local_bytes_mismatches_total += check.mismatches_total;
                    if check.mismatches_total == 0 && row.local_bytes_check_error.is_none() {
                        local_bytes_ok += 1;
                    } else {
                        local_bytes_mismatch_packages += 1;
                    }
                }
                None => local_bytes_mismatch_packages += 1,
            }
        }
        if let Some(rpc) = row.rpc.as_ref() {
            rpc_ok += 1;
            if let Some(diff) = row.rpc_vs_local.as_ref() {
                if diff.missing_in_right.is_empty() && diff.extra_in_right.is_empty() {
                    rpc_module_match += 1;
                }
            }
            let local = row.local;
            let expected_exposed =
                local.functions_public + local.functions_friend + local.private_entry_functions;
            if local.modules == rpc.modules
                && local.structs == rpc.structs
                && expected_exposed == rpc.functions
                && local.key_structs == rpc.key_structs
            {
                rpc_exposed_function_count_match += 1;
            }
        }

        if args.corpus_interface_compare {
            match row.interface_compare.as_ref() {
                Some(s) => {
                    interface_mismatches_total += s.mismatches_total;
                    if s.mismatches_total == 0 {
                        interface_ok += 1;
                    } else {
                        interface_mismatch_packages += 1;
                    }
                }
                None => interface_mismatch_packages += 1,
            }
        }

        serde_json::to_writer(&mut writer, row).context("write corpus JSONL")?;
        writer.write_all(b"\n").ok();
    }

    {
        let file = File::create(&problems_path)
            .with_context(|| format!("create {}", problems_path.display()))?;
        let mut writer = BufWriter::new(file);

        for row in &rows {
            let mut is_problem = row.error.is_some();
            if !row.local_vs_bcs.missing_in_right.is_empty()
                || !row.local_vs_bcs.extra_in_right.is_empty()
            {
                is_problem = true;
            }
            if args.corpus_local_bytes_check {
                if row.local_bytes_check_error.is_some() {
                    is_problem = true;
                }
                match row.local_bytes_check.as_ref() {
                    Some(check) => {
                        if check.mismatches_total != 0 {
                            is_problem = true;
                        }
                    }
                    None => is_problem = true,
                }
            }

            if args.corpus_rpc_compare {
                match row.rpc.as_ref() {
                    None => is_problem = true,
                    Some(rpc) => {
                        let local = row.local;
                        let expected_exposed = local.functions_public
                            + local.functions_friend
                            + local.private_entry_functions;
                        if local.modules != rpc.modules
                            || local.structs != rpc.structs
                            || local.key_structs != rpc.key_structs
                            || expected_exposed != rpc.functions
                        {
                            is_problem = true;
                        }
                        if let Some(diff) = row.rpc_vs_local.as_ref() {
                            if !diff.missing_in_right.is_empty() || !diff.extra_in_right.is_empty()
                            {
                                is_problem = true;
                            }
                        }
                    }
                }
            }

            if args.corpus_interface_compare {
                if let Some(s) = row.interface_compare.as_ref() {
                    if s.mismatches_total != 0 {
                        is_problem = true;
                    }
                } else {
                    is_problem = true;
                }
            }

            if is_problem {
                problems += 1;
                serde_json::to_writer(&mut writer, row).context("write problems JSONL")?;
                writer.write_all(b"\n").ok();
            }
        }
    }

    // Create shared stats for both CorpusSummary and SubmissionSummary
    let stats = CorpusStats {
        total,
        local_ok,
        local_vs_bcs_module_match: bcs_module_match,
        local_bytes_check_enabled: args.corpus_local_bytes_check,
        local_bytes_ok,
        local_bytes_mismatch_packages,
        local_bytes_mismatches_total,
        rpc_enabled: args.corpus_rpc_compare,
        rpc_ok,
        rpc_module_match,
        rpc_exposed_function_count_match,
        interface_compare_enabled: args.corpus_interface_compare,
        interface_ok,
        interface_mismatch_packages,
        interface_mismatches_total,
        problems,
    };

    {
        let summary = CorpusSummary {
            stats: stats.clone(),
            report_jsonl: report_path.display().to_string(),
            index_jsonl: index_path.display().to_string(),
            problems_jsonl: problems_path.display().to_string(),
            sample_ids: args
                .corpus_sample
                .map(|_| sample_ids_path.display().to_string()),
            run_metadata_json: run_metadata_path.display().to_string(),
        };
        let file = File::create(&summary_path)
            .with_context(|| format!("create {}", summary_path.display()))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &summary).context("write corpus summary")?;
        writer.write_all(b"\n").ok();
    }

    {
        let meta = RunMetadata {
            started_at_unix_seconds: run_started_at,
            finished_at_unix_seconds: now_unix_seconds(),
            argv,
            rpc_url: args.rpc_url.clone(),
            bytecode_corpus_root: args
                .bytecode_corpus_root
                .as_ref()
                .map(|p| p.display().to_string()),
            sui_packages_git,
        };
        let file = File::create(&run_metadata_path)
            .with_context(|| format!("create {}", run_metadata_path.display()))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &meta).context("write run metadata")?;
        writer.write_all(b"\n").ok();
    }

    if let Some(path) = submission_summary_path.as_ref() {
        let corpus_name = args
            .bytecode_corpus_root
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());

        let summary = SubmissionSummary {
            tool: "sui-move-interface-extractor".to_string(),
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_unix_seconds: run_started_at,
            finished_at_unix_seconds: now_unix_seconds(),
            rpc_url: args.rpc_url.clone(),
            corpus_name,
            sui_packages_git: git_head_metadata_for_path(
                args.bytecode_corpus_root
                    .as_deref()
                    .unwrap_or_else(|| Path::new(".")),
            ),
            stats: stats.clone(),
        };

        let mut v = serde_json::to_value(summary).context("serialize submission summary")?;
        crate::utils::canonicalize_json_value(&mut v);
        write_canonical_json(path, &v)?;
    }

    eprintln!(
        "corpus done: total={} local_ok={} local_vs_bcs_module_match={} local_bytes_check_enabled={} local_bytes_ok={} local_bytes_mismatch_packages={} local_bytes_mismatches_total={} rpc_ok={} rpc_module_match={} rpc_exposed_function_count_match={} interface_ok={} interface_mismatch_packages={} interface_mismatches_total={} problems={} report={} index={} summary={} run_metadata={}",
        total,
        local_ok,
        bcs_module_match,
        args.corpus_local_bytes_check,
        local_bytes_ok,
        local_bytes_mismatch_packages,
        local_bytes_mismatches_total,
        rpc_ok,
        rpc_module_match,
        rpc_exposed_function_count_match,
        interface_ok,
        interface_mismatch_packages,
        interface_mismatches_total,
        problems,
        report_path.display(),
        index_path.display(),
        summary_path.display(),
        run_metadata_path.display()
    );

    if args.corpus_sample.is_some() {
        eprintln!("corpus sample ids written to {}", sample_ids_path.display());
    }

    Ok(())
}

pub fn collect_package_ids(args: &Args) -> Result<Vec<String>> {
    let mut ids = BTreeSet::<String>::new();

    for id in &args.package_id {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            ids.insert(trimmed.to_string());
        }
    }

    if let Some(path) = args.package_ids_file.as_ref() {
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            ids.insert(line.to_string());
        }
    }

    if let Some(path) = args.mvr_catalog.as_ref() {
        let catalog_text =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let catalog: Value = serde_json::from_str(&catalog_text)
            .with_context(|| format!("parse {}", path.display()))?;
        let Some(names) = catalog.get("names").and_then(Value::as_array) else {
            return Err(anyhow!("mvr catalog missing 'names' array"));
        };

        let field = match args.mvr_network {
            MvrNetwork::Mainnet => "mainnet_package_info_id",
            MvrNetwork::Testnet => "testnet_package_info_id",
        };

        for item in names {
            if let Some(id) = item.get(field).and_then(Value::as_str) {
                let trimmed = id.trim();
                if !trimmed.is_empty() {
                    ids.insert(trimmed.to_string());
                }
            }
        }
    }

    let mut ids: Vec<String> = ids.into_iter().collect();
    if let Some(max) = args.max_packages {
        ids.truncate(max);
    }
    Ok(ids)
}
