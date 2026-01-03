"""
Integration tests for dataset CLI functionality."""

from __future__ import annotations

import json
import subprocess
import tempfile
import shutil
from pathlib import Path
from typing import Generator

import pytest


@pytest.fixture
def mock_corpus(tmp_path: Path) -> Generator[Path, None, None]:
    """Create a minimal mock corpus for testing."""
    corpus_root = tmp_path / "mock_corpus"

    # Create structure for a few packages
    # Use IDs that match what's in our dataset manifests
    pkg_ids = [
        "0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7",
        "0x2df868f30120484cc5e900c3b8b7a04561596cf15a9751159a207930471afff2",
        "0x059f94b85c07eb74d2847f8255d8cc0a67c9a8dcc039eabf9f8b9e23a0de2700",
        "0x0000000000000000000000000000000000000000000000000000000000000001",
    ]

    for pkg_id in pkg_ids:
        # Standard sui-packages structure: <corpus_root>/0x00/<pkgid>
        # Note: the extractor expects 0xXX/PKGNAMEOID format
        # Actually iter_package_dirs does: for prefix in corpus_root.iterdir() -> prefix.iterdir()
        prefix = "0x00"
        pkg_dir = corpus_root / prefix / pkg_id
        pkg_dir.mkdir(parents=True, exist_ok=True)
        (pkg_dir / "bytecode_modules").mkdir(exist_ok=True)
        (pkg_dir / "metadata.json").write_text(json.dumps({"id": pkg_id}))

    yield corpus_root


def test_type_inhabitation_top25_dataset_runs(mock_corpus: Path) -> None:
    """Test that type_inhabitation_top25 dataset runs with mock-empty agent."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)

    try:
        # Run benchmark with dataset
        result = subprocess.run(
            [
                "uv",
                "run",
                "smi-inhabit",
                "--dataset",
                "type_inhabitation_top25",
                "--samples",
                "1",
                "--agent",
                "mock-empty",
                "--corpus-root",
                str(mock_corpus),
                "--out",
                str(tmp_path),
                "--no-log",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent.parent,
        )

        # Check success
        assert result.returncode == 0, f"Benchmark failed: {result.stderr}\nSTDOUT: {result.stdout}"
        assert tmp_path.exists(), "Expected output JSON to exist"

        output_data = json.loads(tmp_path.read_text())
        assert "packages" in output_data, "Expected 'packages' in output"
        assert len(output_data["packages"]) == 1, "Expected 1 package in output"

    finally:
        tmp_path.unlink(missing_ok=True)


def test_packages_with_keys_dataset_runs(mock_corpus: Path) -> None:
    """Test that packages_with_keys dataset runs with mock-empty agent."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)

    try:
        # Run benchmark with dataset
        result = subprocess.run(
            [
                "uv",
                "run",
                "smi-inhabit",
                "--dataset",
                "packages_with_keys",
                "--samples",
                "1",
                "--agent",
                "mock-empty",
                "--corpus-root",
                str(mock_corpus),
                "--out",
                str(tmp_path),
                "--no-log",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent.parent,
        )

        # Check success
        assert result.returncode == 0, f"Benchmark failed: {result.stderr}"
        assert tmp_path.exists(), "Expected output JSON to exist"

    finally:
        tmp_path.unlink(missing_ok=True)


def test_dataset_with_real_agent_simulation(mock_corpus: Path) -> None:
    """Test that dataset works with real agent in simulation mode."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)

    try:
        # Run benchmark with baseline-search agent
        result = subprocess.run(
            [
                "uv",
                "run",
                "smi-inhabit",
                "--dataset",
                "type_inhabitation_top25",
                "--samples",
                "1",
                "--agent",
                "baseline-search",
                "--corpus-root",
                str(mock_corpus),
                "--out",
                str(tmp_path),
                "--no-log",
                "--simulation-mode",
                "build-only",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent.parent,
        )

        # Check success
        assert result.returncode == 0, f"Benchmark failed: {result.stderr}"
        assert tmp_path.exists(), "Expected output JSON to exist"

    finally:
        tmp_path.unlink(missing_ok=True)


def test_all_datasets_are_accessible() -> None:
    """Test that all datasets in manifests/datasets/ are accessible via --dataset flag."""
    datasets_dir = Path(__file__).parent.parent / "manifests" / "datasets"

    # Find all dataset files
    dataset_files = [f for f in datasets_dir.glob("*.txt") if f.is_file()]

    # Test each dataset can be accessed
    for dataset_file in dataset_files:
        dataset_name = dataset_file.stem  # Remove .txt extension

        # Test dataset has valid content
        content = dataset_file.read_text(encoding="utf-8").splitlines()
        package_lines = [line.strip() for line in content if line.strip() and not line.strip().startswith("#")]

        # Should have at least some packages
        assert len(package_lines) > 0, f"Dataset {dataset_name} has no packages"

        # All IDs should start with 0x
        assert all(line.startswith("0x") for line in package_lines), f"Dataset {dataset_name} has invalid package IDs"


def test_dataset_vs_package_ids_file_equivalence(mock_corpus: Path) -> None:
    """Test that --dataset and --package-ids-file produce equivalent results."""
    dataset_path = Path(__file__).parent.parent / "manifests" / "datasets" / "type_inhabitation_top25.txt"

    # Create temp output files
    with (
        tempfile.NamedTemporaryFile(suffix="_dataset.json", delete=False) as tmp1,
        tempfile.NamedTemporaryFile(suffix="_file.json", delete=False) as tmp2,
    ):
        dataset_out = Path(tmp1.name)
        file_out = Path(tmp2.name)

    try:
        # Run with --dataset
        result1 = subprocess.run(
            [
                "uv",
                "run",
                "smi-inhabit",
                "--dataset",
                "type_inhabitation_top25",
                "--samples",
                "1",
                "--agent",
                "mock-empty",
                "--corpus-root",
                str(mock_corpus),
                "--out",
                str(dataset_out),
                "--no-log",
                "--seed",
                "42",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent.parent,
        )

        assert result1.returncode == 0, f"Benchmark with --dataset failed: {result1.stderr}"

        # Run with --package-ids-file
        result2 = subprocess.run(
            [
                "uv",
                "run",
                "smi-inhabit",
                "--package-ids-file",
                str(dataset_path),
                "--samples",
                "1",
                "--agent",
                "mock-empty",
                "--corpus-root",
                str(mock_corpus),
                "--out",
                str(file_out),
                "--no-log",
                "--seed",
                "42",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent.parent,
        )

        assert result2.returncode == 0, f"Benchmark with --package-ids-file failed: {result2.stderr}"

        # Both outputs should exist
        assert dataset_out.exists(), "Expected output from --dataset"
        assert file_out.exists(), "Expected output from --package-ids-file"

        # Both should have processed same package
        data1 = json.loads(dataset_out.read_text())
        data2 = json.loads(file_out.read_text())

        assert len(data1["packages"]) == len(data2["packages"]), "Both should process the same number of packages"

    finally:
        dataset_out.unlink(missing_ok=True)
        file_out.unlink(missing_ok=True)
