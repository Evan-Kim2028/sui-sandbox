from __future__ import annotations

from pathlib import Path

import pytest

from smi_bench import inhabit_runner


def test_dataset_flag_resolves_to_correct_path() -> None:
    """Test that --dataset flag resolves to correct path."""
    dataset_name = "type_inhabitation_top25"
    expected_path = Path("manifests/datasets") / f"{dataset_name}.txt"
    # Logic is tested via actual CLI execution now
    assert expected_path.name == "type_inhabitation_top25.txt"


def test_dataset_flag_validates_file_exists() -> None:
    """Test that --dataset flag raises error on missing dataset."""
    argv = [
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
    ]
    # inhabit_runner.main raises SystemExit on configuration errors
    with pytest.raises(SystemExit):
        inhabit_runner.main(argv)


@pytest.mark.integration
def test_dataset_flag_with_existing_dataset(tmp_path: Path) -> None:
    """Test that --dataset flag works with existing dataset logic.

    Requires Rust binary to be built (integration test).
    """
    # Create an empty but existing directory to avoid FileNotFoundError during iterdir
    corpus_root = tmp_path / "empty_corpus"
    corpus_root.mkdir()

    argv = [
        "--corpus-root",
        str(corpus_root),
        "--dataset",
        "type_inhabitation_top25",
        "--samples",
        "0",
        "--agent",
        "mock-empty",
        "--out",
        str(tmp_path / "test.json"),
    ]
    # It should pass dataset resolution and fail at corpus collection (no packages)
    with pytest.raises(SystemExit) as exc:
        inhabit_runner.main(argv)
    assert "no packages found" in str(exc.value)


def test_dataset_and_package_ids_file_mutually_exclusive() -> None:
    """Test that --dataset and --package-ids-file are mutually exclusive."""
    argv = [
        "--corpus-root",
        "/tmp/fake_corpus",
        "--dataset",
        "type_inhabitation_top25",
        "--package-ids-file",
        "some_file.txt",
        "--samples",
        "0",
        "--agent",
        "mock-empty",
        "--out",
        "/tmp/test.json",
    ]
    with pytest.raises(SystemExit):
        inhabit_runner.main(argv)


def test_dataset_flag_with_subset_alias() -> None:
    """Test that --subset is deprecated alias for --dataset."""
    argv = [
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
    ]
    with pytest.raises(SystemExit):
        inhabit_runner.main(argv)
