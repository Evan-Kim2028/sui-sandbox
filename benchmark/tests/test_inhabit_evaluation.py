"""Tests for the inhabit evaluation module."""

import json

import pytest

from smi_bench.inhabit.evaluation import (
    E_NOT_SUPPORTED,
    AbortCategory,
    AbortInfo,
    ErrorCode,
    ErrorSource,
    EvaluationResult,
    ExecutionTrace,
    Failure,
    FailureContext,
    InhabitationMetrics,
    Phase,
    ScoringCriteria,
    UninhabitedReason,
    UninhabitedType,
    is_unsupported_native_error,
    parse_build_error,
)

# =============================================================================
# Phase Tests
# =============================================================================


class TestPhase:
    def test_phase_values(self):
        assert Phase.BUILD.value == "build"
        assert Phase.RESOLUTION.value == "resolution"
        assert Phase.TYPECHECK.value == "typecheck"
        assert Phase.SYNTHESIS.value == "synthesis"
        assert Phase.EXECUTION.value == "execution"
        assert Phase.VALIDATION.value == "validation"

    def test_code_prefix(self):
        assert Phase.BUILD.code_prefix == 0
        assert Phase.RESOLUTION.code_prefix == 100
        assert Phase.TYPECHECK.code_prefix == 200
        assert Phase.SYNTHESIS.code_prefix == 300
        assert Phase.EXECUTION.code_prefix == 400
        assert Phase.VALIDATION.code_prefix == 500


# =============================================================================
# ErrorCode Tests
# =============================================================================


class TestErrorCode:
    def test_build_error_codes(self):
        assert ErrorCode.MODULE_ADDRESS_UNDEFINED.value == "E001"
        assert ErrorCode.INVALID_MANIFEST.value == "E002"
        assert ErrorCode.IMPORT_RESOLUTION_FAILED.value == "E003"
        assert ErrorCode.TYPE_SYNTAX_ERROR.value == "E004"
        assert ErrorCode.INVALID_ENTRY_SIGNATURE.value == "E005"
        assert ErrorCode.COMPILE_TIME_ABILITY_ERROR.value == "E006"

    def test_numeric_code(self):
        assert ErrorCode.MODULE_ADDRESS_UNDEFINED.numeric_code == 1
        assert ErrorCode.MODULE_NOT_FOUND.numeric_code == 101
        assert ErrorCode.TYPE_MISMATCH.numeric_code == 201
        assert ErrorCode.NO_CONSTRUCTOR.numeric_code == 301
        assert ErrorCode.VM_SETUP_FAILED.numeric_code == 401
        assert ErrorCode.NO_TARGET_MODULES_ACCESSED.numeric_code == 501

    def test_phase_mapping(self):
        # Build phase
        assert ErrorCode.MODULE_ADDRESS_UNDEFINED.phase == Phase.BUILD
        assert ErrorCode.INVALID_ENTRY_SIGNATURE.phase == Phase.BUILD

        # Resolution phase
        assert ErrorCode.MODULE_NOT_FOUND.phase == Phase.RESOLUTION
        assert ErrorCode.FUNCTION_NOT_FOUND.phase == Phase.RESOLUTION

        # TypeCheck phase
        assert ErrorCode.TYPE_MISMATCH.phase == Phase.TYPECHECK
        assert ErrorCode.RECURSIVE_TYPE.phase == Phase.TYPECHECK

        # Synthesis phase
        assert ErrorCode.NO_CONSTRUCTOR.phase == Phase.SYNTHESIS
        assert ErrorCode.CHAIN_TOO_DEEP.phase == Phase.SYNTHESIS

        # Execution phase
        assert ErrorCode.VM_SETUP_FAILED.phase == Phase.EXECUTION
        assert ErrorCode.TARGET_ABORTED.phase == Phase.EXECUTION

        # Validation phase
        assert ErrorCode.NO_TARGET_MODULES_ACCESSED.phase == Phase.VALIDATION

    def test_expected_limitations(self):
        assert ErrorCode.UNSUPPORTED_NATIVE.is_expected_limitation is True
        assert ErrorCode.CHAIN_TOO_DEEP.is_expected_limitation is True
        assert ErrorCode.UNSUPPORTED_CONSTRUCTOR_PARAM.is_expected_limitation is True
        assert ErrorCode.TYPE_MISMATCH.is_expected_limitation is False

    def test_default_error_source(self):
        # Build errors -> LLM error
        assert ErrorCode.TYPE_SYNTAX_ERROR.default_error_source == ErrorSource.LLM_ERROR

        # Infrastructure limitations
        assert ErrorCode.UNSUPPORTED_NATIVE.default_error_source == ErrorSource.INFRASTRUCTURE_LIMITATION
        assert ErrorCode.RECURSIVE_TYPE.default_error_source == ErrorSource.INFRASTRUCTURE_LIMITATION

        # Unknown/context-dependent
        assert ErrorCode.NO_CONSTRUCTOR.default_error_source == ErrorSource.UNKNOWN


