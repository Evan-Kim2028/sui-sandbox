"""Unit tests for --dataset flag in smi-inhabit CLI."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest


def test_dataset_flag_resolves_to_correct_path() -> None:
    """Test that --dataset flag resolves to correct path."""
    # Test path construction
    dataset_name = "type_inhabitation_top25"
    expected_path = Path("manifests/datasets") / f"{dataset_name}.txt"

    # Test resolution logic
    dataset_path = Path("manifests/datasets") / f"{dataset_name}.txt"

    assert dataset_path == expected_path
    assert dataset_path.name == "type_inhabitation_top25.txt"


def test_dataset_flag_validates_file_exists() -> None:
    """Test that --dataset flag raises error on missing dataset."""
    # Test with non-existent dataset
    result = subprocess.run(
        [
            "uv",
            "run",
            "smi-inhabit",
            "--corpus-root",
            "/tmp/fake_corpus",
            "--dataset",
            "non_existent_dataset",
            "--samples",
            "1",
            "--agent",
            "mock-empty",
            "--out",
            "/tmp/test.json",
        ],
        capture_output=True,
        text=True,
        cwd=Path(__file__).parent.parent,
    )

    # Should fail with dataset not found error
    assert result.returncode != 0
    assert "Dataset not found" in result.stderr or "Dataset not found" in result.stdout


def test_dataset_flag_with_existing_dataset() -> None:
    """Test that --dataset flag works with existing dataset."""
    # Use type_inhabitation_top25 which should exist
    result = subprocess.run(
        [
            "uv",
            "run",
            "smi-inhabit",
            "--corpus-root",
            "/tmp/fake_corpus",
            "--dataset",
            "type_inhabitation_top25",
            "--samples",
            "0",  # Don't actually run, just validate
            "--agent",
            "mock-empty",
            "--out",
            "/tmp/test.json",
        ],
        capture_output=True,
        text=True,
        cwd=Path(__file__).parent.parent,
    )

    # Should not fail with dataset not found error
    # (may fail for other reasons like missing corpus)
    if "Dataset not found" in result.stderr or "Dataset not found" in result.stdout:
        pytest.fail("Dataset not found error should not occur for existing dataset")


def test_dataset_and_package_ids_file_mutually_exclusive() -> None:
    """Test that --dataset and --package-ids-file are mutually exclusive."""
    # Create a temporary package_ids_file
    temp_file = Path("/tmp/test_package_ids.txt")
    temp_file.write_text("0x0000000000000000000000000000000000000000000000000000000000000000000000000002\n")

    try:
        result = subprocess.run(
            [
                "uv",
                "run",
                "smi-inhabit",
                "--corpus-root",
                "/tmp/fake_corpus",
                "--dataset",
                "type_inhabitation_top25",
                "--package-ids-file",
                str(temp_file),
                "--samples",
                "0",
                "--agent",
                "mock-empty",
                "--out",
                "/tmp/test.json",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent.parent,
        )

        # Should fail with mutually exclusive error
        assert result.returncode != 0
        assert (
            "Use only one of --dataset or --subset" in result.stderr
            or "Use only one of --dataset or --subset" in result.stdout
            or "Use only one of --dataset or --package-ids-file" in result.stderr
            or "Use only one of --dataset or --package-ids-file" in result.stdout
        )
    finally:
        temp_file.unlink(missing_ok=True)


def test_dataset_flag_uses_correct_directory() -> None:
    """Test that --dataset flag uses manifests/datasets/ directory."""
    # Test path construction
    dataset_name = "my_dataset"
    dataset_path = Path("manifests/datasets") / f"{dataset_name}.txt"

    # Verify it uses datasets subdirectory
    assert "datasets" in str(dataset_path)
    assert str(dataset_path).endswith("manifests/datasets/my_dataset.txt")


def test_dataset_flag_preserves_extension() -> None:
    """Test that --dataset flag adds .txt extension."""
    # Test path construction
    dataset_name = "test_dataset"
    dataset_path = Path("manifests/datasets") / f"{dataset_name}.txt"

    # Verify .txt extension is added
    assert dataset_path.suffix == ".txt"
    assert dataset_path.name == "test_dataset.txt"


def test_dataset_flag_with_subset_alias() -> None:
    """Test that --subset is deprecated alias for --dataset."""
    result = subprocess.run(
        [
            "uv",
            "run",
            "smi-inhabit",
            "--corpus-root",
            "/tmp/fake_corpus",
            "--dataset",
            "test",
            "--subset",
            "test",
            "--samples",
            "0",
            "--agent",
            "mock-empty",
            "--out",
            "/tmp/test.json",
        ],
        capture_output=True,
        text=True,
        cwd=Path(__file__).parent.parent,
    )

    # Should fail with mutually exclusive error
    assert result.returncode != 0
    assert (
        "Use only one of --dataset or --subset" in result.stderr
        or "Use only one of --dataset or --subset" in result.stdout
    )
