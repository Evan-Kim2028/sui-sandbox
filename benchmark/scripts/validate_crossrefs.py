#!/usr/bin/env python3
"""
Validate internal and external links in Markdown documentation.

This script:
1. Finds all [links]() in Markdown files
2. Resolves internal links against file tree
3. Checks external links with HTTP HEAD
4. Reports broken references

Usage:
    python scripts/validate_crossrefs.py
    python scripts/validate_crossrefs.py docs/A2A_GETTING_STARTED.md
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlparse


def extract_links(markdown_path: Path) -> list[tuple[int, str, str]]:
    """
    Extract all Markdown links.

    Returns:
        List of (line_number, link_text, url) tuples
    """
    links = []
    content = markdown_path.read_text(encoding="utf-8")
    lines = content.splitlines()

    # Match [text](url) pattern
    link_pattern = r"\[([^\]]+)\]\(([^)]+)\)"

    for i, line in enumerate(lines, start=1):
        for match in re.finditer(link_pattern, line):
            link_text = match.group(1)
            url = match.group(2)
            links.append((i, link_text, url))

    return links


def is_internal_link(url: str) -> bool:
    """Check if URL is internal (relative path or #anchor)."""
    return not urlparse(url).netloc or url.startswith("#")


def resolve_internal_link(url: str, doc_path: Path, doc_root: Path) -> tuple[bool, str]:
    """
    Resolve internal link against file tree.

    Returns:
        Tuple of (exists, resolved_path_or_reason)
    """
    # Handle anchor links (same document)
    if url.startswith("#"):
        return True, "anchor"

    # Parse URL and path
    parsed = urlparse(url)
    path = parsed.path

    if not path:
        return True, "empty path"

    # Make relative to doc root
    link_path = doc_path.parent / path

    # Resolve any .. references
    try:
        link_path = link_path.resolve()
    except Exception as e:
        return False, f"resolution error: {e}"

    # Check if exists
    if link_path.exists():
        return True, str(link_path)
    else:
        return False, f"not found: {link_path}"


def check_external_link(url: str, timeout: int = 5) -> tuple[bool, str]:
    """
    Check external link with HTTP HEAD.

    Returns:
        Tuple of (success, reason)
    """
    try:
        result = subprocess.run(
            ["curl", "-sI", "-o", "/dev/null", "-w", "%{http_code}", url],
            check=False, timeout=timeout,
            capture_output=True,
            text=True,
        )
        status_code = int(result.stdout)

        # Consider 2xx and 3xx as success
        if 200 <= status_code < 400:
            return True, f"HTTP {status_code}"
        else:
            return False, f"HTTP {status_code}"
    except subprocess.TimeoutExpired:
        return False, "timeout"
    except Exception as e:
        return False, str(e)


def validate_markdown_file(
    md_path: Path,
    doc_root: Path,
    check_external: bool = True,
) -> list[tuple[int, str, str, str]]:
    """
    Validate all links in a Markdown file.

    Returns:
        List of (line_number, link_text, url, status_or_reason) tuples
    """
    links = extract_links(md_path)
    results = []

    for line_num, link_text, url in links:
        if is_internal_link(url):
            exists, status = resolve_internal_link(url, md_path, doc_root)
            results.append((line_num, link_text, url, "OK" if exists else status))
        elif check_external:
            success, status = check_external_link(url)
            results.append((line_num, link_text, url, "OK" if success else status))
        else:
            results.append((line_num, link_text, url, "SKIP_EXTERNAL"))

    return results


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Validate cross-references in Markdown")
    parser.add_argument(
        "files",
        nargs="*",
        type=Path,
        help="Markdown files to validate (default: all docs)",
    )
    parser.add_argument(
        "--doc-root",
        type=Path,
        default=Path("."),
        help="Root directory for internal link resolution",
    )
    parser.add_argument(
        "--skip-external",
        action="store_true",
        help="Skip external link checks (faster, offline)",
    )
    parser.add_argument(
        "--fail-on-warning",
        action="store_true",
        help="Treat broken internal links as errors (not just warnings)",
    )
    args = parser.parse_args(argv)

    missing_files = 0
    # Default to all docs
    if not args.files:
        args.files = [
            Path("benchmark/README.md"),
            Path("benchmark/GETTING_STARTED.md"),
            Path("benchmark/docs/A2A_EXAMPLES.md"),
            Path("benchmark/docs/ARCHITECTURE.md"),
            Path("docs/METHODOLOGY.md"),
            Path("README.md"),
        ]

    total_broken = 0
    total_warnings = 0

    for md_path in args.files:
        if not md_path.exists():
            missing_files += 1
            print(f"Warning: File not found: {md_path}")
            continue

        print(f"\nChecking {md_path}...")

        results = validate_markdown_file(md_path, args.doc_root, check_external=not args.skip_external)

        broken = [
            (line, text, url, status)
            for line, text, url, status in results
            if status != "OK" and status != "SKIP_EXTERNAL"
        ]

        if broken:
            print(f"  Found {len(broken)} issues:")
            for line, text, url, status in broken:
                is_external = not is_internal_link(url)
                severity = "ERROR" if args.fail_on_warning and not is_external else "WARNING"
                print(f"    Line {line}: [{text}]({url}) - {severity}: {status}")

                if args.fail_on_warning or is_external:
                    total_broken += 1
                else:
                    total_warnings += 1
        else:
            print("  OK: All links valid")

    total_warnings += missing_files
    print(f"\nTotal: {total_broken} broken, {total_warnings} warnings")

    return 1 if total_broken > 0 else 0


if __name__ == "__main__":
    sys.exit(main())
