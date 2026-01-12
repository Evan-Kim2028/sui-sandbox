"""
Tests for smi_bench.inhabit.evaluator module.

These tests verify the evaluator's ability to:
1. Parse MM2 mapping entries correctly
2. Extract inhabitation metrics from MM2 data
3. Extract execution traces with abort info
4. Compute scoring criteria accurately
5. Generate complete evaluation results
"""

import pytest

from smi_bench.inhabit.evaluation import (
    ErrorCode,
    Phase,
    ScoringCriteria,
    UninhabitedReason,
)
from smi_bench.inhabit.evaluator import (
    compute_scoring_criteria,
    evaluate_build_failure,
    evaluate_from_validation_report,
    evaluate_mm2_failure,
    extract_execution_trace,
    extract_inhabitation_metrics,
    parse_mm2_entry,
)

# =============================================================================
# Test Data Fixtures
# =============================================================================


@pytest.fixture
def sample_mm2_entry_tier_b_hit():
    """A tier_b_hit entry that executed successfully."""
    return {
        "status": "tier_b_hit",
        "target_function": "transfer",
        "target_module": "my_token",
        # Use a non-framework package address (not 0x1, 0x2, 0x3)
        "target_package": "0xabcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab",
        "tier_a_details": {
            "bcs_roundtrip_verified": True,
            "has_object_params": True,
            "resolved_params": ["object", "address"],
        },
        "tier_b_details": {
            "execution_success": True,
            "target_modules_accessed": ["0xabcd::my_token", "0xabcd::balance"],
        },
    }


@pytest.fixture
def sample_mm2_entry_tier_a_hit():
    """A tier_a_hit entry (resolved but not executed)."""
    return {
        "status": "tier_a_hit",
        "target_function": "new",
        "target_module": "cell",
        "target_package": "0xc35ee7fee75782806890cf8ed8536b52b4ba0ace0fb46b944f1155cc5945baa3",
        "tier_a_details": {
            "bcs_roundtrip_verified": True,
            "has_object_params": False,
            "resolved_params": ["type_param[0]=U64"],
        },
        "tier_b_details": {
            "execution_success": False,
            "error": "execution failed: VMError { major_status: ABORTED, sub_status: Some(0) }",
            "target_modules_accessed": ["0xc35::cell"],
        },
    }


@pytest.fixture
def sample_mm2_entry_miss():
    """A miss entry (could not resolve)."""
    return {
        "status": "miss",
        "target_function": "flash_stake_conclude",
        "target_module": "liquid_staking",
        "target_package": "0xc35ee7fee75782806890cf8ed8536b52b4ba0ace0fb46b944f1155cc5945baa3",
        "failure_reason": "no default value generator for layout",
        "failure_stage": "A3",
    }


@pytest.fixture
def sample_mm2_data(sample_mm2_entry_tier_b_hit, sample_mm2_entry_tier_a_hit, sample_mm2_entry_miss):
    """Complete MM2 mapping data with multiple entries."""
    helper_entry = {
        "status": "tier_b_hit",
        "target_function": "noop",
        "target_module": "helper",
        "target_package": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "tier_a_details": {"bcs_roundtrip_verified": True, "resolved_params": []},
        "tier_b_details": {"execution_success": True, "target_modules_accessed": ["0x0::helper"]},
    }
    return {
        "schema_version": 1,
        "kind": "mm2_mapping",
        "accepted": [helper_entry, sample_mm2_entry_tier_b_hit, sample_mm2_entry_tier_a_hit],
        "rejected": [sample_mm2_entry_miss],
    }


@pytest.fixture
def sample_target_interface():
    """Sample target interface with function counts."""
    return {
        "module_names": ["cell", "fees"],
        "modules": {
            "cell": {
                "functions": {
                    "new": {"is_entry": False, "visibility": "public"},
                    "get": {"is_entry": False, "visibility": "public"},
                    "set": {"is_entry": False, "visibility": "public"},
                    "destroy": {"is_entry": False, "visibility": "public"},
                },
            },
            "fees": {
                "functions": {
                    "calculate": {"is_entry": True, "visibility": "public"},
                },
            },
        },
    }


# =============================================================================
# Test parse_mm2_entry
# =============================================================================


