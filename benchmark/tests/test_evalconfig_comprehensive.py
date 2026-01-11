"""
Comprehensive tests for EvalConfig parsing, validation, and API robustness.

Tests cover:
1. All config fields (P0 and P1)
2. Validation rules and cross-field validation
3. Unknown field detection
4. JSON Schema generation
5. Type coercion
6. Edge cases and error messages
"""

from pathlib import Path

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from smi_bench.a2a_errors import InvalidConfigError
from smi_bench.a2a_green_agent import (
    DEFAULT_RPC_URL,
    KNOWN_CONFIG_FIELDS,
    EvalConfig,
    _detect_unknown_fields,
    _get_config_schema,
    _load_cfg,
    _validate_config_dry_run,
)
from smi_bench.utils import safe_bool
from smi_bench.utils import safe_parse_float as _safe_float
from smi_bench.utils import safe_parse_int as _safe_int


@pytest.fixture
def real_manifest(tmp_path):
    p = tmp_path / "ids.txt"
    p.write_text("0x1\n")
    return str(p)


class TestEvalConfigAllFields:
    """Test that all EvalConfig fields are properly parsed."""

    def test_minimal_config(self, real_manifest):
        """Minimal valid config should use all defaults."""
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp/corpus",
                "package_ids_file": real_manifest,
            }
        )

        assert cfg.corpus_root == "/tmp/corpus"
        assert cfg.package_ids_file == real_manifest
        assert cfg.samples == 0
        assert cfg.agent == "real-openai-compatible"
        assert cfg.rpc_url == DEFAULT_RPC_URL
        assert cfg.simulation_mode == "dry-run"
        assert cfg.per_package_timeout_seconds == 300.0
        assert cfg.max_plan_attempts == 2
        assert cfg.continue_on_error is True
        assert cfg.resume is True
        assert cfg.run_id is None
        assert cfg.model is None
        # P0 defaults
        assert cfg.seed == 0
        assert cfg.sender is None
        assert cfg.gas_budget == 10_000_000
        assert cfg.gas_coin is None
        assert cfg.gas_budget_ladder == "20000000,50000000"
        assert cfg.max_errors == 25
        assert cfg.max_run_seconds is None
        # P1 defaults
        assert cfg.max_planning_calls == 50
        assert cfg.checkpoint_every == 10
        assert cfg.max_heuristic_variants == 4
        assert cfg.baseline_max_candidates == 25
        assert cfg.include_created_types is False
        assert cfg.require_dry_run is False

    def test_full_config_all_fields(self, real_manifest):
        """Full config with all fields specified."""
        cfg = _load_cfg(
            {
                "corpus_root": "/my/corpus",
                "package_ids_file": real_manifest,
                "samples": 100,
                "agent": "baseline-search",
                "rpc_url": "https://custom.rpc.com",
                "simulation_mode": "dry-run",
                "per_package_timeout_seconds": 600.0,
                "max_plan_attempts": 10,
                "continue_on_error": False,
                "resume": False,
                "run_id": "my_run_123",
                "model": "gpt-4o",
                # P0 fields
                "seed": 42,
                "sender": "0xabc123",
                "gas_budget": 50_000_000,
                "gas_coin": "0xgas456",
                "gas_budget_ladder": "10000000,20000000",
                "max_errors": 5,
                "max_run_seconds": 7200.0,
                # P1 fields
                "max_planning_calls": 10,
                "checkpoint_every": 5,
                "max_heuristic_variants": 8,
                "baseline_max_candidates": 100,
                "include_created_types": True,
                "require_dry_run": True,
            }
        )

        assert cfg.corpus_root == "/my/corpus"
        assert cfg.samples == 100
        assert cfg.agent == "baseline-search"
        assert cfg.seed == 42
        assert cfg.sender == "0xabc123"
        assert cfg.gas_budget == 50_000_000
        assert cfg.gas_coin == "0xgas456"
        assert cfg.max_errors == 5
        assert cfg.max_run_seconds == 7200.0
        assert cfg.max_planning_calls == 10
        assert cfg.checkpoint_every == 5
        assert cfg.max_heuristic_variants == 8
        assert cfg.baseline_max_candidates == 100
        assert cfg.include_created_types is True
        assert cfg.require_dry_run is True


