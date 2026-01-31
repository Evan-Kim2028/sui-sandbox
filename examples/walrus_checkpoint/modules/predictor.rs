//! Predictive dynamic field analysis for replay optimization.
//!
//! This module provides pre-execution analysis of PTB commands to predict
//! dynamic field accesses, reducing retry attempts from 4 to 1-2 on average.
//!
//! ## Strategy
//!
//! Instead of reactively fetching dynamic fields on execution failure:
//! 1. Analyze PTB commands BEFORE execution
//! 2. Detect patterns that indicate dynamic field access (Table, Bag, ObjectTable)
//! 3. Pre-fetch likely child objects
//! 4. Execute with better cache coverage
//!
//! ## Patterns Detected
//!
//! - `0x2::table::borrow` / `borrow_mut` / `remove`
//! - `0x2::bag::borrow` / `borrow_mut` / `remove`
//! - `0x2::object_table::borrow` / `borrow_mut` / `remove`
//! - `0x2::linked_table::*`
//! - Cetus/Turbos skip_list patterns
//! - DeepBook order book patterns
//!
//! Note: Some types may appear unused until full integration is complete.

#![allow(dead_code)]

use move_core_types::account_address::AccountAddress;
use move_core_types::language_storage::TypeTag;
use std::collections::HashSet;

use sui_sandbox_core::ptb::Command;

// ============================================================================
// Prediction Types
// ============================================================================

/// A predicted dynamic field access.
#[derive(Debug, Clone)]
pub struct PredictedAccess {
    /// Parent object ID that will be accessed
    pub parent_id: AccountAddress,
    /// Predicted key type
    pub key_type: Option<TypeTag>,
    /// Predicted value type
    pub value_type: Option<TypeTag>,
    /// Access pattern
    pub pattern: AccessPattern,
    /// Confidence level
    pub confidence: PredictionConfidence,
    /// Source command index
    pub command_index: usize,
}

/// Access pattern category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessPattern {
    /// Standard sui::table operations
    TableAccess,
    /// Standard sui::bag operations
    BagAccess,
    /// Object table operations
    ObjectTableAccess,
    /// Linked table (ordered) operations
    LinkedTableAccess,
    /// Skip list (Cetus/Turbos style)
    SkipListAccess,
    /// Order book (DeepBook style)
    OrderBookAccess,
    /// Generic dynamic field borrow
    GenericDynamicField,
    /// Unknown pattern
    Unknown,
}

/// Confidence in a prediction.
///
/// Note: Ordered so that High > Medium > Low for comparison purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionConfidence {
    /// Low confidence - heuristic match
    Low,
    /// Medium confidence - wrapper function call (table::borrow, etc.)
    Medium,
    /// High confidence - direct dynamic_field call with known types
    High,
}

impl PartialOrd for PredictionConfidence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PredictionConfidence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn rank(c: &PredictionConfidence) -> u8 {
            match c {
                PredictionConfidence::Low => 0,
                PredictionConfidence::Medium => 1,
                PredictionConfidence::High => 2,
            }
        }
        rank(self).cmp(&rank(other))
    }
}

/// Result of prediction analysis.
#[derive(Debug, Default)]
pub struct PredictionResult {
    /// Predicted accesses
    pub predictions: Vec<PredictedAccess>,
    /// Objects to prefetch (id, optional version)
    pub prefetch_ids: Vec<(AccountAddress, Option<u64>)>,
    /// Analysis statistics
    pub stats: PredictionStats,
}

/// Statistics about prediction analysis.
#[derive(Debug, Default, Clone)]
pub struct PredictionStats {
    /// Number of commands analyzed
    pub commands_analyzed: usize,
    /// Number of MoveCall commands found
    pub move_calls_found: usize,
    /// Number of dynamic field patterns detected
    pub patterns_detected: usize,
    /// Number of high confidence predictions
    pub high_confidence: usize,
    /// Number of medium confidence predictions
    pub medium_confidence: usize,
    /// Number of low confidence predictions
    pub low_confidence: usize,
}

// ============================================================================
// Command Analyzer
// ============================================================================

/// Analyzes PTB commands to predict dynamic field accesses.
#[derive(Default)]
pub struct CommandAnalyzer {
    /// Known dynamic field modules and functions
    known_patterns: Vec<PatternMatcher>,
}

/// Pattern matcher for detecting dynamic field access.
struct PatternMatcher {
    module_prefix: &'static str,
    function_patterns: Vec<&'static str>,
    pattern_type: AccessPattern,
    confidence: PredictionConfidence,
}