class TestParseMm2Entry:
    def test_parse_tier_b_hit(self, sample_mm2_entry_tier_b_hit):
        parsed = parse_mm2_entry(sample_mm2_entry_tier_b_hit)
        assert parsed["target"] == "my_token::transfer"
        assert parsed["target_package"] == "0xabcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab"
        assert parsed["status"] == "tier_b_hit"
        assert parsed["tier_a"] is not None
        assert parsed["tier_b"] is not None
        assert parsed["tier_b"]["execution_success"] is True

    def test_parse_tier_a_hit(self, sample_mm2_entry_tier_a_hit):
        parsed = parse_mm2_entry(sample_mm2_entry_tier_a_hit)
        assert parsed["target"] == "cell::new"
        assert parsed["status"] == "tier_a_hit"
        assert parsed["tier_b"]["execution_success"] is False

    def test_parse_miss(self, sample_mm2_entry_miss):
        parsed = parse_mm2_entry(sample_mm2_entry_miss)
        assert parsed["target"] == "liquid_staking::flash_stake_conclude"
        assert parsed["status"] == "miss"
        assert parsed["failure_reason"] == "no default value generator for layout"
        assert parsed["failure_stage"] == "A3"


# =============================================================================
# Test extract_inhabitation_metrics
# =============================================================================


class TestExtractInhabitationMetrics:
    def test_basic_metrics(self, sample_mm2_data):
        metrics = extract_inhabitation_metrics(sample_mm2_data)
        # Should have 2 inhabited types (excluding helper)
        assert metrics.target_types_inhabited == 2
        assert "my_token::transfer" in metrics.inhabited_types
        assert "cell::new" in metrics.inhabited_types

    def test_with_target_interface(self, sample_mm2_data, sample_target_interface):
        metrics = extract_inhabitation_metrics(sample_mm2_data, target_interface=sample_target_interface)
        # Interface has 5 functions total
        assert metrics.target_types_total == 5
        # One entry function
        assert metrics.target_entry_functions == 1

    def test_uninhabited_types(self, sample_mm2_data):
        metrics = extract_inhabitation_metrics(sample_mm2_data)
        assert len(metrics.uninhabited_types) == 1
        uninhabited = metrics.uninhabited_types[0]
        assert uninhabited.type_name == "liquid_staking::flash_stake_conclude"
        assert uninhabited.reason == UninhabitedReason.NO_CONSTRUCTOR

    def test_filters_helper_package(self, sample_mm2_data):
        metrics = extract_inhabitation_metrics(sample_mm2_data)
        # helper::noop should NOT be in inhabited types
        assert "helper::noop" not in metrics.inhabited_types

    def test_inhabitation_rate(self, sample_mm2_data, sample_target_interface):
        metrics = extract_inhabitation_metrics(sample_mm2_data, target_interface=sample_target_interface)
        # 2 inhabited out of 5 total = 0.4
        assert metrics.inhabitation_rate() == pytest.approx(0.4)


# =============================================================================
# Test extract_execution_trace
# =============================================================================


class TestExtractExecutionTrace:
    def test_basic_trace(self, sample_mm2_data):
        trace = extract_execution_trace(sample_mm2_data)
        assert trace.execution_attempted is True
        assert len(trace.modules_loaded) > 0

    def test_function_calls(self, sample_mm2_data):
        trace = extract_execution_trace(sample_mm2_data)
        # Should have calls from entries with tier_b details
        assert len(trace.functions_called) >= 1

    def test_abort_info_extracted(self, sample_mm2_entry_tier_a_hit):
        mm2_data = {
            "accepted": [sample_mm2_entry_tier_a_hit],
            "rejected": [],
        }
        trace = extract_execution_trace(mm2_data)
        # Should have abort info from the failed execution
        assert trace.abort_info is not None
        assert trace.abort_info.abort_code == 0

    def test_empty_mm2_data(self):
        trace = extract_execution_trace({"accepted": [], "rejected": []})
        assert trace.execution_attempted is False
        assert len(trace.functions_called) == 0


# =============================================================================
# Test compute_scoring_criteria
# =============================================================================


class TestComputeScoringCriteria:
    def test_build_failed(self):
        criteria = compute_scoring_criteria(build_succeeded=False, mm2_data=None)
        assert criteria.compiles is False
        assert criteria.score() == 0.0
        assert criteria.phase_reached() == Phase.BUILD

    def test_compiles_only(self):
        # Build succeeded but no MM2 data
        criteria = compute_scoring_criteria(build_succeeded=True, mm2_data=None)
        assert criteria.compiles is True
        assert criteria.imports_target is False
        assert criteria.score() == 0.25

    def test_full_success(self, sample_mm2_data):
        criteria = compute_scoring_criteria(build_succeeded=True, mm2_data=sample_mm2_data)
        assert criteria.compiles is True
        assert criteria.imports_target is True
        # tier_b_hit with execution_success = True means executes_cleanly
        assert criteria.executes_cleanly is True
        assert criteria.score() == 1.0

    def test_tier_a_only(self, sample_mm2_entry_tier_a_hit):
        mm2_data = {
            "accepted": [sample_mm2_entry_tier_a_hit],
            "rejected": [],
        }
        criteria = compute_scoring_criteria(build_succeeded=True, mm2_data=mm2_data)
        assert criteria.compiles is True
        assert criteria.imports_target is True
        assert criteria.creates_target_type is True  # resolved_params present
        assert criteria.executes_cleanly is False  # execution_success = False
        assert criteria.score() == 0.75