class TestValidationRules:
    """Test all validation rules for config fields.
    Critical fields (gas_budget, max_errors, timeouts) should raise errors if out of range.
    Less critical fields (seed, samples) are clamped.
    """

    def test_seed_must_be_non_negative(self, real_manifest):
        """Seed is clamped, not error."""
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp",
                "package_ids_file": real_manifest,
                "seed": -1,
            }
        )
        assert cfg.seed == 0

    def test_gas_budget_must_be_positive(self, real_manifest):
        """Gas budget is strictly validated."""
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "gas_budget": 0,
                }
            )
        assert "gas_budget" in str(exc.value)

    def test_max_errors_must_be_positive(self, real_manifest):
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "max_errors": 0,
                }
            )
        assert "max_errors" in str(exc.value)

    def test_max_run_seconds_must_be_at_least_one_if_provided(self, real_manifest):
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "max_run_seconds": 0.5,
                }
            )
        assert "max_run_seconds" in str(exc.value)

    def test_max_planning_calls_must_be_positive(self, real_manifest):
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "max_planning_calls": 0,
                }
            )
        assert "range_validation" in str(exc.value)

    def test_checkpoint_every_must_be_positive(self, real_manifest):
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "checkpoint_every": 0,
                }
            )
        assert "range_validation" in str(exc.value)

    def test_max_heuristic_variants_must_be_at_least_one(self, real_manifest):
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "max_heuristic_variants": 0,
                }
            )
        assert "range_validation" in str(exc.value)

    def test_baseline_max_candidates_must_be_at_least_one(self, real_manifest):
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "baseline_max_candidates": 0,
                }
            )
        assert "range_validation" in str(exc.value)

    def test_null_byte_rejection(self, real_manifest):
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp\0",
                    "package_ids_file": real_manifest,
                }
            )
        assert "null bytes" in str(exc.value)


class TestCrossFieldValidation:
    """Test cross-field validation rules."""

    def test_require_dry_run_needs_dry_run_mode(self, real_manifest):
        """require_dry_run can only be true with simulation_mode='dry-run'."""
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "require_dry_run": True,
                    "simulation_mode": "dev-inspect",
                    "sender": "0x123",  # required for dev-inspect
                }
            )
        assert "require_dry_run" in str(exc.value)
        assert "dry-run" in str(exc.value)

    def test_dev_inspect_requires_sender(self, real_manifest):
        """dev-inspect mode requires sender to be set."""
        with pytest.raises(InvalidConfigError) as exc:
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": real_manifest,
                    "simulation_mode": "dev-inspect",
                    # sender missing
                }
            )
        assert "sender" in str(exc.value)

    def test_dry_run_does_not_require_sender(self, real_manifest):
        """dry-run mode works without sender."""
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp",
                "package_ids_file": real_manifest,
                "simulation_mode": "dry-run",
            }
        )
        assert cfg.sender is None

    def test_build_only_does_not_require_sender(self, real_manifest):
        """build-only mode works without sender."""
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp",
                "package_ids_file": real_manifest,
                "simulation_mode": "build-only",
            }
        )
        assert cfg.sender is None


class TestUnknownFieldDetection:
    """Test detection of unknown config fields."""

    def test_detect_unknown_fields(self):
        unknown = _detect_unknown_fields(
            {
                "corpus_root": "/tmp",
                "typo_field": 123,
                "another_bad": "value",
            }
        )
        assert "typo_field" in unknown
        assert "another_bad" in unknown
        assert "corpus_root" not in unknown

    def test_no_unknown_fields(self, real_manifest):
        unknown = _detect_unknown_fields(
            {
                "corpus_root": "/tmp",
                "package_ids_file": real_manifest,
                "samples": 10,
            }
        )
        assert unknown == []

    def test_known_fields_constant_covers_all(self):
        """Verify KNOWN_CONFIG_FIELDS includes all EvalConfig fields."""
        from dataclasses import fields

        evalconfig_fields = {f.name for f in fields(EvalConfig)}

        # All EvalConfig fields should be in KNOWN_CONFIG_FIELDS
        for field in evalconfig_fields:
            assert field in KNOWN_CONFIG_FIELDS, f"Missing field: {field}"


class TestValidateConfigDryRun:
    """Test the /validate endpoint logic."""

    def test_valid_config_returns_valid_true(self, real_manifest):
        result = _validate_config_dry_run(
            {
                "corpus_root": "/tmp/corpus",
                "package_ids_file": real_manifest,
            }
        )
        assert result["valid"] is True
        assert "error" not in result
        assert "config" in result

    def test_invalid_config_returns_valid_false(self):
        result = _validate_config_dry_run(
            {
                "corpus_root": "",  # empty = invalid
                "package_ids_file": "/tmp/ids.txt",
            }
        )
        assert result["valid"] is False
        assert "error" in result

    def test_unknown_fields_produce_warnings(self, real_manifest):
        result = _validate_config_dry_run(
            {
                "corpus_root": "/tmp/corpus",
                "package_ids_file": real_manifest,
                "typo_field": 123,
            }
        )
        assert result["valid"] is True
        assert len(result["warnings"]) > 0
        assert "typo_field" in result["warnings"][0]

    def test_non_dict_config_returns_error(self):
        result = _validate_config_dry_run("not a dict")
        assert result["valid"] is False
        assert "must be a JSON object" in result["error"]


