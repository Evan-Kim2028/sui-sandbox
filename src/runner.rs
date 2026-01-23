//! Package interface extraction runner.
//!
//! Handles single-package and batch processing modes for extracting
//! Move module interfaces from on-chain packages or local bytecode.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::str::FromStr;
use std::sync::Arc;

use sui_sdk::types::base_types::ObjectID;

use crate::args::{Args, InputKind, RetryConfig, RetryConfigExt};
use crate::bytecode::{
    build_bytecode_interface_value_from_compiled_modules, compiled_module_name,
    extract_sanity_counts, read_local_compiled_modules, read_package_id_from_metadata,
};
use crate::comparator::{
    bytecode_module_check, compare_interface_rpc_vs_bytecode, InterfaceCompareOptions,
};
use crate::corpus::collect_package_ids;
use crate::move_stubs::emit_move_stubs;
use crate::rpc::{
    build_interface_value_for_package, fetch_bcs_module_map_bytes, fetch_bcs_module_names,
    resolve_package_address_from_package_info,
};
use crate::types::{BatchSummaryRow, InterfaceCompareReport};
use crate::utils::{check_stability, write_canonical_json};

pub async fn run_single(
    args: &Args,
    client: Arc<sui_sdk::SuiClient>,
    input_id_str: &str,
) -> Result<()> {
    let input_id = ObjectID::from_str(input_id_str)
        .map_err(|e| anyhow!("invalid --package-id {}: {}", input_id_str, e))?;

    let retry = RetryConfig::from_args(args);

    if args.emit_compare_report.is_some() && !args.compare_bytecode_rpc {
        return Err(anyhow!(
            "--emit-compare-report requires --compare-bytecode-rpc"
        ));
    }

    let (package_oid, module_names, interface_value) = match args.input_kind {
        InputKind::Package => {
            let (names, v) =
                build_interface_value_for_package(Arc::clone(&client), input_id, retry).await?;
            (input_id, names, v)
        }
        InputKind::PackageInfo => {
            let package_id =
                resolve_package_address_from_package_info(Arc::clone(&client), input_id, retry)
                    .await?;
            let (names, v) =
                build_interface_value_for_package(Arc::clone(&client), package_id, retry).await?;
            (package_id, names, v)
        }
        InputKind::Auto => {
            match build_interface_value_for_package(Arc::clone(&client), input_id, retry).await {
                Ok((names, v)) => (input_id, names, v),
                Err(_e) => {
                    let package_id = resolve_package_address_from_package_info(
                        Arc::clone(&client),
                        input_id,
                        retry,
                    )
                    .await?;
                    let (names, v) =
                        build_interface_value_for_package(Arc::clone(&client), package_id, retry)
                            .await?;
                    (package_id, names, v)
                }
            }
        }
    };

    if args.bytecode_check {
        let bcs_names = fetch_bcs_module_names(Arc::clone(&client), package_oid, retry).await?;
        let check = bytecode_module_check(&module_names, &bcs_names);
        eprintln!(
            "bytecode_check: normalized_modules={} bcs_modules={} missing_in_bcs={} extra_in_bcs={} ",
            check.normalized_modules,
            check.bcs_modules,
            check.missing_in_bcs.len(),
            check.extra_in_bcs.len()
        );
    }

    if args.check_stability {
        check_stability(&interface_value)?;
    }

    if args.sanity {
        let counts = extract_sanity_counts(interface_value.get("modules").unwrap_or(&Value::Null));
        eprintln!(
            "sanity: modules={} structs={} functions={} key_structs={}",
            counts.modules, counts.structs, counts.functions, counts.key_structs
        );
    }

    if let Some(path) = args.emit_json.as_ref() {
        write_canonical_json(path, &interface_value)?;
    }

    if args.emit_bytecode_json.is_some() || args.compare_bytecode_rpc {
        let module_map =
            fetch_bcs_module_map_bytes(Arc::clone(&client), package_oid, retry).await?;
        let mut compiled = Vec::with_capacity(module_map.len());
        for (name, bytes) in module_map {
            let module =
                move_binary_format::file_format::CompiledModule::deserialize_with_defaults(&bytes)
                    .with_context(|| format!("deserialize module {} for {}", name, package_oid))?;
            let self_name = compiled_module_name(&module);
            if self_name != name {
                return Err(anyhow!(
                    "module name mismatch for {}: moduleMap key={} compiled.self_id.name={}",
                    package_oid,
                    name,
                    self_name
                ));
            }
            compiled.push(module);
        }

        let (_names, bytecode_value) = build_bytecode_interface_value_from_compiled_modules(
            &package_oid.to_string(),
            &compiled,
        )?;

        if let Some(path) = args.emit_bytecode_json.as_ref() {
            write_canonical_json(path, &bytecode_value)?;
        }

        if args.sanity {
            let counts =
                extract_sanity_counts(bytecode_value.get("modules").unwrap_or(&Value::Null));
            eprintln!(
                "sanity(bytecode): modules={} structs={} functions={} key_structs={}",
                counts.modules, counts.structs, counts.functions, counts.key_structs
            );
        }

        if args.compare_bytecode_rpc {
            let (summary, mismatches) = compare_interface_rpc_vs_bytecode(
                &package_oid.to_string(),
                &interface_value,
                &bytecode_value,
                InterfaceCompareOptions {
                    max_mismatches: args.compare_max_mismatches,
                    include_values: args.emit_compare_report.is_some(),
                },
            );
            eprintln!(
                "interface_compare: modules_compared={} modules_missing_in_bytecode={} modules_extra_in_bytecode={} structs_compared={} struct_mismatches={} functions_compared={} function_mismatches={} mismatches_total={}",
                summary.modules_compared,
                summary.modules_missing_in_bytecode,
                summary.modules_extra_in_bytecode,
                summary.structs_compared,
                summary.struct_mismatches,
                summary.functions_compared,
                summary.function_mismatches,
                summary.mismatches_total
            );

            if let Some(path) = args.emit_compare_report.as_ref() {
                let report = InterfaceCompareReport {
                    package_id: package_oid.to_string(),
                    summary,
                    mismatches,
                };
                let mut report_value =
                    serde_json::to_value(report).context("serialize compare report")?;
                crate::utils::canonicalize_json_value(&mut report_value);
                write_canonical_json(path, &report_value)?;
            }
        }
    }

    let mut list_modules = args.list_modules;
    if !list_modules
        && args.emit_json.is_none()
        && args.emit_bytecode_json.is_none()
        && !args.compare_bytecode_rpc
        && args.emit_compare_report.is_none()
        && !args.sanity
    {
        list_modules = true;
    }

    if list_modules {
        println!("modules={} ", module_names.len());
        for name in &module_names {
            println!("- {}", name);
        }
    }

    Ok(())
}

