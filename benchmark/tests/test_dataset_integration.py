"""Integration tests for dataset CLI functionality (In-process).

These tests require the Rust binary to be built and are marked as integration tests.
Run with: pytest -m integration
"""

from __future__ import annotations

import json
import tempfile
from pathlib import Path

import pytest

from smi_bench import inhabit_runner

pytestmark = pytest.mark.integration


@pytest.fixture(scope="session")
def mock_corpus(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Create a minimal mock corpus for testing (Session scoped)."""
    tmp_dir = tmp_path_factory.mktemp("corpus_root")
    corpus_root = tmp_dir / "mock_corpus"

    # Use IDs that match what's in our dataset manifests
    pkg_ids = [
        "0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7",
        "0x2df868f30120484cc5e900c3b8b7a04561596cf15a9751159a207930471afff2",
        "0x059f94b85c07eb74d2847f8255d8cc0a67c9a8dcc039eabf9f8b9e23a0de2700",
        "0x0000000000000000000000000000000000000000000000000000000000000001",
    ]

    for pkg_id in pkg_ids:
        prefix = "0x00"
        pkg_dir = corpus_root / prefix / pkg_id
        pkg_dir.mkdir(parents=True, exist_ok=True)
        (pkg_dir / "bytecode_modules").mkdir(exist_ok=True)
        (pkg_dir / "metadata.json").write_text(json.dumps({"id": pkg_id}))

    return corpus_root


def test_type_inhabitation_top25_dataset_runs(mock_corpus: Path) -> None:
    """Test that type_inhabitation_top25 dataset runs with mock-empty agent."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)

    try:
        # Run benchmark in-process
        argv = [
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
        ]
        inhabit_runner.main(argv)

        assert tmp_path.exists(), "Expected output JSON to exist"
        output_data = json.loads(tmp_path.read_text())
        assert "packages" in output_data
        assert len(output_data["packages"]) == 1

    finally:
        tmp_path.unlink(missing_ok=True)


def test_packages_with_keys_dataset_runs(mock_corpus: Path) -> None:
    """Test that packages_with_keys dataset runs with mock-empty agent."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)

    try:
        argv = [
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
        ]
        inhabit_runner.main(argv)
        assert tmp_path.exists()
    finally:
        tmp_path.unlink(missing_ok=True)


def test_dataset_with_real_agent_simulation(mock_corpus: Path) -> None:
    """Test that dataset works with baseline-search in-process."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)

    try:
        argv = [
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
        ]
        inhabit_runner.main(argv)
        assert tmp_path.exists()
    finally:
        tmp_path.unlink(missing_ok=True)


def test_all_datasets_are_accessible() -> None:
    """Check that all datasets exist in manifests/."""
    # This remains a unit test on filesystem
    datasets_dir = Path(__file__).parent.parent / "manifests" / "datasets"
    dataset_files = [f for f in datasets_dir.glob("*.txt") if f.is_file()]
    for dataset_file in dataset_files:
        content = dataset_file.read_text(encoding="utf-8").splitlines()
        package_lines = [line.strip() for line in content if line.strip() and not line.strip().startswith("#")]
        assert len(package_lines) > 0
        assert all(line.startswith("0x") for line in package_lines)


def test_dataset_vs_package_ids_file_equivalence(mock_corpus: Path) -> None:
    """Test that --dataset and --package-ids-file produce equivalent results in-process."""
    dataset_path = Path(__file__).parent.parent / "manifests" / "datasets" / "type_inhabitation_top25.txt"

    with (
        tempfile.NamedTemporaryFile(suffix="_dataset.json", delete=False) as tmp1,
        tempfile.NamedTemporaryFile(suffix="_file.json", delete=False) as tmp2,
    ):
        dataset_out = Path(tmp1.name)
        file_out = Path(tmp2.name)

    try:
        # Run with --dataset
        inhabit_runner.main(
            [
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
            ]
        )

        # Run with --package-ids-file
        inhabit_runner.main(
            [
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
            ]
        )

        assert dataset_out.exists()
        assert file_out.exists()
        data1 = json.loads(dataset_out.read_text())
        data2 = json.loads(file_out.read_text())
        assert len(data1["packages"]) == len(data2["packages"])
    finally:
        dataset_out.unlink(missing_ok=True)
        file_out.unlink(missing_ok=True)
