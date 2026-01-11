# Documentation Testing Standards

This document defines testing requirements and procedures for maintaining high-quality documentation in the Sui Move Interface Extractor project.

## Principles

All documentation must be:

1. **Executable** - Commands can be copy-pasted and work
2. **Verifiable** - Claims can be validated automatically
3. **Maintainable** - Changes propagate systematically
4. **Accessible** - Newcomers can learn quickly

## Testing Requirements

### Executable Examples

**Every code example must:**

- Be copy-paste executable from the repository root
- Use clearly marked placeholders: `<CORPUS_ROOT>`, `<PACKAGE_ID>`
- Work on macOS and Linux (primary platforms)
- Specify expected exit codes and outputs where relevant
- Include working directory context (e.g., `cd benchmark`)

**Example format:**
```markdown
```bash
cd benchmark
uv run smi-a2a-smoke \
  --corpus-root <CORPUS_ROOT> \
  --samples 1
```

Expected output:
```
valid
```
```

### Cross-Reference Validation

**Internal links:**

- All `[text](path.md)` links must resolve to existing files
- All `[text](#section)` anchors must exist in target document
- Use relative paths: `docs/A2A_EXAMPLES.md` not `../docs/A2A_EXAMPLES.md`
- Link text should be descriptive, not just "here"

**External links:**

- Prefer stable documentation (not blog posts or social media)
- Use permalinks when possible
- Check quarterly with automated tooling
- Avoid linking to specific GitHub commits (use tags or docs pages)

**Mermaid diagrams:**

- All diagrams in Markdown files must render correctly
- Node names should match code concepts
- No broken connections or undefined nodes

### Schema Synchronization

When `benchmark/docs/evaluation_bundle.schema.json` changes:

1. Update all documentation examples
2. Update `docs/A2A_EXAMPLES.md` reference payloads
3. Update `docs/ARCHITECTURE.md` invariants section
4. Add migration notes if breaking changes
5. Update all validation scripts if new fields added

**Example migration note:**
```markdown
### Schema v1 → v2 Migration

Added field: `metrics.run_metadata` (optional)
Removed field: `config.sender` (moved to run_metadata)

Impact: Old bundles still valid, new bundles recommended to include run_metadata.
```

### Command Verification

**All CLI commands in docs must:**

- Exist in `benchmark/pyproject.toml` `[project.scripts]`
- Use correct flag names (exact match to CLI definitions)
- Document default values accurately
- Use current API (no deprecated flags)

**Verification process:**
```bash
# Extract command name from doc
grep "smi-a2a-smoke" docs/*.md

# Verify it exists in pyproject.toml
grep "smi-a2a-smoke" benchmark/pyproject.toml
```

### Reliability Testing

**All reliability-critical code must have verification tests covering:**

- **Atomic File I/O**: Verify that partial writes do not occur and that `.tmp` files are cleaned up on failure.
- **Subprocess Lifecycle**: Verify that child processes are terminated when the parent is cancelled or crashes.
- **Exponential Backoff**: Verify that retry logic correctly calculates delays and stops after the maximum number of attempts.
- **Input Validation**: Use property-based testing (Hypothesis) to ensure numeric inputs are correctly clamped.

**Example reliability test:**
```python
@pytest.mark.anyio
async def test_subprocess_cleanup():
    async with managed_subprocess("sleep", "60") as proc:
        raise RuntimeError("simulated crash")
    # Verify proc is killed automatically after context exit
```

## Automated Checks

### Pre-commit Validation

Add to `.git/hooks/pre-commit` (or use pre-commit framework):

```bash
# Validate Markdown links
python scripts/validate_crossrefs.py --skip-external

# Test code examples
python scripts/test_doc_examples.py

# Validate JSON schemas against examples
uv run jsonschema -i results/*.json docs/evaluation_bundle.schema.json
```

### Continuous Integration

Add to GitHub Actions (`.github/workflows/docs.yml`):

```yaml
name: Documentation Validation

on: [push, pull_request]

jobs:
  validate-docs:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install uv
        run: curl -LsSf https://astral.sh/uv/install.sh | sh

      - name: Validate cross-references
        run: |
          python benchmark/scripts/validate_crossrefs.py \
            benchmark/GETTING_STARTED.md \
            benchmark/docs/A2A_EXAMPLES.md \
            benchmark/docs/ARCHITECTURE.md

      - name: Test code examples
        run: |
          python benchmark/scripts/test_doc_examples.py

      - name: Check Mermaid syntax
        run: |
          npm install -g @mermaid-js/mermaid-cli
          mmdc -i benchmark/docs/*.md
```

## Documentation Review Checklist

Before merging any doc changes, verify:

### Content Quality

- [ ] All code examples are tested and verified
- [ ] All links resolve (internal + external)
- [ ] Mermaid diagrams render correctly
- [ ] Placeholders are clearly marked with `<...>`
- [ ] Schema examples match current `.json` files
- [ ] Cross-references are bidirectional where appropriate
- [ ] Version-specific notes are clearly dated
- [ ] Commands use correct flag names and defaults

### Accessibility

- [ ] Newcomer can complete smoke test from A2A_GETTING_STARTED.md alone
- [ ] Examples have context and purpose (not just code)
- [ ] Common use cases are covered in A2A_EXAMPLES.md
- [ ] Troubleshooting guidance exists for error conditions
- [ ] Architecture is explained before diving into details

### Maintainability

- [ ] Schema changes update all related docs
- [ ] Cross-references added when new docs created
- [ ] Code examples stay in sync with pyproject.toml
- [ ] No duplicate content (prefer link over copy-paste)
- [ ] Section organization follows pattern (Overview → Examples → Details → References)

## Testing Infrastructure

