"""Property-based tests for A2A configuration and utilities.

These tests use Hypothesis to generate wide ranges of inputs to verify
the robustness of configuration parsing and safe conversion utilities.
"""

from __future__ import annotations

import pytest
from hypothesis import given
from hypothesis import strategies as st

from smi_bench.a2a_errors import InvalidConfigError
from smi_bench.a2a_green_agent import EvalConfig, _load_cfg, _safe_float, _safe_int


class TestConfigPropertyBased:
    """Property-based tests for configuration loading."""

    @given(
        corpus_root=st.text(min_size=1).filter(lambda x: x.strip() != ""),
        package_ids_file=st.text(min_size=1).filter(lambda x: x.strip() != ""),
        samples=st.integers(min_value=0, max_value=10000),
        rpc_url=st.text(),
        simulation_mode=st.text(),
        per_package_timeout_seconds=st.floats(min_value=0, max_value=3600, allow_nan=False, allow_infinity=False),
        max_plan_attempts=st.integers(min_value=0, max_value=100),
        continue_on_error=st.booleans(),
        resume=st.booleans(),
        run_id=st.one_of(st.none(), st.text()),
    )
    def test_valid_configs_always_parse(
        self,
        corpus_root,
        package_ids_file,
        samples,
        rpc_url,
        simulation_mode,
        per_package_timeout_seconds,
        max_plan_attempts,
        continue_on_error,
        resume,
        run_id,
    ) -> None:
        """Valid configurations should always parse successfully."""
        config = {
            "corpus_root": corpus_root,
            "package_ids_file": package_ids_file,
            "samples": samples,
            "rpc_url": rpc_url,
            "simulation_mode": simulation_mode,
            "per_package_timeout_seconds": per_package_timeout_seconds,
            "max_plan_attempts": max_plan_attempts,
            "continue_on_error": continue_on_error,
            "resume": resume,
            "run_id": run_id,
        }

        result = _load_cfg(config)

        assert isinstance(result, EvalConfig)
        assert result.corpus_root == corpus_root
        assert result.package_ids_file == package_ids_file
        assert result.samples == samples
        assert result.continue_on_error == continue_on_error
        assert result.resume == resume

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
        value=st.integers(),
        default=st.integers(),
    )
    def test_safe_int_with_integers(self, value, default) -> None:
        """_safe_int should return the integer itself if provided."""
        assert _safe_int(value, default) == value

    @given(
        value=st.integers().map(str),
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
