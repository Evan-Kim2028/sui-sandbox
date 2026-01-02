import json

import pytest

from smi_bench.utils import run_json_helper, safe_json_loads


def test_safe_json_loads_robustness():
    """Verify that we can extract JSON from a noisy string."""
    valid_obj = {"result": "success", "count": 42}
    valid_json = json.dumps(valid_obj)

    # Case 1: Pure JSON
    assert safe_json_loads(valid_json) == valid_obj

    # Case 2: Leading garbage (logs/warnings)
    noisy_input = "[WARN] Something happened\nDEBUG: 123\n" + valid_json
    assert safe_json_loads(noisy_input) == valid_obj

    # Case 3: Trailing garbage
    # (The heuristic currently finds the LAST '}', so this should work if valid)
    trailing_noisy = valid_json + "\n[INFO] Done"
    assert safe_json_loads(trailing_noisy) == valid_obj

    # Case 4: Mixed garbage
    mixed = "START\n" + valid_json + "\nEND"
    assert safe_json_loads(mixed) == valid_obj


def test_safe_json_loads_failures():
    """Verify that truly invalid JSON still fails with good errors."""
    with pytest.raises(ValueError, match="JSON parse error"):
        safe_json_loads("Not JSON at all")

    with pytest.raises(ValueError, match="JSON parse error"):
        safe_json_loads('{"broken": ')


def test_run_json_helper_mock(monkeypatch):
    """Verify run_json_helper handles subprocess details correctly."""
    import subprocess

    class MockCompletedProcess:
        def __init__(self, stdout):
            self.stdout = stdout
            self.returncode = 0

    def mock_check_output(*args, **kwargs):
        return "[LOG] ignored\n" + json.dumps({"foo": "bar"})

    monkeypatch.setattr(subprocess, "check_output", mock_check_output)

    result = run_json_helper(["dummy"], timeout_s=1.0)
    assert result == {"foo": "bar"}


def test_run_json_helper_timeout(monkeypatch):
    """Verify timeout handling."""
    import subprocess

    def mock_timeout(*args, **kwargs):
        raise subprocess.TimeoutExpired(cmd=["dummy"], timeout=1.0)

    monkeypatch.setattr(subprocess, "check_output", mock_timeout)

    with pytest.raises(TimeoutError, match="subprocess timed out after 1.0s"):
        run_json_helper(["dummy"], timeout_s=1.0)


def test_run_json_helper_failure(monkeypatch):
    """Verify non-zero exit code handling."""
    import subprocess

    def mock_failure(*args, **kwargs):
        raise subprocess.CalledProcessError(returncode=1, cmd=["dummy"], stderr="Kernel panic")

    monkeypatch.setattr(subprocess, "check_output", mock_failure)

    with pytest.raises(RuntimeError, match=r"(?s)subprocess failed \(exit 1\).*Stderr: Kernel panic"):
        run_json_helper(["dummy"], timeout_s=1.0)
