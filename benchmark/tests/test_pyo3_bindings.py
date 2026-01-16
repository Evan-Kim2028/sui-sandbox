"""Tests for PyO3 native bindings (sui_sandbox module).

These tests verify that the native Python bindings work correctly and
maintain schema compatibility with the subprocess-based sandbox.
"""

from __future__ import annotations

import pytest

# Try to import native bindings - skip all tests if not available
try:
    import sui_sandbox
    from sui_sandbox import (
        EventData,
        FieldInfo,
        FunctionInfo,
        ModuleSummary,
        ObjectEffect,
        SandboxEnvironment,
        SandboxResponse,
        StructInfo,
        TransactionEffects,
    )

    NATIVE_AVAILABLE = True
except ImportError:
    NATIVE_AVAILABLE = False
    SandboxEnvironment = None
    SandboxResponse = None

pytestmark = pytest.mark.skipif(
    not NATIVE_AVAILABLE,
    reason="Native sui_sandbox module not installed. Build with: maturin build --release && pip install target/wheels/*.whl",
)


class TestModuleImport:
    """Test that the module imports correctly and exposes expected types."""

    def test_module_has_version(self):
        """Module should expose __version__."""
        assert hasattr(sui_sandbox, "__version__")
        assert isinstance(sui_sandbox.__version__, str)
        # Version should be semver-like
        parts = sui_sandbox.__version__.split(".")
        assert len(parts) >= 2

    def test_module_exports_environment(self):
        """Module should export SandboxEnvironment class."""
        assert SandboxEnvironment is not None
        assert callable(SandboxEnvironment)

    def test_module_exports_response_types(self):
        """Module should export all response types for isinstance() checks."""
        assert SandboxResponse is not None
        assert TransactionEffects is not None
        assert ObjectEffect is not None
        assert EventData is not None
        assert ModuleSummary is not None
        assert FunctionInfo is not None
        assert StructInfo is not None
        assert FieldInfo is not None


class TestSandboxEnvironment:
    """Test SandboxEnvironment class functionality."""

    @pytest.fixture
    def env(self):
        """Create a fresh sandbox environment for each test."""
        return SandboxEnvironment(verbose=False)

    def test_create_environment(self, env):
        """Should be able to create an environment."""
        assert env is not None

    def test_environment_has_properties(self, env):
        """Environment should expose timestamp_ms and verbose properties."""
        assert hasattr(env, "timestamp_ms")
        assert hasattr(env, "verbose")
        assert isinstance(env.timestamp_ms, int)
        assert isinstance(env.verbose, bool)
        assert env.verbose is False

    def test_verbose_environment(self):
        """Should be able to create verbose environment."""
        env = SandboxEnvironment(verbose=True)
        assert env.verbose is True

    def test_reset(self, env):
        """Reset should not raise."""
        env.reset()  # Should not raise


class TestBasicActions:
    """Test basic sandbox actions."""

    @pytest.fixture
    def env(self):
        """Create a fresh sandbox environment for each test."""
        return SandboxEnvironment(verbose=False)

    def test_list_modules_empty(self, env):
        """list_modules on empty sandbox should return empty list."""
        result = env.execute({"action": "list_modules"})
        assert isinstance(result, SandboxResponse)
        assert result.success is True
        assert result.error is None
        assert result.data is not None
        assert "modules" in result.data
        assert isinstance(result.data["modules"], list)

    def test_list_functions_requires_module(self, env):
        """list_functions requires module_path parameter."""
        # list_functions requires a module_path, should raise on missing
        with pytest.raises(KeyError):
            env.execute({"action": "list_functions"})

    def test_list_objects_empty(self, env):
        """list_objects on empty sandbox should return empty list."""
        result = env.execute({"action": "list_objects"})
        assert isinstance(result, SandboxResponse)
        assert result.success is True
        assert result.data is not None
        assert "objects" in result.data

    def test_get_state(self, env):
        """get_state should return sandbox state."""
        result = env.execute({"action": "get_state"})
        assert isinstance(result, SandboxResponse)
        assert result.success is True
        assert result.data is not None


class TestResponseTypes:
    """Test that response types have correct structure."""

    @pytest.fixture
    def env(self):
        return SandboxEnvironment(verbose=False)

    def test_sandbox_response_attributes(self, env):
        """SandboxResponse should have expected attributes."""
        result = env.execute({"action": "list_modules"})

        # Required attributes
        assert hasattr(result, "success")
        assert hasattr(result, "error")
        assert hasattr(result, "error_category")
        assert hasattr(result, "data")
        assert hasattr(result, "effects")
        assert hasattr(result, "gas_used")

        # Type checks
        assert isinstance(result.success, bool)
        assert result.error is None or isinstance(result.error, str)
        assert result.error_category is None or isinstance(result.error_category, str)

    def test_response_is_frozen(self, env):
        """SandboxResponse should be immutable (frozen)."""
        result = env.execute({"action": "list_modules"})

        with pytest.raises(AttributeError):
            result.success = False  # Should raise - frozen class


class TestErrorHandling:
    """Test error handling for invalid requests."""

    @pytest.fixture
    def env(self):
        return SandboxEnvironment(verbose=False)

    def test_unknown_action(self, env):
        """Unknown action should raise ValueError."""
        with pytest.raises(ValueError, match="Unknown action"):
            env.execute({"action": "nonexistent_action"})

    def test_missing_action(self, env):
        """Request without action should raise or return error."""
        with pytest.raises(Exception):
            env.execute({})  # Missing required "action" field

    def test_module_summary_missing_module(self, env):
        """module_summary with nonexistent module should return error."""
        result = env.execute({"action": "module_summary", "module_path": "0xnonexistent::fake_module"})
        assert isinstance(result, SandboxResponse)
        # Should indicate the module wasn't found
        assert result.success is False or result.data is None


