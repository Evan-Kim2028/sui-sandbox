"""Property-based tests for A2A configuration and utilities.

These tests use Hypothesis to generate wide ranges of inputs to verify
the robustness of configuration parsing and safe conversion utilities.
"""

from __future__ import annotations

import sys
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from smi_bench.a2a_errors import InvalidConfigError
from smi_bench.a2a_green_agent import EvalConfig, _load_cfg
from smi_bench.utils import safe_parse_float as _safe_float
from smi_bench.utils import safe_parse_int as _safe_int


class TestConfigPropertyBased:
    """Property-based tests for configuration loading."""

    def setup_method(self):
        """Create a real manifest file for Hypothesis tests."""
        from pathlib import Path

        self.manifest_dir = Path("/tmp/smi_test_manifests_a2a")
        self.manifest_dir.mkdir(parents=True, exist_ok=True)
        self.manifest = self.manifest_dir / "ids.txt"
        self.manifest.write_text("0x1\n")

    # Strategy for a valid Sui address (0x + 64 hex chars)
    sui_address_strategy = st.from_regex(r"^0x[0-9a-fA-F]{64}$")

    @given(
        st.fixed_dictionaries(
            {
                "corpus_root": st.text(min_size=1).filter(lambda x: x.strip() != "" and "\0" not in x),
                "samples": st.integers(min_value=0, max_value=10000),
                "rpc_url": st.text(),
                "simulation_mode": st.sampled_from(["dry-run", "build-only", "dev-inspect", "execute"]),
                "per_package_timeout_seconds": st.floats(
                    min_value=1.0, max_value=3600, allow_nan=False, allow_infinity=False
                ),
                "max_plan_attempts": st.integers(min_value=1, max_value=100),
                "continue_on_error": st.booleans(),
                "resume": st.booleans(),
                "run_id": st.one_of(st.none(), st.text()),
                "sender": st.one_of(st.none(), sui_address_strategy),
            }
        ).filter(lambda cfg: not (cfg["simulation_mode"] in ("dev-inspect", "execute") and not cfg.get("sender")))
    )
    def test_valid_configs_always_parse(self, config_dict: dict[str, Any]) -> None:
        """Valid configurations should always parse successfully."""
        # Inject the real manifest path
        config_dict["package_ids_file"] = str(self.manifest)
        result = _load_cfg(config_dict)

        assert isinstance(result, EvalConfig)
        assert result.corpus_root == config_dict["corpus_root"]
        assert result.package_ids_file == config_dict["package_ids_file"]
        assert result.samples == config_dict["samples"]
        assert result.continue_on_error == config_dict["continue_on_error"]
        assert result.resume == config_dict["resume"]

    @given(
        invalid_field=st.sampled_from(["corpus_root", "package_ids_file"]),
    )
    def test_missing_required_fields_raise_error(self, invalid_field) -> None:
        """Missing required fields should raise InvalidConfigError."""
        config = {
            "corpus_root": "/test/corpus",
            "package_ids_file": "manifest.txt",
        }
        config.pop(invalid_field)

        with pytest.raises(InvalidConfigError) as exc_info:
            _load_cfg(config)

        assert exc_info.value.data["field"] == invalid_field

    @given(
        raw_config=st.one_of(
            st.text(),
            st.integers(),
            st.lists(st.text()),
            st.none(),
        )
    )
    def test_non_dict_config_raises_error(self, raw_config) -> None:
        """Providing a non-dictionary as config should raise InvalidConfigError."""
        with pytest.raises(InvalidConfigError) as exc_info:
            _load_cfg(raw_config)

        assert exc_info.value.data["field"] == "config"


class TestSafeConversionsPropertyBased:
    """Property-based tests for safe conversion utilities."""

    @given(
        value=st.integers(min_value=-sys.maxsize, max_value=sys.maxsize),
        default=st.integers(),
    )
    def test_safe_int_with_integers(self, value, default) -> None:
        """_safe_int should return the integer itself if within range."""
        assert _safe_int(value, default) == value

    def test_safe_int_clamping(self):
        """Verify that safe_int clamps to max_val."""
        large_val = sys.maxsize + 100
        assert _safe_int(large_val, 0) == sys.maxsize

    @given(
        value=st.integers(min_value=-sys.maxsize, max_value=sys.maxsize).map(str),
        default=st.integers(),
    )
    def test_safe_int_with_numeric_strings(self, value, default) -> None:
        """_safe_int should correctly parse numeric strings."""
        assert _safe_int(value, default) == int(value)

    @given(
        value=st.text().filter(lambda x: not x.strip().isdigit() and not (x.startswith("-") and x[1:].isdigit())),
        default=st.integers(),
    )
    def test_safe_int_with_invalid_strings_returns_default(self, value, default) -> None:
        """_safe_int should return default for non-numeric strings."""
        assert _safe_int(value, default) == default

    @given(
        value=st.floats(allow_nan=False, allow_infinity=False),
        default=st.floats(allow_nan=False, allow_infinity=False),
    )
    def test_safe_float_with_floats(self, value, default) -> None:
        """_safe_float should return the float itself if provided."""
        assert _safe_float(value, default) == value

    @given(
        value=st.floats(allow_nan=False, allow_infinity=False).map(str),
        default=st.floats(allow_nan=False, allow_infinity=False),
    )
    def test_safe_float_with_numeric_strings(self, value, default) -> None:
        """_safe_float should correctly parse numeric strings."""
        assert _safe_float(value, default) == float(value)

    @given(
        value=st.text().filter(lambda x: x.strip() == "" or (not x.replace(".", "", 1).replace("-", "", 1).isdigit())),
        default=st.floats(allow_nan=False, allow_infinity=False),
    )
    def test_safe_float_with_invalid_strings_returns_default(self, value, default) -> None:
        """_safe_float should return default for non-numeric strings."""
        # Some strings might still be valid floats (e.g. "1e10"), so we filter carefully
        try:
            float(value)
            # If it's a valid float, skipping this case
            return
        except ValueError:
            assert _safe_float(value, default) == default
