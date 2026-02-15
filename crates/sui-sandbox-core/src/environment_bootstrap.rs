//! Generic environment bootstrap helpers for protocol examples and tooling.
//!
//! These APIs are protocol-agnostic: callers provide package roots, object ids
//! (optionally version-pinned), and sender identity. The helpers fetch state
//! from mainnet and build a ready-to-run `SimulationEnvironment`.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use move_core_types::account_address::AccountAddress;
use serde::{Deserialize, Serialize};
use sui_state_fetcher::{HistoricalStateProvider, PackageData};
use sui_transport::grpc::GrpcOwner;

use crate::bootstrap::{
    build_package_registration_plan, configure_environment_mainnet_fetchers,
    create_mainnet_provider, load_fetched_objects_into_env, preload_dynamic_field_objects,
    register_packages_with_linkage_plan, BootstrapFetchedObjectData,
};
use crate::simulation::SimulationEnvironment;

/// One object hydration request (latest or explicit historical version).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainnetObjectRequest {
    pub object_id: String,
    #[serde(default)]
    pub version: Option<u64>,
}

/// Mainnet hydration plan for package closure + selected objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainnetHydrationPlan {
    pub package_roots: Vec<AccountAddress>,
    #[serde(default)]
    pub objects: Vec<MainnetObjectRequest>,
    #[serde(default)]
    pub historical_mode: bool,
    #[serde(default = "default_allow_latest_object_fallback")]
    pub allow_latest_object_fallback: bool,
}

fn default_allow_latest_object_fallback() -> bool {
    true
}

/// Hydrated package/object state with the backing provider.
pub struct MainnetHydrationResult {
    pub provider: HistoricalStateProvider,
    pub packages: HashMap<AccountAddress, PackageData>,
    pub objects: HashMap<String, BootstrapFetchedObjectData>,
}

/// Environment build controls.
#[derive(Debug, Clone)]
pub struct EnvironmentBuildPlan {
    pub sender: AccountAddress,
    pub fail_on_object_load: bool,
}

impl Default for EnvironmentBuildPlan {
    fn default() -> Self {
        Self {
            sender: AccountAddress::ZERO,
            fail_on_object_load: true,
        }
    }
}

/// Build output for `SimulationEnvironment` initialization.
pub struct EnvironmentBuildResult {
    pub env: SimulationEnvironment,
    pub package_registration: crate::bootstrap::PackageRegistrationResult,
    pub objects_loaded: usize,
}

/// Optional runtime hydration/fetcher wiring controls applied after build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentFinalizePlan {
    /// Parent object IDs whose dynamic-field wrappers should be preloaded.
    #[serde(default)]
    pub dynamic_field_parents: Vec<String>,
    /// Per-parent dynamic-field scan limit.
    #[serde(default = "default_dynamic_field_limit")]
    pub dynamic_field_limit: usize,
    /// Whether to wire on-demand gRPC fetchers into the environment.
    #[serde(default = "default_configure_fetchers")]
    pub configure_fetchers: bool,
}

fn default_dynamic_field_limit() -> usize {
    32
}

fn default_configure_fetchers() -> bool {
    true
}

impl Default for EnvironmentFinalizePlan {
    fn default() -> Self {
        Self {
            dynamic_field_parents: Vec::new(),
            dynamic_field_limit: default_dynamic_field_limit(),
            configure_fetchers: default_configure_fetchers(),
        }
    }
}

/// Finalize result emitted after optional dynamic-field preload/fetcher wiring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentFinalizeResult {
    pub dynamic_fields_loaded: usize,
    pub fetchers_configured: bool,
}

/// Combined result for a hydrate + build flow.
pub struct HydratedEnvironmentResult {
    pub hydration: MainnetHydrationResult,
    pub build: EnvironmentBuildResult,
}

/// Combined result for hydrate + build + finalize flow.
pub struct FinalizedHydratedEnvironmentResult {
    pub hydration: MainnetHydrationResult,
    pub build: EnvironmentBuildResult,
    pub finalize: EnvironmentFinalizeResult,
}

