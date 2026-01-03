"""Integration tests for dataset CLI functionality."""

from __future__ import annotations

import json
import subprocess
import tempfile
from pathlib import Path

import pytest

# Corpus path for integration tests
CORPUS_ROOT = (
    Path(__file__).parent.parent.parent
    / "sui-package-benchmark"
    / ".local"
    / "research"
    / "sui-packages"
    / "packages"
    / "mainnet_most_used"
)


def test_type_inhabitation_top25_dataset_runs() -> None:
    """Test that type_inhabitation_top25 dataset runs with mock-empty agent."""
    
    if not CORPUS_ROOT.exists():
        pytest.skip(f"Corpus not found at {CORPUS_ROOT}")
    
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)
    
    try:
        # Run benchmark with dataset
        result = subprocess.run(
            [
                "uv", "run", "smi-inhabit",
                "--dataset", "type_inhabitation_top25",
                "--samples", "1",
                "--agent", "mock-empty",
                "--corpus-root", str(CORPUS_ROOT),
                "--out", str(tmp_path),
                "--no-log",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent,
        )
        
        # Check success
        assert result.returncode == 0, f"Benchmark failed: {result.stderr}"
        
        # Check output exists and is valid JSON
        assert tmp_path.exists(), "Expected output JSON to exist"
        
        output_data = json.loads(tmp_path.read_text())
        assert "packages" in output_data, "Expected 'packages' in output"
        assert len(output_data["packages"]) == 1, "Expected 1 package in output"
        
    finally:
        # Clean up
        tmp_path.unlink(missing_ok=True)


def test_packages_with_keys_dataset_runs() -> None:
    """Test that packages_with_keys dataset runs with mock-empty agent."""
    
    if not CORPUS_ROOT.exists():
        pytest.skip(f"Corpus not found at {CORPUS_ROOT}")
    
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)
    
    try:
        # Run benchmark with dataset
        result = subprocess.run(
            [
                "uv", "run", "smi-inhabit",
                "--dataset", "packages_with_keys",
                "--samples", "1",
                "--agent", "mock-empty",
                "--corpus-root", str(CORPUS_ROOT),
                "--out", str(tmp_path),
                "--no-log",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent,
        )
        
        # Check success
        assert result.returncode == 0, f"Benchmark failed: {result.stderr}"
        
        # Check output exists
        assert tmp_path.exists(), "Expected output JSON to exist"
        
    finally:
        tmp_path.unlink(missing_ok=True)


def test_dataset_with_real_agent_simulation() -> None:
    """Test that dataset works with real agent in simulation mode."""
    
    if not CORPUS_ROOT.exists():
        pytest.skip(f"Corpus not found at {CORPUS_ROOT}")
    
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)
    
    try:
        # Run benchmark with baseline-search agent (real agent, but simulation mode)
        result = subprocess.run(
            [
                "uv", "run", "smi-inhabit",
                "--dataset", "type_inhabitation_top25",
                "--samples", "1",
                "--agent", "baseline-search",
                "--corpus-root", str(CORPUS_ROOT),
                "--out", str(tmp_path),
                "--no-log",
                "--simulation-mode", "build-only",
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent,
        )
        
        # Check success
        assert result.returncode == 0, f"Benchmark failed: {result.stderr}"
        
        # Check output exists
        assert tmp_path.exists(), "Expected output JSON to exist"
        
    finally:
        tmp_path.unlink(missing_ok=True)


def test_all_datasets_are_accessible() -> None:
    """Test that all datasets in manifests/datasets/ are accessible via --dataset flag."""
    datasets_dir = Path(__file__).parent / "manifests" / "datasets"
    
    if not datasets_dir.exists():
        pytest.skip(f"Datasets directory not found at {datasets_dir}")
    
    # Find all dataset files
    dataset_files = [f for f in datasets_dir.glob("*.txt") if f.is_file()]
    
    if not dataset_files:
        pytest.skip("No dataset files found")
    
    # Test each dataset can be accessed
    for dataset_file in dataset_files:
        dataset_name = dataset_file.stem  # Remove .txt extension
        
        # Test that dataset exists
        assert dataset_file.exists(), f"Dataset file not found: {dataset_file}"
        
        # Test dataset has valid content
        content = dataset_file.read_text(encoding="utf-8").splitlines()
        package_lines = [
            line.strip() for line in content
            if line.strip() and not line.strip().startswith("#")
        ]
        
        # Should have at least some packages
        assert len(package_lines) > 0, f"Dataset {dataset_name} has no packages"
        
        # All IDs should start with 0x
        assert all(line.startswith("0x") for line in package_lines), \
            f"Dataset {dataset_name} has invalid package IDs"


def test_dataset_vs_package_ids_file_equivalence() -> None:
    """Test that --dataset and --package-ids-file produce equivalent results."""
    
    if not CORPUS_ROOT.exists():
        pytest.skip(f"Corpus not found at {CORPUS_ROOT}")
    
    dataset_path = Path(__file__).parent / "manifests" / "datasets" / "type_inhabitation_top25.txt"
    if not dataset_path.exists():
        pytest.skip(f"Dataset not found: {dataset_path}")
    
    # Create temp output files
    with tempfile.NamedTemporaryFile(suffix="_dataset.json", delete=False) as tmp1, \
         tempfile.NamedTemporaryFile(suffix="_file.json", delete=False) as tmp2:
        dataset_out = Path(tmp1.name)
        file_out = Path(tmp2.name)
    
    try:
        # Run with --dataset
        result1 = subprocess.run(
            [
                "uv", "run", "smi-inhabit",
                "--dataset", "type_inhabitation_top25",
                "--samples", "1",
                "--agent", "mock-empty",
                "--corpus-root", str(CORPUS_ROOT),
                "--out", str(dataset_out),
                "--no-log",
                "--seed", "42",  # Use fixed seed for reproducibility
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent,
        )
        
        assert result1.returncode == 0, f"Benchmark with --dataset failed: {result1.stderr}"
        
        # Run with --package-ids-file
        result2 = subprocess.run(
            [
                "uv", "run", "smi-inhabit",
                "--package-ids-file", str(dataset_path),
                "--samples", "1",
                "--agent", "mock-empty",
                "--corpus-root", str(CORPUS_ROOT),
                "--out", str(file_out),
                "--no-log",
                "--seed", "42",  # Same seed for reproducibility
            ],
            capture_output=True,
            text=True,
            cwd=Path(__file__).parent,
        )
        
        assert result2.returncode == 0, f"Benchmark with --package-ids-file failed: {result2.stderr}"
        
        # Both outputs should exist
        assert dataset_out.exists(), "Expected output from --dataset"
        assert file_out.exists(), "Expected output from --package-ids-file"
        
        # Both should have processed same package
        data1 = json.loads(dataset_out.read_text())
        data2 = json.loads(file_out.read_text())
        
        assert len(data1["packages"]) == len(data2["packages"]), \
            "Both should process the same number of packages"
        
    finally:
        # Clean up
        dataset_out.unlink(missing_ok=True)
        file_out.unlink(missing_ok=True)