# =============================================================================
# ErrorSource Tests
# =============================================================================


class TestErrorSource:
    def test_counts_against_llm(self):
        assert ErrorSource.LLM_ERROR.counts_against_llm is True
        assert ErrorSource.INFRASTRUCTURE_LIMITATION.counts_against_llm is False
        assert ErrorSource.TARGET_PACKAGE_LIMITATION.counts_against_llm is False
        assert ErrorSource.UNKNOWN.counts_against_llm is False


# =============================================================================
# Failure Tests
# =============================================================================


class TestFailure:
    def test_from_code(self):
        failure = Failure.from_code(ErrorCode.TYPE_MISMATCH, "expected u64, got bool")
        assert failure.phase == Phase.TYPECHECK
        assert failure.code == ErrorCode.TYPE_MISMATCH
        assert failure.message == "expected u64, got bool"
        assert failure.is_expected_limitation is False
        assert failure.error_source == ErrorSource.LLM_ERROR

    def test_with_context(self):
        ctx = FailureContext(
            module="0x1::test",
            function="do_thing",
            param_index=0,
        )
        failure = Failure.with_context(ErrorCode.TYPE_MISMATCH, "type error", ctx)
        assert failure.context is not None
        assert failure.context.module == "0x1::test"
        assert failure.context.param_index == 0

    def test_set_source(self):
        failure = Failure.from_code(ErrorCode.NO_CONSTRUCTOR, "no ctor")
        failure.set_source(ErrorSource.TARGET_PACKAGE_LIMITATION)
        assert failure.error_source == ErrorSource.TARGET_PACKAGE_LIMITATION
        assert failure.is_expected_limitation is True

    def test_to_dict(self):
        failure = Failure.from_code(ErrorCode.MODULE_NOT_FOUND, "module foo not found")
        d = failure.to_dict()
        assert d["phase"] == "resolution"
        assert d["code"] == "E101"
        assert d["message"] == "module foo not found"
        assert d["error_source"] == "llm_error"

    def test_serialization_roundtrip(self):
        failure = Failure.from_code(ErrorCode.TARGET_ABORTED, "abort at 0x1::test")
        json_str = json.dumps(failure.to_dict())
        assert "target_aborted" not in json_str  # Uses E403, not enum name


# =============================================================================
# ScoringCriteria Tests
# =============================================================================


