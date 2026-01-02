"""Manifest filtering tests for smi-phase2-filter-manifest.

Tests cover:
- Filtering by min_targets threshold
- Boundary handling
- File creation and directory creation
- Error handling for malformed data
- Order preservation
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from smi_bench import manifest_filter


def test_main_filters_by_min_targets_threshold(tmp_path: Path) -> None:
    """Filtering logic keeps packages above threshold."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    # Create test data
    test_data = {
        "packages": [
            {"package_id": "0x1", "score": {"targets": 5}},
            {"package_id": "0x2", "score": {"targets": 2}},
            {"package_id": "0x3", "score": {"targets": 1}},
        ]
    }
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "2", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    # Verify only packages with targets >= 2 are kept
    result = out_manifest.read_text()
    lines = result.strip().split("\n")
    assert len(lines) == 2
    assert "0x1" in lines
    assert "0x2" in lines
    assert "0x3" not in lines


def test_main_skips_packages_below_threshold(tmp_path: Path) -> None:
    """Boundary handling skips packages exactly at threshold - 1."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    test_data = {
        "packages": [
            {"package_id": "0x1", "score": {"targets": 3}},
            {"package_id": "0x2", "score": {"targets": 2}},
            {"package_id": "0x3", "score": {"targets": 1}},
        ]
    }
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "2", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    result = out_manifest.read_text()
    lines = result.strip().split("\n")
    assert len(lines) == 2
    # Package with 1 target is skipped (below threshold)
    assert "0x3" not in lines


def test_main_creates_output_manifest_file(tmp_path: Path) -> None:
    """File creation logic writes output file."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    test_data = {"packages": [{"package_id": "0x1", "score": {"targets": 2}}]}
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "1", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    assert out_manifest.exists()
    assert out_manifest.is_file()


def test_main_missing_packages_array_raises_systemexit(tmp_path: Path) -> None:
    """Error handling for missing packages array."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    # Missing packages array (get returns None)
    test_data = {"schema_version": 1}
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "1", "--out-manifest", str(out_manifest)]

    with pytest.raises(SystemExit) as exc_info:
        manifest_filter.main(args)

    assert "invalid" in str(exc_info.value).lower()


def test_main_malformed_score_field_skips_package(tmp_path: Path) -> None:
    """Data validation skips packages with malformed score field."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    test_data = {
        "packages": [
            {"package_id": "0x1", "score": {"targets": 2}},
            {"package_id": "0x2", "score": "not_a_dict"},  # Malformed
            {"package_id": "0x3", "score": {"targets": 3}},
        ]
    }
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "1", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    result = out_manifest.read_text()
    # Should skip package 0x2 but keep 0x1 and 0x3
    assert "0x1" in result
    assert "0x3" in result
    assert "0x2" not in result


def test_main_empty_packages_list_writes_empty_manifest(tmp_path: Path) -> None:
    """Empty list handling writes empty manifest file."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    test_data = {"packages": []}
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "1", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    assert out_manifest.exists()
    result = out_manifest.read_text()
    # Empty manifest should have no content (or just newline)
    assert result == "" or result == "\n"


def test_main_creates_output_directory_if_missing(tmp_path: Path) -> None:
    """Directory creation logic creates parent directory if missing."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "nested" / "dir" / "filtered_manifest.txt"

    test_data = {"packages": [{"package_id": "0x1", "score": {"targets": 2}}]}
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "1", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    # Verify directory was created
    assert out_manifest.parent.exists()
    assert out_manifest.parent.is_dir()
    assert out_manifest.exists()


def test_main_preserves_order_of_filtered_packages(tmp_path: Path) -> None:
    """Order preservation maintains original package order."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    test_data = {
        "packages": [
            {"package_id": "0xaaa", "score": {"targets": 3}},
            {"package_id": "0xbbb", "score": {"targets": 2}},
            {"package_id": "0xccc", "score": {"targets": 1}},
            {"package_id": "0xddd", "score": {"targets": 4}},
        ]
    }
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "2", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    result = out_manifest.read_text()
    lines = result.strip().split("\n")

    # Verify order is preserved (aaa, bbb, ddd - ccc filtered out)
    assert lines[0] == "0xaaa"
    assert lines[1] == "0xbbb"
    assert lines[2] == "0xddd"
    assert len(lines) == 3


def test_main_malformed_package_id_skips_package(tmp_path: Path) -> None:
    """Data validation skips packages with malformed package_id."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    test_data = {
        "packages": [
            {"package_id": "0x1", "score": {"targets": 2}},
            {"package_id": "", "score": {"targets": 3}},  # Empty (skipped)
            {"package_id": None, "score": {"targets": 2}},  # None (skipped by dict.get returning None)
            {"package_id": "0x2", "score": {"targets": 4}},
        ]
    }
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "1", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    result = out_manifest.read_text()
    # Should skip empty and None package_ids
    # Note: package_id=None will be skipped because row.get("package_id") returns None
    # and the check `if not isinstance(pkg, str) or not pkg:` handles both None and ""
    assert "0x1" in result
    assert "0x2" in result
    # Count the lines (should be 2, not 4)
    lines = [line for line in result.strip().split("\n") if line]
    assert len(lines) == 2


def test_main_malformed_targets_skips_package(tmp_path: Path) -> None:
    """Data validation skips packages with malformed targets value."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    test_data = {
        "packages": [
            {"package_id": "0x1", "score": {"targets": 2}},
            {"package_id": "0x2", "score": {"targets": "not_an_int"}},  # Malformed
            {"package_id": "0x3", "score": {"targets": None}},  # None
        ]
    }
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "1", "--out-manifest", str(out_manifest)]
    manifest_filter.main(args)

    result = out_manifest.read_text()
    # Should skip packages with malformed targets
    assert "0x1" in result
    assert "0x2" not in result
    assert "0x3" not in result


def test_main_non_list_packages_raises_systemexit(tmp_path: Path) -> None:
    """Data validation raises SystemExit for non-list packages field."""
    out_json = tmp_path / "phase2_output.json"
    out_manifest = tmp_path / "filtered_manifest.txt"

    test_data = {"packages": "not_a_list"}  # Wrong type
    out_json.write_text(json.dumps(test_data))

    args = [str(out_json), "--min-targets", "1", "--out-manifest", str(out_manifest)]

    with pytest.raises(SystemExit) as exc_info:
        manifest_filter.main(args)

    assert "invalid" in str(exc_info.value).lower()
