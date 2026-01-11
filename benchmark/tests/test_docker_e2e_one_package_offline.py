from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest

ROOT_DIR = Path(__file__).resolve().parents[2]


def is_docker_available() -> bool:
    try:
        subprocess.run(["docker", "--version"], capture_output=True, check=True)
        return True
    except Exception:
        return False


@pytest.mark.docker
def test_docker_can_run_e2e_one_package_offline_and_persist_artifacts(tmp_path: Path) -> None:
    if not is_docker_available():
        pytest.skip("Docker not available")

    # Ensure we don't require external network calls for the test.
    env = os.environ.copy()
    env.pop("SMI_E2E_REAL_LLM", None)

    out_dir = tmp_path / "docker_results"
    out_dir.mkdir(parents=True, exist_ok=True)

    # Run the E2E script inside the built image without publishing ports.
    # Mount repo into container so it uses the same local corpus/test fixtures.
    cmd = [
        "docker",
        "run",
        "--rm",
        "--entrypoint",
        "/bin/bash",
        "-e",
        "SMI_E2E_REAL_LLM=0",
        "-e",
        "UV_PROJECT_ENVIRONMENT=/tmp/venv",
        "-v",
        f"{ROOT_DIR}:/app",
        "-w",
        "/app/benchmark",
        "sui-move-interface-extractor-smi-bench:latest",
        "-lc",
        "uv run --no-cache python3 scripts/e2e_one_package.py "
        "--corpus-root tests/fake_corpus "
        "--package-id 0x1 --samples 1 "
        "--out-dir /app/results/docker_e2e "
        "--persist-tmp-dir /tmp/smi_bench "
        "--per-package-timeout-seconds 120",
    ]

    # Ensure image exists (compose build must have been run by docker tests).
    proc = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=600)
    assert proc.returncode == 0, f"stdout={proc.stdout}\nstderr={proc.stderr}"

    # Verify artifacts persisted back to host under results/docker_e2e.
    host_out = ROOT_DIR / "results" / "docker_e2e"
    runs = [p for p in host_out.glob("e2e_*") if p.is_dir()]
    assert runs, f"expected e2e_* run dir under {host_out}"
    run_dir = max(runs, key=lambda p: p.stat().st_mtime)

    report = json.loads((run_dir / "validation_report.json").read_text(encoding="utf-8"))
    assert report.get("ok") is True
    assert (run_dir / "helper_pkg" / "Move.toml").exists()
    assert (run_dir / "mm2_mapping.json").exists()
    assert (run_dir / "txsim_source.json").exists()

    # tmp persistence: should include benchmark-local logs if present
    assert (run_dir / "persisted_tmp").exists()
