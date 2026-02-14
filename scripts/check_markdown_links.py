#!/usr/bin/env python3
"""Lightweight markdown link check for repository-local references."""

from __future__ import annotations

import argparse
import re
from pathlib import Path

INLINE_LINK_RE = re.compile(r"!\[[^\]]*\]\(([^)]+)\)")
REF_DEF_RE = re.compile(r"^\s*\[[^\]]+\]:\s+(.+?)\s*$")
SCHEME_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


def is_external_or_anchor(link: str) -> bool:
    if not link:
        return True
    if link.startswith("#"):
        return True
    if link.startswith("http://") or link.startswith("https://"):
        return True
    if link.startswith("mailto:") or link.startswith("ftp://") or link.startswith("javascript:"):
        return True
    if SCHEME_RE.match(link):
        return True
    return False


def normalize_link_target(raw: str) -> str:
    target = raw.strip()
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1].strip()
    if " " in target:
        target = target.split()[0]
    return target.split("#", 1)[0]


def extract_links(path: Path):
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except UnicodeDecodeError:
        return []

    links = []
    for line_no, line in enumerate(lines, start=1):
        for match in INLINE_LINK_RE.finditer(line):
            links.append((line_no, normalize_link_target(match.group(1))))
        m = REF_DEF_RE.match(line)
        if m:
            links.append((line_no, normalize_link_target(m.group(1))))
    return links


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default=".", help="Repository root")
    args = parser.parse_args()

    base = Path(args.base).resolve()
    md_files = [p for p in base.rglob("*.md") if p.is_file()]

    # Ignore generated/build artifacts and vendored docs.
    ignored = {base / ".git", base / "target", base / ".venv"}
    md_files = [p for p in md_files if not any(parent in ignored for parent in p.parents)]

    missing = []
    for md in md_files:
        for line_no, target in extract_links(md):
            if not target:
                continue
            if is_external_or_anchor(target):
                continue
            if target.startswith("/"):
                continue
            target_path = (md.parent / target).resolve()
            if target_path.exists():
                continue
            missing.append((md, line_no, target))

    if not missing:
        print(f"Markdown link check: OK ({len(md_files)} files)")
        return 0

    print("Markdown link check: FAILED")
    for md, line_no, target in missing:
        rel_file = md.relative_to(base)
        print(f"{rel_file}:{line_no} -> missing: {target}")
    print(f"Total missing local links: {len(missing)}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
