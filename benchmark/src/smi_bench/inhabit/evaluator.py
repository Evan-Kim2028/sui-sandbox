"""
Evaluator for type inhabitation benchmark results.

This module provides functions to:
1. Parse MM2 mapping results into InhabitationMetrics
2. Extract ExecutionTrace from tier_b execution data
3. Compute ScoringCriteria based on pipeline progress
4. Generate complete EvaluationResult from benchmark artifacts

The evaluator bridges the gap between raw benchmark output (JSON files)
and the structured evaluation types defined in evaluation.py.
"""

from __future__ import annotations

import json
import logging
import re
from pathlib import Path
from typing import Any

from smi_bench.constants import FRAMEWORK_ADDRESSES, HELPER_PACKAGE_ADDRESS
from smi_bench.inhabit.evaluation import (
    AbortInfo,
    ErrorCode,
    EvaluationResult,
    ExecutionTrace,
    Failure,
    FunctionCall,
    InhabitationMetrics,
    ScoringCriteria,
    UninhabitedReason,
    UninhabitedType,
    parse_build_error,
)

logger = logging.getLogger(__name__)

# =============================================================================
# MM2 Result Parsing
# =============================================================================
#
# MM2 Terminology:
# ----------------
# "tier_a_hit" = SYNTHESIS_SUCCESS: Arguments were successfully synthesized
#     - BCS round-trip verified
#     - Type parameters resolved
#     - Default values or constructors found for all params
#     - Does NOT mean execution was attempted
#
# "tier_b_hit" = EXECUTION_SUCCESS: Function was successfully executed in VM
#     - All tier_a requirements met
#     - VM execution completed without abort/error
#     - Target modules were actually accessed
#
# "miss" = SYNTHESIS_FAILED: Could not synthesize arguments
#     - May be due to unsupported param types, no constructors, etc.
#
# Failure stages:
#     A1-A3: Synthesis failures (parameter resolution)
#     B1: Constructor execution failed (building a type to pass as argument)
#     B2: Target function execution failed (running the actual function)
# =============================================================================


def parse_mm2_entry(entry: dict[str, Any]) -> dict[str, Any]:
    """Parse a single MM2 mapping entry into a normalized format.

    Returns a dict with:
        - target: "module::function" string
        - target_package: package address
        - status: "tier_a_hit" (synthesis succeeded), "tier_b_hit" (execution succeeded), or "miss"
        - tier_a: tier_a_details (synthesis info) or None
        - tier_b: tier_b_details (execution info) or None
        - failure_reason: string or None
        - failure_stage: "A1", "A2", "A3", "B1", "B2" or None
    """
    target_module = entry.get("target_module", "")
    target_function = entry.get("target_function", "")
    target = f"{target_module}::{target_function}"

    return {
        "target": target,
        "target_package": entry.get("target_package", ""),
        "status": entry.get("status", "miss"),
        "tier_a": entry.get("tier_a_details"),
        "tier_b": entry.get("tier_b_details"),
        "failure_reason": entry.get("failure_reason"),
        "failure_stage": entry.get("failure_stage"),
    }