# =============================================================================
# Test evaluate_build_failure
# =============================================================================


class TestEvaluateBuildFailure:
    def test_address_undefined_error(self):
        error = "error[E03001]: address with no value\n  module helper_pkg::helper {"
        result = evaluate_build_failure(error)
        assert result.ok is False
        assert result.failure is not None
        assert result.failure.code == ErrorCode.MODULE_ADDRESS_UNDEFINED

    def test_invalid_entry_signature(self):
        error = "error[Sui E02002]: invalid 'entry' function signature"
        result = evaluate_build_failure(error)
        assert result.ok is False
        assert result.failure.code == ErrorCode.INVALID_ENTRY_SIGNATURE

    def test_unknown_error(self):
        error = "some random compiler error"
        result = evaluate_build_failure(error)
        assert result.ok is False
        # Should default to TYPE_SYNTAX_ERROR
        assert result.failure.code == ErrorCode.TYPE_SYNTAX_ERROR


# =============================================================================
# Test evaluate_mm2_failure
# =============================================================================


class TestEvaluateMm2Failure:
    def test_no_constructor_failure(self):
        result = evaluate_mm2_failure(
            reason="no default value generator for layout",
            stage="A3",
        )
        assert result.ok is False
        assert result.failure.code == ErrorCode.NO_CONSTRUCTOR

    def test_execution_aborted_failure(self):
        result = evaluate_mm2_failure(
            reason="execution aborted with code 0",
            stage="B2",
        )
        assert result.ok is False
        assert result.failure.code == ErrorCode.TARGET_ABORTED


# =============================================================================
# Test evaluate_from_validation_report
# =============================================================================


class TestEvaluateFromValidationReport:
    def test_successful_report(self):
        report = {"ok": True, "errors": []}
        result = evaluate_from_validation_report(report)
        assert result.ok is True
        assert result.score == 1.0

    def test_build_failed_report(self):
        report = {"ok": False, "errors": ["build failed", "error[E03001]"]}
        result = evaluate_from_validation_report(report)
        assert result.ok is False
        assert result.failure is not None

    def test_no_tier_b_hit_report(self):
        report = {"ok": False, "errors": ["no tier_b_hit with target package"]}
        result = evaluate_from_validation_report(report)
        assert result.ok is False
        assert result.criteria.compiles is True
        assert result.criteria.imports_target is True


# =============================================================================
# Integration Tests
# =============================================================================


class TestIntegration:
    def test_full_pipeline(self, sample_mm2_data, sample_target_interface):
        """Test the full evaluation pipeline with realistic data."""
        # Extract metrics
        metrics = extract_inhabitation_metrics(
            mm2_data=sample_mm2_data,
            target_interface=sample_target_interface,
        )

        # Extract trace
        trace = extract_execution_trace(sample_mm2_data)

        # Compute criteria
        criteria = compute_scoring_criteria(
            build_succeeded=True,
            mm2_data=sample_mm2_data,
        )

        # Verify coherent results
        assert metrics.target_types_inhabited > 0
        assert trace.execution_attempted is True
        assert criteria.compiles is True
        assert criteria.imports_target is True

        # Score should reflect the state
        assert criteria.score() > 0.0

    def test_scoring_consistency(self):
        """Verify ScoringCriteria.from_phase produces consistent results."""
        # from_phase maps phases to criteria that indicate reaching that phase
        # The mapping is: BUILD -> nothing, RESOLUTION -> compiles,
        # TYPECHECK/SYNTHESIS -> compiles+imports, EXECUTION -> +creates_type,
        # VALIDATION -> +executes_cleanly
        expected_scores = {
            Phase.BUILD: 0.0,
            Phase.RESOLUTION: 0.25,
            Phase.TYPECHECK: 0.5,
            Phase.SYNTHESIS: 0.5,
            Phase.EXECUTION: 0.75,
            Phase.VALIDATION: 1.0,
        }
        for phase in Phase:
            criteria = ScoringCriteria.from_phase(phase)
            assert criteria.score() == expected_scores[phase], (
                f"Phase {phase} should have score {expected_scores[phase]}"
            )
