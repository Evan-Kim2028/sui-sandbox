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
    build_package_registration_plan, create_mainnet_provider, load_fetched_objects_into_env,
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
