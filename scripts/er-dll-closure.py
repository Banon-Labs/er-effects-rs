#!/usr/bin/env python3
"""Which ME3-loadable DLLs does this branch actually affect?

Answers the question a generated profile has to get right: given the changes on this
branch, which cdylibs must be loaded for the run to be testing them -- and is that set
safe to load together?

THE DIFF BASE IS `origin/main`, ALWAYS, AND ALWAYS THE WORKING TREE
-------------------------------------------------------------------
Stacked branches are the norm here, and a PR near the tip of a stack has a tiny diff
against its immediate parent while the *stack* changes a great deal. Runtime-testing the
tip means testing everything below it, so the base is `merge-base(origin/main, HEAD)` --
never local `main` (which drifts) and never the parent branch.

The far end of the diff is the WORKING TREE, not `HEAD`. Cargo compiles what is on disk:
uncommitted edits and new untracked files are in the DLL whether or not they are committed.
Diffing to `HEAD` would omit the crate whose code is genuinely loaded, which is the one
failure this tool cannot afford.

WHY A CLOSURE AND NOT JUST THE TOUCHED CRATE
--------------------------------------------
`er-game-base` is a path dependency of 26 crates and `er-hook` of 15. Editing either
changes the code inside DLLs whose own directories were never touched, so "the crates you
edited" systematically under-reports. The walk is therefore over reverse dependencies.

That same fan-out is why the conflict table exists: a wide closure will happily propose
loading the product next to `er_loading_portrait_dll.dll`, which is documented in-tree as
a double-Present-hook corruption.

WHY CONFLICTS ARE RESOLVED LOUDLY RATHER THAN REFUSED OUTRIGHT
--------------------------------------------------------------
The first cut of this script refused on any conflicting pair. Measuring it against the real
graph killed that: a change to `er-game-base` closes over all 16 shells and hits all five
conflicts, and `er-hook` closes over 12 and hits the same five. A tool that refuses on the
two most-edited shared crates is a tool nobody can use.

The danger was never the dropping -- it was dropping *silently*, which means launching while
believing you are testing a DLL that is not loaded. So a conflict against the product is
resolved in the product's favour and the exclusion is carried everywhere the run is
described: this output, the profile header, the running block, and the run state. An
excluded DLL is a stated non-result, not an omission.

Two cases still refuse outright, because neither can be resolved without guessing:
  * a conflict between two NON-product DLLs -- nothing ranks them;
  * a DLL named explicitly with `--with` that a conflict would exclude -- an explicit
    request must never be quietly overridden, and must never corrupt the process either.

Usage:
    python3 scripts/er-dll-closure.py                 # human-readable
    python3 scripts/er-dll-closure.py --json          # machine-readable
    python3 scripts/er-dll-closure.py --no-fetch      # skip the origin refresh
    python3 scripts/er-dll-closure.py --selftest

Exit status: 0 sound, 1 hard error (bad base, git failure), 2 conflicting closure.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import subprocess
import sys
import tomllib
from collections import deque
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = REPO_ROOT / "crates"
CONFLICTS_TOML = REPO_ROOT / "scripts" / "me3-dll-conflicts.toml"
DLL_LIST = REPO_ROOT / "scripts" / "me3-dll-list.py"

# Every agent-run shell op in this repo is capped well under a minute; a hung `git fetch`
# must fail fast rather than eat the caller's budget.
GIT_TIMEOUT_SECONDS = 25

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_CONFLICT = 2


class ClosureError(RuntimeError):
    """A condition the caller must fix -- never something to paper over with a default."""


def git(*args: str, cwd: Path = REPO_ROOT, check: bool = True) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=cwd,
        text=True,
        capture_output=True,
        timeout=GIT_TIMEOUT_SECONDS,
    )
    if check and proc.returncode != 0:
        raise ClosureError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
    return proc.stdout


def shipped_pairs() -> list[tuple[str, str]]:
    spec = importlib.util.spec_from_file_location("me3_dll_list", DLL_LIST)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.dll_pairs()


def path_dependents(crates_dir: Path = CRATES_DIR) -> dict[str, set[str]]:
    """dep -> {packages that path-depend on it}, read from each crate's Cargo.toml."""
    reverse: dict[str, set[str]] = {}
    for manifest in sorted(crates_dir.glob("*/Cargo.toml")):
        package = manifest.parent.name
        text = manifest.read_text(encoding="utf-8", errors="replace")
        for dep in re.findall(r"^\s*([A-Za-z0-9_-]+)\s*=\s*\{[^}]*path\s*=", text, re.M):
            reverse.setdefault(dep, set()).add(package)
    return reverse