def extract_inhabitation_metrics(
    mm2_data: dict[str, Any],
    target_interface: dict[str, Any] | None = None,
    target_package_id: str | None = None,
) -> InhabitationMetrics:
    """Extract InhabitationMetrics from MM2 mapping results.

    Args:
        mm2_data: The parsed mm2_mapping.json or mm2_combined_mapping.json
        target_interface: Optional target_interface.json for total counts
        target_package_id: Optional package ID to filter target-only results (unused for now,
            since MM2 uses on-chain addresses which may differ from directory names)

    Returns:
        InhabitationMetrics with populated fields
    """
    metrics = InhabitationMetrics()

    # Get totals from target interface if available
    if target_interface:
        modules = target_interface.get("modules", {})
        for mod_name, mod_data in modules.items():
            if isinstance(mod_data, dict):
                funcs = mod_data.get("functions", {})
                for fn_name, fn_data in funcs.items():
                    if isinstance(fn_data, dict):
                        metrics.target_types_total += 1
                        if fn_data.get("is_entry"):
                            metrics.target_entry_functions += 1

    # Parse accepted entries
    accepted = mm2_data.get("accepted", [])
    inhabited_types: set[str] = set()
    entry_functions_called: set[str] = set()
    modules_accessed: set[str] = set()

    for entry in accepted:
        if not isinstance(entry, dict):
            continue

        parsed = parse_mm2_entry(entry)
        pkg = parsed["target_package"]

        # Skip helper package
        if pkg == HELPER_PACKAGE_ADDRESS:
            continue

        # Skip framework packages
        if pkg in FRAMEWORK_ADDRESSES:
            continue

        status = parsed["status"]
        target = parsed["target"]

        # Track inhabited types based on tier status
        if status in {"tier_a_hit", "tier_b_hit"}:
            inhabited_types.add(target)

            # Check if this is an entry function that was called
            tier_b = parsed.get("tier_b")
            if tier_b and tier_b.get("execution_success"):
                entry_functions_called.add(target)

            # Track modules accessed
            if tier_b:
                for mod in tier_b.get("target_modules_accessed", []):
                    modules_accessed.add(mod)

    # Parse rejected entries for uninhabited types
    rejected = mm2_data.get("rejected", [])
    uninhabited: list[UninhabitedType] = []

    for entry in rejected:
        if not isinstance(entry, dict):
            continue

        parsed = parse_mm2_entry(entry)
        target = parsed["target"]
        reason_str = parsed.get("failure_reason", "")
        stage = parsed.get("failure_stage", "")

        # Map failure reason to UninhabitedReason
        reason = _map_failure_to_uninhabited_reason(reason_str, stage)
        uninhabited.append(
            UninhabitedType(
                type_name=target,
                reason=reason,
                details=reason_str if reason_str else None,
            )
        )

    # Populate metrics
    metrics.target_types_inhabited = len(inhabited_types)
    metrics.inhabited_types = sorted(inhabited_types)
    metrics.uninhabited_types = uninhabited
    metrics.entry_functions_called = len(entry_functions_called)

    # Extract stdlib types from modules accessed
    stdlib_types = [m for m in modules_accessed if m.startswith("0x1::") or m.startswith("0x2::")]
    metrics.stdlib_types_used = sorted(stdlib_types)

    return metrics


def _map_failure_to_uninhabited_reason(reason: str, stage: str) -> UninhabitedReason:
    """Map MM2 failure reason to UninhabitedReason enum."""
    reason_lower = reason.lower() if reason else ""

    if "no constructor" in reason_lower or "no default value" in reason_lower:
        return UninhabitedReason.NO_CONSTRUCTOR
    if "unsupported" in reason_lower or "unsupported param" in reason_lower:
        return UninhabitedReason.UNSUPPORTED_PARAM
    if "chain" in reason_lower and "deep" in reason_lower:
        return UninhabitedReason.CHAIN_TOO_DEEP
    if "recursive" in reason_lower:
        return UninhabitedReason.RECURSIVE_TYPE
    if "ability" in reason_lower:
        return UninhabitedReason.ABILITY_CONSTRAINT
    if "runtime" in reason_lower or "object" in reason_lower:
        return UninhabitedReason.REQUIRES_RUNTIME_VALUE

    return UninhabitedReason.UNKNOWN


# =============================================================================
# Execution Trace Extraction
# =============================================================================


