use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;

use sui_sandbox_core::tx_replay::{self, EffectsReconcilePolicy};
use sui_state_fetcher::{
    build_aliases as build_aliases_shared, fetch_child_object, HistoricalStateProvider,
};

use super::super::SandboxState;
use super::deps::fetch_dependency_closure;
use super::effects::build_effects_summary;
use super::hydration::{
    build_historical_state_provider, build_replay_state, ReplayHydrationConfig,
};
use super::support::{
    build_replay_object_maps, build_simulation_config, emit_linkage_debug_info,
    hydrate_resolver_from_replay_state, maybe_patch_replay_objects,
};
use super::{
    ComparisonResult, FetchStrategy, ReplayCmd, ReplayExecutionPath, ReplayOutput, ReplaySource,
    SourceApiCalls, SourceComparisonResult,
};

/// Lightweight result from a single-source replay pipeline.
struct SingleSourceResult {
    output: ReplayOutput,
    graphql_requests: u64,
    grpc_requests: u64,
    duration: std::time::Duration,
}

impl ReplayCmd {
    /// Run GraphQL-only and hybrid replays, compare results side-by-side.
    pub(super) async fn execute_compare_sources(
        &self,
        state: &SandboxState,
        _verbose: bool,
    ) -> Result<ReplayOutput> {
        let allow_fallback = self.hydration.allow_fallback && !self.vm_only;
        let digest = self.digest_required()?;

        eprintln!("[compare] building providers for graphql and hybrid...");

        // Build both providers concurrently
        let (gql_provider, hyb_provider) = tokio::try_join!(
            build_historical_state_provider(state, ReplaySource::Graphql, allow_fallback, false),
            build_historical_state_provider(state, ReplaySource::Hybrid, allow_fallback, false),
        )?;

        let enable_dynamic_fields =
            !self.hydration.no_prefetch && self.fetch_strategy == FetchStrategy::Full;
        let hydration_config = ReplayHydrationConfig {
            prefetch_dynamic_fields: enable_dynamic_fields,
            prefetch_depth: self.hydration.prefetch_depth,
            prefetch_limit: self.hydration.prefetch_limit,
            auto_system_objects: self.hydration.auto_system_objects,
        };

        eprintln!("[compare] fetching replay state from both sources...");

        // Fetch replay states concurrently (this is where most API calls happen)
        let (gql_state_result, hyb_state_result) = tokio::join!(
            build_replay_state(gql_provider.as_ref(), digest, hydration_config),
            build_replay_state(hyb_provider.as_ref(), digest, hydration_config),
        );

        // Run each pipeline sequentially (VM execution is fast)
        eprintln!("[compare] running graphql pipeline...");
        let gql_result = run_pipeline(
            self,
            state,
            &gql_provider,
            gql_state_result,
            "graphql",
            allow_fallback,
            enable_dynamic_fields,
        );

        eprintln!("[compare] running hybrid pipeline...");
        let hyb_result = run_pipeline(
            self,
            state,
            &hyb_provider,
            hyb_state_result,
            "hybrid",
            allow_fallback,
            enable_dynamic_fields,
        );

        // Build comparison
        let gql = gql_result;
        let hyb = hyb_result;

        let results_match = match (&gql, &hyb) {
            (Ok(g), Ok(h)) => {
                g.output.local_success == h.output.local_success
                    && g.output.local_error == h.output.local_error
                    && g.output.commands_executed == h.output.commands_executed
            }
            (Err(_), Err(_)) => true, // both failed to run
            _ => false,
        };

        let mut notes = Vec::new();
        if let (Ok(g), Ok(h)) = (&gql, &hyb) {
            if g.output.local_success != h.output.local_success {
                notes.push(format!(
                    "status differs: graphql={} hybrid={}",
                    if g.output.local_success {
                        "success"
                    } else {
                        "failed"
                    },
                    if h.output.local_success {
                        "success"
                    } else {
                        "failed"
                    },
                ));
            }
            if g.output.commands_executed != h.output.commands_executed {
                notes.push(format!(
                    "commands: graphql={} hybrid={}",
                    g.output.commands_executed, h.output.commands_executed,
                ));
            }
            if g.output.local_error != h.output.local_error {
                notes.push("error messages differ".to_string());
            }
        }

        let source_comparison = SourceComparisonResult {
            graphql_success: gql
                .as_ref()
                .map(|r| r.output.local_success)
                .unwrap_or(false),
            hybrid_success: hyb
                .as_ref()
                .map(|r| r.output.local_success)
                .unwrap_or(false),
            results_match,
            graphql_error: match &gql {
                Ok(r) => r.output.local_error.clone(),
                Err(e) => Some(e.to_string()),
            },
            hybrid_error: match &hyb {
                Ok(r) => r.output.local_error.clone(),
                Err(e) => Some(e.to_string()),
            },
            graphql_api_calls: SourceApiCalls {
                graphql: gql.as_ref().map(|r| r.graphql_requests).unwrap_or(0),
                grpc: gql.as_ref().map(|r| r.grpc_requests).unwrap_or(0),
            },
            hybrid_api_calls: SourceApiCalls {
                graphql: hyb.as_ref().map(|r| r.graphql_requests).unwrap_or(0),
                grpc: hyb.as_ref().map(|r| r.grpc_requests).unwrap_or(0),
            },
            graphql_duration_ms: gql.as_ref().map(|r| r.duration.as_millis()).unwrap_or(0),
            hybrid_duration_ms: hyb.as_ref().map(|r| r.duration.as_millis()).unwrap_or(0),
            notes,
        };

        // Use the graphql result as primary output, attach comparison
        let mut output = match gql {
            Ok(r) => r.output,
            Err(e) => ReplayOutput {
                digest: digest.to_string(),
                local_success: false,
                local_error: Some(e.to_string()),
                diagnostics: None,
                execution_path: ReplayExecutionPath {
                    requested_source: "compare".to_string(),
                    effective_source: "graphql".to_string(),
                    ..Default::default()
                },
                comparison: None,
                analysis: None,
                effects: None,
                effects_full: None,
                commands_executed: 0,
                source_comparison: None,
                batch_summary_printed: false,
            },
        };
        output.source_comparison = Some(source_comparison);
        output.execution_path.requested_source = "compare".to_string();
        Ok(output)
    }
}