/// Fetch package closure and requested objects from mainnet.
pub async fn hydrate_mainnet_state(plan: &MainnetHydrationPlan) -> Result<MainnetHydrationResult> {
    let provider = create_mainnet_provider(plan.historical_mode).await?;
    let packages = provider
        .fetch_packages_with_deps(&plan.package_roots, None, None)
        .await
        .context("failed to fetch package closure")?;

    let grpc = provider.grpc();
    let mut objects = HashMap::new();
    for request in &plan.objects {
        let by_version = match request.version {
            Some(version) => grpc
                .get_object_at_version(&request.object_id, Some(version))
                .await
                .with_context(|| {
                    format!(
                        "failed to fetch object {} at version {}",
                        request.object_id, version
                    )
                })?,
            None => None,
        };
        let object = if by_version.is_some() {
            by_version
        } else if request.version.is_some() && !plan.allow_latest_object_fallback {
            None
        } else {
            grpc.get_object(&request.object_id)
                .await
                .with_context(|| format!("failed to fetch latest object {}", request.object_id))?
        };

        let object = object.ok_or_else(|| {
            if let Some(version) = request.version {
                anyhow!(
                    "object {} (version {}) not found and latest fallback disabled",
                    request.object_id,
                    version
                )
            } else {
                anyhow!("object {} not found", request.object_id)
            }
        })?;

        let bcs = object
            .bcs
            .ok_or_else(|| anyhow!("object {} missing BCS payload", request.object_id))?;
        let is_shared = matches!(object.owner, GrpcOwner::Shared { .. });
        objects.insert(
            request.object_id.clone(),
            (bcs, object.type_string, object.version, is_shared),
        );
    }

    Ok(MainnetHydrationResult {
        provider,
        packages,
        objects,
    })
}

/// Create a local `SimulationEnvironment` from already-hydrated state.
pub fn build_environment_from_hydrated_state(
    hydration: &MainnetHydrationResult,
    plan: &EnvironmentBuildPlan,
) -> Result<EnvironmentBuildResult> {
    let mut env = SimulationEnvironment::new()?;
    env.set_sender(plan.sender);

    let registration_plan = build_package_registration_plan(&hydration.packages);
    let package_registration =
        register_packages_with_linkage_plan(&mut env, &hydration.packages, &registration_plan);

    let objects_loaded =
        load_fetched_objects_into_env(&mut env, &hydration.objects, plan.fail_on_object_load)?;

    Ok(EnvironmentBuildResult {
        env,
        package_registration,
        objects_loaded,
    })
}

/// Apply optional runtime support after environment build:
/// - dynamic-field wrapper preloading under selected parents
/// - on-demand gRPC object/child fetcher wiring
pub fn finalize_environment_runtime_support(
    hydration: &MainnetHydrationResult,
    env: &mut SimulationEnvironment,
    plan: &EnvironmentFinalizePlan,
) -> Result<EnvironmentFinalizeResult> {
    let mut dynamic_fields_loaded = 0usize;
    let endpoint = hydration.provider.grpc_endpoint().to_string();

    if !plan.dynamic_field_parents.is_empty() && plan.dynamic_field_limit > 0 {
        dynamic_fields_loaded = run_with_runtime(|rt| {
            let parent_refs: Vec<&str> = plan
                .dynamic_field_parents
                .iter()
                .map(String::as_str)
                .collect();
            let wrappers = preload_dynamic_field_objects(
                rt,
                hydration.provider.graphql(),
                hydration.provider.grpc(),
                &parent_refs,
                plan.dynamic_field_limit,
            );
            load_fetched_objects_into_env(env, &wrappers, false)
        })?;
    }

    if plan.configure_fetchers {
        run_with_runtime(|rt| {
            rt.block_on(configure_environment_mainnet_fetchers(
                env,
                &endpoint,
                HashMap::new(),
                None,
                true,
            ))?;
            Ok(())
        })?;
    }

    Ok(EnvironmentFinalizeResult {
        dynamic_fields_loaded,
        fetchers_configured: plan.configure_fetchers,
    })
}

fn run_with_runtime<T>(operation: impl FnOnce(&tokio::runtime::Runtime) -> Result<T>) -> Result<T> {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| {
            let runtime = tokio::runtime::Runtime::new()?;
            operation(&runtime)
        })
    } else {
        let runtime = tokio::runtime::Runtime::new()?;
        operation(&runtime)
    }
}

/// Convenience helper: hydrate state from mainnet and build a local environment.
pub async fn hydrate_and_build_mainnet_environment(
    hydration_plan: &MainnetHydrationPlan,
    build_plan: &EnvironmentBuildPlan,
) -> Result<HydratedEnvironmentResult> {
    let hydration = hydrate_mainnet_state(hydration_plan).await?;
    let build = build_environment_from_hydrated_state(&hydration, build_plan)?;
    Ok(HydratedEnvironmentResult { hydration, build })
}

/// Convenience helper: hydrate, build, then finalize runtime support.
pub async fn hydrate_build_and_finalize_mainnet_environment(
    hydration_plan: &MainnetHydrationPlan,
    build_plan: &EnvironmentBuildPlan,
    finalize_plan: &EnvironmentFinalizePlan,
) -> Result<FinalizedHydratedEnvironmentResult> {
    let hydration = hydrate_mainnet_state(hydration_plan).await?;
    let mut build = build_environment_from_hydrated_state(&hydration, build_plan)?;
    let finalize = finalize_environment_runtime_support(&hydration, &mut build.env, finalize_plan)?;
    Ok(FinalizedHydratedEnvironmentResult {
        hydration,
        build,
        finalize,
    })
}
