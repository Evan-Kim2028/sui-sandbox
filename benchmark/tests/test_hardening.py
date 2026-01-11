import asyncio
import json

import pytest

from smi_bench.utils import (
    atomic_write_json,
    atomic_write_text,
    compute_json_checksum,
    managed_subprocess,
    safe_parse_float,
    safe_parse_int,
    safe_read_json,
    safe_read_text,
)


def test_safe_read_json(tmp_path):
    p = tmp_path / "test.json"
    p.write_text('{"a": 1}', encoding="utf-8")
    assert safe_read_json(p) == {"a": 1}

    p.write_text("invalid json", encoding="utf-8")
    assert safe_read_json(p) is None

    non_existent = tmp_path / "none.json"
    assert safe_read_json(non_existent) is None


def test_safe_read_text(tmp_path):
    p = tmp_path / "test.txt"
    p.write_text("hello", encoding="utf-8")
    assert safe_read_text(p) == "hello"

    non_existent = tmp_path / "none.txt"
    assert safe_read_text(non_existent) is None


def test_atomic_write(tmp_path):
    p = tmp_path / "atomic.txt"
    atomic_write_text(p, "content")
    assert p.read_text(encoding="utf-8") == "content"

    p_json = tmp_path / "atomic.json"
    atomic_write_json(p_json, {"b": 2})
    assert json.loads(p_json.read_text(encoding="utf-8")) == {"b": 2}


@pytest.mark.anyio
async def test_managed_subprocess():
    # Test successful completion
    async with managed_subprocess("echo", "hello", stdout=asyncio.subprocess.PIPE) as proc:
        stdout, _ = await proc.communicate()
        assert stdout.decode().strip() == "hello"
        assert proc.returncode == 0

    # Test cleanup on failure
    proc_ref = None
    try:
        async with managed_subprocess("sleep", "10") as proc:
            proc_ref = proc
            raise RuntimeError("test failure")
    except RuntimeError:
        pass

    # Wait a bit for cleanup
    await asyncio.sleep(0.1)
    assert proc_ref.returncode is not None


def test_safe_parsing():
    assert safe_parse_int("123", 0) == 123
    assert safe_parse_int("abc", 10) == 10
    assert safe_parse_int("-5", 0, min_val=0) == 0
    assert safe_parse_int("100", 0, max_val=50) == 50

    assert safe_parse_float("1.5", 0.0) == 1.5
    assert safe_parse_float("abc", 10.0) == 10.0
    assert safe_parse_float("-1.0", 0.0, min_val=0.0) == 0.0
    assert safe_parse_float("10.0", 0.0, max_val=5.0) == 5.0


def test_checksum_consistency():
    data = {"z": 1, "a": 2}
    c1 = compute_json_checksum(data)
    c2 = compute_json_checksum({"a": 2, "z": 1})
    assert c1 == c2
    assert len(c1) == 8


def test_run_id_uniqueness():
    from smi_bench.logging import default_run_id

    # Generate 1000 IDs and ensure very high uniqueness
    # With 24-bit random suffix, birthday paradox gives ~0.003% collision chance
    # at 1000 samples, so we allow up to 3 collisions to avoid flakiness
    ids = {default_run_id(prefix="test") for _ in range(1000)}
    assert len(ids) >= 997, f"Expected at least 997 unique IDs, got {len(ids)}"

    # Check format
    one_id = next(iter(ids))
    assert one_id.startswith("test_")
    assert "_pid" in one_id
    # Should have the 6-char random suffix at the end (total 3 bytes hex = 6 chars)
    assert len(one_id.split("_")[-1]) == 6