pub async fn run_single_local_bytecode_dir(args: &Args) -> Result<()> {
    let Some(dir) = args.bytecode_package_dir.as_ref() else {
        return Err(anyhow!("missing --bytecode-package-dir"));
    };

    if args.compare_bytecode_rpc || args.emit_compare_report.is_some() {
        return Err(anyhow!(
            "--compare-bytecode-rpc/--emit-compare-report require RPC; use --package-id mode"
        ));
    }

    let package_id = match read_package_id_from_metadata(dir) {
        Ok(id) => id,
        Err(_) => {
            let ids = collect_package_ids(args)?;
            if ids.len() != 1 {
                return Err(anyhow!(
                    "--bytecode-package-dir requires metadata.json with 'id' or exactly one --package-id"
                ));
            }
            ids[0].clone()
        }
    };

    let compiled = read_local_compiled_modules(dir)?;
    let (module_names, bytecode_value) =
        build_bytecode_interface_value_from_compiled_modules(&package_id, &compiled)?;

    if let Some(path) = args.emit_bytecode_json.as_ref() {
        write_canonical_json(path, &bytecode_value)?;
    }

    // Generate Move source stubs if requested
    if let Some(stubs_dir) = args.emit_move_stubs.as_ref() {
        // Use a reasonable package alias (could be configurable in the future)
        let pkg_alias = "target_pkg";
        let stubs = emit_move_stubs(&compiled, pkg_alias, stubs_dir)?;
        eprintln!(
            "emit_move_stubs: wrote {} stub files to {}",
            stubs.len(),
            stubs_dir.display()
        );
    }

    if args.sanity {
        let counts = extract_sanity_counts(bytecode_value.get("modules").unwrap_or(&Value::Null));
        eprintln!(
            "sanity(bytecode): modules={} structs={} functions={} key_structs={}",
            counts.modules, counts.structs, counts.functions, counts.key_structs
        );
    }

    let mut list_modules = args.list_modules;
    if !list_modules
        && args.emit_bytecode_json.is_none()
        && args.emit_move_stubs.is_none()
        && !args.sanity
    {
        list_modules = true;
    }
    if list_modules {
        println!("modules={} ", module_names.len());
        for name in &module_names {
            println!("- {}", name);
        }
    }

    Ok(())
}

