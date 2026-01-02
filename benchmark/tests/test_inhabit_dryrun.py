from __future__ import annotations

from smi_bench.inhabit.dryrun import classify_dry_run_response


def test_classify_dry_run_response_success() -> None:
    dry_run = {"effects": {"status": {"status": "success"}}}
    ok, failure = classify_dry_run_response(dry_run)
    assert ok is True
    assert failure is None


def test_classify_dry_run_response_missing_effects() -> None:
    ok, failure = classify_dry_run_response({})
    assert ok is False
    assert failure is not None
    assert failure.error == "missing effects"


def test_classify_dry_run_response_failure_parses_abort_code_and_location() -> None:
    err = "MoveAbort { location: 0x2::coin::withdraw, code: 123 }"
    dry_run = {"effects": {"status": {"status": "failure", "error": err}}}
    ok, failure = classify_dry_run_response(dry_run)
    assert ok is False
    assert failure is not None
    assert failure.status == "failure"
    assert failure.abort_code == 123
    assert failure.abort_location == "0x2::coin::withdraw"