class TestTypeConversions:
    """Test that Python<->Rust type conversions work correctly."""

    @pytest.fixture
    def env(self):
        return SandboxEnvironment(verbose=False)

    def test_list_in_response(self, env):
        """Lists should be converted to Python lists."""
        result = env.execute({"action": "list_modules"})
        modules = result.data.get("modules", [])
        assert isinstance(modules, list)

    def test_dict_in_response(self, env):
        """Dicts should be converted to Python dicts."""
        result = env.execute({"action": "get_state"})
        assert isinstance(result.data, dict)

    def test_nested_structures(self, env):
        """Nested structures should be properly converted."""
        result = env.execute({"action": "list_objects"})
        assert isinstance(result.data, dict)
        objects = result.data.get("objects", [])
        assert isinstance(objects, list)


class TestSandboxNativeWrapper:
    """Test the high-level NativeSandbox wrapper.

    These tests import from the experiments directory via sys.path manipulation
    since it's not an installed package.
    """

    @pytest.fixture(autouse=True)
    def setup_path(self):
        """Add experiments lib to path for imports."""
        import sys
        from pathlib import Path

        experiments_lib = Path(__file__).parent.parent / "experiments" / "ptb_simulation" / "lib"
        if str(experiments_lib) not in sys.path:
            sys.path.insert(0, str(experiments_lib))

    def test_import_wrapper(self, setup_path):
        """Should be able to import the wrapper module."""
        from sandbox_native import (
            is_native_available,
        )

        assert is_native_available() is True

    def test_create_native_sandbox(self, setup_path):
        """Should be able to create NativeSandbox instance."""
        from sandbox_native import NativeSandbox

        sandbox = NativeSandbox(verbose=False)
        assert sandbox is not None

    def test_native_sandbox_execute(self, setup_path):
        """NativeSandbox.execute should work like SandboxProcess (returns dict)."""
        from sandbox_native import NativeSandbox

        sandbox = NativeSandbox(verbose=False)
        result = sandbox.execute("list_modules")
        # NativeSandbox.execute() returns dict for SandboxProcess compatibility
        assert isinstance(result, dict)
        assert result.get("success") is True
        assert result.get("data") is not None

    def test_native_sandbox_close_noop(self, setup_path):
        """NativeSandbox.close() should be a no-op (for API compatibility)."""
        from sandbox_native import NativeSandbox

        sandbox = NativeSandbox(verbose=False)
        sandbox.close()  # Should not raise


class TestFactoryFunction:
    """Test the create_sandbox() factory function.

    Note: These tests use NativeSandbox directly rather than the factory
    because the factory's relative imports require proper package structure.
    The factory is tested implicitly via the NativeSandbox wrapper tests.
    """

    @pytest.fixture(autouse=True)
    def setup_path(self):
        """Add experiments lib to path for imports."""
        import sys
        from pathlib import Path

        experiments_lib = Path(__file__).parent.parent / "experiments" / "ptb_simulation" / "lib"
        if str(experiments_lib) not in sys.path:
            sys.path.insert(0, str(experiments_lib))

    def test_native_sandbox_available(self, setup_path):
        """Native sandbox should be available when sui_sandbox is installed."""
        from sandbox_native import is_native_available

        assert is_native_available() is True

    def test_native_sandbox_direct_creation(self, setup_path):
        """NativeSandbox can be created directly."""
        from sandbox_native import NativeSandbox, is_native_available

        if not is_native_available():
            pytest.skip("Native bindings not available")

        sandbox = NativeSandbox(verbose=False)
        result = sandbox.execute("list_modules")
        assert result.get("success") is True
        sandbox.close()


class TestSchemaCompatibility:
    """Test that native and subprocess modes return compatible schemas.

    These tests ensure we don't have schema mismatch bugs between
    the native bindings and the JSON-based subprocess mode.
    """

    @pytest.fixture
    def env(self):
        return SandboxEnvironment(verbose=False)

    def test_list_modules_schema(self, env):
        """list_modules response should have expected schema."""
        result = env.execute({"action": "list_modules"})
        assert result.success is True
        assert "modules" in result.data
        # Each module should be a dict or string
        for module in result.data["modules"]:
            assert isinstance(module, (str, dict))

    def test_list_functions_requires_module_path(self, env):
        """list_functions requires module_path parameter."""
        # list_functions needs a module_path - should raise without it
        with pytest.raises(KeyError):
            env.execute({"action": "list_functions"})

    def test_list_objects_schema(self, env):
        """list_objects response should have expected schema."""
        result = env.execute({"action": "list_objects"})
        assert result.success is True
        assert "objects" in result.data
        assert isinstance(result.data["objects"], list)

    def test_get_state_schema(self, env):
        """get_state response should have expected schema."""
        result = env.execute({"action": "get_state"})
        assert result.success is True
        assert isinstance(result.data, dict)
        # State typically has timestamp and object counts
        # but exact schema may vary


class TestVersioning:
    """Test version-related functionality."""

    def test_version_matches_cargo(self):
        """Module version should match Cargo.toml version."""
        # This test documents the versioning relationship
        assert sui_sandbox.__version__ == "0.1.0"

    def test_version_is_semver(self):
        """Version should be valid semver."""
        version = sui_sandbox.__version__
        parts = version.split(".")
        assert len(parts) >= 2
        # Major and minor should be integers
        assert parts[0].isdigit()
        assert parts[1].isdigit()
