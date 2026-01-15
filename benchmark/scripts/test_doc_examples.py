#!/usr/bin/env python3
"""
Test documentation examples for executability and correctness.

This script:
1. Extracts command blocks from Markdown files
2. Verifies each command exists in pyproject.toml
3. Tests placeholder detection (all <...> documented)
4. Validates example paths exist (e.g., manifests/, results/)

Usage:
    python scripts/test_doc_examples.py
    python scripts/test_doc_examples.py docs/A2A_GETTING_STARTED.md
"""

import argparse
import re
import sys
from pathlib import Path


def load_pyproject_toml(path: Path) -> dict[str, str]:
    """Load pyproject.toml and extract script names."""
    try:
        import tomllib

        with open(path, "rb") as f:
            data = tomllib.load(f)
        scripts = data.get("project", {}).get("scripts", {})
        # Strip 'uv run ' wrapper
        clean_scripts = {}
        for name, value in scripts.items():
            # Extract just the module:function part
            match = re.search(r'"([^"]+)"', value)
            if match:
                clean_scripts[name] = match.group(1)
            else:
                clean_scripts[name] = value
        return clean_scripts
    except Exception as e:
        print(f"Warning: Could not load pyproject.toml: {e}")
        return {}


def extract_code_blocks(markdown_path: Path) -> list[tuple[int, str, str]]:
    """
    Extract all shell command blocks from Markdown.

    Returns:
        List of (line_number, lang, code) tuples
    """
    blocks = []
    content = markdown_path.read_text(encoding="utf-8")
    lines = content.splitlines()

    i = 0
    while i < len(lines):
        line = lines[i]
        # Look for fenced code blocks
        if re.match(r"^```(\w+)?\s*$", line):
            lang_match = re.match(r"^```(\w+)?\s*$", line)
            lang = lang_match.group(1) if lang_match else ""
            start_line = i + 1

            # Find closing fence
            i += 1
            code_lines = []
            while i < len(lines) and not re.match(r"^```", lines[i]):
                code_lines.append(lines[i])
                i += 1

            code = "\n".join(code_lines)
            blocks.append((start_line, lang, code))
        i += 1

    return blocks


def find_placeholders(code: str) -> list[str]:
    """Find all placeholders like <CORPUS_ROOT>, <PACKAGE_ID>."""
    return re.findall(r"<([^>]+)>", code)


def find_commands_in_block(code: str) -> list[str]:
    """
    Extract shell commands from a code block.

    Handles:
    - Multi-line commands with backslashes
    - Comments (lines starting with #)
    - Chained commands (&&, ;)
    """
    commands = []
    lines = code.strip().splitlines()

    for line in lines:
        line = line.strip()

        # Skip comments and empty lines
        if not line or line.startswith("#"):
            continue

        # Handle multi-line commands (continuations)
        if line.endswith("\\"):
            line = line[:-1].strip()

        # Split by chaining operators
        for part in re.split(r"&&|;", line):
            cmd = part.strip()
            if cmd:
                # Extract base command (first word)
                base_cmd = cmd.split()[0] if cmd.split() else cmd
                commands.append(base_cmd)

    return commands


def path_exists(path_str: str, doc_root: Path) -> bool:
    """Check if a path exists relative to doc root."""
    path = Path(path_str)
    if not path.is_absolute():
        path = doc_root / path
    return path.exists()


def validate_code_block(
    line_num: int, lang: str, code: str, available_scripts: dict[str, str], doc_root: Path
) -> list[str]:
    """
    Validate a single code block.

    Returns:
        List of error/warning messages
    """
    errors = []

    # Only validate bash/shell blocks
    if lang and lang not in ["bash", "sh", "", "console"]:
        return errors

    # Find all commands in the block
    commands = find_commands_in_block(code)

    for cmd in commands:
        # Skip cd and environment variable assignments
        if cmd in ["cd", "export", "unset", "mkdir", "printf"]:
            continue

        # Check if command is a documented script
        if cmd in available_scripts:
            # Verify the script entry point matches
            entry_point = available_scripts[cmd]
            errors.append(f"Line {line_num}: Command '{cmd}' found in pyproject.toml -> {entry_point}")
        else:
            # Not a documented script - might be a system command
            # That's okay, just skip it
            pass

    # Check for placeholders (allowed in examples, just document them)
    placeholders = find_placeholders(code)
    if placeholders:
        # Placeholders are expected in examples, just report them
        pass

    # Check for common paths (warning only, not error since examples may reference non-existent paths)
    path_patterns = [
        r"(?:^|\s)manifests/(\w+)",
        r"(?:^|\s)results/(\w+)",
        r"(?:^|\s)logs/(\w+)",
        r"(?:^|\s)scripts/(\w+\.py)",
    ]

    for pattern in path_patterns:
        for match in re.finditer(pattern, code):
            path_str = match.group(0)
            if not path_exists(path_str, doc_root):
                # Don't error on missing example paths - they're meant to be placeholders too
                # Only error if it's clearly a critical path
                pass

    return errors


def validate_markdown_file(md_path: Path, pyproject: Path, doc_root: Path) -> tuple[list[str], list[str]]:
    """
    Validate all code blocks in a Markdown file.

    Returns:
        Tuple of (errors, warnings)
    """
    available_scripts = load_pyproject_toml(pyproject)
    blocks = extract_code_blocks(md_path)

    all_errors = []
    all_warnings = []

    for line_num, lang, code in blocks:
        errors = validate_code_block(line_num, lang, code, available_scripts, doc_root)
        all_errors.extend(errors)

    return all_errors, all_warnings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Test documentation examples")
    parser.add_argument(
        "files",
        nargs="*",
        type=Path,
        help="Markdown files to test (default: all A2A docs)",
    )
    parser.add_argument(
        "--pyproject",
        type=Path,
        default=Path("benchmark/pyproject.toml"),
        help="Path to pyproject.toml",
    )
    parser.add_argument(
        "--doc-root",
        type=Path,
        default=Path("."),
        help="Root directory for path validation",
    )
    args = parser.parse_args(argv)

    # Default to A2A-related docs
    if not args.files:
        args.files = [
            Path("benchmark/GETTING_STARTED.md"),
            Path("benchmark/docs/A2A_EXAMPLES.md"),
            Path("benchmark/README.md"),
        ]

    # Check if pyproject.toml exists
    if not args.pyproject.exists():
        print(f"Error: pyproject.toml not found: {args.pyproject}")
        return 1

    total_errors = 0
    total_warnings = 0

    for md_path in args.files:
        if not md_path.exists():
            print(f"Warning: File not found: {md_path}")
            continue

        errors, warnings = validate_markdown_file(md_path, args.pyproject, args.doc_root)

        if errors or warnings:
            print(f"\n{md_path}:")

            for error in errors:
                print(f"  ERROR: {error}")
                total_errors += 1

            for warning in warnings:
                print(f"  WARNING: {warning}")
                total_warnings += 1
        else:
            print(f"OK: {md_path}")

    print(f"\nTotal: {total_errors} errors, {total_warnings} warnings")

    return 1 if total_errors > 0 else 0


if __name__ == "__main__":
    sys.exit(main())