def extract_execution_trace(
    mm2_data: dict[str, Any],
    target_package_id: str | None = None,
) -> ExecutionTrace:
    """Extract ExecutionTrace from MM2 mapping results.

    Args:
        mm2_data: The parsed mm2_mapping.json
        target_package_id: Optional package ID to identify target module accesses

    Returns:
        ExecutionTrace with function calls and abort info
    """
    trace = ExecutionTrace()

    accepted = mm2_data.get("accepted", [])
    if not accepted:
        return trace

    trace.execution_attempted = True
    modules_loaded: set[str] = set()

    for entry in accepted:
        if not isinstance(entry, dict):
            continue

        parsed = parse_mm2_entry(entry)
        target = parsed["target"]
        tier_b = parsed.get("tier_b")

        if not tier_b:
            continue

        # Record modules loaded
        for mod in tier_b.get("target_modules_accessed", []):
            modules_loaded.add(mod)

        # Record function call
        module, function = target.split("::") if "::" in target else ("", target)
        succeeded = tier_b.get("execution_success", False)

        call = FunctionCall(
            module=module,
            function=function,
            succeeded=succeeded,
        )

        # Extract error info if execution failed
        error = tier_b.get("error")
        if error and not succeeded:
            call.error = error

            # Parse abort info from error
            abort_info = _parse_abort_from_error(error)
            if abort_info and trace.abort_info is None:
                trace.abort_info = abort_info

        trace.functions_called.append(call)

    trace.modules_loaded = sorted(modules_loaded)
    return trace


def _parse_abort_from_error(error: str) -> AbortInfo | None:
    """Parse abort information from a VM error string.

    Example error format:
    "execution failed: VMError { major_status: ABORTED, sub_status: Some(0),
     message: Some(\"0x...::cell::new at offset 1\"), ... }"
    """
    if "ABORTED" not in error:
        return None

    # Extract abort code from sub_status
    abort_code = None
    sub_status_match = re.search(r"sub_status:\s*Some\((\d+)\)", error)
    if sub_status_match:
        abort_code = int(sub_status_match.group(1))

    # Extract location from message
    location = None
    message_match = re.search(r'message:\s*Some\("([^"]+)"\)', error)
    if message_match:
        location = message_match.group(1)

    # Extract module location
    module_match = re.search(r'Module\(ModuleId\s*\{\s*address:\s*([a-f0-9]+),\s*name:\s*Identifier\("(\w+)"\)', error)
    module_location = None
    if module_match:
        addr = module_match.group(1)
        name = module_match.group(2)
        module_location = f"0x{addr}::{name}"

    abort_info = AbortInfo.from_move_abort(
        code=abort_code or 0,
        location=location or module_location,
        message=error,
    )

    # Add stack frame if we have module info
    if module_location and location:
        func_match = re.search(r"::(\w+)\s+at\s+offset", location)
        if func_match:
            abort_info.push_frame(
                module=module_location,
                function=func_match.group(1),
            )

    return abort_info


# =============================================================================
# Scoring and Evaluation
# =============================================================================


def compute_scoring_criteria(
    build_succeeded: bool,
    mm2_data: dict[str, Any] | None,
    target_package_id: str | None = None,
) -> ScoringCriteria:
    """Compute ScoringCriteria based on pipeline progress.

    Args:
        build_succeeded: Whether the helper package compiled
        mm2_data: The parsed mm2_mapping.json (or None if build failed)
        target_package_id: Package ID to filter target-only results (unused for now,
            since MM2 uses on-chain addresses which may differ from directory names)

    Returns:
        ScoringCriteria with appropriate flags set
    """
    criteria = ScoringCriteria()

    if not build_succeeded:
        return criteria

    criteria.compiles = True

    if mm2_data is None:
        return criteria

    accepted = mm2_data.get("accepted", [])

    # Check if any target package functions were imported (tier_a_hit)
    has_target_imports = False
    has_target_type_created = False
    has_clean_execution = False

    for entry in accepted:
        if not isinstance(entry, dict):
            continue

        parsed = parse_mm2_entry(entry)
        pkg = parsed["target_package"]
        status = parsed["status"]

        # Skip helper package
        if pkg == HELPER_PACKAGE_ADDRESS:
            continue

        # Skip framework packages
        if pkg in FRAMEWORK_ADDRESSES:
            continue

        # Any non-helper, non-framework package is a target
        if status in {"tier_a_hit", "tier_b_hit"}:
            has_target_imports = True

            # Check tier_a for type creation evidence
            tier_a = parsed.get("tier_a")
            if tier_a:
                resolved = tier_a.get("resolved_params", [])
                # If we resolved parameters, we created/used target types
                if resolved:
                    has_target_type_created = True

            # Check tier_b for clean execution
            tier_b = parsed.get("tier_b")
            if tier_b and tier_b.get("execution_success"):
                has_clean_execution = True
                has_target_type_created = True  # Successful execution implies type creation

    criteria.imports_target = has_target_imports
    criteria.creates_target_type = has_target_type_created
    criteria.executes_cleanly = has_clean_execution

    return criteria


