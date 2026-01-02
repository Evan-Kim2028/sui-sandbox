"""Property-based tests for PTB Causality Validator.

Ensures that the validator correctly identifies and rejects
physically impossible transaction plans.
"""

from __future__ import annotations

import hypothesis.strategies as st
from hypothesis import given

from smi_bench.inhabit.validator import PTBCausalityError, validate_ptb_causality


@st.composite
def ptb_call_strategy(draw, max_results):
    """Generates a single PTB call that might reference previous results."""
    args = []
    num_args = draw(st.integers(0, 5))
    for _ in range(num_args):
        # include some malformed types to stress test the validator
        arg_type = draw(st.sampled_from(["pure", "imm_or_owned_object", "shared_object", "result", "garbage"]))
        if arg_type == "result":
            # Generate both valid and invalid indices
            idx = draw(st.one_of(st.integers(min_value=-2, max_value=max_results + 5), st.text()))
            args.append({"result": idx})
        elif arg_type == "pure":
            args.append({"pure": [1, 2, 3]})
        elif arg_type == "garbage":
            args.append({"what": "is this"})
        else:
            args.append({arg_type: "0x" + ("a" * 64)})

    return {"target": "0x2::m::f", "args": args}


@st.composite
def ptb_spec_strategy(draw):
    """Generates a full PTB spec with 1-10 calls."""
    num_calls = draw(st.integers(1, 10))
    calls = []
    for i in range(num_calls):
        calls.append(draw(ptb_call_strategy(i)))
    return {"calls": calls}


@given(ptb_spec_strategy())
def test_validator_detects_causality_violations(ptb_spec):
    """
    Test that if the validator passes, no call references a future or negative result.
    """
    try:
        validate_ptb_causality(ptb_spec)

        # If it passed, verify it was actually valid according to our rules
        for i, call in enumerate(ptb_spec["calls"]):
            for arg in call.get("args", []):
                if "result" in arg:
                    idx = arg["result"]
                    # If it passed, it must be an int and 0 <= idx < i
                    assert isinstance(idx, int)
                    assert 0 <= idx < i

    except PTBCausalityError:
        # If it failed, it should be because of a real violation
        violations_found = False

        # Check for non-list calls
        if not isinstance(ptb_spec.get("calls"), list):
            violations_found = True

        # Check each call
        if not violations_found:
            for i, call in enumerate(ptb_spec.get("calls", [])):
                if not isinstance(call, dict):
                    violations_found = True
                    break

                for arg in call.get("args", []):
                    if not isinstance(arg, dict):
                        violations_found = True
                        break

                    if "result" in arg:
                        idx = arg["result"]
                        if not isinstance(idx, int) or not (0 <= idx < i):
                            violations_found = True
                            break
                if violations_found:
                    break

        assert violations_found, f"Validator raised PTBCausalityError but we couldn't find a violation in {ptb_spec}"
