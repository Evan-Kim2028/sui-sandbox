# Testing Documentation

## Overview

This directory contains the test suite for the `sui-move-interface-extractor` benchmark harness. The test suite follows a philosophy of **fast, isolated, and maintainable** tests that provide confidence in code correctness and prevent regressions.

## Test Statistics

- **Total tests**: 100+ (and growing)
- **Test coverage**: ~35% (targeting 60%+)
- **Test execution time**: < 1 second for 100 tests
- **Test pass rate**: 100%

## Test Categorization

### Unit Tests (90% of tests)

**Purpose**: Test individual functions and classes in isolation.

**Characteristics**:
- Use `unittest.mock` to isolate dependencies
- No network I/O or file system side effects
- Fast execution (microseconds to milliseconds)
- Clear, descriptive test names

**Examples**:
- `test_extract_price_float_returns_float`
- `test_score_key_types_perfect_score`
- `test_mock_agent_perfect_behavior`

### Integration Tests (8% of tests)

**Purpose**: Test end-to-end flows and component interactions.

**Characteristics**:
- Use real components but mock external services
- Test critical user flows (Phase I → II → Execution)
- May use temporary directories for file I/O
- Slower than unit tests but still fast (< 100ms)

**Examples**:
- `test_phase1_full_run_with_mock_agent`
- `test_phase2_checkpoint_and_resume`
- `test_a2a_green_agent_full_request_response_cycle`

### End-to-End Tests (2% of tests)

**Purpose**: Test complete A2A protocol workflows from request to completion.

**Characteristics:**
- Use TestClient for in-process HTTP testing
- Mock subprocess execution to avoid real command execution
- Test full task lifecycle, cancellation, error recovery
- Validate A2A protocol compliance end-to-end

**Examples:**
- `test_full_task_lifecycle` - Complete task submission to completion
- `test_task_cancellation` - Task cancellation workflow
- `test_concurrent_tasks` - Multiple simultaneous tasks

### Property-Based Tests (2% of tests)

**Purpose**: Use property-based testing (Hypothesis) to find edge cases.

**Characteristics**:
- Test invariants and mathematical properties
- Find bugs that example-based tests miss
- Generate hundreds of random test cases
- Slower but catch complex bugs

**Examples**:
- `test_score_key_types_symmetric_difference`
- `test_score_inhabitation_boundaries`
- `test_compute_phase2_metrics_properties`

## Test Philosophy

### 1. Fast Feedback Loop

Tests should run quickly to enable tight TDD loops.
- **Target**: Entire test suite runs in < 5 seconds
- **Practice**: Avoid sleep(), time.sleep(), or long loops in tests
- **Exception**: Integration tests may take up to 100ms

### 2. Isolation

Each test should be independent and not depend on execution order.
- **Practice**: Use `tmp_path` fixture for file I/O
- **Practice**: Use `monkeypatch` for environment variables
- **Avoid**: Global state, shared fixtures between tests

### 3. Clarity

Test names should describe what they test and what the expected outcome is.
- **Pattern**: `test_{unit}_{scenario}_{expected_outcome}`
- **Good**: `test_check_path_exists_missing_raises_systemexit`
- **Bad**: `test_paths`

### 4. Maintainability

Tests should be as easy to understand as the code they test.
- **Practice**: One assertion per logical concept
- **Practice**: Use descriptive variable names
- **Avoid**: Complex test setup that obscures intent

## Running Tests

### Run all tests

```bash
cd benchmark
python -m pytest tests/ -v
```

### Run specific test file

```bash
python -m pytest tests/test_a2a_smoke_config.py -v
```

### Run specific test

```bash
python -m pytest tests/test_a2a_smoke_config.py::test_a2a_smoke_default_request_serializes_all_parameters -v
```

### Run with coverage

```bash
python -m pytest --cov=smi_bench --cov-report=term-missing tests/
```

### Run with coverage threshold

```bash
python -m pytest --cov=smi_bench --cov-fail-under=80 tests/
```

### Run only unit tests (fast)

```bash
python -m pytest tests/ -v -m "not integration"
```

### Run only integration tests

```bash
python -m pytest tests/ -v -m integration
```

### Run tests that match pattern

```bash
python -m pytest tests/ -v -k "score"
```

## Mock Patterns

### Mocking Network Calls

Use `unittest.mock.patch` to replace HTTP clients:

```python
from unittest.mock import patch
import httpx

def test_fetch_openrouter_models_success(monkeypatch):
    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {"data": []}

    with patch("httpx.get", return_value=mock_response):
        models = fetch_openrouter_models(base_url="...", api_key="...")
        assert len(models) == 0
```