class TestScoringCriteria:
    def test_default_score(self):
        criteria = ScoringCriteria()
        assert criteria.score() == 0.0

    def test_full_score(self):
        criteria = ScoringCriteria(
            compiles=True,
            imports_target=True,
            creates_target_type=True,
            executes_cleanly=True,
        )
        assert criteria.score() == 1.0

    def test_partial_scores(self):
        assert ScoringCriteria(compiles=True).score() == 0.25
        assert ScoringCriteria(compiles=True, imports_target=True).score() == 0.5
        assert ScoringCriteria(compiles=True, imports_target=True, creates_target_type=True).score() == 0.75

    def test_phase_reached(self):
        assert ScoringCriteria().phase_reached() == Phase.BUILD
        assert ScoringCriteria(compiles=True).phase_reached() == Phase.RESOLUTION
        assert ScoringCriteria(compiles=True, imports_target=True).phase_reached() == Phase.SYNTHESIS
        assert (
            ScoringCriteria(compiles=True, imports_target=True, creates_target_type=True).phase_reached()
            == Phase.EXECUTION
        )
        assert (
            ScoringCriteria(
                compiles=True, imports_target=True, creates_target_type=True, executes_cleanly=True
            ).phase_reached()
            == Phase.VALIDATION
        )

    def test_from_phase(self):
        build = ScoringCriteria.from_phase(Phase.BUILD)
        assert not build.compiles
        assert build.score() == 0.0

        resolution = ScoringCriteria.from_phase(Phase.RESOLUTION)
        assert resolution.compiles
        assert not resolution.imports_target
        assert resolution.score() == 0.25

        validation = ScoringCriteria.from_phase(Phase.VALIDATION)
        assert validation.compiles
        assert validation.imports_target
        assert validation.creates_target_type
        assert validation.executes_cleanly
        assert validation.score() == 1.0

    def test_to_dict(self):
        criteria = ScoringCriteria(compiles=True, imports_target=True)
        d = criteria.to_dict()
        assert d["compiles"] is True
        assert d["imports_target"] is True
        assert d["creates_target_type"] is False
        assert d["executes_cleanly"] is False
        assert d["score"] == 0.5
        assert d["phase_reached"] == "synthesis"


# =============================================================================
# InhabitationMetrics Tests
# =============================================================================


class TestInhabitationMetrics:
    def test_default_metrics(self):
        metrics = InhabitationMetrics()
        assert metrics.inhabitation_rate() == 0.0
        assert metrics.entry_coverage() == 0.0

    def test_inhabitation_rate(self):
        metrics = InhabitationMetrics(
            target_types_total=10,
            target_types_inhabited=3,
        )
        assert metrics.inhabitation_rate() == 0.3

    def test_entry_coverage(self):
        metrics = InhabitationMetrics(
            target_entry_functions=5,
            entry_functions_called=2,
        )
        assert metrics.entry_coverage() == 0.4

    def test_to_dict(self):
        metrics = InhabitationMetrics(
            target_types_total=10,
            target_types_inhabited=5,
            inhabited_types=["Foo", "Bar"],
            target_entry_functions=3,
            entry_functions_called=1,
        )
        d = metrics.to_dict()
        assert d["target_types_total"] == 10
        assert d["target_types_inhabited"] == 5
        assert d["inhabitation_rate"] == 0.5
        assert d["entry_coverage"] == pytest.approx(1 / 3)
        assert d["inhabited_types"] == ["Foo", "Bar"]


# =============================================================================
# ExecutionTrace Tests
# =============================================================================


class TestExecutionTrace:
    def test_record_call(self):
        trace = ExecutionTrace()
        trace.record_call("0x2::coin", "mint", ["0x1::sui::SUI"])
        assert len(trace.functions_called) == 1
        assert trace.functions_called[0].module == "0x2::coin"
        assert trace.functions_called[0].function == "mint"
        assert trace.functions_called[0].succeeded is True

    def test_mark_last_failed(self):
        trace = ExecutionTrace()
        trace.record_call("0x2::coin", "mint")
        trace.mark_last_failed("assertion failed")
        assert trace.functions_called[0].succeeded is False
        assert trace.functions_called[0].error == "assertion failed"

    def test_to_dict(self):
        trace = ExecutionTrace(
            execution_attempted=True,
            modules_loaded=["0x2::coin"],
            gas_used=1000,
        )
        trace.record_call("0x2::coin", "mint")
        d = trace.to_dict()
        assert d["execution_attempted"] is True
        assert d["modules_loaded"] == ["0x2::coin"]
        assert d["gas_used"] == 1000
        assert len(d["functions_called"]) == 1


# =============================================================================
# AbortInfo Tests
# =============================================================================