def evaluate_build_failure(error_text: str) -> EvaluationResult:
    """Create an EvaluationResult for a build failure.

    Args:
        error_text: The build error output

    Returns:
        EvaluationResult with appropriate failure info
    """
    error_code = parse_build_error(error_text)
    if error_code is None:
        error_code = ErrorCode.TYPE_SYNTAX_ERROR  # Default build error

    failure = Failure.from_code(error_code, error_text[:500])
    return EvaluationResult.failed(failure)


def evaluate_mm2_failure(reason: str, stage: str) -> EvaluationResult:
    """Create an EvaluationResult for an MM2 mapping failure.

    Args:
        reason: The failure reason from MM2
        stage: The failure stage (A1, A2, A3, B1, B2)

    Returns:
        EvaluationResult with appropriate failure info
    """
    # Map stage to phase and error code
    if stage.startswith("A"):
        # Tier A failures are resolution/synthesis errors
        if "not found" in reason.lower():
            error_code = ErrorCode.MODULE_NOT_FOUND
        elif "no constructor" in reason.lower() or "no default" in reason.lower():
            error_code = ErrorCode.NO_CONSTRUCTOR
        else:
            error_code = ErrorCode.UNSUPPORTED_CONSTRUCTOR_PARAM
    # Tier B failures are execution errors
    elif "aborted" in reason.lower():
        error_code = ErrorCode.TARGET_ABORTED
    elif "unsupported" in reason.lower() or "native" in reason.lower():
        error_code = ErrorCode.UNSUPPORTED_NATIVE
    else:
        error_code = ErrorCode.VM_SETUP_FAILED

    failure = Failure.from_code(error_code, reason[:500])
    return EvaluationResult.failed(failure)


# =============================================================================
# Complete Evaluation from Artifacts
# =============================================================================


