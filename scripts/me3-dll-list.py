#!/usr/bin/env python3
"""Single source of truth for "which cdylibs does this workspace ship".

The list already exists, exactly once, as the `me3_shells=( pkg:artifact ... )`
array in `scripts/check-rust-build.sh` -- the gate that proves every ME3-loadable
shell still links -- and `scripts/check-me3-shell-coverage.py` already owns the
parser for it (and separately proves the array is complete and not stale). This
module reuses that parser rather than adding a second regex over the same text:
two independent parsers of one array is the drift this repo keeps closing.

The product crate `er-effects-rs` is not in that array (the bare `cargo xwin
build` covers it via `default-members`), so it is prepended here.

Modes:
    --cargo-args   ->  -p er-effects-rs -p er-armament-icons ...
    --artifacts    ->  er_effects_rs.dll er_armament_icons.dll ...
    --pairs        ->  er-effects-rs:er_effects_rs ...   (one per line)

Note the artifact name is NOT the package name with dashes swapped for
underscores -- four crates override `[lib] name` (er-better-refills-dll produces
er_better_refills.dll, and so on), which is why the array stores both halves.
"""

from __future__ import annotations

import argparse
import importlib.util
import pathlib
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CHECK_SCRIPT = REPO_ROOT / "scripts" / "check-rust-build.sh"
COVERAGE_CHECK = REPO_ROOT / "scripts" / "check-me3-shell-coverage.py"


def _coverage_module():
    """Import scripts/check-me3-shell-coverage.py (dashed name, so not importable directly)."""
    spec = importlib.util.spec_from_file_location("check_me3_shell_coverage", COVERAGE_CHECK)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def dll_pairs() -> list[tuple[str, str]]:
    """Return [(package, artifact_stem), ...] for every shipped cdylib."""
    coverage = _coverage_module()
    pairs = coverage.parse_me3_shells(CHECK_SCRIPT.read_text(encoding="utf-8"))
    if not pairs:
        raise SystemExit(
            f"{CHECK_SCRIPT}: the me3_shells array parsed as empty.\n"
            "That array is the single source of truth for the shipped cdylib set; "
            "check-me3-shell-coverage.py owns the parser."
        )
    product = (coverage.PRODUCT_PACKAGE, coverage.PRODUCT_PACKAGE.replace("-", "_"))
    return [product] + pairs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--cargo-args", action="store_true")
    group.add_argument("--artifacts", action="store_true")
    group.add_argument("--pairs", action="store_true")
    args = parser.parse_args()

    pairs = dll_pairs()
    if args.cargo_args:
        print(" ".join(f"-p {pkg}" for pkg, _ in pairs))
    elif args.artifacts:
        print(" ".join(f"{artifact}.dll" for _, artifact in pairs))
    else:
        for pkg, artifact in pairs:
            print(f"{pkg}:{artifact}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
