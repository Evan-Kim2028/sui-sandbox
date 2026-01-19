"""
SMI Benchmark Doctor - Environment validation and troubleshooting.

Run this to verify your environment is correctly configured before running benchmarks.

Usage:
    uv run smi-bench-doctor              # Quick check (no corpus validation)
    uv run smi-bench-doctor --full       # Full check including corpus
    uv run smi-bench-doctor --fix        # Attempt to fix common issues
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

from rich.console import Console
from rich.panel import Panel
from rich.table import Table

console = Console()

# Repository and benchmark roots
BENCH_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = BENCH_ROOT.parents[1]


# ---------------------------------------------------------------------------
# Check Functions
# ---------------------------------------------------------------------------


def check_rust_binary() -> tuple[bool, str, str | None]:
    """
    Check if the Rust extractor binary is available.

    Returns:
        (ok, message, fix_command)
    """
    # Check SMI_RUST_BIN first
    env_bin = os.environ.get("SMI_RUST_BIN")
    if env_bin:
        path = Path(env_bin)
        if path.is_file():
            return True, f"Rust binary found (SMI_RUST_BIN): {path}", None
        return False, f"SMI_RUST_BIN set but file not found: {path}", None

    # Check target/release
    exe = "sui_move_interface_extractor.exe" if os.name == "nt" else "sui_move_interface_extractor"
    local = REPO_ROOT / "target" / "release" / exe
    if local.is_file():
        return True, f"Rust binary found: {local}", None

    # Check system path
    system_bin = shutil.which("sui_move_interface_extractor")
    if system_bin:
        return True, f"Rust binary found in PATH: {system_bin}", None

    return (
        False,
        "Rust binary not found. The benchmark requires the compiled extractor.",
        "cargo build --release --locked",
    )


def check_sui_cli() -> tuple[bool, str, str | None]:
    """Check if Sui CLI is available (needed for Move compilation)."""
    sui_bin = shutil.which("sui")
    if sui_bin:
        try:
            result = subprocess.run(
                ["sui", "--version"],
                check=False,
                capture_output=True,
                text=True,
                timeout=10,
            )
            if result.returncode == 0:
                version = result.stdout.strip().split("\n")[0]
                return True, f"Sui CLI found: {version}", None
        except (subprocess.TimeoutExpired, FileNotFoundError, OSError):
            pass
    return (
        False,
        "Sui CLI not found. Required for Move package compilation.",
        "cargo install --locked --git https://github.com/MystenLabs/sui.git sui",
    )


def check_api_keys() -> tuple[bool, str, str | None]:
    """Check if API keys are configured."""
    keys_to_check = ["OPENROUTER_API_KEY", "SMI_API_KEY", "OPENAI_API_KEY"]
    found_keys = []

    for key in keys_to_check:
        val = os.environ.get(key)
        if val and val.strip():
            # Mask the key value
            masked = val[:8] + "..." if len(val) > 12 else "***"
            found_keys.append(f"{key}={masked}")

    if found_keys:
        return True, f"API key(s) configured: {', '.join(found_keys)}", None

    return (
        False,
        "No API key found. Set one of: OPENROUTER_API_KEY, SMI_API_KEY, or OPENAI_API_KEY",
        "export OPENROUTER_API_KEY=sk-or-v1-your-key-here",
    )


def check_env_file() -> tuple[bool, str, str | None]:
    """Check if .env file exists."""
    env_file = BENCH_ROOT / ".env"
    env_example = BENCH_ROOT / ".env.example"

    if env_file.exists():
        return True, f".env file found: {env_file}", None

    if env_example.exists():
        return (
            False,
            ".env file not found. Copy from .env.example and configure.",
            f"cp {env_example} {env_file}",
        )

    return False, ".env file not found and no .env.example available.", None


def check_corpus(corpus_root: Path | None) -> tuple[bool, str, str | None]:
    """Check if corpus exists and has valid packages."""
    if corpus_root is None:
        # Try common locations
        common_paths = [
            REPO_ROOT.parent / "sui-packages" / "packages" / "mainnet_most_used",
            REPO_ROOT / "sui-packages" / "packages" / "mainnet_most_used",
            Path.home() / "sui-packages" / "packages" / "mainnet_most_used",
        ]
        for p in common_paths:
            if p.exists() and p.is_dir():
                corpus_root = p
                break

    if corpus_root is None:
        return (
            False,
            "Corpus not found. Clone sui-packages repository.",
            "git clone --depth 1 https://github.com/MystenLabs/sui-packages.git ../sui-packages",
        )

    if not corpus_root.exists():
        return False, f"Corpus path does not exist: {corpus_root}", None

    # Count valid packages
    from smi_bench.dataset import iter_package_dirs

    valid_count = 0
    for pkg_dir in iter_package_dirs(corpus_root):
        resolved = pkg_dir.resolve()
        if (resolved / "metadata.json").exists() and (resolved / "bytecode_modules").is_dir():
            valid_count += 1
            if valid_count >= 10:  # Don't scan entire corpus for quick check
                break

    if valid_count == 0:
        return False, f"No valid packages found in corpus: {corpus_root}", None

    return True, f"Corpus found with {valid_count}+ valid packages: {corpus_root}", None


def check_python_deps() -> tuple[bool, str, str | None]:
    """Check if Python dependencies are installed."""
    try:
        import httpx  # noqa: F401
        import rich  # noqa: F401

        return True, "Python dependencies installed", None
    except ImportError as e:
        return (
            False,
            f"Missing Python dependency: {e.name}",
            "uv sync --group dev",
        )


def check_manifest(corpus_root: Path, manifest_path: Path) -> tuple[bool, str, str | None]:
    """Check if manifest file is valid and packages exist in corpus."""
    from smi_bench.utils import safe_read_lines

    if not manifest_path.exists():
        return False, f"Manifest file does not exist: {manifest_path}", None

    lines = safe_read_lines(manifest_path, context="doctor-manifest")
    package_ids = [line.split("#")[0].strip() for line in lines if line.split("#")[0].strip()]

    if not package_ids:
        return False, f"Manifest file is empty: {manifest_path}", None

    # Check a sample of packages
    missing = []
    checked = 0
    for pkg_id in package_ids[:20]:  # Check first 20
        if not pkg_id.startswith("0x"):
            continue
        checked += 1
        # Try common corpus layouts
        found = False
        for layout in [
            corpus_root / pkg_id,
            corpus_root / pkg_id[:4].lower() / pkg_id,
            corpus_root / f"0x{pkg_id[2:4]}" / pkg_id[4:],
        ]:
            if layout.exists():
                found = True
                break
        if not found:
            missing.append(pkg_id)

    if missing:
        return (
            False,
            f"Manifest references {len(missing)}/{checked} packages not found in corpus",
            None,
        )

    return True, f"Manifest valid: {len(package_ids)} packages, all checked found in corpus", None


# ---------------------------------------------------------------------------
# Main Doctor Logic
# ---------------------------------------------------------------------------


def run_checks(
    full: bool = False,
    corpus_root: Path | None = None,
    manifest: Path | None = None,
) -> list[tuple[str, bool, str, str | None]]:
    """
    Run all environment checks.

    Returns:
        List of (check_name, passed, message, fix_command)
    """
    results: list[tuple[str, bool, str, str | None]] = []

    # Core checks (always run)
    ok, msg, fix = check_rust_binary()
    results.append(("Rust Binary", ok, msg, fix))
    ok, msg, fix = check_api_keys()
    results.append(("API Keys", ok, msg, fix))
    ok, msg, fix = check_env_file()
    results.append((".env File", ok, msg, fix))
    ok, msg, fix = check_python_deps()
    results.append(("Python Deps", ok, msg, fix))

    # Optional checks
    if full:
        ok, msg, fix = check_sui_cli()
        results.append(("Sui CLI", ok, msg, fix))
        ok, msg, fix = check_corpus(corpus_root)
        results.append(("Corpus", ok, msg, fix))
        if manifest and corpus_root:
            ok, msg, fix = check_manifest(corpus_root, manifest)
            results.append(("Manifest", ok, msg, fix))

    return results


def print_results(results: list[tuple[str, bool, str, str | None]]) -> bool:
    """Print check results and return overall status."""
    table = Table(title="SMI Benchmark Environment Check", show_header=True)
    table.add_column("Check", style="cyan", width=15)
    table.add_column("Status", width=6)
    table.add_column("Details", style="dim")

    all_passed = True
    fixes: list[tuple[str, str]] = []

    for name, passed, message, fix in results:
        status = "[green]✓[/green]" if passed else "[red]✗[/red]"
        table.add_row(name, status, message)
        if not passed:
            all_passed = False
            if fix:
                fixes.append((name, fix))

    console.print(table)

    if fixes:
        console.print()
        console.print(
            Panel.fit(
                "\n".join([f"[bold]{name}:[/bold] {cmd}" for name, cmd in fixes]),
                title="[yellow]Suggested Fixes[/yellow]",
                border_style="yellow",
            )
        )

    return all_passed


def run_fixes(results: list[tuple[str, bool, str, str | None]]) -> None:
    """Attempt to run fix commands for failed checks."""
    for name, passed, message, fix in results:
        if passed or not fix:
            continue

        console.print(f"\n[bold]Attempting to fix: {name}[/bold]")
        console.print(f"  Running: {fix}")

        # Only auto-run safe commands
        safe_commands = [
            "cargo build",
            "uv sync",
            "cp ",
        ]

        is_safe = any(fix.startswith(cmd) for cmd in safe_commands)
        if not is_safe:
            console.print(f"  [yellow]Skipped (manual action required): {fix}[/yellow]")
            continue

        try:
            result = subprocess.run(
                fix,
                check=False,
                shell=True,
                cwd=str(REPO_ROOT),
                capture_output=True,
                text=True,
                timeout=300,
            )
            if result.returncode == 0:
                console.print("  [green]Success![/green]")
            else:
                console.print(f"  [red]Failed:[/red] {result.stderr[:200]}")
        except (subprocess.TimeoutExpired, OSError, FileNotFoundError) as e:
            console.print(f"  [red]Error:[/red] {e}")


def main(argv: list[str] | None = None) -> None:
    """
    SMI Benchmark Doctor - Validate your environment.

    Run without arguments for a quick check, or with --full for comprehensive validation.
    """
    parser = argparse.ArgumentParser(
        description="SMI Benchmark Doctor - Environment validation and troubleshooting",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  smi-bench-doctor              # Quick environment check
  smi-bench-doctor --full       # Full check including corpus and Sui CLI
  smi-bench-doctor --fix        # Attempt to fix common issues
  smi-bench-doctor --corpus-root ../sui-packages/packages/mainnet_most_used
        """,
    )
    parser.add_argument(
        "--full",
        action="store_true",
        help="Run full checks including corpus and Sui CLI",
    )
    parser.add_argument(
        "--fix",
        action="store_true",
        help="Attempt to automatically fix common issues",
    )
    parser.add_argument(
        "--corpus-root",
        type=Path,
        help="Path to bytecode corpus (for full check)",
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        help="Optional manifest file to validate against corpus",
    )
    args = parser.parse_args(argv)

    console.print("[bold blue]SMI Benchmark Doctor[/bold blue]")
    console.print()

    results = run_checks(
        full=args.full,
        corpus_root=args.corpus_root,
        manifest=args.manifest,
    )

    all_passed = print_results(results)

    if args.fix and not all_passed:
        console.print()
        run_fixes(results)
        # Re-run checks after fixes
        console.print("\n[bold]Re-checking after fixes...[/bold]\n")
        results = run_checks(full=args.full, corpus_root=args.corpus_root, manifest=args.manifest)
        all_passed = print_results(results)

    console.print()
    if all_passed:
        console.print("[bold green]✓ All checks passed! Environment is ready.[/bold green]")
    else:
        console.print("[bold red]✗ Some checks failed. See suggested fixes above.[/bold red]")
        sys.exit(1)


if __name__ == "__main__":
    main()
