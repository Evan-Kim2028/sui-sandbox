use anyhow::{anyhow, Result};

use crate::sandbox_cli::output::format_effects;

use super::ReplayOutput;

pub(super) fn print_replay_result(result: &ReplayOutput, show_comparison: bool, verbose: bool) {
    println!("\x1b[1mTransaction Replay: {}\x1b[0m\n", result.digest);

    if let Some(effects) = result.effects_full.as_ref() {
        println!("\x1b[1mLocal PTB Result:\x1b[0m");
        println!("{}", format_effects(effects, verbose));
        println!("  Commands executed: {}", result.commands_executed);
    } else if result.local_success {
        println!("\x1b[32m✓ Local execution succeeded\x1b[0m");
        println!("  Commands executed: {}", result.commands_executed);
    } else {
        println!("\x1b[31m✗ Local execution failed\x1b[0m");
        if let Some(err) = &result.local_error {
            println!("  Error: {}", err);
        }
        println!("  Commands executed: {}", result.commands_executed);
    }

    println!("\n\x1b[1mExecution Path:\x1b[0m");
    println!(
        "  Source: requested={} effective={}",
        result.execution_path.requested_source, result.execution_path.effective_source
    );
    println!(
        "  Flags: vm_only={} allow_fallback={} auto_system_objects={}",
        result.execution_path.vm_only,
        result.execution_path.allow_fallback,
        result.execution_path.auto_system_objects
    );
    println!(
        "  Fallback used: {}",
        if result.execution_path.fallback_used {
            "yes"
        } else {
            "no"
        }
    );
    if !result.execution_path.fallback_reasons.is_empty() {
        println!(
            "  Fallback reasons: {}",
            result.execution_path.fallback_reasons.join(", ")
        );
    }
    println!(
        "  Prefetch: enabled={} depth={} limit={}",
        result.execution_path.dynamic_field_prefetch,
        result.execution_path.prefetch_depth,
        result.execution_path.prefetch_limit
    );
    println!(
        "  Dependencies: mode={} fetched={}",
        result.execution_path.dependency_fetch_mode,
        result.execution_path.dependency_packages_fetched
    );

    if show_comparison {
        if let Some(cmp) = &result.comparison {
            println!("\n\x1b[1mComparison with on-chain:\x1b[0m");
            println!(
                "  Status: {} (local: {}, on-chain: {})",
                if cmp.status_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[31m✗ mismatch\x1b[0m"
                },
                cmp.local_status,
                cmp.on_chain_status
            );
            println!(
                "  Created objects: {}",
                if cmp.created_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
            println!(
                "  Mutated objects: {}",
                if cmp.mutated_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
            println!(
                "  Deleted objects: {}",
                if cmp.deleted_match {
                    "\x1b[32m✓ match\x1b[0m"
                } else {
                    "\x1b[33m~ count differs\x1b[0m"
                }
            );
        } else {
            println!("\n\x1b[33mNote: No on-chain effects available for comparison\x1b[0m");
        }
    }
}

pub(super) fn build_replay_debug_json(
    category: &str,
    error: &str,
    output: Option<&ReplayOutput>,
    allow_fallback: bool,
) -> Result<String> {
    let payload = if let Some(out) = output {
        serde_json::json!({
            "command": "replay",
            "category": category,
            "error": error,
            "allow_fallback": allow_fallback,
            "digest": out.digest,
            "execution_path": &out.execution_path,
            "local_success": out.local_success,
            "failed_command_index": out.effects.as_ref().and_then(|e| e.failed_command_index),
            "failed_command_description": out.effects.as_ref().and_then(|e| e.failed_command_description.clone()),
            "hints": replay_hints_from_output(out),
            "timestamp_utc": chrono::Utc::now().to_rfc3339(),
        })
    } else {
        serde_json::json!({
            "command": "replay",
            "category": category,
            "error": error,
            "allow_fallback": allow_fallback,
            "hints": [
                "Check endpoint and authentication configuration",
                "Retry with --verbose for transport-level details",
                "Try --source grpc if walrus/hybrid data is unavailable"
            ],
            "timestamp_utc": chrono::Utc::now().to_rfc3339(),
        })
    };
    Ok(serde_json::to_string_pretty(&payload)?)
}

fn replay_hints_from_output(output: &ReplayOutput) -> Vec<String> {
    let mut hints = Vec::new();
    if output.execution_path.vm_only && output.execution_path.fallback_used {
        hints.push("vm-only is enabled but fallback was used; inspect command flags".to_string());
    }
    if !output.execution_path.allow_fallback && !output.local_success {
        hints.push("Retry with --allow-fallback to permit secondary data sources".to_string());
    }
    if output.execution_path.dependency_packages_fetched == 0 {
        hints.push(
            "Run `sui-sandbox analyze replay <DIGEST>` for package/input readiness hints"
                .to_string(),
        );
    }
    if hints.is_empty() {
        hints.push("Retry with --verbose and review failed command details above".to_string());
    }
    hints
}

pub(super) fn enforce_strict(output: &ReplayOutput) -> Result<()> {
    if !output.local_success {
        return Err(anyhow!(
            "strict replay failed: {}",
            output
                .local_error
                .as_deref()
                .unwrap_or("local execution failed")
        ));
    }
    if let Some(comp) = output.comparison.as_ref() {
        let ok =
            comp.status_match && comp.created_match && comp.mutated_match && comp.deleted_match;
        if !ok {
            return Err(anyhow!(
                "strict replay mismatch: status={} created={} mutated={} deleted={}",
                comp.status_match,
                comp.created_match,
                comp.mutated_match,
                comp.deleted_match
            ));
        }
    }
    Ok(())
}