class TestAbortInfo:
    def test_from_move_abort_unsupported_native(self):
        abort = AbortInfo.from_move_abort(E_NOT_SUPPORTED, "0x2::random", "random not supported")
        assert abort.is_expected is True
        assert abort.category == AbortCategory.UNSUPPORTED_NATIVE
        assert abort.abort_code == E_NOT_SUPPORTED

    def test_from_move_abort_assertion(self):
        abort = AbortInfo.from_move_abort(42, "0x1::test", "assertion failed in test")
        assert abort.is_expected is False
        assert abort.category == AbortCategory.ASSERTION_FAILED

    def test_categorize_abort(self):
        # Test various message patterns
        abort = AbortInfo.from_move_abort(100, None, "overflow detected")
        assert abort.category == AbortCategory.ARITHMETIC_ERROR

        abort = AbortInfo.from_move_abort(100, None, "vector index out of bounds")
        assert abort.category == AbortCategory.VECTOR_BOUNDS_ERROR

        abort = AbortInfo.from_move_abort(100, None, "object ownership error")
        assert abort.category == AbortCategory.OBJECT_ERROR

    def test_push_frame(self):
        abort = AbortInfo.from_move_abort(1, None, "error")
        abort.push_frame("0x2::coin", "mint", 42)
        abort.push_frame("0x1::test", "main")
        assert len(abort.call_stack) == 2
        assert abort.call_stack[0].function == "mint"
        assert abort.call_stack[0].instruction_offset == 42
        assert abort.call_stack[1].instruction_offset is None

    def test_to_dict(self):
        abort = AbortInfo.from_move_abort(E_NOT_SUPPORTED, "0x2::random", "unsupported")
        abort.push_frame("0x2::random", "new")
        d = abort.to_dict()
        assert d["abort_code"] == E_NOT_SUPPORTED
        assert d["abort_location"] == "0x2::random"
        assert d["category"] == "unsupported_native"
        assert d["is_expected"] is True
        assert len(d["call_stack"]) == 1


# =============================================================================
# EvaluationResult Tests
# =============================================================================


class TestEvaluationResult:
    def test_success(self):
        result = EvaluationResult.success()
        assert result.ok is True
        assert result.score == 1.0
        assert result.failure is None

    def test_failed_build_phase(self):
        failure = Failure.from_code(ErrorCode.TYPE_SYNTAX_ERROR, "bad syntax")
        result = EvaluationResult.failed(failure)
        assert result.ok is False
        assert result.score == 0.0  # Build phase = 0 points
        assert result.failure is not None
        assert result.partial_credit_reason is None  # No partial credit for 0 score

    def test_failed_with_partial_credit(self):
        failure = Failure.from_code(ErrorCode.TARGET_ABORTED, "abort")
        result = EvaluationResult.failed(failure)
        assert result.ok is False
        assert result.score == 0.75  # Execution phase = compiles + imports + creates
        assert result.partial_credit_reason is not None
        assert "execution" in result.partial_credit_reason.lower()

    def test_with_metrics(self):
        metrics = InhabitationMetrics(
            target_types_total=5,
            target_types_inhabited=3,
        )
        result = EvaluationResult.success().with_metrics(metrics)
        assert result.inhabitation_metrics is not None
        assert result.inhabitation_metrics.target_types_inhabited == 3

    def test_with_trace(self):
        trace = ExecutionTrace(execution_attempted=True, duration_ms=150)
        result = EvaluationResult.success().with_trace(trace)
        assert result.execution_trace is not None
        assert result.execution_trace.duration_ms == 150

    def test_to_dict(self):
        result = EvaluationResult.success()
        d = result.to_dict()
        assert d["ok"] is True
        assert d["score"] == 1.0
        assert "criteria" in d

    def test_failed_to_dict_with_all_details(self):
        failure = Failure.from_code(ErrorCode.TARGET_ABORTED, "abort")
        criteria = ScoringCriteria.from_phase(Phase.EXECUTION)
        metrics = InhabitationMetrics(target_types_total=10, target_types_inhabited=5)
        trace = ExecutionTrace(execution_attempted=True)
        trace.abort_info = AbortInfo.from_move_abort(42, "0x1::test", "assert failed")

        result = EvaluationResult.failed_with_details(failure, criteria, metrics, trace)
        d = result.to_dict()

        assert d["ok"] is False
        assert d["failure"]["code"] == "E403"
        assert d["inhabitation_metrics"]["target_types_total"] == 10
        assert d["execution_trace"]["abort_info"]["abort_code"] == 42