/// Run the core replay pipeline for a single source.
fn run_pipeline(
    cmd: &ReplayCmd,
    state: &SandboxState,
    provider: &Arc<HistoricalStateProvider>,
    replay_state_result: Result<sui_state_fetcher::ReplayState>,
    source_label: &str,
    allow_fallback: bool,
    enable_dynamic_fields: bool,
) -> Result<SingleSourceResult> {
    let start = Instant::now();

    let replay_state = replay_state_result?;

    let pkg_aliases = build_aliases_shared(
        &replay_state.packages,
        Some(provider.as_ref()),
        replay_state.checkpoint,
    );

    let mut resolver = hydrate_resolver_from_replay_state(
        state,
        &replay_state,
        &pkg_aliases.linkage_upgrades,
        &pkg_aliases.aliases,
    );

    let fetched_deps = fetch_dependency_closure(
        &mut resolver,
        provider.graphql(),
        replay_state.checkpoint,
        false, // quiet
    )
    .unwrap_or(0);

    emit_linkage_debug_info(&resolver, &pkg_aliases.aliases);

    let mut maps = build_replay_object_maps(&replay_state, &pkg_aliases.versions);
    maybe_patch_replay_objects(
        &resolver,
        &replay_state,
        &pkg_aliases.versions,
        &pkg_aliases.aliases,
        &mut maps,
        false,
        false,
    );
    let versions_str = maps.versions_str.clone();
    let cached_objects = maps.cached_objects;
    let version_map = maps.version_map;

    let reconcile_policy = if cmd.reconcile_dynamic_fields {
        EffectsReconcilePolicy::DynamicFields
    } else {
        EffectsReconcilePolicy::Strict
    };

    let config = build_simulation_config(&replay_state);
    let mut harness = sui_sandbox_core::vm::VMHarness::with_config(&resolver, false, config)?;
    harness.set_address_aliases_with_versions(pkg_aliases.aliases.clone(), versions_str.clone());

    // Set up dynamic field fetcher if enabled
    if enable_dynamic_fields {
        let provider_clone = Arc::clone(provider);
        let checkpoint = replay_state.checkpoint;
        let max_version = version_map.values().copied().max().unwrap_or(0);
        let fetcher =
            move |_parent: move_core_types::account_address::AccountAddress,
                  child_id: move_core_types::account_address::AccountAddress| {
                fetch_child_object(&provider_clone, child_id, checkpoint, max_version)
            };
        harness.set_versioned_child_fetcher(Box::new(fetcher));
    }

    let replay_result = tx_replay::replay_with_version_tracking_with_policy_with_effects(
        &replay_state.transaction,
        &mut harness,
        &cached_objects,
        &pkg_aliases.aliases,
        Some(&versions_str),
        reconcile_policy,
    );

    let graphql_requests = provider.graphql().request_count();
    let grpc_requests = provider.grpc().request_count();
    let duration = start.elapsed();

    match replay_result {
        Ok(execution) => {
            let result = execution.result;
            let effects_summary = build_effects_summary(&execution.effects);
            let comparison = if cmd.compare {
                result.comparison.map(|c| ComparisonResult {
                    status_match: c.status_match,
                    created_match: c.created_count_match,
                    mutated_match: c.mutated_count_match,
                    deleted_match: c.deleted_count_match,
                    on_chain_status: if c.status_match && result.local_success {
                        "success".to_string()
                    } else if c.status_match && !result.local_success {
                        "failed".to_string()
                    } else {
                        "unknown".to_string()
                    },
                    local_status: if result.local_success {
                        "success".to_string()
                    } else {
                        "failed".to_string()
                    },
                    notes: c.notes.clone(),
                })
            } else {
                None
            };
            Ok(SingleSourceResult {
                output: ReplayOutput {
                    digest: replay_state.transaction.digest.0.clone(),
                    local_success: result.local_success,
                    local_error: result.local_error,
                    diagnostics: None,
                    execution_path: ReplayExecutionPath {
                        requested_source: source_label.to_string(),
                        effective_source: source_label.to_string(),
                        allow_fallback,
                        dynamic_field_prefetch: enable_dynamic_fields,
                        dependency_fetch_mode: "graphql_dependency_closure".to_string(),
                        dependency_packages_fetched: fetched_deps,
                        graphql_requests,
                        grpc_requests,
                        ..Default::default()
                    },
                    comparison,
                    analysis: None,
                    effects: Some(effects_summary),
                    effects_full: Some(execution.effects),
                    commands_executed: result.commands_executed,
                    source_comparison: None,
                    batch_summary_printed: false,
                },
                graphql_requests,
                grpc_requests,
                duration,
            })
        }
        Err(e) => Ok(SingleSourceResult {
            output: ReplayOutput {
                digest: replay_state.transaction.digest.0.clone(),
                local_success: false,
                local_error: Some(e.to_string()),
                diagnostics: None,
                execution_path: ReplayExecutionPath {
                    requested_source: source_label.to_string(),
                    effective_source: source_label.to_string(),
                    allow_fallback,
                    dynamic_field_prefetch: enable_dynamic_fields,
                    dependency_fetch_mode: "graphql_dependency_closure".to_string(),
                    dependency_packages_fetched: fetched_deps,
                    graphql_requests,
                    grpc_requests,
                    ..Default::default()
                },
                comparison: None,
                analysis: None,
                effects: None,
                effects_full: None,
                commands_executed: 0,
                source_comparison: None,
                batch_summary_printed: false,
            },
            graphql_requests,
            grpc_requests,
            duration,
        }),
    }
}
