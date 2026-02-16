use serde::Deserialize;
use std::env;
use std::path::PathBuf;

const MANIFEST_JSON: &str = include_str!("data/deepbook_margin_state/scenario_manifest.json");
pub const DEFAULT_SCENARIO: &str = "position_a_snapshot";
pub const SCENARIO_ENV: &str = "DEEPBOOK_SCENARIO";

#[derive(Debug, Clone, Deserialize)]
pub struct DeepbookScenario {
    pub id: String,
    pub kind: String,
    pub description: Option<String>,
    pub versions_file: Option<String>,
    pub request_file: Option<String>,
    pub series_file: Option<String>,
    pub schema_file: Option<String>,
    pub object_json_file: Option<String>,
    pub target_margin_manager: Option<String>,
    pub package_roots: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ScenarioManifest {
    default_scenario: String,
    scenarios: Vec<DeepbookScenario>,
}

fn load_manifest() -> Result<ScenarioManifest, String> {
    serde_json::from_str(MANIFEST_JSON)
        .map_err(|err| format!("Failed to parse deepbook scenario manifest: {err}"))
}

fn format_scenario_ids(scenarios: &[DeepbookScenario]) -> String {
    scenarios
        .iter()
        .map(|scenario| scenario.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn scenario_ids_for_error(scenarios: &[DeepbookScenario]) -> String {
    if scenarios.is_empty() {
        "none".to_string()
    } else {
        format_scenario_ids(scenarios)
    }
}

pub fn scenario_data_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

pub fn default_scenario() -> String {
    load_manifest()
        .ok()
        .map(|manifest| manifest.default_scenario)
        .unwrap_or_else(|| DEFAULT_SCENARIO.to_string())
}

pub fn resolve_scenario(requested: Option<&str>) -> Result<DeepbookScenario, String> {
    let manifest = load_manifest()?;
    let requested = requested
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env::var(SCENARIO_ENV).ok())
        .unwrap_or_else(default_scenario)
        .to_ascii_lowercase();

    let scenarios = manifest.scenarios;
    let found = scenarios
        .iter()
        .find(|scenario| scenario.id.eq_ignore_ascii_case(&requested))
        .cloned()
        .ok_or_else(|| {
            format!(
                "Unknown deepbook scenario '{requested}'. Available: {}",
                scenario_ids_for_error(&scenarios)
            )
        })?;

    Ok(found)
}

pub fn require_kind<'a>(
    scenario: &'a DeepbookScenario,
    expected_kind: &str,
) -> Result<&'a DeepbookScenario, String> {
    if scenario.kind.eq_ignore_ascii_case(expected_kind) {
        Ok(scenario)
    } else {
        Err(format!(
            "Scenario '{}' is '{}' (expected '{}'). Available scenarios: {}",
            scenario.id,
            scenario.kind,
            expected_kind,
            scenario_ids_for_error(
                &load_manifest()
                    .map(|manifest| manifest.scenarios)
                    .unwrap_or_default()
            )
        ))
    }
}