# =============================================================================
# Helper Function Tests
# =============================================================================


class TestParseBuilderror:
    def test_address_undefined(self):
        error = "error[E03001]: address with no value"
        assert parse_build_error(error) == ErrorCode.MODULE_ADDRESS_UNDEFINED

    def test_invalid_entry_signature(self):
        error = "error[Sui E02002]: invalid 'entry' function signature"
        assert parse_build_error(error) == ErrorCode.INVALID_ENTRY_SIGNATURE

    def test_type_syntax_error(self):
        error = "error[E03006]: invalid qualified path in field"
        assert parse_build_error(error) == ErrorCode.TYPE_SYNTAX_ERROR

    def test_unbound_module(self):
        error = "error[E04001]: unbound module '0x123::foo'"
        assert parse_build_error(error) == ErrorCode.IMPORT_RESOLUTION_FAILED

    def test_unknown_error(self):
        error = "some random error"
        assert parse_build_error(error) is None


class TestIsUnsupportedNativeError:
    def test_positive(self):
        assert is_unsupported_native_error("VMError: MoveAbort(1000)") is True
        assert is_unsupported_native_error("execution failed: MoveAbort with code 1000") is True

    def test_negative(self):
        assert is_unsupported_native_error("VMError: MoveAbort(42)") is False
        assert is_unsupported_native_error("some other error") is False


# =============================================================================
# Serialization/Deserialization Tests
# =============================================================================


class TestJsonSerialization:
    def test_full_evaluation_result_serializes(self):
        """Test that a complete EvaluationResult with all fields serializes properly."""
        failure = Failure.with_context(
            ErrorCode.TARGET_ABORTED,
            "abort at test",
            FailureContext(module="0x1::test", function="main"),
        )
        criteria = ScoringCriteria.from_phase(Phase.EXECUTION)
        metrics = InhabitationMetrics(
            target_types_total=10,
            target_types_inhabited=5,
            inhabited_types=["Foo", "Bar", "Baz", "Qux", "Quux"],
            uninhabited_types=[
                UninhabitedType("NoConstructor", UninhabitedReason.NO_CONSTRUCTOR, "No public new()"),
            ],
            target_entry_functions=3,
            entry_functions_called=2,
        )
        trace = ExecutionTrace(
            execution_attempted=True,
            modules_loaded=["0x1::test", "0x2::coin"],
            gas_used=5000,
            duration_ms=100,
        )
        trace.record_call("0x1::test", "main")
        trace.record_call("0x2::coin", "mint", ["0x1::sui::SUI"])
        trace.mark_last_failed("abort")
        trace.abort_info = AbortInfo.from_move_abort(42, "0x2::coin", "insufficient balance")
        trace.abort_info.push_frame("0x2::coin", "mint", 10)
        trace.abort_info.push_frame("0x1::test", "main", 5)

        result = EvaluationResult.failed_with_details(failure, criteria, metrics, trace)
        d = result.to_dict()

        # Should serialize without errors
        json_str = json.dumps(d, indent=2)
        assert len(json_str) > 0

        # Parse back and verify structure
        parsed = json.loads(json_str)
        assert parsed["ok"] is False
        assert parsed["score"] == 0.75
        assert parsed["failure"]["phase"] == "execution"
        assert parsed["failure"]["code"] == "E403"
        assert parsed["failure"]["context"]["module"] == "0x1::test"
        assert parsed["inhabitation_metrics"]["inhabitation_rate"] == 0.5
        assert len(parsed["execution_trace"]["functions_called"]) == 2
        assert parsed["execution_trace"]["abort_info"]["category"] == "unknown"  # "insufficient balance"
