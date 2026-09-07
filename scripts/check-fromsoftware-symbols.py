#!/usr/bin/env python3
"""Fail when this repo names a `fromsoftware-rs` item the CI-pinned revision does not have.

Why this is a gate and not a NOTE
---------------------------------
The workspace uses `../fromsoftware-rs` path dependencies, and CI clones that sibling at one
exact revision (`FROMSOFTWARE_RS_REV` in `.github/workflows/check.yml`). A developer's sibling
is whatever they have checked out -- frequently a fork branch carrying types upstream has not
merged. Nothing reconciles the two, so `scripts/check.sh` compiles against the fork, goes green,
and CI then fails on `unresolved import` for a name that only ever existed on one machine.

That is not hypothetical: PRs #322 and #323 could not compile in CI at all because they used
`eldenring::cs::MsgRepositoryImp`, which exists only in a local fork. Both were locally green.

Rebuilding the whole workspace against the pinned revision would catch it, and costs a second
full cross-compile. This is the cheap 99%: extract every `fromsoftware-rs` item this repo names
by path, and confirm the pinned revision defines it. Pure text against `git show`, no checkout,
no compile, no network.

What it cannot see
------------------
Type-level drift -- a field that moved, a signature that changed, a trait that lost a method.
Only a real build catches those. This gate is about the failure that actually happened twice:
naming something upstream does not have at all.

Usage:
    python3 scripts/check-fromsoftware-symbols.py
    python3 scripts/check-fromsoftware-symbols.py --selftest

Skips (exit 0) when the sibling clone is absent or does not contain the pinned revision -- a
machine without it cannot answer the question, and a gate that fails on absence would just be
noise. CI always has it.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "check.yml"
CRATES_DIR = REPO_ROOT / "crates"

# Where the sibling clone lives. Not configurable on purpose: the path dependencies are literally
# `../fromsoftware-rs`, so this is the only directory cargo will ever read, and letting the check
# look somewhere else would let it pass against a tree the build never sees.
SIBLING = REPO_ROOT.parent / "fromsoftware-rs"

# Every `git` call here is a local object read on an already-cloned repository -- no network, no
# checkout. `scripts/check-no-timeouts.py` caps agent-run subprocesses at 30s, and that is ample:
# the slowest of these (`cat-file --batch` over the tree's ~700 .rs blobs) is well under a second.
GIT_TIMEOUT_SECONDS = 30

# `use eldenring::cs::{A, B}` / `use eldenring::cs::A` / bare `eldenring::cs::A::method()`.
# Only PascalCase leaves are collected: lowercase segments are modules, which a missing-item
# check would report as false positives (`cs`, `fd4`, `param`).
PATH_ITEM = re.compile(
    r"\b(eldenring|fromsoftware_shared)((?:::[a-z_][a-z0-9_]*)*)::(\{[^}]*\}|[A-Za-z_][A-Za-z0-9_]*)"
)
PASCAL = re.compile(r"^[A-Z][A-Za-z0-9_]*$")


def pinned_rev(workflow_text: str) -> str | None:
    match = re.search(r"^\s*FROMSOFTWARE_RS_REV:\s*([0-9a-fA-F]{7,40})\s*$", workflow_text, re.M)
    return match.group(1) if match else None


def referenced_items(sources: dict[str, str]) -> dict[str, set[str]]:
    """Every PascalCase item named through an `eldenring` / `fromsoftware_shared` path."""
    found: dict[str, set[str]] = {}
    for path, text in sources.items():
        for _crate, _mods, leaf in PATH_ITEM.findall(text):
            names = (
                [n.strip() for n in leaf.strip("{}").split(",")]
                if leaf.startswith("{")
                else [leaf]
            )
            for name in names:
                # `Type as Alias` in a use list: the imported name is the left half.
                name = name.split(" as ")[0].strip()
                if PASCAL.match(name):
                    found.setdefault(name, set()).add(path)
    return found


def pinned_sources(sibling: Path, rev: str) -> str | None:
    """Concatenate every `.rs` file in the pinned revision, or None when it is not available."""
    probe = subprocess.run(
        ["git", "-C", str(sibling), "cat-file", "-t", rev],
        capture_output=True,
        text=True,
        timeout=GIT_TIMEOUT_SECONDS,
    )
    if probe.returncode != 0 or probe.stdout.strip() != "commit":
        return None
    listing = subprocess.run(
        ["git", "-C", str(sibling), "ls-tree", "-r", "--name-only", rev],
        capture_output=True,
        text=True,
        timeout=GIT_TIMEOUT_SECONDS,
    )
    if listing.returncode != 0:
        return None
    files = [f for f in listing.stdout.splitlines() if f.endswith(".rs")]
    if not files:
        return None
    blobs = subprocess.run(
        ["git", "-C", str(sibling), "cat-file", "--batch"],
        input="".join(f"{rev}:{f}\n" for f in files),
        capture_output=True,
        text=True,
        timeout=GIT_TIMEOUT_SECONDS,
    )
    return blobs.stdout if blobs.returncode == 0 else None


def defines(pinned_text: str, name: str) -> bool:
    """Does the pinned revision define this item under any name-introducing form?

    Deliberately generous. Upstream declares param tables through a macro table
    (`(Magic, MAGIC_PARAM_ST, 14)`), so a `struct|enum|trait` search alone reports real types as
    missing -- the first version of this check did exactly that for `Magic`. A false pass is a
    build error someone still sees; a false fail blocks a correct change, which is worse.
    """
    return re.search(rf"\b{re.escape(name)}\b", pinned_text) is not None


def repo_sources() -> dict[str, str]:
    return {
        str(p.relative_to(REPO_ROOT)): p.read_text(encoding="utf-8", errors="replace")
        for p in sorted(CRATES_DIR.rglob("*.rs"))
        if ".worktrees" not in p.parts and "target" not in p.parts
    }


def selftest() -> int:
    failures = 0

    def case(name: str, condition: bool) -> None:
        nonlocal failures
        if not condition:
            print(f"selftest FAIL: {name}", file=sys.stderr)
            failures += 1

    case("reads the pinned rev", pinned_rev("env:\n  FROMSOFTWARE_RS_REV: abc1234\n") == "abc1234")
    case("missing rev is None", pinned_rev("env:\n  OTHER: 1\n") is None)

    items = referenced_items(
        {
            "a.rs": "use eldenring::cs::{CSTaskImp, WorldChrMan};\n",
            "b.rs": "use eldenring::cs::MsgRepositoryImp;\nfromsoftware_shared::FromStatic;\n",
            "c.rs": "use eldenring::cs::CSTaskGroupIndex as Group;\n",
            "d.rs": "let x = eldenring::cs::chr_ins::module::Foo::new();\n",
        }
    )
    case("collects braced use lists", {"CSTaskImp", "WorldChrMan"} <= set(items))
    case("collects single imports", "MsgRepositoryImp" in items)
    case("collects shared-crate items", "FromStatic" in items)
    case("an aliased import records the real name", "CSTaskGroupIndex" in items)
    case("a deep module path still finds the leaf", "Foo" in items)
    case("module segments are not items", "cs" not in items and "chr_ins" not in items)
    case("records where each item came from", items["MsgRepositoryImp"] == {"b.rs"})

    # The regression this gate exists for, and the false positive that nearly broke it.
    upstream = "pub struct CSTaskImp;\n(Magic, MAGIC_PARAM_ST, 14),\n"
    case("a macro-declared param type counts as defined", defines(upstream, "Magic"))
    case("a present type passes", defines(upstream, "CSTaskImp"))
    case("a fork-only type is caught", not defines(upstream, "MsgRepositoryImp"))

    if failures:
        print(f"selftest: {failures} case(s) failed", file=sys.stderr)
        return 1
    print("[check-fromsoftware-symbols] selftest ok (12 cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    if not WORKFLOW.exists():
        print(f"[check-fromsoftware-symbols] missing {WORKFLOW}", file=sys.stderr)
        return 1
    rev = pinned_rev(WORKFLOW.read_text(encoding="utf-8"))
    if rev is None:
        print(
            "[check-fromsoftware-symbols] no FROMSOFTWARE_RS_REV in the check workflow -- the "
            "pinned revision is what this gate compares against, so its absence is a failure",
            file=sys.stderr,
        )
        return 1

    if not (SIBLING / ".git").exists():
        print(f"[check-fromsoftware-symbols] SKIP: no sibling clone at {SIBLING}")
        return 0
    pinned = pinned_sources(SIBLING, rev)
    if pinned is None:
        print(
            f"[check-fromsoftware-symbols] SKIP: {SIBLING} does not contain {rev[:12]} "
            "(fetch it to enable this check)"
        )
        return 0

    items = referenced_items(repo_sources())
    missing = {name: sorted(where) for name, where in items.items() if not defines(pinned, name)}
    if missing:
        print(
            f"[check-fromsoftware-symbols] FAIL: {len(missing)} item(s) named here are absent from "
            f"the CI-pinned fromsoftware-rs {rev[:12]}. Your sibling checkout has them; CI's will "
            "not, so this cannot compile there. Resolve it in THIS repo (AGENTS.md: never file "
            "upstream) -- read the value from the game yourself, as `er-game-base::rva` does for "
            "the message repository singleton.",
            file=sys.stderr,
        )
        for name, where in sorted(missing.items()):
            print(f"  - {name}: {', '.join(where[:4])}", file=sys.stderr)
        return 1

    print(
        f"[check-fromsoftware-symbols] ok -- {len(items)} fromsoftware-rs item(s) named here all "
        f"exist at the pinned {rev[:12]}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
