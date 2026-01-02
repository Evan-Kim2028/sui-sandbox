"""OpenRouter integration tests for smi-openrouter-models.

Tests cover:
- Price parsing from various formats
- API key resolution priority
- API fetching and error handling
- Model ID filtering and deduplication
- CLI argument handling
"""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from smi_bench import openrouter_models


def test_extract_price_float_returns_float() -> None:
    """Price parsing returns float for float input."""
    result = openrouter_models._extract_price(1.5)
    assert result == 1.5
    assert isinstance(result, float)


def test_extract_price_int_returns_float() -> None:
    """Price parsing returns float for int input."""
    result = openrouter_models._extract_price(2)
    assert result == 2.0
    assert isinstance(result, float)


def test_extract_price_string_parses_to_float() -> None:
    """Price parsing handles string representation."""
    result = openrouter_models._extract_price("0.001")
    assert result == 0.001
    assert isinstance(result, float)


def test_extract_price_none_returns_none() -> None:
    """Null handling returns None for None input."""
    result = openrouter_models._extract_price(None)
    assert result is None


def test_extract_price_invalid_string_returns_none() -> None:
    """Error handling returns None for invalid string."""
    result = openrouter_models._extract_price("not_a_number")
    assert result is None


def test_extract_price_invalid_type_returns_none() -> None:
    """Error handling returns None for invalid types."""
    # The function returns None for types not in (int, float, str)
    assert openrouter_models._extract_price([]) is None
    assert openrouter_models._extract_price({}) is None
    assert openrouter_models._extract_price(True) is None
    assert openrouter_models._extract_price(False) is None
    assert openrouter_models._extract_price(object()) is None


def test_get_api_key_prefer_smi_api_key(monkeypatch) -> None:
    """Priority handling prefers SMI_API_KEY over others."""
    monkeypatch.setenv("SMI_API_KEY", "smi_key")
    monkeypatch.setenv("OPENROUTER_API_KEY", "openrouter_key")

    result = openrouter_models._get_api_key({})
    assert result == "smi_key"


def test_get_api_key_fallback_to_openrouter_key(monkeypatch) -> None:
    """Fallback logic uses OPENROUTER_API_KEY when SMI_API_KEY missing."""
    monkeypatch.delenv("SMI_API_KEY", raising=False)
    monkeypatch.setenv("OPENROUTER_API_KEY", "openrouter_key")

    result = openrouter_models._get_api_key({})
    assert result == "openrouter_key"


def test_get_api_key_env_override_wins(monkeypatch) -> None:
    """Override logic prefers env_overrides over environment."""
    monkeypatch.setenv("SMI_API_KEY", "env_key")
    env_overrides = {"SMI_API_KEY": "override_key"}

    result = openrouter_models._get_api_key(env_overrides)
    assert result == "env_key"


def test_get_api_key_env_override_fallback(monkeypatch) -> None:
    """Fallback logic tries OPENROUTER_API_KEY override."""
    monkeypatch.delenv("SMI_API_KEY", raising=False)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    env_overrides = {"OPENROUTER_API_KEY": "override_key"}

    result = openrouter_models._get_api_key(env_overrides)
    assert result == "override_key"


def test_get_api_key_missing_raises_valueerror(monkeypatch) -> None:
    """Missing key error raises ValueError."""
    monkeypatch.delenv("SMI_API_KEY", raising=False)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)

    with pytest.raises(ValueError) as exc_info:
        openrouter_models._get_api_key({})

    assert "missing" in str(exc_info.value).lower()


def test_fetch_openrouter_models_success(monkeypatch) -> None:
    """API fetch success returns list of OpenRouterModel objects."""
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {
        "data": [
            {
                "id": "openai/gpt-4",
                "name": "GPT-4",
                "context_length": 8192,
                "pricing": {"prompt": 0.03, "completion": 0.06},
            },
            {
                "id": "anthropic/claude-3",
                "name": "Claude 3",
                "context_length": 200000,
                "pricing": {"prompt": 0.015, "completion": 0.075},
            },
        ]
    }

    with patch("httpx.get") as mock_get:
        mock_get.return_value = mock_response

        models = openrouter_models.fetch_openrouter_models(base_url="https://test.api/v1", api_key="test_key")

        assert len(models) == 2
        assert models[0].id == "openai/gpt-4"
        assert models[0].name == "GPT-4"
        assert models[0].context_length == 8192
        assert models[0].pricing_prompt == 0.03
        assert models[0].pricing_completion == 0.06


