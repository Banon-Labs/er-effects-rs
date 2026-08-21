#!/usr/bin/env python3
"""Run rustfmt on experiment include children that `cargo fmt` cannot see.

Several files under `crates/er-effects-rs/src/experiments/` are pulled into the
crate with `include!`. Cargo/rustfmt only walks the module graph, so those files
can drift forever while `cargo fmt --all -- --check` stays green. This guard
checks the real files directly until the include shims are converted to modules.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


DEFAULT_EXPERIMENTS_DIR = Path("crates/er-effects-rs/src/experiments")


def rust_files(root: Path) -> list[Path]:
    return sorted(path for path in root.rglob("*.rs") if path.is_file())


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="repository root (default: inferred from this script)",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="rewrite files instead of checking formatting",
    )
    args = parser.parse_args()

    repo_root = args.root.resolve()
    experiments_dir = repo_root / DEFAULT_EXPERIMENTS_DIR
    if not experiments_dir.is_dir():
        raise SystemExit(f"experiments directory not found: {experiments_dir}")

    files = rust_files(experiments_dir)
    if not files:
        raise SystemExit(f"no Rust files found under {experiments_dir}")

    cmd = ["rustfmt", "--edition", "2024"]
    if not args.apply:
        cmd.append("--check")
    cmd.extend(str(path) for path in files)

    print(
        f"checking rustfmt for {len(files)} experiment Rust files under "
        f"{DEFAULT_EXPERIMENTS_DIR}"
    )
    try:
        result = subprocess.run(cmd, cwd=repo_root, check=False, timeout=30)
    except FileNotFoundError:
        print("missing required command: rustfmt", file=sys.stderr)
        return 127
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