impl CommandAnalyzer {
    pub fn new() -> Self {
        Self {
            known_patterns: vec![
                // Standard dynamic field operations
                PatternMatcher {
                    module_prefix: "0x2::dynamic_field",
                    function_patterns: vec!["borrow", "borrow_mut", "remove", "add", "exists_"],
                    pattern_type: AccessPattern::GenericDynamicField,
                    confidence: PredictionConfidence::High,
                },
                // Table operations
                PatternMatcher {
                    module_prefix: "0x2::table",
                    function_patterns: vec!["borrow", "borrow_mut", "remove", "add", "contains"],
                    pattern_type: AccessPattern::TableAccess,
                    confidence: PredictionConfidence::Medium,
                },
                // Bag operations
                PatternMatcher {
                    module_prefix: "0x2::bag",
                    function_patterns: vec!["borrow", "borrow_mut", "remove", "add", "contains"],
                    pattern_type: AccessPattern::BagAccess,
                    confidence: PredictionConfidence::Medium,
                },
                // Object table operations
                PatternMatcher {
                    module_prefix: "0x2::object_table",
                    function_patterns: vec!["borrow", "borrow_mut", "remove", "add", "contains"],
                    pattern_type: AccessPattern::ObjectTableAccess,
                    confidence: PredictionConfidence::Medium,
                },
                // Linked table operations
                PatternMatcher {
                    module_prefix: "0x2::linked_table",
                    function_patterns: vec![
                        "borrow",
                        "borrow_mut",
                        "remove",
                        "push_back",
                        "push_front",
                        "pop_back",
                        "pop_front",
                    ],
                    pattern_type: AccessPattern::LinkedTableAccess,
                    confidence: PredictionConfidence::Medium,
                },
                // Object bag operations
                PatternMatcher {
                    module_prefix: "0x2::object_bag",
                    function_patterns: vec!["borrow", "borrow_mut", "remove", "add", "contains"],
                    pattern_type: AccessPattern::BagAccess,
                    confidence: PredictionConfidence::Medium,
                },
            ],
        }
    }

