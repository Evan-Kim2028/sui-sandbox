"""Preflight validation tests for smi-a2a-preflight.

Tests cover:
- Path validation
- RPC connectivity checks
- Port detection
- Server startup/skip logic
- Smoke tool invocation
"""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock, patch

import httpx
import pytest

from smi_bench import a2a_preflight


def test_check_path_exists_missing_raises_systemexit(tmp_path: Path) -> None:
    """Path validation raises SystemExit for missing path."""
    missing_path = tmp_path / "does_not_exist"

    with pytest.raises(SystemExit) as exc_info:
        a2a_preflight._check_path_exists("test_label", missing_path)

    assert "missing" in str(exc_info.value).lower()
    assert "test_label" in str(exc_info.value)


def test_check_path_exists_valid_no_error(tmp_path: Path) -> None:
    """Valid path handling doesn't raise error."""
    valid_path = tmp_path / "exists"
    valid_path.touch()

    # Should not raise
    a2a_preflight._check_path_exists("test_label", valid_path)


def test_check_rpc_reachable_success(monkeypatch) -> None:
    """RPC connectivity validation succeeds for reachable endpoint."""
    # Mock successful HTTP response
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {"jsonrpc": "2.0"}

    with patch("httpx.Client") as mock_client_class:
        mock_client = MagicMock()
        mock_client_class.return_value.__enter__.return_value = mock_client
        mock_client.post.return_value = mock_response

        # Should not raise
        a2a_preflight._check_rpc_reachable("https://test.rpc", timeout_s=5.0)

        mock_client.post.assert_called_once()


def test_check_rpc_reachable_timeout_raises_systemexit(monkeypatch) -> None:
    """Timeout error handling raises SystemExit with clear message."""
    with patch("httpx.Client") as mock_client_class:
        mock_client = MagicMock()
        mock_client_class.return_value.__enter__.return_value = mock_client
        mock_client.post.side_effect = httpx.TimeoutException("timeout")

        with pytest.raises(SystemExit) as exc_info:
            a2a_preflight._check_rpc_reachable("https://test.rpc", timeout_s=5.0)

        assert "rpc_unreachable" in str(exc_info.value).lower()
        assert "timeout" in str(exc_info.value).lower()


def test_check_rpc_reachable_http_error_raises_systemexit(monkeypatch) -> None:
    """HTTP error handling raises SystemExit with clear message."""
    with patch("httpx.Client") as mock_client_class:
        mock_client = MagicMock()
        mock_client_class.return_value.__enter__.return_value = mock_client
        mock_client.post.side_effect = httpx.ConnectError("connection failed")

        with pytest.raises(SystemExit) as exc_info:
            a2a_preflight._check_rpc_reachable("https://test.rpc", timeout_s=5.0)

        assert "rpc_unreachable" in str(exc_info.value).lower()


def test_is_listening_true_when_port_open(monkeypatch) -> None:
    """Port detection returns True when port is open."""
    with patch("socket.create_connection") as mock_conn:
        mock_socket = MagicMock()
        mock_conn.return_value.__enter__.return_value = mock_socket

        result = a2a_preflight._is_listening("127.0.0.1", 9999)

        assert result is True
        mock_conn.assert_called_once_with(("127.0.0.1", 9999), timeout=0.5)


def test_is_listening_false_when_port_closed(monkeypatch) -> None:
    """Port detection returns False when port is closed."""
    with patch("socket.create_connection") as mock_conn:
        mock_conn.side_effect = OSError("connection refused")

        result = a2a_preflight._is_listening("127.0.0.1", 9999)

        assert result is False


def test_main_corpusr_root_missing_required_error(tmp_path: Path, monkeypatch) -> None:
    """Arg validation requires corpus_root."""
    args = [
        "--corpus-root",
        str(tmp_path / "missing"),
        "--rpc-url",
        "https://test.rpc",
    ]

    with pytest.raises(SystemExit) as exc_info:
        a2a_preflight.main(args)

    assert "missing" in str(exc_info.value).lower()


def test_main_scenario_launches_server_when_not_listening(tmp_path: Path, monkeypatch) -> None:
    """Server startup logic launches scenario server when port not listening."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    args = [
        "--corpus-root",
        str(corpus_root),
        "--scenario",
        str(tmp_path / "scenario"),
        "--rpc-url",
        "https://test.rpc",
    ]

    with (
        patch("smi_bench.a2a_preflight._is_listening") as mock_listening,
        patch("subprocess.Popen") as mock_popen,
        patch("subprocess.run") as mock_run,
    ):
        mock_listening.return_value = False  # Server not listening
        mock_run.return_value = MagicMock(returncode=0)

        a2a_preflight.main(args)

        # Verify subprocess.Popen was called to start server
        assert mock_popen.called
        call_args = mock_popen.call_args[0][0]
        assert "uv" in call_args
        assert "smi-agentbeats-scenario" in call_args


def test_main_scenario_skips_server_when_already_listening(tmp_path: Path, monkeypatch) -> None:
    """Server skip logic skips launch when port already listening."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()

    args = [
        "--corpus-root",
        str(corpus_root),
        "--scenario",
        str(tmp_path / "scenario"),
        "--rpc-url",
        "https://test.rpc",
    ]

    with (
        patch("smi_bench.a2a_preflight._is_listening") as mock_listening,
        patch("subprocess.Popen") as mock_popen,
        patch("subprocess.run") as mock_run,
    ):
        mock_listening.return_value = True  # Server already listening
        mock_run.return_value = MagicMock(returncode=0)

        a2a_preflight.main(args)

        # Verify subprocess.Popen was NOT called (server skip)
        assert not mock_popen.called


def test_main_calls_smoke_tool_after_checks(tmp_path: Path, monkeypatch) -> None:
    """Smoke tool invocation after all preflight checks."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    manifest = tmp_path / "manifest.txt"
    manifest.touch()

    args = [
        "--corpus-root",
        str(corpus_root),
        "--package-ids-file",
        str(manifest),
        "--rpc-url",
        "https://test.rpc",
    ]

    with (
        patch("smi_bench.a2a_preflight._is_listening", return_value=True),
        patch("subprocess.Popen"),
        patch("subprocess.run") as mock_run,
    ):
        mock_run.return_value = MagicMock(returncode=0)

        a2a_preflight.main(args)

        # Verify subprocess.run was called for smoke tool
        assert mock_run.called
        call_args = mock_run.call_args[0][0]
        assert "uv" in call_args
        assert "smi-a2a-smoke" in call_args
        assert "--corpus-root" in call_args
        assert str(corpus_root) in call_args


def test_main_success_prints_ready_for_full_run(tmp_path: Path, monkeypatch, capsys) -> None:
    """Success message printed when all checks pass."""
    corpus_root = tmp_path / "corpus"
    corpus_root.mkdir()
    manifest = tmp_path / "manifest.txt"
    manifest.touch()

    args = [
        "--corpus-root",
        str(corpus_root),
        "--package-ids-file",
        str(manifest),
        "--rpc-url",
        "https://test.rpc",
    ]

    with (
        patch("smi_bench.a2a_preflight._is_listening", return_value=True),
        patch("subprocess.Popen"),
        patch("subprocess.run", return_value=MagicMock(returncode=0)),
    ):
        a2a_preflight.main(args)

        captured = capsys.readouterr()
        # Check stdout for success message
        assert "ready_for_full_run" in captured.out or "packages_kept" in captured.out