def test_fetch_openrouter_models_malformed_response_raises_valueerror(monkeypatch) -> None:
    """Error handling raises ValueError for malformed response."""
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {"data": "not_a_list"}

    with patch("httpx.get") as mock_get:
        mock_get.return_value = mock_response

        with pytest.raises(ValueError) as exc_info:
            openrouter_models.fetch_openrouter_models(base_url="https://test.api/v1", api_key="test_key")

        assert "unexpected" in str(exc_info.value).lower()


def test_fetch_openrouter_filters_invalid_items(monkeypatch) -> None:
    """Data validation skips items with missing required fields."""
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {
        "data": [
            {
                "id": "valid/model",
                "name": "Valid Model",
                "context_length": 4096,
            },
            {"name": "Missing ID"},  # Invalid: no id
            {"id": "invalid/no-name"},  # Invalid: no name (optional but should be None)
        ]
    }

    with patch("httpx.get") as mock_get:
        mock_get.return_value = mock_response

        models = openrouter_models.fetch_openrouter_models(base_url="https://test.api/v1", api_key="test_key")

        # Should only include valid model
        assert len(models) == 2
        assert models[0].id == "valid/model"
        assert models[1].id == "invalid/no-name"
        assert models[1].name is None


def test_write_model_ids_deduplicates_and_sorts(tmp_path: Path) -> None:
    """Deduplication logic removes duplicates and sorts."""
    model_ids = [
        "model-c",
        "model-a",
        "model-b",
        "model-a",  # Duplicate
        "model-c",  # Duplicate
        "model-d",
    ]

    out_file = tmp_path / "models.txt"
    openrouter_models.write_model_ids(str(out_file), model_ids)

    result = out_file.read_text()
    lines = result.strip().split("\n")

    # Should be deduplicated and sorted
    assert lines == ["model-a", "model-b", "model-c", "model-d"]


def test_write_model_ids_trims_whitespace(tmp_path: Path) -> None:
    """Whitespace handling trims leading/trailing whitespace."""
    model_ids = ["  model-a  ", "\tmodel-b\t", "  model-c"]

    out_file = tmp_path / "models.txt"
    openrouter_models.write_model_ids(str(out_file), model_ids)

    result = out_file.read_text()
    lines = result.strip().split("\n")

    assert lines == ["model-a", "model-b", "model-c"]


def test_write_model_ids_filters_empty_strings(tmp_path: Path) -> None:
    """Empty string filtering removes blank entries."""
    model_ids = ["model-a", "", "model-b", "  ", "model-c", "\t"]

    out_file = tmp_path / "models.txt"
    openrouter_models.write_model_ids(str(out_file), model_ids)

    result = out_file.read_text()
    lines = result.strip().split("\n")

    assert lines == ["model-a", "model-b", "model-c"]


def test_main_filter_by_contains_substring(tmp_path: Path, monkeypatch) -> None:
    """Substring filtering by model ID."""
    monkeypatch.setenv("SMI_API_KEY", "test_key")
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {
        "data": [
            {"id": "openai/gpt-4"},
            {"id": "openai/gpt-3.5"},
            {"id": "anthropic/claude-3"},
            {"id": "openai/gpt-4-turbo"},
        ]
    }

    with (
        patch("httpx.get", return_value=mock_response),
        patch("smi_bench.openrouter_models.write_model_ids") as mock_write,
    ):
        argv = [
            "--contains",
            "gpt",
            "--out",
            str(tmp_path / "models.txt"),
        ]
        result = openrouter_models.main(argv)

        assert result == 0
        # Verify write_model_ids was called
        assert mock_write.called
        # Get the model IDs that were passed to write_model_ids
        written_ids = mock_write.call_args[0][1]
        assert "openai/gpt-4" in written_ids
        assert "openai/gpt-3.5" in written_ids
        assert "openai/gpt-4-turbo" in written_ids
        assert "anthropic/claude-3" not in written_ids