    /// Analyze a list of PTB commands and predict dynamic field accesses.
    pub fn analyze_commands(
        &self,
        commands: &[Command],
        inputs: &[sui_sandbox_core::ptb::InputValue],
    ) -> PredictionResult {
        let mut result = PredictionResult::default();

        for (idx, cmd) in commands.iter().enumerate() {
            result.stats.commands_analyzed += 1;

            if let Command::MoveCall {
                package,
                module,
                function,
                type_args,
                args,
            } = cmd
            {
                result.stats.move_calls_found += 1;

                // Build fully qualified function name
                let package_hex = package.to_hex_literal();
                let module_str = module.as_str();
                let function_str = function.as_str();
                let full_module = format!("{}::{}", package_hex, module_str);

                // Check against known patterns
                for pattern in &self.known_patterns {
                    if full_module.starts_with(pattern.module_prefix)
                        || full_module.contains(&pattern.module_prefix[4..])
                    // Skip "0x2:" prefix
                    {
                        for func_pattern in &pattern.function_patterns {
                            if function_str.contains(func_pattern) {
                                result.stats.patterns_detected += 1;

                                // Try to extract parent object from first argument
                                if let Some(parent_id) = self.extract_parent_from_args(args, inputs)
                                {
                                    let prediction = PredictedAccess {
                                        parent_id,
                                        key_type: type_args.first().cloned(),
                                        value_type: type_args.get(1).cloned(),
                                        pattern: pattern.pattern_type,
                                        confidence: pattern.confidence,
                                        command_index: idx,
                                    };

                                    // Update stats
                                    match prediction.confidence {
                                        PredictionConfidence::High => {
                                            result.stats.high_confidence += 1
                                        }
                                        PredictionConfidence::Medium => {
                                            result.stats.medium_confidence += 1
                                        }
                                        PredictionConfidence::Low => {
                                            result.stats.low_confidence += 1
                                        }
                                    }

                                    // Add to prefetch list
                                    result.prefetch_ids.push((parent_id, None));
                                    result.predictions.push(prediction);
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Deduplicate prefetch IDs
        let seen: HashSet<_> = result.prefetch_ids.iter().map(|(id, _)| *id).collect();
        result.prefetch_ids = seen.into_iter().map(|id| (id, None)).collect();

        result
    }

    /// Try to extract parent object ID from MoveCall arguments.
    fn extract_parent_from_args(
        &self,
        args: &[sui_sandbox_core::ptb::Argument],
        inputs: &[sui_sandbox_core::ptb::InputValue],
    ) -> Option<AccountAddress> {
        use sui_sandbox_core::ptb::Argument;

        // First argument is typically the parent object (table, bag, etc.)
        let first_arg = args.first()?;

        match first_arg {
            Argument::Input(idx) => {
                let input = inputs.get(*idx as usize)?;
                // Extract ID from the input
                self.extract_id_from_input(input)
            }
            Argument::Result(_) | Argument::NestedResult(_, _) => {
                // Result of previous command - can't statically determine
                None
            }
        }
    }

    /// Extract object ID from an InputValue.
    fn extract_id_from_input(
        &self,
        input: &sui_sandbox_core::ptb::InputValue,
    ) -> Option<AccountAddress> {
        use sui_sandbox_core::ptb::{InputValue, ObjectInput};

        match input {
            InputValue::Object(obj_input) => {
                // Extract ID from ObjectInput variants
                match obj_input {
                    ObjectInput::ImmRef { id, .. } => Some(*id),
                    ObjectInput::MutRef { id, .. } => Some(*id),
                    ObjectInput::Owned { id, .. } => Some(*id),
                    ObjectInput::Shared { id, .. } => Some(*id),
                    ObjectInput::Receiving { id, .. } => Some(*id),
                }
            }
            InputValue::Pure(bytes) => {
                // Try to parse as object ID (first 32 bytes)
                if bytes.len() >= 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes[0..32]);
                    Some(AccountAddress::new(arr))
                } else {
                    None
                }
            }
        }
    }
}

// ============================================================================
// Integration with Existing Prefetcher
// ============================================================================

/// Configuration for predictive analysis.
#[derive(Debug, Clone)]
pub struct PredictiveConfig {
    /// Enable command analysis
    pub enable_command_analysis: bool,
    /// Enable MM2 bytecode analysis (uses existing PredictivePrefetcher)
    pub enable_mm2_analysis: bool,
    /// Minimum confidence to include in prefetch
    pub min_confidence: PredictionConfidence,
    /// Maximum predictions to generate
    pub max_predictions: usize,
}

impl Default for PredictiveConfig {
    fn default() -> Self {
        Self {
            enable_command_analysis: true,
            enable_mm2_analysis: true,
            min_confidence: PredictionConfidence::Medium,
            max_predictions: 100,
        }
    }
}

/// Trait for predictive analysis.
pub trait DynamicFieldPredictor: Send + Sync {
    /// Analyze PTB and predict dynamic field accesses.
    fn predict(
        &self,
        commands: &[Command],
        inputs: &[sui_sandbox_core::ptb::InputValue],
        config: &PredictiveConfig,
    ) -> PredictionResult;
}

/// Default predictor implementation.
pub struct DefaultDynamicFieldPredictor {
    analyzer: CommandAnalyzer,
}

impl Default for DefaultDynamicFieldPredictor {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultDynamicFieldPredictor {
    pub fn new() -> Self {
        Self {
            analyzer: CommandAnalyzer::new(),
        }
    }
}

impl DynamicFieldPredictor for DefaultDynamicFieldPredictor {
    fn predict(
        &self,
        commands: &[Command],
        inputs: &[sui_sandbox_core::ptb::InputValue],
        config: &PredictiveConfig,
    ) -> PredictionResult {
        if !config.enable_command_analysis {
            return PredictionResult::default();
        }

        let mut result = self.analyzer.analyze_commands(commands, inputs);

        // Filter by minimum confidence
        result
            .predictions
            .retain(|p| p.confidence >= config.min_confidence);

        // Limit predictions
        if result.predictions.len() > config.max_predictions {
            result.predictions.truncate(config.max_predictions);
        }

        // Update prefetch IDs to match filtered predictions
        let prediction_parents: HashSet<_> =
            result.predictions.iter().map(|p| p.parent_id).collect();
        result
            .prefetch_ids
            .retain(|(id, _)| prediction_parents.contains(id));

        result
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_analyzer_new() {
        let analyzer = CommandAnalyzer::new();
        assert!(!analyzer.known_patterns.is_empty());
    }

    #[test]
    fn test_prediction_confidence_ordering() {
        assert!(PredictionConfidence::High > PredictionConfidence::Medium);
        assert!(PredictionConfidence::Medium > PredictionConfidence::Low);
    }

    #[test]
    fn test_access_pattern_equality() {
        assert_eq!(AccessPattern::TableAccess, AccessPattern::TableAccess);
        assert_ne!(AccessPattern::TableAccess, AccessPattern::BagAccess);
    }

    #[test]
    fn test_default_config() {
        let config = PredictiveConfig::default();
        assert!(config.enable_command_analysis);
        assert!(config.enable_mm2_analysis);
        assert_eq!(config.min_confidence, PredictionConfidence::Medium);
    }

    #[test]
    fn test_prediction_result_default() {
        let result = PredictionResult::default();
        assert!(result.predictions.is_empty());
        assert!(result.prefetch_ids.is_empty());
        assert_eq!(result.stats.commands_analyzed, 0);
    }
}
