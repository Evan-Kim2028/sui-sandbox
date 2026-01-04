import subprocess
from unittest.mock import patch

import pytest

from smi_bench.rust import emit_bytecode_json


def test_emit_bytecode_json_retries_on_failure(tmp_path):
    rust_bin = tmp_path / "fake_rust"
    rust_bin.touch()
    pkg_dir = tmp_path / "pkg"
    pkg_dir.mkdir()

    # Mock subprocess.check_output to fail twice then succeed
    mock_responses = [
        subprocess.CalledProcessError(1, "cmd", stderr=b"transient error"),
        subprocess.CalledProcessError(1, "cmd", stderr=b"transient error"),
        '{"package_id": "0x123", "modules": {}}',
    ]

    with patch("subprocess.check_output", side_effect=mock_responses) as mock_run:
        # We need to mock retry_with_backoff's sleep to keep tests fast
        with patch("time.sleep"):
            result = emit_bytecode_json(package_dir=pkg_dir, rust_bin=rust_bin)

            assert result["package_id"] == "0x123"
            assert mock_run.call_count == 3


def test_emit_bytecode_json_raises_after_max_retries(tmp_path):
    rust_bin = tmp_path / "fake_rust"
    rust_bin.touch()
    pkg_dir = tmp_path / "pkg"
    pkg_dir.mkdir()

    # Mock subprocess.check_output to always fail
    error = subprocess.CalledProcessError(1, "cmd", stderr=b"perm error")
    with patch("subprocess.check_output", side_effect=error) as mock_run:
        with patch("time.sleep"):
            with pytest.raises(RuntimeError, match="Rust extractor failed"):
                emit_bytecode_json(package_dir=pkg_dir, rust_bin=rust_bin)

            assert mock_run.call_count == 3