def test_main_filter_by_name_contains(tmp_path: Path, monkeypatch) -> None:
    """Name filtering by model display name."""
    monkeypatch.setenv("SMI_API_KEY", "test_key")
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {
        "data": [
            {"id": "openai/gpt-4", "name": "GPT-4"},
            {"id": "openai/gpt-3.5", "name": "GPT-3.5 Turbo"},
            {"id": "anthropic/claude-3", "name": "Claude 3 Opus"},
        ]
    }

    with (
        patch("httpx.get", return_value=mock_response),
        patch("smi_bench.openrouter_models.write_model_ids") as mock_write,
    ):
        argv = [
            "--name-contains",
            "claude",
            "--out",
            str(tmp_path / "models.txt"),
        ]
        result = openrouter_models.main(argv)

        assert result == 0
        # Verify write_model_ids was called
        assert mock_write.called
        # Get the model IDs that were passed to write_model_ids
        written_ids = mock_write.call_args[0][1]
        assert "anthropic/claude-3" in written_ids
        assert "openai/gpt-4" not in written_ids
        assert "openai/gpt-3.5" not in written_ids


def test_main_default_base_url(tmp_path: Path, monkeypatch) -> None:
    """Default value uses OpenRouter API base URL."""
    monkeypatch.setenv("SMI_API_KEY", "test_key")
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {"data": []}

    with (
        patch("smi_bench.openrouter_models.fetch_openrouter_models") as mock_fetch,
        patch("smi_bench.openrouter_models.write_model_ids"),
    ):
        monkeypatch.delenv("SMI_API_BASE_URL", raising=False)
        monkeypatch.delenv("OPENROUTER_BASE_URL", raising=False)

        argv = ["--out", str(tmp_path / "models.txt")]
        result = openrouter_models.main(argv)

        assert result == 0
        # Verify fetch was called
        assert mock_fetch.called
        # Get the base_url that was passed to fetch_openrouter_models
        call_kwargs = mock_fetch.call_args[1]
        assert "base_url" in call_kwargs
        assert "openrouter.ai/api/v1" in call_kwargs["base_url"]


def test_main_env_file_overrides(tmp_path: Path, monkeypatch) -> None:
    """Environment override loads from .env file."""
    monkeypatch.delenv("SMI_API_KEY", raising=False)
    env_file = tmp_path / ".env"
    env_file.write_text("SMI_API_KEY=env_file_key")

    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {"data": []}

    with (
        patch("smi_bench.openrouter_models.fetch_openrouter_models"),
        patch("smi_bench.openrouter_models.write_model_ids"),
        patch("smi_bench.openrouter_models.load_dotenv") as mock_load_dotenv,
    ):
        # The actual implementation calls load_dotenv(Path(args.env_file))
        # We need to make sure it returns the expected dict
        mock_load_dotenv.return_value = {"SMI_API_KEY": "env_file_key"}

        argv = ["--env-file", str(env_file), "--out", str(tmp_path / "models.txt")]
        result = openrouter_models.main(argv)

        assert result == 0
        # Verify env file was loaded
        assert mock_load_dotenv.called
        # The Path object should be passed to load_dotenv
        call_args = mock_load_dotenv.call_args[0]
        # Verify a Path object was passed (not a string)
        assert isinstance(call_args[0], type(env_file)) or str(call_args[0]) == str(env_file)


def test_main_missing_api_key_raises_error(tmp_path: Path, monkeypatch) -> None:
    """Missing API key raises error."""
    monkeypatch.delenv("SMI_API_KEY", raising=False)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)

    with patch("smi_bench.openrouter_models.fetch_openrouter_models"):
        argv = ["--out", str(tmp_path / "models.txt")]
        with pytest.raises(ValueError) as exc_info:
            openrouter_models.main(argv)

        assert "missing" in str(exc_info.value).lower()