### Mocking File I/O

Use `tmp_path` fixture for temporary directories:

```python
def test_main_creates_output_file(tmp_path):
    out_file = tmp_path / "output.json"
    main(["--out", str(out_file)])
    assert out_file.exists()
```

### Mocking Environment Variables

Use `monkeypatch` fixture to modify environment:

```python
def test_get_api_key_prefer_smi_api_key(monkeypatch):
    monkeypatch.setenv("SMI_API_KEY", "smi_key")
    monkeypatch.setenv("OPENROUTER_API_KEY", "openrouter_key")

    result = _get_api_key({})
    assert result == "smi_key"
```

### Mocking Subprocess Calls

Use `unittest.mock.patch` to replace subprocess:

```python
def test_main_calls_smoke_tool(monkeypatch):
    with patch("subprocess.run") as mock_run:
        mock_run.return_value = MagicMock(returncode=0)
        main(args)
        mock_run.assert_called_once()
```

## Fixtures

### Golden Fixtures

Located in `tests/fixtures/`:
- `phase1_golden_run.json` - Example Phase I output
- `phase2_golden_run.json` - Example Phase II output
- `events_golden.jsonl` - Example event log

These fixtures are used to validate schema compatibility and prevent accidental renames.

### Fake Corpus

Located in `tests/fake_corpus/`:
- Minimal package structure for testing
- Avoids needing real Sui Move packages

## Adding New Tests

### 1. Choose Test File

- **Unit test**: Add to `test_<module>.py`
- **Integration test**: Add to `test_integration_<phase>.py`
- **Property test**: Add to `test_property_<module>.py`

### 2. Write Test Function

```python
def test_<unit>_<scenario>_<expected>() -> None:
    """Brief description of what this tests."""
    # Arrange
    setup_data = {...}

    # Act
    result = function_under_test(setup_data)

    # Assert
    assert result == expected_value
```

### 3. Follow Naming Conventions

- **Pattern**: `test_{module}_{scenario}_{expected_outcome}`
- **Prefix**: Test names start with `test_`
- **Lowercase**: Use lowercase with underscores
- **Descriptive**: Name should be self-documenting

### 4. Add Type Hints

```python
def test_function_returns_string(tmp_path: Path) -> None:
    """Type hints make tests more readable."""
    result = function(tmp_path)
    assert isinstance(result, str)
```

## Test Quality Checklist

Before submitting a new test, ensure:

- [ ] Test name follows naming convention
- [ ] Test has a docstring describing what it tests
- [ ] Test uses type hints for parameters and return type
- [ ] Test uses appropriate fixtures (tmp_path, monkeypatch)
- [ ] Test has exactly one logical assertion (or grouped related assertions)
- [ ] Test cleans up after itself (no side effects)
- [ ] Test is isolated (doesn't depend on other tests)
- [ ] Test is fast (runs in < 100ms)

## CI/CD Integration

### GitHub Actions

Tests run on every push and pull request:

```yaml
- name: Run tests
  run: |
    cd benchmark
    python -m pytest --cov=smi_bench --cov-report=xml tests/
```

### Coverage Threshold

Minimum coverage enforced in CI: **80%**

## Debugging Failed Tests

### Print Debug Information

```python
def test_failing_case():
    result = function_under_test(data)
    print(f"DEBUG: result={result}, expected={expected}")  # For debugging
    assert result == expected
```

### Use pdb

```python
def test_failing_case():
    import pdb; pdb.set_trace()  # Breakpoint
    result = function_under_test(data)
    assert result == expected
```

### Show Local Variables

```bash
python -m pytest tests/test_failing.py -v -l
```

## Resources

- [pytest documentation](https://docs.pytest.org/)
- [Hypothesis documentation](https://hypothesis.readthedocs.io/)
- [unittest.mock documentation](https://docs.python.org/3/library/unittest.mock.html)
- [pytest-cov documentation](https://pytest-cov.readthedocs.io/)

## Contributing

When adding new features:
1. Write tests first (TDD)
2. Ensure all tests pass
3. Run coverage report: `pytest --cov=smi_bench`
4. Document any new test patterns in this README

## Troubleshooting

### Tests fail with import errors

Ensure you're running from the `benchmark` directory:

```bash
cd benchmark
python -m pytest tests/
```

### Tests timeout

Increase timeout in `pytest.ini` or run individually:

```bash
python -m pytest tests/test_slow.py -v --timeout=10
```

### Coverage report missing lines

Use `--cov-report=term-missing` to see which lines aren't covered:

```bash
python -m pytest --cov=smi_bench --cov-report=term-missing tests/
```
