from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


class PTBCausalityError(ValueError):
    """Raised when a PTB plan violates causality or references missing inputs."""

    pass


@dataclass
class CausalityValidation:
    """Result of PTB causality validation with detailed metrics."""

    valid: bool
    errors: list[str] = field(default_factory=list)
    call_count: int = 0
    result_references_valid: int = 0
    result_references_total: int = 0

    @property
    def causality_score(self) -> float:
        """Ratio of valid result references (1.0 if no references or all valid)."""
        if self.result_references_total == 0:
            return 1.0
        return self.result_references_valid / self.result_references_total


def validate_ptb_causality_detailed(ptb_spec: dict[str, Any]) -> CausalityValidation:
    """
    Validate PTB spec causality and return detailed metrics.

    This is a non-throwing version that returns structured validation results,
    useful for scoring planning intelligence even when transactions fail to build.

    Invariants checked:
    1. PTB spec must be a dict with a 'calls' list
    2. Each call must be a dict
    3. Result references must point to earlier calls (i < current_call_index)
    4. Result indices must be non-negative integers

    Returns:
        CausalityValidation with validity status, errors, and reference metrics.
    """
    errors: list[str] = []
    call_count = 0
    result_refs_valid = 0
    result_refs_total = 0

    if not isinstance(ptb_spec, dict):
        return CausalityValidation(
            valid=False,
            errors=["PTB spec must be a dictionary"],
            call_count=0,
            result_references_valid=0,
            result_references_total=0,
        )

    calls = ptb_spec.get("calls")
    if not isinstance(calls, list):
        return CausalityValidation(
            valid=False,
            errors=["PTB spec must contain a 'calls' list"],
            call_count=0,
            result_references_valid=0,
            result_references_total=0,
        )

    call_count = len(calls)

    for i, call in enumerate(calls):
        if not isinstance(call, dict):
            errors.append(f"Call at index {i} must be a dictionary")
            continue

        args = call.get("args")
        if not isinstance(args, list):
            continue

        for arg_i, arg in enumerate(args):
            if not isinstance(arg, dict):
                errors.append(f"Argument {arg_i} in call {i} must be a dictionary")
                continue

            # Check for result reference
            if "result" in arg:
                result_refs_total += 1
                res_idx = arg["result"]

                if not isinstance(res_idx, int):
                    errors.append(
                        f"Result index in call {i}, arg {arg_i} must be an integer (got {type(res_idx).__name__})"
                    )
                elif res_idx < 0:
                    errors.append(f"Result index in call {i}, arg {arg_i} cannot be negative (got {res_idx})")
                elif res_idx >= i:
                    errors.append(
                        f"Causality violation in call {i}: references result {res_idx} which hasn't been produced yet"
                    )
                else:
                    result_refs_valid += 1

            # Check for nested_result reference
            if "nested_result" in arg:
                nested = arg["nested_result"]
                if isinstance(nested, list) and len(nested) >= 1:
                    result_refs_total += 1
                    res_idx = nested[0]

                    if not isinstance(res_idx, int):
                        errors.append(
                            f"Nested result index in call {i}, arg {arg_i} must be an integer "
                            f"(got {type(res_idx).__name__})"
                        )
                    elif res_idx < 0:
                        errors.append(
                            f"Nested result index in call {i}, arg {arg_i} cannot be negative (got {res_idx})"
                        )
                    elif res_idx >= i:
                        errors.append(
                            f"Causality violation in call {i}: nested_result references result {res_idx} "
                            f"which hasn't been produced yet"
                        )
                    else:
                        result_refs_valid += 1

    return CausalityValidation(
        valid=len(errors) == 0,
        errors=errors,
        call_count=call_count,
        result_references_valid=result_refs_valid,
        result_references_total=result_refs_total,
    )


def validate_ptb_causality(ptb_spec: dict[str, Any]) -> None:
    """
    Validate that a PTB spec is logically coherent.

    Invariants:
    1. If a call uses Result(i), then i must be < current_call_index.
    2. Every argument must be a known type (imm_or_owned_object, shared_object, pure, result).

    Raises:
        PTBCausalityError: If any causality invariant is violated.
    """
    result = validate_ptb_causality_detailed(ptb_spec)
    if not result.valid:
        raise PTBCausalityError(result.errors[0] if result.errors else "Unknown causality error")