### Script: `benchmark/scripts/test_doc_examples.py`

**Purpose:** Validate documentation code examples

**Checks:**
1. Extracts command blocks from Markdown
2. Verifies each command exists in `pyproject.toml`
3. Detects placeholders (all `<...>` documented)
4. Validates example paths exist (`manifests/`, `results/`)

**Usage:**
```bash
# Test all A2A docs
python benchmark/scripts/test_doc_examples.py

# Test specific file
python benchmark/scripts/test_doc_examples.py benchmark/GETTING_STARTED.md

# Skip external checks for CI
python benchmark/scripts/test_doc_examples.py --skip-external
```

**Exit codes:**
- `0`: All checks passed
- `1`: Found errors (commands not found, paths missing, placeholders undocumented)

### Script: `benchmark/scripts/validate_crossrefs.py`

**Purpose:** Validate Markdown links

**Checks:**
1. Finds all `[text](url)` links
2. Resolves internal links against file tree
3. Checks external links with HTTP HEAD
4. Reports broken references

**Usage:**
```bash
# Test all docs
python benchmark/scripts/validate_crossrefs.py

# Test specific file
python benchmark/scripts/validate_crossrefs.py benchmark/GETTING_STARTED.md

# Skip external link checks (faster, offline)
python benchmark/scripts/validate_crossrefs.py --skip-external

# Treat broken internal links as errors (not warnings)
python benchmark/scripts/validate_crossrefs.py --fail-on-warning
```

**Exit codes:**
- `0`: All links valid
- `1`: Found broken links (internal or external)

## Maintenance Process

### Weekly Tasks

- [ ] **External link check**: Run `validate_crossrefs.py` with full checks
- [ ] **Example verification**: Randomly test 2-3 examples from docs
- [ ] **Placeholder audit**: Ensure all placeholders are documented

### Per-Release Tasks

- [ ] **Example command verification**: Test all commands against current `pyproject.toml`
- [ ] **Schema sync check**: Compare documented examples against latest schema
- [ ] **Cross-reference audit**: Ensure new docs have links from existing docs

### Per-Schema-Change Tasks

- [ ] **Update all examples**: Find and update all `evaluation_bundle` examples
- [ ] **Update ARCHITECTURE.md**: Add new fields to invariants section
- [ ] **Add migration guide**: Document breaking changes and upgrade path
- [ ] **Test validation scripts**: Update `validate_bundle.py` if needed

### Quarterly Tasks

- [ ] **Documentation audit**: Check for stale content, outdated examples
- [ ] **Link health check**: Verify all external links are still active
- [ ] **Accessibility review**: Ensure newcomers can navigate docs successfully
- [ ] **Cross-link review**: Verify bidirectional linking is maintained

## Test Fixtures

The project includes Move test fixtures for testing failure detection at each validation stage.

### Fixture Location

```
tests/fixture/build/
├── fixture/                    # Standard test modules
│   └── sources/
│       ├── simple.move        # Basic passing tests
│       └── complex_layouts.move # Nested structs, generics, vectors
└── failure_cases/             # Failure stage triggers
    └── sources/
        ├── a1_function_not_found.move   # A1: Missing function
        ├── a1_private_function.move     # A1: Non-public function
        ├── a4_object_param.move         # A4: Object parameter detection
        ├── a5_generic_function.move     # A5: Generic function (unsupported)
        └── b2_abort_function.move       # B2: Runtime abort
```

### Using Fixtures in Tests

```rust
// Rust tests
let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
let fixture_dir = manifest_dir.join("tests/fixture/build/fixture");
```

```python
# Python tests
from pathlib import Path
FIXTURE_DIR = Path(__file__).parents[2] / "tests" / "fixture" / "build" / "fixture"
```

See [TEST_FIXTURES.md](../../docs/TEST_FIXTURES.md) for complete documentation.

---

## Common Issues and Solutions

### Issue: Placeholder Not Documented

**Symptom:** `test_doc_examples.py` reports "Found placeholders" without documentation

**Solution:**
Add a "Placeholders" section to the doc:
```markdown
**Placeholders:**
- `<CORPUS_ROOT>` - Path to sui-packages checkout
- `<PACKAGE_ID>` - Full package ID (e.g., 0x00db9a10bb...)
```

### Issue: Command Not Found in pyproject.toml

**Symptom:** `test_doc_examples.py` reports command doesn't exist

**Solution:**
Either:
1. Add the command to `pyproject.toml` `[project.scripts]`, or
2. Remove/update the example in documentation

### Issue: Broken Internal Link

**Symptom:** `validate_crossrefs.py` reports internal link not found

**Solution:**
1. Check if target file exists
2. Check if anchor name matches section heading
3. Update or remove the link

### Issue: External Link Timeout

**Symptom:** `validate_crossrefs.py` reports "timeout" on external link

**Solution:**
1. Verify link is correct
2. If site is down, add `--skip-external` to skip external checks
3. Consider archiving or finding alternative source

## Resources

- **Testing scripts**: `benchmark/scripts/test_doc_examples.py`, `benchmark/scripts/validate_crossrefs.py`
- **Schema definition**: `benchmark/docs/evaluation_bundle.schema.json`
- **Example docs**: `benchmark/GETTING_STARTED.md`, `benchmark/docs/A2A_EXAMPLES.md`
- **Architecture docs**: `benchmark/docs/ARCHITECTURE.md`
- **Test fixtures**: See [TEST_FIXTURES.md](../../docs/TEST_FIXTURES.md) for fixture organization and failure case modules
- **Local benchmark spec**: See [NO_CHAIN_TYPE_INHABITATION_SPEC.md](../../docs/NO_CHAIN_TYPE_INHABITATION_SPEC.md) for Tier A/B validation stages