def evaluate_from_run_dir(run_dir: Path) -> EvaluationResult:
    """Generate a complete EvaluationResult from a benchmark run directory.

    Args:
        run_dir: Path to the e2e run directory containing artifacts

    Returns:
        EvaluationResult with full evaluation data
    """
    # Check if build succeeded by looking for bytecode
    helper_pkg = run_dir / "helper_pkg"
    build_succeeded = (helper_pkg / "build").exists()

    # If no build directory, check for build errors
    if not build_succeeded:
        stderr_log = run_dir / "helper_build_stderr.log"
        if stderr_log.exists():
            error_text = stderr_log.read_text(encoding="utf-8", errors="replace")
            return evaluate_build_failure(error_text)
        return EvaluationResult.failed(Failure.from_code(ErrorCode.INVALID_MANIFEST, "Build failed - no output"))

    # Load MM2 mapping results
    # Prefer combined mapping (has real target bytecode) over plain mm2_mapping (has stubs)
    mm2_combined_path = run_dir / "mm2_combined_mapping.json"
    mm2_path = run_dir / "mm2_mapping.json"
    mm2_data = None

    # Try combined mapping first - it has real target package bytecode
    if mm2_combined_path.exists():
        try:
            mm2_data = json.loads(mm2_combined_path.read_text(encoding="utf-8"))
        except Exception as e:
            logger.warning("Failed to parse mm2_combined_mapping.json: %s", e)

    # Fall back to plain mm2_mapping (may have stub bytecode for dependencies)
    if mm2_data is None and mm2_path.exists():
        try:
            mm2_data = json.loads(mm2_path.read_text(encoding="utf-8"))
        except Exception as e:
            logger.warning("Failed to parse mm2_mapping.json: %s", e)

    if mm2_data is None:
        return EvaluationResult.failed(Failure.from_code(ErrorCode.VM_SETUP_FAILED, "MM2 mapping failed"))

    # Load target interface for metrics
    target_interface = None
    interface_path = run_dir / "target_interface.json"
    if interface_path.exists():
        try:
            target_interface = json.loads(interface_path.read_text(encoding="utf-8"))
        except Exception as e:
            logger.debug("Failed to parse target_interface.json: %s", e)

    # Get target package ID from run config
    target_package_id = None
    config_path = run_dir / "run_config.json"
    if config_path.exists():
        try:
            config = json.loads(config_path.read_text(encoding="utf-8"))
            target_package_id = config.get("package_id")
        except Exception as e:
            logger.debug("Failed to parse run_config.json: %s", e)

    # Compute scoring criteria
    criteria = compute_scoring_criteria(
        build_succeeded=True,
        mm2_data=mm2_data,
        target_package_id=target_package_id,
    )

    # Extract metrics and trace
    metrics = extract_inhabitation_metrics(
        mm2_data=mm2_data,
        target_interface=target_interface,
        target_package_id=target_package_id,
    )
    trace = extract_execution_trace(
        mm2_data=mm2_data,
        target_package_id=target_package_id,
    )

    # Determine success/failure
    if criteria.executes_cleanly:
        return EvaluationResult.success_with_details(metrics=metrics, trace=trace)

    # Partial success - find the failure point
    if not criteria.imports_target:
        failure = Failure.from_code(
            ErrorCode.NO_TARGET_MODULES_ACCESSED,
            "Helper package compiled but did not import any target package modules",
        )
    elif not criteria.creates_target_type:
        failure = Failure.from_code(
            ErrorCode.NO_CONSTRUCTOR, "Imported target modules but did not create any target types"
        )
    else:
        # Has types but execution failed
        failure = Failure.from_code(
            ErrorCode.TARGET_ABORTED, "Created target types but execution did not complete cleanly"
        )

    return EvaluationResult.failed_with_details(
        failure=failure,
        criteria=criteria,
        metrics=metrics,
        trace=trace,
    )


def evaluate_from_validation_report(
    validation_report: dict[str, Any],
    run_dir: Path | None = None,
) -> EvaluationResult:
    """Generate EvaluationResult from an existing validation_report.json.

    This is useful for re-evaluating existing results with the new schema.

    Args:
        validation_report: The parsed validation_report.json
        run_dir: Optional run directory to load additional artifacts

    Returns:
        EvaluationResult with evaluation data
    """
    ok = validation_report.get("ok", False)
    errors = validation_report.get("errors", [])

    if ok:
        # Load additional data if run_dir provided
        if run_dir:
            return evaluate_from_run_dir(run_dir)

        # Basic success without details
        return EvaluationResult.success()

    # Failed - determine failure type from errors
    error_text = "\n".join(errors) if errors else "Unknown failure"

    # Check for specific error patterns
    if any("build failed" in e.lower() for e in errors):
        return evaluate_build_failure(error_text)

    if any("mm2 mapping failed" in e.lower() for e in errors):
        return EvaluationResult.failed(Failure.from_code(ErrorCode.VM_SETUP_FAILED, error_text))

    if any("no tier_b_hit" in e.lower() or "no target" in e.lower() for e in errors):
        failure = Failure.from_code(ErrorCode.NO_TARGET_MODULES_ACCESSED, error_text)
        # This means build succeeded but no execution
        criteria = ScoringCriteria(compiles=True, imports_target=True)
        return EvaluationResult.failed_with_details(
            failure=failure,
            criteria=criteria,
        )

    # Generic failure
    return EvaluationResult.failed(Failure.from_code(ErrorCode.TYPE_SYNTAX_ERROR, error_text))