def affected_packages(seeds: set[str], reverse: dict[str, set[str]]) -> set[str]:
    """Transitive closure of `seeds` under 'is a path dependency of'."""
    seen = set(seeds)
    queue = deque(seeds)
    while queue:
        current = queue.popleft()
        for dependent in reverse.get(current, ()):
            if dependent not in seen:
                seen.add(dependent)
                queue.append(dependent)
    return seen


def owning_packages(changed: list[str]) -> tuple[set[str], list[str]]:
    """Split changed paths into (crate packages they belong to, paths owned by no crate)."""
    packages: set[str] = set()
    outside: list[str] = []
    for path in changed:
        parts = Path(path).parts
        if len(parts) >= 2 and parts[0] == "crates":
            packages.add(parts[1])
        else:
            outside.append(path)
    return packages, outside


def changed_paths(base: str) -> list[str]:
    """Paths differing between `base` and the WORKING TREE, plus untracked non-ignored files."""
    tracked = git("diff", "--name-only", base).splitlines()
    untracked = git(
        "ls-files", "--others", "--exclude-standard"
    ).splitlines()
    return sorted({line.strip() for line in (*tracked, *untracked) if line.strip()})


def resolve_base(base_ref: str, fetch: bool) -> tuple[str, str]:
    """Return (merge_base_sha, head_sha), refreshing `base_ref` from the remote first.

    A stale local `origin/main` silently narrows the diff, so the fetch is the default and
    skipping it is an explicit choice the caller has to make.
    """
    if fetch and "/" in base_ref:
        remote, branch = base_ref.split("/", 1)
        git("fetch", remote, branch, check=False)
    verify = subprocess.run(
        ["git", "rev-parse", "--verify", f"{base_ref}^{{commit}}"],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        timeout=GIT_TIMEOUT_SECONDS,
    )
    if verify.returncode != 0:
        raise ClosureError(
            f"base ref {base_ref!r} does not resolve. Fetch it, or pass --base with one that does."
        )
    merge_base = git("merge-base", base_ref, "HEAD").strip()
    head = git("rev-parse", "HEAD").strip()
    return merge_base, head


PRODUCT_PACKAGE = "er-effects-rs"


def find_conflicts(packages: set[str], table: dict) -> list[dict]:
    hits = []
    for entry in table.get("conflict", []):
        a, b = entry.get("a"), entry.get("b")
        if a in packages and b in packages:
            hits.append(
                {
                    "a": a,
                    "b": b,
                    "kind": entry.get("kind"),
                    "reason": " ".join((entry.get("reason") or "").split()),
                    "evidence": entry.get("evidence"),
                }
            )
    return hits


def resolve_conflicts(
    selected: set[str], table: dict, pinned: set[str]
) -> tuple[set[str], list[dict], list[dict]]:
    """Drop the non-product side of each conflict.

    Returns (kept, excluded, unresolvable). `pinned` names packages the caller asked for
    explicitly: excluding one of those would silently override a direct request, so it is
    reported as unresolvable instead.
    """
    kept = set(selected)
    excluded: list[dict] = []
    unresolvable: list[dict] = []

    for conflict in find_conflicts(selected, table):
        a, b = conflict["a"], conflict["b"]
        if PRODUCT_PACKAGE not in (a, b):
            unresolvable.append({**conflict, "why": "neither side is the product; nothing ranks them"})
            continue
        loser = b if a == PRODUCT_PACKAGE else a
        if loser in pinned:
            unresolvable.append(
                {**conflict, "why": f"{loser} was requested with --with but conflicts with the product"}
            )
            continue
        if loser in kept:
            kept.discard(loser)
            excluded.append(
                {
                    "package": loser,
                    "kind": conflict["kind"],
                    "because": conflict["reason"],
                    "evidence": conflict["evidence"],
                }
            )

    return kept, excluded, unresolvable


