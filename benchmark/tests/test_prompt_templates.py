"""Tests for prompt template loading and variable substitution."""

from pathlib import Path

# Path to templates directory (relative to benchmark root)
TEMPLATES_DIR = Path(__file__).parent.parent / "templates"


class TestTemplateFiles:
    """Test that all expected template files exist and are valid."""

    def test_templates_directory_exists(self) -> None:
        """Templates directory must exist."""
        assert TEMPLATES_DIR.exists(), f"Templates directory not found: {TEMPLATES_DIR}"
        assert TEMPLATES_DIR.is_dir(), f"Templates path is not a directory: {TEMPLATES_DIR}"

    def test_default_template_exists(self) -> None:
        """Default type inhabitation template must exist."""
        template_file = TEMPLATES_DIR / "type_inhabitation.txt"
        assert template_file.exists(), f"Default template not found: {template_file}"

    def test_detailed_template_exists(self) -> None:
        """Detailed type inhabitation template must exist."""
        template_file = TEMPLATES_DIR / "type_inhabitation_detailed.txt"
        assert template_file.exists(), f"Detailed template not found: {template_file}"

    def test_repair_template_exists(self) -> None:
        """Repair build error template must exist."""
        template_file = TEMPLATES_DIR / "repair_build_error.txt"
        assert template_file.exists(), f"Repair template not found: {template_file}"

    def test_readme_exists(self) -> None:
        """Templates README must exist."""
        readme = TEMPLATES_DIR / "README.md"
        assert readme.exists(), f"README not found: {readme}"


class TestTemplateVariables:
    """Test that templates contain expected variables."""

    REQUIRED_VARS = ["{{PACKAGE_ID}}", "{{INTERFACE_SUMMARY}}", "{{MAX_ATTEMPTS}}", "{{MOVE_EDITION}}"]
    # Note: ATTEMPT_NUMBER and MAX_ATTEMPTS are documented but only BUILD_ERRORS is used in body
    REPAIR_VARS = ["{{BUILD_ERRORS}}"]

    def test_default_template_has_required_vars(self) -> None:
        """Default template must contain all required variables."""
        template = (TEMPLATES_DIR / "type_inhabitation.txt").read_text()
        for var in self.REQUIRED_VARS:
            assert var in template, f"Default template missing variable: {var}"

    def test_detailed_template_has_required_vars(self) -> None:
        """Detailed template must contain all required variables."""
        template = (TEMPLATES_DIR / "type_inhabitation_detailed.txt").read_text()
        for var in self.REQUIRED_VARS:
            assert var in template, f"Detailed template missing variable: {var}"

    def test_repair_template_has_required_vars(self) -> None:
        """Repair template must contain build error variables."""
        template = (TEMPLATES_DIR / "repair_build_error.txt").read_text()
        for var in self.REPAIR_VARS:
            assert var in template, f"Repair template missing variable: {var}"


class TestTemplateSubstitution:
    """Test template variable substitution logic."""

    def _load_and_substitute(self, template_name: str, **kwargs: str) -> str:
        """Load template, strip comments, and substitute variables."""
        template_path = TEMPLATES_DIR / template_name
        template = template_path.read_text(encoding="utf-8")

        # Strip comment lines (matching e2e_one_package.py logic)
        template_lines = [ln for ln in template.split("\n") if not ln.strip().startswith("#")]
        template = "\n".join(template_lines).strip()

        # Substitute variables
        for key, value in kwargs.items():
            template = template.replace(f"{{{{{key}}}}}", value)

        return template

    def test_substitute_default_template(self) -> None:
        """Default template substitution works correctly."""
        result = self._load_and_substitute(
            "type_inhabitation.txt",
            PACKAGE_ID="0x123abc",
            INTERFACE_SUMMARY="test interface summary",
            MAX_ATTEMPTS="3",
            MOVE_EDITION="2024.beta",
        )

        assert "0x123abc" in result
        assert "test interface summary" in result
        assert "3 attempts" in result or "3" in result
        assert "2024.beta" in result
        # Original variables should be replaced
        assert "{{PACKAGE_ID}}" not in result
        assert "{{INTERFACE_SUMMARY}}" not in result

    def test_substitute_detailed_template(self) -> None:
        """Detailed template substitution works correctly."""
        result = self._load_and_substitute(
            "type_inhabitation_detailed.txt",
            PACKAGE_ID="0xdeadbeef",
            INTERFACE_SUMMARY="complex interface\nwith\nmultiple\nlines",
            MAX_ATTEMPTS="5",
            MOVE_EDITION="2024",
        )

        assert "0xdeadbeef" in result
        assert "complex interface" in result
        assert "multiple" in result
        assert "5" in result
        assert "2024" in result

    def test_substitute_repair_template(self) -> None:
        """Repair template substitution works correctly."""
        result = self._load_and_substitute(
            "repair_build_error.txt",
            BUILD_ERRORS="error[E03006]: invalid type",
        )

        assert "error[E03006]" in result
        # Template should mention fixing errors
        assert "fix" in result.lower() or "Fix" in result

    def test_comments_are_stripped(self) -> None:
        """Comment lines (starting with #) are removed from output."""
        result = self._load_and_substitute(
            "type_inhabitation.txt",
            PACKAGE_ID="test",
            INTERFACE_SUMMARY="test",
            MAX_ATTEMPTS="1",
            MOVE_EDITION="2024.beta",
        )

        # Should not contain comment header
        assert "# Type Inhabitation Prompt Template" not in result
        assert "# ============" not in result


class TestTemplateContent:
    """Test that template content is appropriate for the task."""

    def test_default_template_mentions_json_output(self) -> None:
        """Default template should describe expected JSON output format."""
        template = (TEMPLATES_DIR / "type_inhabitation.txt").read_text()
        assert "JSON" in template or "json" in template
        assert "move_toml" in template
        assert "files" in template
        assert "entrypoints" in template

    def test_detailed_template_has_move_examples(self) -> None:
        """Detailed template should contain Move code examples."""
        template = (TEMPLATES_DIR / "type_inhabitation_detailed.txt").read_text()
        assert "```move" in template
        assert "module" in template
        assert "public entry fun" in template

    def test_detailed_template_mentions_move_toml(self) -> None:
        """Detailed template should explain Move.toml structure."""
        template = (TEMPLATES_DIR / "type_inhabitation_detailed.txt").read_text()
        assert "[package]" in template
        assert "[addresses]" in template
        assert "edition" in template

    def test_repair_template_is_concise(self) -> None:
        """Repair template should be concise (not bloat the context)."""
        template = (TEMPLATES_DIR / "repair_build_error.txt").read_text()
        # Should be under 1000 chars (not including comments)
        lines = [ln for ln in template.split("\n") if not ln.strip().startswith("#")]
        content = "\n".join(lines)
        assert len(content) < 1000, f"Repair template too long: {len(content)} chars"


class TestNoVersionNumbers:
    """Ensure v1/v2 naming is not used in filenames."""

    def test_no_v1_in_filenames(self) -> None:
        """No template files should have _v1 in name."""
        for f in TEMPLATES_DIR.glob("*.txt"):
            assert "_v1" not in f.name, f"Found v1 in filename: {f.name}"

    def test_no_v2_in_filenames(self) -> None:
        """No template files should have _v2 in name."""
        for f in TEMPLATES_DIR.glob("*.txt"):
            assert "_v2" not in f.name, f"Found v2 in filename: {f.name}"
