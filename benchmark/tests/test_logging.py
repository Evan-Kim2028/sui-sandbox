from __future__ import annotations

import json
from pathlib import Path

from smi_bench.logging import JsonlLogger


def test_jsonl_logger_writes_files(tmp_path: Path) -> None:
    logger = JsonlLogger(base_dir=tmp_path, run_id="run1")
    logger.write_run_metadata({"a": 1})
    logger.event("run_started", x=1)
    logger.package_row({"package_id": "0x1"})

    meta = json.loads(logger.paths.run_metadata.read_text())
    assert meta["a"] == 1

    events = logger.paths.events.read_text().strip().splitlines()
    assert len(events) == 1
    e0 = json.loads(events[0])
    assert e0["event"] == "run_started"

    rows = logger.paths.packages.read_text().strip().splitlines()
    assert len(rows) == 1
    r0 = json.loads(rows[0])
    assert r0["package_id"] == "0x1"
