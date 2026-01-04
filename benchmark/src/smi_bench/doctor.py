from __future__ import annotations

import argparse
import sys
from pathlib import Path

from rich.console import Console
from rich.table import Table

from smi_bench.dataset import iter_package_dirs
from smi_bench.utils import safe_read_lines

console = Console()


def check_corpus(corpus_root: Path) -> bool:
    console.print(f"[bold]Checking corpus root:[/bold] {corpus_root}")
    if not corpus_root.exists():
        console.print(f"[red]Error: corpus_root does not exist: {corpus_root}[/red]")
        return False
    if not corpus_root.is_dir():
        console.print(f"[red]Error: corpus_root is not a directory: {corpus_root}[/red]")
        return False

    total_dirs = 0
    missing_metadata = 0
    missing_bytecode = 0
    valid_packages = 0

    table = Table(title="Corpus Statistics")
    table.add_column("Metric", style="cyan")
    table.add_column("Value", style="magenta")

    for pkg_dir in iter_package_dirs(corpus_root):
        total_dirs += 1
        resolved = pkg_dir.resolve()

        has_metadata = (resolved / "metadata.json").exists()
        has_bytecode = (resolved / "bytecode_modules").is_dir()

        if not has_metadata:
            missing_metadata += 1
        if not has_bytecode:
            missing_bytecode += 1

        if has_metadata and has_bytecode:
            valid_packages += 1

    table.add_row("Total directories scanned", str(total_dirs))
    table.add_row("Valid packages (metadata + bytecode)", str(valid_packages))
    table.add_row("Missing metadata.json", str(missing_metadata))
    table.add_row("Missing bytecode_modules/", str(missing_bytecode))

    console.print(table)

    if valid_packages == 0:
        console.print("[red]Error: No valid packages found in corpus![/red]")
        return False

    return True


def check_manifest(corpus_root: Path, manifest_path: Path) -> bool:
    console.print(f"[bold]Checking manifest:[/bold] {manifest_path}")
    lines = safe_read_lines(manifest_path, context="doctor-manifest")
    if not lines and not manifest_path.exists():
        console.print(f"[red]Error: manifest file does not exist: {manifest_path}[/red]")
        return False

    package_ids = [line.split("#")[0].strip() for line in lines if line.split("#")[0].strip()]

    missing_in_corpus = []
    for pkg_id in package_ids:
        # Check if 0x...
        if not pkg_id.startswith("0x"):
            console.print(f"[yellow]Warning: package_id '{pkg_id}' in manifest does not start with 0x[/yellow]")
            continue

        # Try to find it in the corpus
        prefix = pkg_id[:4].lower()  # 0x00
        pkg_dir = corpus_root / prefix / pkg_id
        if not pkg_dir.exists():
            missing_in_corpus.append(pkg_id)

    if missing_in_corpus:
        console.print(f"[red]Error: {len(missing_in_corpus)} packages from manifest are missing in corpus[/red]")
        if len(missing_in_corpus) <= 10:
            for m in missing_in_corpus:
                console.print(f"  - {m}")
        else:
            for m in missing_in_corpus[:10]:
                console.print(f"  - {m}")
            console.print(f"  ... and {len(missing_in_corpus) - 10} more")
        return False

    console.print(f"[green]Success: All {len(package_ids)} packages in manifest found in corpus.[/green]")
    return True


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description="Corpus and Manifest integrity doctor")
    parser.add_argument("--corpus-root", type=Path, required=True, help="Path to bytecode corpus")
    parser.add_argument("--manifest", type=Path, help="Optional manifest file to validate against corpus")
    args = parser.parse_args(argv)

    ok = check_corpus(args.corpus_root)
    if args.manifest:
        ok = check_manifest(args.corpus_root, args.manifest) and ok

    if not ok:
        sys.exit(1)
    console.print("[bold green]Doctor found no critical issues.[/bold green]")


if __name__ == "__main__":
    main()