def compute(base_ref: str, fetch: bool, pinned: set[str] | None = None) -> dict:
    pinned = pinned or set()
    merge_base, head = resolve_base(base_ref, fetch)
    changed = changed_paths(merge_base)
    seeds, outside = owning_packages(changed)
    reverse = path_dependents()
    affected = affected_packages(seeds, reverse)

    pairs = shipped_pairs()
    shipped = {package for package, _ in pairs}
    artifact_of = dict(pairs)

    unknown = pinned - shipped
    if unknown:
        raise ClosureError(
            f"--with names packages that are not ME3-loadable shells: {', '.join(sorted(unknown))}"
        )

    candidates = set(affected & shipped) | pinned
    fallback = None
    if not candidates:
        # Nothing on this branch feeds any DLL (docs, scripts, CI). A run still has to load
        # something, and the product is the baseline every conflict is expressed against --
        # and a one-DLL closure has nothing for it to conflict with.
        candidates = {PRODUCT_PACKAGE}
        fallback = "no changed file feeds any cdylib; falling back to the product DLL alone"

    with CONFLICTS_TOML.open("rb") as handle:
        table = tomllib.load(handle)
    kept, excluded, unresolvable = resolve_conflicts(candidates, table, pinned)

    selected = sorted(kept)
    dirty = bool(git("status", "--porcelain").strip())

    return {
        "base_ref": base_ref,
        "merge_base": merge_base,
        "head": head,
        "dirty": dirty,
        "changed_file_count": len(changed),
        "changed_outside_crates": len(outside),
        "seed_crates": sorted(seeds),
        "affected_crates": sorted(affected),
        "pinned": sorted(pinned),
        "packages": selected,
        "artifacts": [f"{artifact_of[p]}.dll" for p in selected],
        "excluded": [
            {**entry, "artifact": f"{artifact_of[entry['package']]}.dll"} for entry in excluded
        ],
        "fallback": fallback,
        "unresolvable": unresolvable,
    }


def render(result: dict) -> str:
    lines = [
        f"base      {result['base_ref']} -> {result['merge_base'][:12]}",
        f"head      {result['head'][:12]}{'  (WORKING TREE IS DIRTY)' if result['dirty'] else ''}",
        f"changed   {result['changed_file_count']} paths "
        f"({result['changed_outside_crates']} outside crates/)",
        f"seeds     {', '.join(result['seed_crates']) or '(none)'}",
        f"affected  {len(result['affected_crates'])} crates",
        "",
        "DLLs to load:",
    ]
    lines.extend(f"  {artifact}" for artifact in result["artifacts"])
    if result["fallback"]:
        lines.append(f"  ^ {result['fallback']}")
    if result["excluded"]:
        lines.append("")
        lines.append("EXCLUDED -- affected by this branch, but NOT loaded, so NOT tested here:")
        for entry in result["excluded"]:
            lines.append(f"  {entry['artifact']}   [{entry['kind']}]")
            lines.append(f"      {entry['because']}")
            lines.append(f"      evidence: {entry['evidence']}")
    if result["unresolvable"]:
        lines.append("")
        lines.append("REFUSING -- this closure cannot be loaded as one profile:")
        for conflict in result["unresolvable"]:
            lines.append(f"  {conflict['a']}  X  {conflict['b']}   [{conflict['kind']}]")
            lines.append(f"      {conflict['why']}")
            lines.append(f"      {conflict['reason']}")
    return "\n".join(lines)


