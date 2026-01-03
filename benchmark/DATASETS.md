# Datasets Guide

This document provides a comprehensive guide for creating, testing, and integrating new datasets into the Sui Move Interface Extractor benchmark suite.

## Table of Contents

1. [Overview](#overview)
2. [Dataset File Format](#dataset-file-format)
3. [Creating a New Dataset](#creating-a-new-dataset)
4. [Dataset Generation Patterns](#dataset-generation-patterns)
5. [Testing Datasets](#testing-datasets)
6. [Documentation](#documentation)
7. [Integration Checklist](#integration-checklist)
8. [Common Pitfalls](#common-pitfalls)
9. [Examples](#examples)

---

## Overview

### What Are Datasets?

Datasets are curated lists of Sui Move package IDs used for benchmark evaluation. They enable:

- **Reproducible evaluations** - Same workload across different agents/versions
- **Targeted testing** - Focus on specific package characteristics
- **Efficient iteration** - Smaller subsets for faster development cycles
- **Specialized use cases** - Test specific scenarios (high complexity, edge cases, etc.)

### Dataset Locations

All dataset files are stored in:
```
benchmark/manifests/datasets/
```

### Existing Datasets

- `type_inhabitation_top25.txt` (25 packages) - Fast iteration subset for Type Inhabitation
- `packages_with_keys.txt` (variable count) - Packages with key structs
- `standard_phase2_benchmark.txt` (292 packages) - Primary Phase II benchmark
- `comparison_*.txt` (5-25 packages) - Quick test sets

---

## Dataset File Format

### Basic Format

Dataset files use a simple text format:

```
# <Purpose> Dataset (<n> packages)
# Generated: <ISO8601 timestamp>
0x<package_id_1>
0x<package_id_2>
0x<package_id_3>
...
```

### Specifications

1. **Header Comments** (Required)
   - Line 1: Purpose description with count
   - Line 2: Generation timestamp (ISO8601 format)
   - Lines starting with `#` are ignored by the loader

2. **Package IDs**
   - One ID per line (no empty lines)
   - Must start with `0x` prefix
   - No trailing whitespace
   - No duplicates

3. **Sorting**
   - Packages should be sorted by interest/score (most interesting first)
   - Alphabetical sorting is acceptable for filtered lists

### Example

```
# Type Inhabitation Top-25 Dataset
# Generated: 2026-01-03T15:20:09
0xc681beced336875c26f1410ee5549138425301b08725ee38e625544b9eaaade7
0x2df868f30120484cc5e900c3b8b7a04561596cf15a9751159a207930471afff2
0x2e6f45795a146e96f3249a2936b862564151999c360f3f0682c85fbab512c682
```

---

## Creating a New Dataset

### Step-by-Step Workflow

#### Step 1: Define Purpose and Scope

Before creating a dataset, define:

1. **Purpose**: What is this dataset for?
   - Examples: Fast iteration, specific package types, regression testing, etc.

2. **Target Size**: How many packages?
   - 5-25: Quick iteration, smoke tests
   - 50-100: Comprehensive subset, CI/CD
   - 200+: Full evaluation, production

3. **Selection Criteria**: How will packages be chosen?
   - Random sampling
   - Metric-based scoring (key structs, entry functions, etc.)
   - Filtering (e.g., min complexity, max complexity)
   - Manual curation

4. **Use Cases**: When should this dataset be used?
   - Agent development
   - CI/CD pipelines
   - Performance benchmarking
   - Debugging

#### Step 2: Choose Generation Method

**Option A: Manual Curation**
- Use for small datasets (<10 packages)
- Use when you want specific packages
- Use for smoke tests or regression testing

**Option B: Scripted Generation**
- Use for medium-to-large datasets (10+ packages)
- Use for metric-based selection
- Use when reproducibility is important

**Option C: Existing Dataset Filtering**
- Use when creating a subset of an existing dataset
- Use for progressive evaluation (small → medium → large)

#### Step 3: Create Dataset File

Place your dataset file at:
```
benchmark/manifests/datasets/<dataset_name>.txt
```

**Naming Convention:**
```
<purpose>_<size>_<optional_suffix>.txt
```

Examples:
- `type_inhabitation_top25.txt`
- `quick_smoke_5.txt`
- `high_complexity_50.txt`
- `regression_tests_10.txt`

#### Step 4: Add Tests

Create tests in `tests/test_datasets.py`:

```python
from pathlib import Path

def test_<dataset_name>_exists() -> None:
    """Test that dataset file exists and is valid."""
    p = Path("manifests/datasets/<dataset_name>.txt")
    assert p.exists(), f"Expected dataset file to exist: {p}"
    
    content = p.read_text(encoding="utf-8").strip().splitlines()
    
    # Check header comments (lines 1-2)
    assert any(line.strip().startswith("#") for line in content[:2]), \
        "Expected header comments with purpose and timestamp"
    
    # Extract package lines (skip comments)
    package_lines = [line.strip() for line in content 
                    if line.strip() and not line.strip().startswith("#")]
    
    # Check count
    expected_count = <n>  # e.g., 25
    assert len(package_lines) == expected_count, \
        f"Expected {expected_count} packages, got {len(package_lines)}"
    
    # Check format (0x prefix)
    assert all(line.startswith("0x") for line in package_lines), \
        "All package IDs must be 0x-prefixed"
    
    # Check no duplicates
    assert len(package_lines) == len(set(package_lines)), \
        "Expected no duplicate package IDs"
```

#### Step 5: Document in `manifests/README.md`

Add a section for your dataset:

```markdown
## `<dataset_name>.txt` (n=<count>)

**Purpose:** <One-sentence description>

### Why use this list?
<Bullet points explaining when to use this dataset>

**Selection Methodology:**
<How packages were selected, including any filters or scoring>

**Use Cases:**
<Specific scenarios where this dataset is appropriate>

### Usage
```bash
uv run smi-inhabit \
  --corpus-root ../../sui-packages/packages/mainnet_most_used \
  --dataset <dataset_name> \
  --agent <agent_name> \
  --out results/<run_name>.json
```

**Generation:**
```bash
cd benchmark
python scripts/generate_<dataset_name>.py
```
```

#### Step 6: Verify Integration

Test that your dataset works with the benchmark CLI:

```bash
cd benchmark
uv run smi-inhabit \
  --dataset <dataset_name> \
  --samples 1 \
  --agent mock-empty \
  --corpus-root <path/to/corpus> \
  --out /tmp/test_dataset.json
```

Expected output:
- No errors
- Progress bar completes
- Output JSON is generated
- Package from your dataset was processed

---

## Dataset Generation Patterns

### Pattern 1: Score-Based Selection (Python)

**Use case:** Select packages based on composite metrics (e.g., complexity, diversity)

**Example:** `scripts/generate_top25_dataset.py`

```python
#!/usr/bin/env python3
"""Generate top-<n> dataset based on composite score."""

import argparse
import json
from pathlib import Path
from typing import Any

def load_corpus_report(path: Path) -> dict[str, dict[str, Any]]:
    """Load corpus report JSONL into package_id -> data mapping."""
    packages: dict[str, dict[str, Any]] = {}
    for line in path.read_text(encoding="utf-8").strip().splitlines():
        if not line:
            continue
        data = json.loads(line)
        pkg_id = data.get("package_id")
        if isinstance(pkg_id, str) and pkg_id:
            packages[pkg_id] = data
    return packages

def compute_scores(packages: dict[str, dict[str, Any]]) -> list[tuple[str, float]]:
    """Compute composite scores for packages."""
    scored = []
    for pkg_id, data in packages.items():
        local = data.get("local", {})
        
        # Example metrics
        key_structs = local.get("key_structs", 0)
        entry_functions = local.get("entry_functions", 0)
        modules = local.get("modules", 0)
        
        # Composite score (customize weights as needed)
        score = 0.4 * key_structs + 0.3 * entry_functions + 0.3 * modules
        
        scored.append((pkg_id, score))
    
    # Sort by score (highest first)
    scored.sort(key=lambda t: t[1], reverse=True)
    return scored

def write_dataset(scored: list[tuple[str, float]], output_path: Path, n: int) -> None:
    """Write top-n packages to dataset file."""
    from datetime import datetime
    
    header = f"# <Purpose> Dataset ({n} packages)\n"
    header += f"# Generated: {datetime.now().isoformat()}\n"
    
    selected = scored[:n]
    content = header + "\n".join(pkg_id for pkg_id, _ in selected) + "\n"
    
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(content, encoding="utf-8")

def main() -> None:
    p = argparse.ArgumentParser(description="Generate top-n dataset")
    p.add_argument("--corpus-report", type=Path, required=True,
                   help="Path to corpus_report.jsonl")
    p.add_argument("--output-dataset", type=Path, required=True,
                   help="Output dataset path")
    p.add_argument("--count", type=int, default=25,
                   help="Number of packages to select")
    args = p.parse_args()
    
    packages = load_corpus_report(args.corpus_report)
    scored = compute_scores(packages)
    write_dataset(scored, args.output_dataset, args.count)

if __name__ == "__main__":
    main()
```

**Key components:**
1. Load corpus report JSONL
2. Compute scores with custom metrics
3. Sort by score (descending)
4. Write header + top-n package IDs

### Pattern 2: Filter-Based Selection (Python)

**Use case:** Filter packages from benchmark results or existing dataset

**Example:** `scripts/filter_packages_by_hit_rate.py`

```python
#!/usr/bin/env python3
"""Filter packages by minimum hit rate."""

import argparse
import json
from pathlib import Path

def main() -> None:
    p = argparse.ArgumentParser(description="Filter packages by hit rate")
    p.add_argument("results_json", type=Path,
                   help="Phase II results JSON")
    p.add_argument("--min-hits", type=int, default=1,
                   help="Minimum created_hits required")
    p.add_argument("--out", type=Path, required=True,
                   help="Output manifest path")
    args = p.parse_args()
    
    # Load results
    data = json.loads(args.results_json.read_text())
    pkgs = data.get("packages", [])
    
    # Filter packages
    out_ids = []
    for row in pkgs:
        pkg_id = row.get("package_id")
        score = row.get("score", {})
        hits = score.get("created_hits", 0)
        
        if isinstance(pkg_id, str) and isinstance(hits, int) and hits >= args.min_hits:
            out_ids.append(pkg_id)
    
    # Write manifest
    content = "".join(f"{pid}\n" for pid in out_ids)
    args.out.write_text(content)

if __name__ == "__main__":
    main()
```

**Key components:**
1. Load results JSON
2. Filter by metric threshold
3. Write package IDs (no header for pure filtered lists)

### Pattern 3: Subset from Existing Dataset (Python)

**Use case:** Create progressive subsets from a larger dataset

```python
#!/usr/bin/env python3
"""Create random subset from existing dataset."""

import argparse
from pathlib import Path

def main() -> None:
    p = argparse.ArgumentParser(description="Create subset from dataset")
    p.add_argument("--source-dataset", type=Path, required=True,
                   help="Source dataset file")
    p.add_argument("--output-dataset", type=Path, required=True,
                   help="Output dataset path")
    p.add_argument("--count", type=int, required=True,
                   help="Number of packages to select")
    p.add_argument("--seed", type=int, default=0,
                   help="Random seed for reproducibility")
    args = p.parse_args()
    
    # Load source dataset
    lines = args.source_dataset.read_text(encoding="utf-8").splitlines()
    
    # Separate header and package IDs
    header_lines = []
    package_ids = []
    
    for line in lines:
        if line.strip().startswith("#"):
            header_lines.append(line)
        elif line.strip():
            package_ids.append(line.strip())
    
    # Randomly sample (use seed for reproducibility)
    random.seed(args.seed)
    selected = random.sample(package_ids, min(args.count, len(package_ids)))
    
    # Write with original header (update count)
    output_header = []
    for h in header_lines:
        if "packages)" in h:
            # Update count in header
            output_header.append(h.replace(") packages)", f") {len(selected)} packages"))
        else:
            output_header.append(h)
    
    content = "\n".join(output_header) + "\n" + "\n".join(selected) + "\n"
    args.output_dataset.write_text(content, encoding="utf-8")

if __name__ == "__main__":
    import random
    main()
```

### Pattern 4: Simple Shell Script (Bash)

**Use case:** Simple filtering or concatenation of existing datasets

**Example:** `scripts/generate_dataset_packages_with_keys.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail

# Generate packages_with_keys dataset by scanning corpus

CORPUS_ROOT="${1:-../../sui-packages/packages/mainnet_most_used}"
OUT_JSON="results/datasets_scan.json"
OUT_DATASET="manifests/datasets/packages_with_keys.txt"

echo "Scanning corpus for key-struct targets..."
uv run smi-inhabit \
  --corpus-root "${CORPUS_ROOT}" \
  --samples 1000 \
  --seed 0 \
  --agent baseline-search \
  --simulation-mode build-only \
  --continue-on-error \
  --no-log \
  --out "${OUT_JSON}"

echo "Filtering dataset: targets >= 1"
uv run smi-phase2-filter-manifest "${OUT_JSON}" --min-hits 1 --out-manifest "${OUT_DATASET}"

echo "Wrote dataset: ${OUT_DATASET}"
```

**When to use Bash:**
- Simple workflows (scan + filter)
- Calling existing CLI tools
- No complex data manipulation
- Prefer Python for: scoring, filtering, data transformation

---

## Testing Datasets

### Test Categories

#### 1. Unit Tests (Format & Existence)

**Location:** `tests/test_datasets.py`

**Purpose:** Validate file format, count, and basic structure

**What to test:**
- File exists
- Header comments present
- Package count matches expected
- All IDs start with `0x`
- No duplicate IDs
- No trailing whitespace

**Example:**
```python
def test_my_dataset_exists() -> None:
    p = Path("manifests/datasets/my_dataset.txt")
    assert p.exists(), "Expected dataset file to exist"
    
    content = p.read_text(encoding="utf-8").strip().splitlines()
    package_lines = [line.strip() for line in content 
                    if line.strip() and not line.strip().startswith("#")]
    
    assert len(package_lines) == 50, "Expected 50 packages"
    assert all(line.startswith("0x") for line in package_lines)
    assert len(package_lines) == len(set(package_lines))
```

**Run:** `pytest tests/test_datasets.py::test_my_dataset_exists -v`

#### 2. Validation Tests (Corpus Membership)

**Location:** `tests/test_datasets.py`

**Purpose:** Ensure all packages exist in corpus and are valid

**What to test:**
- All packages exist in corpus directory structure
- All packages have valid metadata
- For Phase II datasets: all packages are inhabitable

**Example:**
```python
def test_my_dataset_packages_in_corpus() -> None:
    from smi_bench.dataset import collect_packages
    
    # Load dataset
    dataset_path = Path("manifests/datasets/my_dataset.txt")
    dataset_ids = set(
        line.strip() for line in dataset_path.read_text().splitlines()
        if line.strip() and not line.strip().startswith("#")
    )
    
    # Load corpus
    corpus_packages = collect_packages(Path("../sui-packages/packages/mainnet_most_used"))
    corpus_ids = {p.package_id for p in corpus_packages}
    
    # Check membership
    missing = dataset_ids - corpus_ids
    assert len(missing) == 0, f"Packages not in corpus: {missing}"
```

**Run:** `pytest tests/test_datasets.py::test_my_dataset_packages_in_corpus -v`

#### 3. Integration Tests (CLI Execution)

**Location:** `tests/test_dataset_integration.py`

**Purpose:** Verify dataset works with benchmark CLI end-to-end

**What to test:**
- `--dataset` flag resolves correctly
- Dataset loads without errors
- Benchmark runs successfully with dataset
- Output is valid JSON

**Example:**
```python
from pathlib import Path
import tempfile

def test_my_dataset_runs_with_mock_agent() -> None:
    """Test that dataset runs with mock-empty agent."""
    dataset_path = Path("manifests/datasets/my_dataset.txt")
    
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as tmp:
        tmp_path = Path(tmp.name)
    
    # Run benchmark
    result = subprocess.run(
        [
            "uv", "run", "smi-inhabit",
            "--dataset", "my_dataset",
            "--samples", "1",
            "--agent", "mock-empty",
            "--corpus-root", "../sui-packages/packages/mainnet_most_used",
            "--out", str(tmp_path),
            "--no-log",
        ],
        capture_output=True,
        text=True,
    )
    
    # Check success
    assert result.returncode == 0, f"Benchmark failed: {result.stderr}"
    
    # Check output exists
    assert tmp_path.exists(), "Expected output JSON to exist"
    
    # Clean up
    tmp_path.unlink()
```

**Run:** `pytest tests/test_dataset_integration.py::test_my_dataset_runs_with_mock_agent -v`

#### 4. CLI Unit Tests (Flag Resolution)

**Location:** `tests/test_inhabit_dataset_cli.py`

**Purpose:** Verify `--dataset` flag behavior

**What to test:**
- Flag resolves to correct path (`manifests/datasets/<name>.txt`)
- Error on non-existent dataset
- Mutually exclusive with `--package-ids-file`

**Example:**
```python
def test_dataset_flag_resolves_path() -> None:
    """Test that --dataset flag resolves to correct path."""
    from smi_bench.inhabit_runner import _resolve_dataset_path
    
    expected = Path("manifests/datasets/my_dataset.txt")
    actual = _resolve_dataset_path("my_dataset")
    
    assert actual == expected, f"Expected {expected}, got {actual}"

def test_dataset_flag_validates_file_exists() -> None:
    """Test that --dataset flag raises error on missing file."""
    from smi_bench.inhabit_runner import _resolve_dataset_path
    
    with pytest.raises(SystemExit, match="Dataset not found"):
        _resolve_dataset_path("non_existent_dataset")
```

**Run:** `pytest tests/test_inhabit_dataset_cli.py::test_dataset_flag_resolves_path -v`

### Test Coverage Requirements

All new datasets must have:

1. ✅ **Unit test** for format and count (`tests/test_datasets.py`)
2. ✅ **Validation test** for corpus membership (`tests/test_datasets.py`)
3. ✅ **Integration test** for CLI execution (`tests/test_dataset_integration.py`)
4. ✅ All tests passing

**Test all dataset tests:**
```bash
cd benchmark
pytest tests/test_datasets.py tests/test_dataset_integration.py tests/test_inhabit_dataset_cli.py -v
```

---

## Documentation

### Required Documentation

#### 1. Dataset File Documentation (`manifests/README.md`)

Add a section for your dataset following this template:

```markdown
## `<dataset_name>.txt` (n=<count>)

**Purpose:** <One-sentence description>

### Why use this list?
<2-3 bullet points explaining when to use this dataset>

**Selection Methodology:**
<How packages were selected, including any filters, scoring, or constraints>

**Use Cases:**
<3-4 bullet points with specific scenarios>

### Usage
```bash
uv run smi-inhabit \
  --corpus-root ../../sui-packages/packages/mainnet_most_used \
  --dataset <dataset_name> \
  --agent <agent_name> \
  --out results/<run_name>.json
```

**Generation:**
```bash
cd benchmark
python scripts/generate_<dataset_name>.py
```
```

#### 2. Generation Script Documentation

Add docstring and `--help` to your generation script:

```python
def main() -> None:
    """Generate <purpose> dataset.
    
    Selects packages based on <criteria> from corpus report.
    
    Usage:
        cd benchmark
        python scripts/generate_<dataset_name>.py
    """
    p = argparse.ArgumentParser(
        description="Generate <purpose> dataset"
    )
    p.add_argument("--corpus-report", type=Path, required=True,
                   help="Path to corpus_report.jsonl")
    p.add_argument("--output-dataset", type=Path, required=True,
                   help="Output dataset path")
    p.add_argument("--count", type=int, default=25,
                   help="Number of packages to select (default: 25)")
    args = p.parse_args()
    # ... rest of script
```

#### 3. Reference in Main Documentation

If dataset is widely used, add reference to:
- `benchmark/GETTING_STARTED.md` - Add to dataset examples
- `benchmark/docs/A2A_EXAMPLES.md` - Add to A2A usage examples
- Root `README.md` - Add to documentation map if major

---

## Integration Checklist

Before committing a new dataset, verify:

### File Structure
- [ ] Dataset file in `manifests/datasets/`
- [ ] Naming convention followed (`<purpose>_<size>_<suffix>.txt`)
- [ ] File tracked in git (not in `.gitignore`)

### File Format
- [ ] Header comments present (purpose + timestamp)
- [ ] Package IDs start with `0x`
- [ ] No duplicate IDs
- [ ] No trailing whitespace
- [ ] Sorted by interest/score (if applicable)

### Code Quality
- [ ] Generation script follows patterns (if scripted)
- [ ] Type hints on all functions
- [ ] Docstrings on all public functions
- [ ] `argparse` with `--help` documentation
- [ ] Error messages are clear and actionable

### Testing
- [ ] Unit test added to `tests/test_datasets.py`
- [ ] Validation test added (corpus membership)
- [ ] Integration test added to `tests/test_dataset_integration.py`
- [ ] CLI unit test added to `tests/test_inhabit_dataset_cli.py`
- [ ] All tests passing

### Documentation
- [ ] Section added to `manifests/README.md`
- [ ] Generation instructions included
- [ ] Usage examples provided
- [ ] Cross-references to related datasets

### Integration
- [ ] Works with `--dataset` flag
- [ ] Works with `--package-ids-file` flag
- [ ] Tested with mock-empty agent
- [ ] Tested with real agent (if appropriate)
- [ ] No errors in manual verification

### Performance
- [ ] Runtime documented (e.g., "runs in ~5 minutes")
- [ ] Timeout recommendations provided
- [ ] Appropriate for intended use case (fast iteration vs comprehensive)

---

## Common Pitfalls

### Pitfall 1: Duplicate Package IDs

**Problem:** Same package ID appears multiple times

**Solution:** Use set to deduplicate when generating:
```python
package_ids = list(set(package_ids))
```

**Detection:** Test checks for duplicates:
```python
assert len(package_ids) == len(set(package_ids))
```

### Pitfall 2: Missing 0x Prefix

**Problem:** Package IDs without `0x` prefix cause loading errors

**Solution:** Always include prefix when writing:
```python
package_id = pkg_id if pkg_id.startswith("0x") else f"0x{pkg_id}"
```

**Detection:** Test validates prefix:
```python
assert all(line.startswith("0x") for line in package_lines)
```

### Pitfall 3: Packages Not in Corpus

**Problem:** Dataset contains IDs that don't exist in corpus

**Solution:** Verify corpus membership:
```python
corpus_ids = {p.package_id for p in collect_packages(corpus_root)}
assert dataset_ids.issubset(corpus_ids)
```

**Detection:** Validation test checks membership:
```python
missing = dataset_ids - corpus_ids
assert len(missing) == 0, f"Packages not in corpus: {missing}"
```

### Pitfall 4: Wrong File Path

**Problem:** Using `--package-ids-file` with full path instead of `--dataset`

**Solution:** Use `--dataset` flag (shorter, validates existence):
```bash
# Wrong:
smi-inhabit --package-ids-file /full/path/to/manifests/datasets/my_dataset.txt

# Right:
smi-inhabit --dataset my_dataset
```

### Pitfall 5: No Header Comments

**Problem:** Dataset file lacks purpose and timestamp documentation

**Solution:** Always include header:
```python
from datetime import datetime
header = f"# <Purpose> Dataset ({len(selected)} packages)\n"
header += f"# Generated: {datetime.now().isoformat()}\n"
content = header + "\n".join(package_ids) + "\n"
```

### Pitfall 6: Trailing Whitespace

**Problem:** Empty lines or trailing spaces cause parsing issues

**Solution:** Strip whitespace when writing:
```python
lines = [line.strip() for line in content if line.strip()]
content = "\n".join(lines) + "\n"
```

### Pitfall 7: Non-Deterministic Selection

**Problem:** Random selection without seed produces different results each run

**Solution:** Always use seed for reproducibility:
```python
import random
random.seed(42)  # Fixed seed
selected = random.sample(packages, n)
```

### Pitfall 8: Overly Large Datasets

**Problem:** Dataset too large for intended use case

**Solution:** Define use case and size upfront:
- **Fast iteration:** 5-25 packages
- **CI/CD:** 25-50 packages
- **Development:** 50-100 packages
- **Comprehensive:** 100-292 packages

---

## Examples

### Example 1: Small Smoke Test Dataset

**Purpose:** Quick validation of changes

**Size:** 5 packages

**Selection:** Manual curation (simple, high-activity packages)

**Generation:** Manual file creation

```txt
# Smoke Test Dataset (5 packages)
# Generated: 2026-01-03T16:00:00
0x0000000000000000000000000000000000000000000000000000000000000000000002
0x0000000000000000000000000000000000000000000000000000000000000000000003
0x00db9a10bb9536ab367b7d1ffa404c1d6c55f009076df1139dc108dd86608bbe
0x059f94b85c07eb74d2847f8255d8cc0a67c9a8dcc039eabf9f8b9e23a0de2700
0x05f51d02fa049194239ffeac3e446a0020e7bbfc5d9149ff888366c24b2456b1
```

### Example 2: High Complexity Dataset

**Purpose:** Test agent on complex packages

**Size:** 50 packages

**Selection:** Score-based (high module count, high function count)

**Generation:** Python script

```python
# Select top 50 by (modules * functions_total)
for pkg_id, data in packages.items():
    local = data.get("local", {})
    score = local.get("modules", 0) * local.get("functions_total", 0)
    scored.append((pkg_id, score))

scored.sort(key=lambda t: t[1], reverse=True)
selected = [pkg_id for pkg_id, _ in scored[:50]]
```

### Example 3: Regression Test Dataset

**Purpose:** Validate fixes for known problematic packages

**Size:** 10 packages

**Selection:** Manual (packages that previously caused failures)

**Generation:** Manual file creation

**Use case:** Run after bug fixes to ensure no regressions

### Example 4: Random Sample Dataset

**Purpose:** Unbiased evaluation on representative sample

**Size:** 100 packages

**Selection:** Random sampling with fixed seed

**Generation:** Shell script or Python

```bash
# Random sample from standard_phase2_benchmark.txt
cat manifests/standard_phase2_benchmark.txt | \
  grep -v '^#' | \
  shuf --random-source=42 | \
  head -n 100 > \
  manifests/datasets/random_sample_100.txt
```

---

## Additional Resources

### Related Documentation
- `benchmark/manifests/README.md` - Dataset file reference
- `benchmark/GETTING_STARTED.md` - Benchmark usage guide
- `benchmark/docs/ARCHITECTURE.md` - Code architecture
- `benchmark/docs/TESTING.md` - Testing standards

### Scripts Reference
- `scripts/generate_top25_dataset.py` - Score-based selection example
- `scripts/filter_packages_by_hit_rate.py` - Filter-based selection example
- `scripts/generate_dataset_packages_with_keys.sh` - Shell script example
- `scripts/manifest_remaining_from_jsonl.py` - Manifest utilities

### Code Utilities
- `smi_bench/dataset.py` - Corpus utilities (`collect_packages`, `sample_packages`)
- `smi_bench/manifest_filter.py` - CLI tool for filtering manifests

---

## Quick Reference

### Dataset File Locations
```
benchmark/manifests/datasets/<dataset_name>.txt
```

### CLI Usage
```bash
smi-inhabit --dataset <dataset_name> --samples <n>
```

### Test Location
```
tests/test_datasets.py
```

### Documentation Location
```
benchmark/manifests/README.md
```

### Generation Script Location
```
benchmark/scripts/generate_<dataset_name>.py
```

---

**Last Updated:** 2026-01-03
**Maintainer:** Benchmark Team
