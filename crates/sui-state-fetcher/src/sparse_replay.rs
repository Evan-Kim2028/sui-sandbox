//! Sparse replay reporting utilities.
//!
//! This module makes replay data gaps explicit by recording where we
//! had to fall back to non-historical data or where data was missing.

use move_core_types::account_address::AccountAddress;

use crate::types::{ObjectID, ReplayState};

/// Policy knobs for sparse replay fetching.
#[derive(Debug, Clone)]
pub struct SparseReplayPolicy {
    /// Whether to prefetch dynamic field children.
    pub prefetch_dynamic_fields: bool,
    /// Maximum depth for dynamic field discovery.
    pub df_depth: usize,
    /// Maximum children per parent during dynamic field discovery.
    pub df_limit: usize,
    /// Whether to allow GraphQL current-version fallback when historical gRPC data is missing.
    pub allow_graphql_fallback: bool,
}

impl Default for SparseReplayPolicy {
    fn default() -> Self {
        Self {
            prefetch_dynamic_fields: true,
            df_depth: 3,
            df_limit: 200,
            allow_graphql_fallback: true,
        }
    }
}

/// Replay state plus a sparse replay report.
#[derive(Debug, Clone)]
pub struct SparseReplayOutcome {
    pub state: ReplayState,
    pub report: SparseReplayReport,
}

/// Report capturing where replay data was missing or degraded.
#[derive(Debug, Clone, Default)]
pub struct SparseReplayReport {
    pub object_fetches: Vec<ObjectFetchRecord>,
    pub package_fetches: Vec<PackageFetchRecord>,
    pub dynamic_fields_discovered: usize,
    pub dynamic_fields_fetched: usize,
    pub dynamic_field_failures: Vec<DynamicFieldFailure>,
    /// On-demand child fetch summary captured during execution.
    pub on_demand: OnDemandFetchSummary,
    pub notes: Vec<String>,
}

impl SparseReplayReport {
    /// Summarize the report into aggregate counts.
    pub fn summary(&self) -> SparseReplaySummary {
        let mut summary = SparseReplaySummary {
            objects_total: self.object_fetches.len(),
            packages_total: self.package_fetches.len(),
            dynamic_fields_discovered: self.dynamic_fields_discovered,
            dynamic_fields_fetched: self.dynamic_fields_fetched,
            dynamic_fields_failed: self.dynamic_field_failures.len(),
            on_demand_attempted: self.on_demand.attempted,
            on_demand_resolved: self.on_demand.resolved,
            on_demand_cache: self.on_demand.cache,
            on_demand_grpc: self.on_demand.grpc,
            on_demand_graphql: self.on_demand.graphql,
            on_demand_dynamic_fields: self.on_demand.dynamic_fields,
            on_demand_failed: self.on_demand.failed,
            ..Default::default()
        };

        for record in &self.object_fetches {
            match record.outcome {
                ObjectFetchOutcome::CacheHit => summary.objects_cached += 1,
                ObjectFetchOutcome::GrpcHistorical => summary.objects_grpc += 1,
                ObjectFetchOutcome::GraphqlFallbackCurrent { .. } => {
                    summary.objects_graphql_fallback += 1
                }
                ObjectFetchOutcome::PrefetchedDynamicField => summary.objects_prefetched += 1,
                ObjectFetchOutcome::Missing { .. } => summary.objects_missing += 1,
                ObjectFetchOutcome::Incomplete { .. } => summary.objects_incomplete += 1,
            }
        }

        for record in &self.package_fetches {
            match record.outcome {
                PackageFetchOutcome::CacheHit => summary.packages_cached += 1,
                PackageFetchOutcome::Grpc => summary.packages_grpc += 1,
                PackageFetchOutcome::Missing { .. } => summary.packages_missing += 1,
            }
        }

        summary
    }

    /// Returns true if no gaps or fallbacks were recorded.
    pub fn is_complete(&self) -> bool {
        let summary = self.summary();
        summary.objects_missing == 0
            && summary.objects_incomplete == 0
            && summary.objects_graphql_fallback == 0
            && summary.packages_missing == 0
    }
}

/// Aggregated counts derived from a sparse replay report.
#[derive(Debug, Clone, Default)]
pub struct SparseReplaySummary {
    pub objects_total: usize,
    pub objects_cached: usize,
    pub objects_grpc: usize,
    pub objects_graphql_fallback: usize,
    pub objects_prefetched: usize,
    pub objects_missing: usize,
    pub objects_incomplete: usize,
    pub packages_total: usize,
    pub packages_cached: usize,
    pub packages_grpc: usize,
    pub packages_missing: usize,
    pub dynamic_fields_discovered: usize,
    pub dynamic_fields_fetched: usize,
    pub dynamic_fields_failed: usize,
    pub on_demand_attempted: usize,
    pub on_demand_resolved: usize,
    pub on_demand_cache: usize,
    pub on_demand_grpc: usize,
    pub on_demand_graphql: usize,
    pub on_demand_dynamic_fields: usize,
    pub on_demand_failed: usize,
}

/// Summary of on-demand child fetching during execution.
#[derive(Debug, Clone, Default)]
pub struct OnDemandFetchSummary {
    pub attempted: usize,
    pub resolved: usize,
    pub cache: usize,
    pub grpc: usize,
    pub graphql: usize,
    pub dynamic_fields: usize,
    pub failed: usize,
    /// Sample of resolved child IDs (truncated).
    pub resolved_ids: Vec<String>,
    /// Sample of failed child IDs (truncated).
    pub failed_ids: Vec<String>,
}

/// Outcome of fetching a specific object.
#[derive(Debug, Clone)]
pub struct ObjectFetchRecord {
    pub id: ObjectID,
    pub requested_version: u64,
    pub outcome: ObjectFetchOutcome,
}

/// Object fetch outcome classification.
#[derive(Debug, Clone)]
pub enum ObjectFetchOutcome {
    CacheHit,
    GrpcHistorical,
    GraphqlFallbackCurrent { observed_version: u64 },
    PrefetchedDynamicField,
    Missing { reason: String },
    Incomplete { reason: String },
}

/// Outcome of fetching a specific package.
#[derive(Debug, Clone)]
pub struct PackageFetchRecord {
    pub id: AccountAddress,
    pub outcome: PackageFetchOutcome,
}

/// Package fetch outcome classification.
#[derive(Debug, Clone)]
pub enum PackageFetchOutcome {
    CacheHit,
    Grpc,
    Missing { reason: String },
}

/// Failure encountered when prefetching a dynamic field child.
#[derive(Debug, Clone)]
pub struct DynamicFieldFailure {
    pub parent_id: String,
    pub child_id: Option<String>,
    pub reason: String,
}