pub async fn run_batch(
    args: &Args,
    client: Arc<sui_sdk::SuiClient>,
    input_ids: Vec<String>,
) -> Result<()> {
    let Some(out_dir) = args.out_dir.as_ref() else {
        return Err(anyhow!("batch mode requires --out-dir"));
    };

    if args.emit_json.is_some() {
        return Err(anyhow!("--emit-json is only valid for single-package mode"));
    }

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

    if args.list_modules {
        return Err(anyhow!(
            "--list-modules is only valid for single-package mode"
        ));
    }

    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let summary_path = args
        .summary_jsonl
        .clone()
        .unwrap_or_else(|| out_dir.join("summary.jsonl"));

    let concurrency = args.concurrency.max(1);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    // Include input_id in return type so we can identify which task panicked
    let mut join_set: tokio::task::JoinSet<(String, Result<BatchSummaryRow>)> =
        tokio::task::JoinSet::new();
    let retry = RetryConfig::from_args(args);

    for input_id_str in input_ids {
        let client = Arc::clone(&client);
        let semaphore = Arc::clone(&semaphore);
        let out_dir = out_dir.clone();
        let sanity_enabled = args.sanity;
        let check_stability_enabled = args.check_stability;
        let skip_existing = args.skip_existing;
        let input_kind = args.input_kind;
        let retry_cfg = retry;
        let bytecode_check_enabled = args.bytecode_check;

        // Capture input_id for panic context before moving into async block
        let input_id_for_panic = input_id_str.clone();
        join_set.spawn(async move {
            let result: Result<BatchSummaryRow> = async {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|e| anyhow!("semaphore closed: {}", e))?;

                let input_id_for_row = input_id_str.clone();

            let input_oid = match ObjectID::from_str(&input_id_str) {
                Ok(id) => id,
                Err(e) => {
                    return Ok(BatchSummaryRow {
                        input_id: input_id_for_row,
                        package_id: None,
                        resolved_from_package_info: false,
                        ok: false,
                        skipped: false,
                        output_path: None,
                        sanity: None,
                        bytecode: None,
                        error: Some(format!("invalid id: {}", e)),
                    });
                }
            };

            let mut resolved_from_package_info = false;
            let mut package_oid = input_oid;

            if matches!(input_kind, InputKind::PackageInfo) {
                match resolve_package_address_from_package_info(Arc::clone(&client), input_oid, retry_cfg).await {
                    Ok(resolved) => {
                        resolved_from_package_info = true;
                        package_oid = resolved;
                    }
                    Err(e) => {
                        return Ok(BatchSummaryRow {
                            input_id: input_id_for_row,
                            package_id: None,
                            resolved_from_package_info: true,
                            ok: false,
                            skipped: false,
                            output_path: None,
                            sanity: None,
                            bytecode: None,
                            error: Some(format!("{:#}", e)),
                        });
                    }
                }
            }

            let mut package_id_str = package_oid.to_string();
            let mut output_path = out_dir.join(format!("{}.json", package_id_str));

            if skip_existing && output_path.exists() {
                return Ok(BatchSummaryRow {
                    input_id: input_id_for_row,
                    package_id: Some(package_id_str),
                    resolved_from_package_info,
                    ok: true,
                    skipped: true,
                    output_path: Some(output_path.display().to_string()),
                    sanity: None,
                    bytecode: None,
                    error: None,
                });
            }

            let fetch_result: Result<(Vec<String>, Value)> = match input_kind {
                InputKind::Package => build_interface_value_for_package(Arc::clone(&client), package_oid, retry_cfg).await,
                InputKind::PackageInfo => build_interface_value_for_package(Arc::clone(&client), package_oid, retry_cfg).await,
                InputKind::Auto => {
                    match build_interface_value_for_package(Arc::clone(&client), input_oid, retry_cfg).await {
                        Ok(v) => Ok(v),
                        Err(first_err) => {
                            let resolved_oid = match resolve_package_address_from_package_info(Arc::clone(&client), input_oid, retry_cfg).await {
                                Ok(resolved) => resolved,
                                Err(resolve_err) => {
                                    return Ok(BatchSummaryRow {
                                        input_id: input_id_for_row,
                                        package_id: None,
                                        resolved_from_package_info: false,
                                        ok: false,
                                        skipped: false,
                                        output_path: None,
                                        sanity: None,
                                        bytecode: None,
                                        error: Some(format!(
                                            "auto failed\n- as package: {:#}\n- as package-info: {:#}",
                                            first_err, resolve_err
                                        )),
                                    });
                                }
                            };

                            resolved_from_package_info = true;
                            package_oid = resolved_oid;
                            package_id_str = package_oid.to_string();
                            output_path = out_dir.join(format!("{}.json", package_id_str));

                            if skip_existing && output_path.exists() {
                                return Ok(BatchSummaryRow {
                                    input_id: input_id_for_row,
                                    package_id: Some(package_id_str),
                                    resolved_from_package_info,
                                    ok: true,
                                    skipped: true,
                                    output_path: Some(output_path.display().to_string()),
                                    sanity: None,
                                    bytecode: None,
                                    error: None,
                                });
                            }

                            build_interface_value_for_package(Arc::clone(&client), package_oid, retry_cfg).await
                        }
                    }
                }
            };

            let (module_names, interface_value) = match fetch_result {
                Ok(v) => v,
                Err(e) => {
                    return Ok(BatchSummaryRow {
                        input_id: input_id_for_row,
                        package_id: Some(package_id_str),
                        resolved_from_package_info,
                        ok: false,
                        skipped: false,
                        output_path: Some(output_path.display().to_string()),
                        sanity: None,
                        bytecode: None,
                        error: Some(format!("{:#}", e)),
                    });
                }
            };

            if check_stability_enabled {
                if let Err(e) = crate::utils::check_stability(&interface_value) {
                    return Ok(BatchSummaryRow {
                        input_id: input_id_for_row,
                        package_id: Some(package_id_str),
                        resolved_from_package_info,
                        ok: false,
                        skipped: false,
                        output_path: Some(output_path.display().to_string()),
                        sanity: None,
                        bytecode: None,
                        error: Some(format!("{:#}", e)),
                    });
                }
            }

            let sanity = if sanity_enabled {
                Some(extract_sanity_counts(
                    interface_value.get("modules").unwrap_or(&Value::Null),
                ))
            } else {
                None
            };

            let bytecode = if bytecode_check_enabled {
                match fetch_bcs_module_names(Arc::clone(&client), package_oid, retry_cfg).await {
                    Ok(bcs_names) => Some(bytecode_module_check(&module_names, &bcs_names)),
                    Err(e) => {
                        return Ok(BatchSummaryRow {
                            input_id: input_id_for_row,
                            package_id: Some(package_id_str),
                            resolved_from_package_info,
                            ok: false,
                            skipped: false,
                            output_path: Some(output_path.display().to_string()),
                            sanity: None,
                            bytecode: None,
                            error: Some(format!("bytecode_check failed: {:#}", e)),
                        });
                    }
                }
            } else {
                None
            };

            if let Err(e) = write_canonical_json(&output_path, &interface_value) {
                return Ok(BatchSummaryRow {
                    input_id: input_id_for_row,
                    package_id: Some(package_id_str),
                    resolved_from_package_info,
                    ok: false,
                    skipped: false,
                    output_path: Some(output_path.display().to_string()),
                    sanity: None,
                    bytecode: None,
                    error: Some(format!("{:#}", e)),
                });
            }

            Ok(BatchSummaryRow {
                input_id: input_id_for_row,
                package_id: Some(package_id_str),
                resolved_from_package_info,
                ok: true,
                skipped: false,
                output_path: Some(output_path.display().to_string()),
                sanity,
                bytecode,
                error: None,
            })
            }
            .await;
            (input_id_for_panic, result)
        });
    }

    let mut rows: Vec<BatchSummaryRow> = Vec::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok((_, Ok(row))) => rows.push(row),
            Ok((input_id, Err(e))) => rows.push(BatchSummaryRow {
                input_id,
                package_id: None,
                resolved_from_package_info: false,
                ok: false,
                skipped: false,
                output_path: None,
                sanity: None,
                bytecode: None,
                error: Some(format!("{:#}", e)),
            }),
            Err(e) => {
                // Task panicked or was cancelled - we lose the input_id context
                rows.push(BatchSummaryRow {
                    input_id: "<task_failed>".to_string(),
                    package_id: None,
                    resolved_from_package_info: false,
                    ok: false,
                    skipped: false,
                    output_path: None,
                    sanity: None,
                    bytecode: None,
                    error: Some(format!("task failed: {}", e)),
                });
            }
        }
    }

    rows.sort_by(|a, b| a.input_id.cmp(&b.input_id));

    let file = File::create(&summary_path)
        .with_context(|| format!("create {}", summary_path.display()))?;
    let mut writer = BufWriter::new(file);

    let mut ok = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for row in &rows {
        if row.ok {
            ok += 1;
        } else {
            failed += 1;
        }
        if row.skipped {
            skipped += 1;
        }

        serde_json::to_writer(&mut writer, row).context("write summary JSONL")?;
        writer
            .write_all(b"\n")
            .context("write summary JSONL newline")?;
    }

    eprintln!(
        "batch done: total={} ok={} failed={} skipped={} summary={} out_dir={}",
        rows.len(),
        ok,
        failed,
        skipped,
        summary_path.display(),
        out_dir.display()
    );

    Ok(())
}