class TestConfigSchema:
    """Test JSON Schema generation."""

    def test_schema_has_required_fields(self):
        schema = _get_config_schema()
        assert "corpus_root" in schema["required"]
        assert "package_ids_file" in schema["required"]

    def test_schema_has_all_properties(self):
        schema = _get_config_schema()
        props = schema["properties"]

        # Core fields
        assert "corpus_root" in props
        assert "package_ids_file" in props
        assert "samples" in props
        assert "agent" in props

        # P0 fields
        assert "seed" in props
        assert "sender" in props
        assert "gas_budget" in props
        assert "gas_coin" in props
        assert "gas_budget_ladder" in props
        assert "max_errors" in props
        assert "max_run_seconds" in props

        # P1 fields
        assert "max_planning_calls" in props
        assert "checkpoint_every" in props
        assert "max_heuristic_variants" in props
        assert "baseline_max_candidates" in props
        assert "include_created_types" in props
        assert "require_dry_run" in props

    def test_schema_disallows_additional_properties(self):
        schema = _get_config_schema()
        assert schema["additionalProperties"] is False


class TestTypeCoercion:
    """Test type coercion helpers."""

    def test_safe_bool_true_values(self):
        assert safe_bool(True, False) is True
        assert safe_bool("true", False) is True
        assert safe_bool("True", False) is True
        assert safe_bool("TRUE", False) is True
        assert safe_bool("1", False) is True
        assert safe_bool("yes", False) is True
        assert safe_bool("YES", False) is True

    def test_safe_bool_false_values(self):
        assert safe_bool(False, True) is False
        assert safe_bool("false", True) is False
        assert safe_bool("0", True) is False
        assert safe_bool("no", True) is False

    def test_safe_bool_none_uses_default(self):
        assert safe_bool(None, True) is True
        assert safe_bool(None, False) is False

    def test_safe_int_with_strings(self):
        assert _safe_int("42", 0) == 42
        assert _safe_int("invalid", 99) == 99

    def test_safe_float_with_strings(self):
        assert _safe_float("3.14", 0.0) == 3.14
        assert _safe_float("invalid", 1.5) == 1.5


class TestPropertyBasedConfigParsing:
    """Property-based tests for config parsing robustness."""

    def setup_method(self):
        """Create a real manifest file for Hypothesis tests."""
        self.manifest_dir = Path("/tmp/smi_test_manifests")
        self.manifest_dir.mkdir(parents=True, exist_ok=True)
        self.manifest = self.manifest_dir / "ids.txt"
        self.manifest.write_text("0x1\n")

    @given(st.integers(min_value=0, max_value=1000))
    @settings(max_examples=50)
    def test_valid_seed_always_parses(self, seed: int):
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp",
                "package_ids_file": str(self.manifest),
                "seed": seed,
            }
        )
        assert cfg.seed == seed

    @given(st.integers(min_value=1_000_000, max_value=100_000_000))
    @settings(max_examples=50)
    def test_valid_gas_budget_always_parses(self, gas_budget: int):
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp",
                "package_ids_file": str(self.manifest),
                "gas_budget": gas_budget,
            }
        )
        assert cfg.gas_budget == gas_budget

    @given(st.floats(min_value=1.0, max_value=86400.0, allow_nan=False, allow_infinity=False))
    @settings(max_examples=50)
    def test_valid_max_run_seconds_always_parses(self, max_run_seconds: float):
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp",
                "package_ids_file": str(self.manifest),
                "max_run_seconds": max_run_seconds,
            }
        )
        assert abs(cfg.max_run_seconds - max_run_seconds) < 0.0001

    def test_max_run_seconds_clamping(self):
        """Test max_run_seconds below 1.0 clamping behavior.

        Though _load_cfg currently uses validate_range which errors.
        """
        # Note: _load_cfg uses validate_range for max_run_seconds, so it errors.
        # This test verifies our understanding of which fields error vs clamp.
        with pytest.raises(InvalidConfigError):
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": str(self.manifest),
                    "max_run_seconds": 0.5,
                }
            )

    @given(st.integers(max_value=-1))
    @settings(max_examples=20)
    def test_negative_seed_clamped(self, seed: int):
        cfg = _load_cfg(
            {
                "corpus_root": "/tmp",
                "package_ids_file": str(self.manifest),
                "seed": seed,
            }
        )
        assert cfg.seed == 0

    @given(st.integers(max_value=0))
    @settings(max_examples=20)
    def test_non_positive_gas_budget_always_rejected(self, gas_budget: int):
        with pytest.raises(InvalidConfigError):
            _load_cfg(
                {
                    "corpus_root": "/tmp",
                    "package_ids_file": str(self.manifest),
                    "gas_budget": gas_budget,
                }
            )

    @given(st.text().map(lambda x: x + "\0"))
    @settings(max_examples=10)
    def test_path_null_bytes_always_rejected(self, bad_path: str):
        with pytest.raises(InvalidConfigError):
            _load_cfg(
                {
                    "corpus_root": bad_path,
                    "package_ids_file": str(self.manifest),
                }
            )