def selftest() -> int:
    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
            print(f"  FAIL {label}")
        else:
            print(f"  ok   {label}")

    reverse = {"base": {"mid", "shell-a"}, "mid": {"shell-b"}, "lonely": set()}
    check(
        affected_packages({"base"}, reverse) == {"base", "mid", "shell-a", "shell-b"},
        "closure follows reverse deps transitively (base -> mid -> shell-b)",
    )
    check(
        affected_packages({"lonely"}, reverse) == {"lonely"},
        "a crate nothing depends on closes over only itself",
    )
    check(affected_packages(set(), reverse) == set(), "an empty seed set stays empty")

    seeds, outside = owning_packages(
        ["crates/er-hook/src/lib.rs", "docs/x.md", "crates/er-gfx/Cargo.toml", "README.md"]
    )
    check(seeds == {"er-hook", "er-gfx"}, "changed paths map to their owning crate")
    check(outside == ["docs/x.md", "README.md"], "non-crate paths are reported, not dropped")

    table = {
        "conflict": [
            {"a": "prod", "b": "bad", "kind": "hook-collision", "reason": "r", "evidence": "e"}
        ]
    }
    check(len(find_conflicts({"prod", "bad"}, table)) == 1, "a conflicting pair is detected")
    check(find_conflicts({"prod", "safe"}, table) == [], "a non-conflicting pair passes")
    check(find_conflicts({"bad"}, table) == [], "one half of a pair alone is not a conflict")

    product_table = {
        "conflict": [
            {
                "a": PRODUCT_PACKAGE,
                "b": "bad",
                "kind": "hook-collision",
                "reason": "r",
                "evidence": "e",
            }
        ]
    }
    kept, excluded, unresolvable = resolve_conflicts(
        {PRODUCT_PACKAGE, "bad", "safe"}, product_table, set()
    )
    check(kept == {PRODUCT_PACKAGE, "safe"}, "a product conflict drops the non-product side")
    check(
        [e["package"] for e in excluded] == ["bad"] and not unresolvable,
        "the dropped DLL is reported as an exclusion, not lost",
    )

    _, _, pinned_block = resolve_conflicts(
        {PRODUCT_PACKAGE, "bad"}, product_table, pinned={"bad"}
    )
    check(
        len(pinned_block) == 1 and "--with" in pinned_block[0]["why"],
        "an explicitly requested DLL is never quietly excluded",
    )

    peer_table = {
        "conflict": [
            {"a": "safe", "b": "bad", "kind": "hook-collision", "reason": "r", "evidence": "e"}
        ]
    }
    _, _, peer_block = resolve_conflicts({"safe", "bad"}, peer_table, set())
    check(
        len(peer_block) == 1 and "nothing ranks them" in peer_block[0]["why"],
        "a conflict between two non-product DLLs refuses",
    )

    # The measurement that forced loud-resolution over outright refusal: the two most-edited
    # shared crates close over every conflict in the real table, and must still yield a
    # loadable profile.
    live_table = tomllib.loads(CONFLICTS_TOML.read_text(encoding="utf-8"))
    live_rev = path_dependents()
    live_shipped = {package for package, _ in shipped_pairs()}
    for seed in ("er-game-base", "er-hook"):
        closure = affected_packages({seed}, live_rev) & live_shipped
        kept, excluded, unresolvable = resolve_conflicts(closure, live_table, set())
        check(
            not unresolvable and PRODUCT_PACKAGE in kept and excluded,
            f"a {seed} change still yields a loadable profile ({len(kept)} kept, {len(excluded)} excluded)",
        )
        check(
            find_conflicts(kept, live_table) == [],
            f"the {seed} profile that survives has no remaining conflicts",
        )

    # The real workspace must agree with the premise this tool is built on.
    live = path_dependents()
    check(
        len(live.get("er-game-base", ())) > 10,
        f"er-game-base really is a wide dependency ({len(live.get('er-game-base', ()))} dependents)",
    )
    shipped = {package for package, _ in shipped_pairs()}
    check(
        affected_packages({"er-game-base"}, live) & shipped >= {"er-effects-rs"},
        "a change to er-game-base reaches the product DLL",
    )

    print("selftest:", "PASS" if ok else "FAIL")
    return EXIT_OK if ok else EXIT_ERROR


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="origin/main", help="diff base (default: origin/main)")
    parser.add_argument(
        "--no-fetch",
        action="store_true",
        help="do not refresh the base ref from the remote first (a stale base narrows the diff)",
    )
    parser.add_argument(
        "--with",
        dest="pinned",
        action="append",
        default=[],
        metavar="PACKAGE",
        help="force-include a shell (repeatable); refuses rather than excluding it on conflict",
    )
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    try:
        result = compute(args.base, fetch=not args.no_fetch, pinned=set(args.pinned))
    except ClosureError as err:
        print(f"er-dll-closure: {err}", file=sys.stderr)
        return EXIT_ERROR
    except subprocess.TimeoutExpired:
        print(
            f"er-dll-closure: a git call exceeded {GIT_TIMEOUT_SECONDS}s (network down?)",
            file=sys.stderr,
        )
        return EXIT_ERROR

    print(json.dumps(result, indent=2) if args.json else render(result))
    return EXIT_CONFLICT if result["unresolvable"] else EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
