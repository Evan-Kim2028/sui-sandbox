//! Report types for fuzz testing results.

use serde::{Deserialize, Serialize};

use super::classifier::ClassifiedFunction;

/// Complete report from a fuzz run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzReport {
    /// Target function (e.g., "0x2::math::sqrt_u128").
    pub target: String,
    /// Total iterations requested.
    pub total_iterations: u64,
    /// Iterations actually completed (may be less if fail_fast triggered).
    pub completed_iterations: u64,
    /// Random seed used.
    pub seed: u64,
    /// Elapsed time in milliseconds.
    pub elapsed_ms: u64,
    /// Parameter classification.
    pub classification: ClassifiedFunction,
    /// Outcome summary.
    pub outcomes: FuzzOutcomeSummary,
    /// Gas usage profile.
    pub gas_profile: GasProfile,
    /// Interesting cases (first occurrence of each distinct abort/error).
    pub interesting_cases: Vec<InterestingCase>,
}

/// Summary of fuzz outcomes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzOutcomeSummary {
    /// Number of successful executions.
    pub successes: u64,
    /// Number of gas exhaustion events.
    pub gas_exhaustions: u64,
    /// Abort info grouped by abort code.
    pub aborts: Vec<AbortInfo>,
    /// Error info grouped by error message.
    pub errors: Vec<ErrorInfo>,
}

/// Information about a specific abort code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbortInfo {
    /// The abort code.
    pub code: u64,
    /// Module location where the abort occurred (if available).
    pub location: Option<String>,
    /// Number of times this abort was triggered.
    pub count: u64,
    /// Human-readable representation of the first input that triggered this abort.
    pub sample_inputs: Vec<String>,
    /// BCS-encoded inputs (hex) for reproducibility.
    pub sample_inputs_bcs: Vec<String>,
}

/// Information about a specific error type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// Error message (may be truncated for grouping).
    pub message: String,
    /// Number of times this error occurred.
    pub count: u64,
}

/// Gas usage profile across all iterations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasProfile {
    pub min: u64,
    pub max: u64,
    pub avg: u64,
    pub p50: u64,
    pub p99: u64,
    /// The input that used the most gas (human-readable).
    pub max_input: Vec<String>,
}

impl GasProfile {
    /// Compute gas profile from a list of gas values.
    pub fn from_values(gas_values: &mut [u64], max_input: Vec<String>) -> Self {
        if gas_values.is_empty() {
            return Self {
                min: 0,
                max: 0,
                avg: 0,
                p50: 0,
                p99: 0,
                max_input,
            };
        }

        gas_values.sort_unstable();
        let len = gas_values.len();
        let sum: u64 = gas_values.iter().sum();

        Self {
            min: gas_values[0],
            max: gas_values[len - 1],
            avg: sum / len as u64,
            p50: gas_values[len / 2],
            p99: gas_values[(len as f64 * 0.99) as usize],
            max_input,
        }
    }
}

/// A single interesting case discovered during fuzzing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestingCase {
    /// Which iteration this occurred on.
    pub iteration: u64,
    /// The outcome.
    pub outcome: Outcome,
    /// Human-readable inputs.
    pub inputs_human: Vec<String>,
    /// BCS-encoded inputs (hex) for reproducibility.
    pub inputs_bcs_hex: Vec<String>,
    /// Gas used for this execution.
    pub gas_used: u64,
}

/// Outcome of a single fuzz execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Outcome {
    Success,
    Abort { code: u64, location: Option<String> },
    Error { message: String },
    GasExhaustion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_profile_from_values() {
        let mut values = vec![100, 200, 300, 400, 500];
        let profile = GasProfile::from_values(&mut values, vec!["test".into()]);
        assert_eq!(profile.min, 100);
        assert_eq!(profile.max, 500);
        assert_eq!(profile.avg, 300);
        assert_eq!(profile.p50, 300);
    }

    #[test]
    fn test_gas_profile_empty() {
        let mut values = vec![];
        let profile = GasProfile::from_values(&mut values, vec![]);
        assert_eq!(profile.min, 0);
        assert_eq!(profile.max, 0);
    }

    #[test]
    fn test_outcome_serialization() {
        let outcome = Outcome::Abort {
            code: 42,
            location: Some("0x2::math::sqrt".into()),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("\"type\":\"Abort\""));
        assert!(json.contains("\"code\":42"));
    }
}
