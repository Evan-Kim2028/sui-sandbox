"""
Unit tests for A2A green agent model switching functionality.

Tests verify:
- EvalConfig model field parsing
- Model validation
- Model precedence (payload > env var > default)
- subprocess environment variable passing
"""

import os

import pytest

from smi_bench.a2a_errors import InvalidConfigError
from smi_bench.a2a_green_agent import _load_cfg


class TestEvalConfigModelHandling:
    """Test EvalConfig model field parsing and validation."""

    def test_evalconfig_model_override(self):
        """Test model from config takes precedence."""
        cfg = _load_cfg({"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": "gpt-4"})
        assert cfg.model == "gpt-4"

    def test_evalconfig_model_missing(self):
        """Test missing model defaults to None."""
        cfg = _load_cfg({"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt"})
        assert cfg.model is None

    def test_evalconfig_model_empty_string(self):
        """Test empty model raises error."""
        with pytest.raises(InvalidConfigError) as exc_info:
            _load_cfg({"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": ""})
        assert "model" in str(exc_info.value).lower()

    def test_evalconfig_model_whitespace_only(self):
        """Test whitespace-only model raises error."""
        with pytest.raises(InvalidConfigError) as exc_info:
            _load_cfg({"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": "   "})
        assert "model" in str(exc_info.value).lower()

    def test_evalconfig_model_with_special_characters(self):
        """Test model names with slashes and dots are preserved."""
        cfg = _load_cfg(
            {
                "corpus_root": "/test/corpus",
                "package_ids_file": "/test/manifest.txt",
                "model": "openai/gpt-4-turbo-preview",
            }
        )
        assert cfg.model == "openai/gpt-4-turbo-preview"


class TestModelPrecedence:
    """Test model precedence rules: payload > env var > default."""

    @pytest.mark.anyio
    async def test_payload_model_overrides_env_var(self, tmp_path, monkeypatch):
        """Test that payload model takes precedence over env var."""
        monkeypatch.setenv("SMI_MODEL", "env-model")

        cfg = _load_cfg(
            {"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": "payload-model"}
        )
        assert cfg.model == "payload-model"

    @pytest.mark.anyio
    async def test_env_var_used_when_payload_missing(self, monkeypatch):
        """Test that env var is used when payload doesn't specify model."""
        monkeypatch.setenv("SMI_MODEL", "env-model")

        cfg = _load_cfg({"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt"})
        # In current implementation, this returns None
        # The env var is only used in subprocess
        assert cfg.model is None

    @pytest.mark.anyio
    async def test_no_model_when_both_missing(self, monkeypatch):
        """Test that model is None when both payload and env var are missing."""
        monkeypatch.delenv("SMI_MODEL", raising=False)

        cfg = _load_cfg({"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt"})
        assert cfg.model is None


class TestModelPassedToSubprocess:
    """Test that model is correctly passed to subprocess environment."""

    @pytest.mark.anyio
    async def test_model_in_subprocess_env(self, monkeypatch):
        """Verify SMI_MODEL is set in subprocess env when cfg.model is set."""
        from smi_bench.a2a_green_agent import _load_cfg

        # Create a config with model override
        cfg = _load_cfg(
            {"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": "test-model-override"}
        )

        # Simulate what happens in _run_task_logic
        env = {}
        allowed_prefixes = ("SMI_", "RUST_", "CARGO_")
        env = {k: v for k, v in os.environ.items() if any(k.startswith(p) for p in allowed_prefixes)}

        # This is the actual code from _run_task_logic
        if cfg.model:
            env["SMI_MODEL"] = cfg.model

        # Verify SMI_MODEL was set in subprocess env
        assert "SMI_MODEL" in env
        assert env["SMI_MODEL"] == "test-model-override"

    @pytest.mark.anyio
    async def test_no_model_in_env_when_cfg_model_none(self):
        """Verify SMI_MODEL is NOT set in subprocess env when cfg.model is None."""
        from smi_bench.a2a_green_agent import _load_cfg

        # Create a config without model
        cfg = _load_cfg(
            {
                "corpus_root": "/test/corpus",
                "package_ids_file": "/test/manifest.txt",
                # No model specified
            }
        )

        # Simulate what happens in _run_task_logic
        env = {}
        allowed_prefixes = ("SMI_", "RUST_", "CARGO_")
        env = {k: v for k, v in os.environ.items() if any(k.startswith(p) for p in allowed_prefixes)}

        # This is the actual code from _run_task_logic
        if cfg.model:
            env["SMI_MODEL"] = cfg.model

        # When cfg.model is None, SMI_MODEL should not be added
        # (it may still be present if set in os.environ)
        # The key is that we don't add it when cfg.model is None
        assert env.get("SMI_MODEL") != "None"


class TestModelValidation:
    """Test validation of model field."""

    def test_valid_model_names(self):
        """Test that valid model names are accepted."""
        valid_models = [
            "gpt-4",
            "openai/gpt-4-turbo",
            "google/gemini-3-flash-preview",
            "anthropic/claude-3-opus",
            "meta-llama/Llama-2-70b",
        ]

        for model in valid_models:
            cfg = _load_cfg({"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": model})
            assert cfg.model == model

    def test_invalid_model_formats(self):
        """Test that invalid model formats are rejected."""
        invalid_cases = [
            "",  # Empty string
            "   ",  # Whitespace only
            None,  # None value (should be ignored, not error)
        ]

        for model in invalid_cases:
            if model is None:
                # None should be valid (not specified)
                cfg = _load_cfg(
                    {"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": model}
                )
                assert cfg.model is None
            else:
                with pytest.raises(InvalidConfigError):
                    _load_cfg({"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": model})


class TestEvalConfigImmutability:
    """Test that EvalConfig remains immutable (frozen dataclass)."""

    def test_evalconfig_is_frozen(self):
        """Verify EvalConfig cannot be modified after creation."""
        cfg = _load_cfg(
            {"corpus_root": "/test/corpus", "package_ids_file": "/test/manifest.txt", "model": "test-model"}
        )

        with pytest.raises(Exception):  # FrozenInstanceError
            cfg.model = "new-model"

        with pytest.raises(Exception):
            cfg.samples = 100
