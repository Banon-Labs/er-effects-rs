#!/usr/bin/env python3
"""Which ME3-loadable DLLs does this branch actually affect?

Answers the question a generated profile has to get right: given the changes on this
branch, which cdylibs must be loaded for the run to be testing them -- and is that set
safe to load together?

The diff base is `origin/main`, always, and always the working tree
-------------------------------------------------------------------
Stacked branches are the norm here, and a PR near the tip of a stack has a tiny diff
against its immediate parent while the *stack* changes a great deal. Runtime-testing the
tip means testing everything below it, so the base is `merge-base(origin/main, HEAD)` --
never local `main` (which drifts) and never the parent branch.

The far end of the diff is the working tree, not `HEAD`. Cargo compiles what is on disk:
uncommitted edits and new untracked files are in the DLL whether or not they are committed.
Diffing to `HEAD` would omit the crate whose code is genuinely loaded, which is the one
failure this tool cannot afford.

Why a closure and not just the touched crate
--------------------------------------------
`er-game-base` is a path dependency of 26 crates and `er-hook` of 15. Editing either
changes the code inside DLLs whose own directories were never touched, so "the crates you
edited" systematically under-reports. The walk is therefore over reverse dependencies.

That same fan-out is why the conflict table exists: a wide closure will happily propose
loading the product next to `er_loading_portrait.dll`, which is documented in-tree as
a double-Present-hook corruption.

Why conflicts are resolved loudly rather than refused outright
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

Default-on shells, and why the diff is not the only input
--------------------------------------------------------
Selecting on the diff answers "what is this run testing". It does not answer "what does the
player expect to be playing", and the two came apart on 2026-09-08: `er-quickload` has no
reverse dependents, so a branch editing only the product closes over exactly one shell and
every gameplay companion leaves the profile without a word. The `[always]` table in the
conflict file is the standing half of `--with` -- packages unioned into every closure after
the diff has been walked. They are not pinned, so conflict ranking still drops them, and
`--without` still removes them; what they skip is having to be named on the command line.

Two cases still refuse outright, because neither can be resolved without guessing:
  * a conflict between two non-product DLLs -- nothing ranks them;
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
    """Paths differing between `base` and the working tree, plus untracked non-ignored files."""
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


PRODUCT_PACKAGE = "er-quickload"
# The one conflict kind that is a claim about who is driving, not about corruption. `--agent-driven`
# may accept it; every other kind stays fatal. See `resolve_conflicts`.
AGENT_DRIVEN_CONFLICT_KIND = "drives-input"


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
    selected: set[str], table: dict, pinned: set[str], agent_driven: bool = False
) -> tuple[set[str], list[dict], list[dict]]:
    """Drop opt-in-only DLLs, then the non-product side of each conflict.

    Returns (kept, excluded, unresolvable). `pinned` names packages the caller asked for
    explicitly: excluding one of those would silently override a direct request, so a pinned
    conflict loser is reported as unresolvable instead, and a pinned opt-in-only DLL is simply
    kept -- naming it with `--with` is the opt-in.

    `agent_driven` accepts the one conflict kind that is a statement about who is driving rather
    than about corruption: `drives-input`. Its whole content is "a run that loads this cannot be
    described as user-driven", which is not a defect when the run is declared agent-driven --
    AGENTS.md's 2026-07-22 standing order requires the agent to drive every input, and the
    input harness is how. It is a narrow admission, not a bypass: no other kind is affected, the
    loser must still be `--with`-pinned, and the acceptance is recorded in `excluded` (kind
    `drives-input-accepted`) so the run block states that the user was not in control.
    """
    kept = set(selected)
    excluded: list[dict] = []
    unresolvable: list[dict] = []
    accepted: list[dict] = []

    # OPT-in-only DLLs come out first, before any conflict ranking. They are co-loadable --
    # nothing about them corrupts a run -- but they change the game the user sees, and a
    # dependency-closure walk is not consent. A gameplay mod nobody asked for arriving because
    # it happens to depend on a crate this branch touched is how a run stops being the run the
    # user wanted. `--with` is the consent, and it is the only way in.
    for name in sorted(set(table.get("opt_in_only", {})) & kept):
        if name in pinned:
            continue
        kept.discard(name)
        excluded.append(
            {
                "package": name,
                "kind": "opt-in-only",
                "because": " ".join((table["opt_in_only"][name] or "").split()),
                "evidence": "scripts/me3-dll-conflicts.toml [opt_in_only]",
            }
        )

    # Product-ranked conflicts first, and a pair whose members are already gone is no longer a
    # pair. The single pass this replaced evaluated every conflict against the set as it stood
    # before any exclusion, so two shells that conflict with each other were reported
    # unresolvable even when the product had already displaced both of them -- a refusal to emit
    # any profile at all, over two DLLs the profile was never going to carry. Measured on
    # 2026-09-11 with `er-build-import` and `er-quit-menu`, which conflict with the product and,
    # since that day, with each other.
    ranked = [c for c in find_conflicts(kept, table) if PRODUCT_PACKAGE in (c["a"], c["b"])]
    unranked = [c for c in find_conflicts(kept, table) if PRODUCT_PACKAGE not in (c["a"], c["b"])]
    for conflict in ranked + unranked:
        a, b = conflict["a"], conflict["b"]
        if PRODUCT_PACKAGE not in (a, b):
            # Both sides still standing, or there is nothing left to rank.
            if a not in kept or b not in kept:
                continue
            unresolvable.append({**conflict, "why": "neither side is the product; nothing ranks them"})
            continue
        loser = b if a == PRODUCT_PACKAGE else a
        if loser in pinned:
            if agent_driven and conflict["kind"] == AGENT_DRIVEN_CONFLICT_KIND:
                # Not `excluded` -- the package is kept. It goes in its own list so the run block
                # can say "the player was not in control" without listing a loaded DLL under a
                # heading that means "withheld".
                accepted.append(
                    {
                        "package": loser,
                        "kind": AGENT_DRIVEN_CONFLICT_KIND,
                        "because": (
                            "KEPT, and this run is therefore AGENT-DRIVEN: the harness writes the "
                            "game's input memory every frame, so the player is NOT in control. "
                            "Accepted only because --agent-driven declared it. " + conflict["reason"]
                        ),
                        "evidence": conflict["evidence"],
                    }
                )
                continue
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

    return kept, excluded, unresolvable, accepted


def compute(
    base_ref: str,
    fetch: bool,
    pinned: set[str] | None = None,
    dropped: set[str] | None = None,
    agent_driven: bool = False,
) -> dict:
    pinned = pinned or set()
    dropped = dropped or set()
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
    unknown = dropped - shipped
    if unknown:
        raise ClosureError(
            f"--without names packages that are not ME3-loadable shells: "
            f"{', '.join(sorted(unknown))}"
        )
    # Asking for a DLL and refusing it in the same breath has no correct reading, and guessing
    # one would silently do the opposite of half the request.
    both = pinned & dropped
    if both:
        raise ClosureError(
            f"named by BOTH --with and --without: {', '.join(sorted(both))}"
        )
    # Excluding the product leaves a profile with nothing to express conflicts against, and
    # every companion chains onto its hook union. That is not a run worth staging.
    if PRODUCT_PACKAGE in dropped:
        raise ClosureError(
            f"--without cannot drop {PRODUCT_PACKAGE}: it is the baseline every conflict is "
            f"ranked against and the owner of the hook union the companions chain onto"
        )

    with CONFLICTS_TOML.open("rb") as handle:
        table = tomllib.load(handle)

    candidates = set(affected & shipped) | pinned
    fallback = None
    if not candidates:
        # Nothing on this branch feeds any DLL (docs, scripts, CI). A run still has to load
        # something, and the product is the baseline every conflict is expressed against --
        # and a one-DLL closure has nothing for it to conflict with.
        candidates = {PRODUCT_PACKAGE}
        # Not "the product alone": `[always]` is unioned in below, and a fallback line that
        # said "alone" beside a two-DLL list would be the report contradicting itself.
        fallback = (
            "no changed file feeds any cdylib; falling back to the product DLL as the baseline"
        )
    elif PRODUCT_PACKAGE not in candidates:
        # The product is never optional (bd er-effects-rs-l9tu, fixed 2026-09-04). `--with X` on a
        # tree whose changes feed no cdylib used to produce a closure of exactly X: naming any
        # package made `candidates` non-empty, which skipped the fallback above, and the product
        # left the profile without a word. That run is not merely surprising, it is unreadable --
        # the staged sidecar is still `er-quickload.toml` and the launcher's testimony step still
        # waits for the product's own `runtime-config: loaded` line, so the run either hangs at
        # testimony or prints a block crediting the product for a load that was not its.
        #
        # Unioning it in rather than refusing, because every companion chains onto the product's
        # hook union and `--without er-quickload` is already refused outright above: a closure
        # without it was never a thing the caller could legitimately ask for.
        candidates.add(PRODUCT_PACKAGE)
        fallback = (
            f"{PRODUCT_PACKAGE} was not selected by the changed files or by --with, and was "
            f"added: it owns the hook union the companions chain onto, and it is what the "
            f"sidecar and the launcher's load testimony both name"
        )

    # The standing half of `--with`, read from `[always]`. It is unioned in after the two
    # fallbacks above so neither is disturbed: "no changed file feeds any cdylib" stays a true
    # statement about the diff, and the product is still added for its own reasons rather than
    # because a companion dragged it in. These packages are not pinned, so a conflict against the
    # product still drops them and still reports them in `excluded` -- being default-on is a
    # decision about consent, never a licence to co-load something that corrupts the run.
    always = set(table.get("always", {})) & shipped
    added_by_default = sorted(always - candidates)
    candidates |= always

    kept, excluded, unresolvable, accepted = resolve_conflicts(
        candidates, table, pinned, agent_driven
    )

    # `--without` is applied last, after conflict ranking, so an exclusion cannot be undone by a
    # later rule -- and it is recorded in `excluded` with the same shape as a conflict drop, so
    # the run block says which DLL was withheld. A silent omission is how an A/B turns into two
    # runs nobody can tell apart. Its use case is the param-patching class: any DLL that mutates
    # a param row at runtime moves the Seamless lobby-key fingerprint and drops the player out
    # of matchmaking, and the only way to prove which one is to re-run without it.
    for name in sorted(dropped & kept):
        kept.discard(name)
        excluded.append(
            {
                "package": name,
                "kind": "withheld",
                "because": "excluded by --without on the command line",
                "evidence": "caller request",
            }
        )

    # Product first, then the rest alphabetically. me3 loads natives in profile order, and the
    # companions resolve the product's `er_effects_union_register` export to chain onto prologues it
    # already owns (scripts/me3-launch-lib.sh says the same). A plain `sorted()` put
    # `er-armament-icons` ahead of `er-quickload`, so the companion's install thread could run
    # before the product image was even loaded -- it would then find no export, fall back to its own
    # MinHook instance, and recreate the collision the [[shared]] entry exists to prevent. The
    # companion still polls briefly, so this is belt-and-braces rather than the sole guarantee.
    selected = sorted(kept)
    if PRODUCT_PACKAGE in kept:
        selected = [PRODUCT_PACKAGE] + [p for p in selected if p != PRODUCT_PACKAGE]
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
        "always": sorted(always),
        "added_by_default": added_by_default,
        "agent_driven": bool(agent_driven),
        "accepted_conflicts": accepted,
        "withheld": sorted(dropped),
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
    # A DLL the diff never reached is still in the profile, so say which and why. Silence here is
    # the same defect as a silent exclusion: someone reads this to know what the run was.
    if result.get("added_by_default"):
        lines.append(
            f"  ^ on by default, not because this branch reached them: "
            f"{', '.join(result['added_by_default'])} "
            f"(scripts/me3-dll-conflicts.toml [always])"
        )
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
    kept, excluded, unresolvable, _ = resolve_conflicts(
        {PRODUCT_PACKAGE, "bad", "safe"}, product_table, set()
    )
    check(kept == {PRODUCT_PACKAGE, "safe"}, "a product conflict drops the non-product side")
    check(
        [e["package"] for e in excluded] == ["bad"] and not unresolvable,
        "the dropped DLL is reported as an exclusion, not lost",
    )

    _, _, pinned_block, _ = resolve_conflicts(
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
    _, _, peer_block, _ = resolve_conflicts({"safe", "bad"}, peer_table, set())
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
        kept, excluded, unresolvable, _ = resolve_conflicts(closure, live_table, set())
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
        affected_packages({"er-game-base"}, live) & shipped >= {"er-quickload"},
        "a change to er-game-base reaches the product DLL",
    )

    # --- --without: an exclusion that is recorded, not silent ---------------------------
    # These call compute() through its argument-validation path only (no git), so they prove the
    # refusals without needing a repo state. The keep/record behaviour is proven on the same
    # resolve_conflicts + drop sequence compute() runs.
    live_shipped_pairs = shipped_pairs()
    a_real_shell = next(
        package for package, _ in live_shipped_pairs if package != PRODUCT_PACKAGE
    )

    def refuses(**kwargs) -> str:
        try:
            compute("origin/main", fetch=False, **kwargs)
        except ClosureError as err:
            return str(err)
        return ""

    check(
        "not ME3-loadable shells" in refuses(dropped={"no-such-crate"}),
        "--without refuses a package that is not a shipped shell",
    )
    check(
        "BOTH --with and --without" in refuses(
            pinned={a_real_shell}, dropped={a_real_shell}
        ),
        "naming one DLL with both --with and --without refuses instead of guessing",
    )
    check(
        "cannot drop" in refuses(dropped={PRODUCT_PACKAGE}),
        "--without refuses to drop the product DLL",
    )

    # And the drop itself: withheld comes out of `kept` and lands in `excluded` with a reason,
    # so an A/B pair is distinguishable in the run block rather than being two identical-looking
    # runs. This mirrors the sequence compute() applies after conflict ranking.
    kept_ab, excluded_ab, _, _ = resolve_conflicts({PRODUCT_PACKAGE, a_real_shell}, {}, set())
    for name in sorted({a_real_shell} & kept_ab):
        kept_ab.discard(name)
        excluded_ab.append({"package": name, "kind": "withheld", "because": "x", "evidence": "y"})
    check(
        a_real_shell not in kept_ab
        and any(e["kind"] == "withheld" for e in excluded_ab)
        and PRODUCT_PACKAGE in kept_ab,
        "a withheld DLL leaves `kept` and is RECORDED in `excluded`, product untouched",
    )

    # --- opt-in-only: co-loadable, but consent is required ------------------------------
    opt_table = {"opt_in_only": {"mush": "wears a costume nobody asked for"}}
    kept, excluded, unresolvable, _ = resolve_conflicts(
        {PRODUCT_PACKAGE, "mush"}, opt_table, set()
    )
    check(
        "mush" not in kept and not unresolvable,
        "an opt-in-only DLL is dropped from a closure that merely reached it",
    )
    check(
        [e["package"] for e in excluded] == ["mush"]
        and excluded[0]["kind"] == "opt-in-only"
        and "costume" in excluded[0]["because"],
        "the dropped opt-in-only DLL is REPORTED with its player-facing reason, not silently lost",
    )
    kept_pinned, excluded_pinned, _, _ = resolve_conflicts(
        {PRODUCT_PACKAGE, "mush"}, opt_table, {"mush"}
    )
    check(
        "mush" in kept_pinned and not excluded_pinned,
        "--with is the opt-in: a pinned opt-in-only DLL is kept",
    )
    # The real table, against the real closure: the mushroom mod must never arrive unasked.
    with CONFLICTS_TOML.open("rb") as handle:
        live = tomllib.load(handle)
    check(
        "mushroom-man-runtime" in live.get("opt_in_only", {}),
        "mushroom-man-runtime is declared opt-in-only in the shipped table",
    )
    every = {package for package, _ in shipped_pairs()}
    kept_all, _, _, _ = resolve_conflicts(every, live, set())
    check(
        "mushroom-man-runtime" not in kept_all,
        "even a closure that selects EVERY shell does not load the mushroom mod",
    )

    # --- [always]: the standing half of --with ------------------------------------------
    # The regression this pins is the one the user reported on 2026-09-08. `er-lockon-filter`
    # was [opt_in_only], so it was absent from every launch that did not name it, and the
    # lock-on the user was describing was vanilla lock-on. Reclassifying it [compatible] is not
    # enough on its own: `er-quickload` has no reverse dependents, so a branch editing only the
    # product closes over one shell and the filter would go missing again.
    always_table = {"always": {"tag": "on by user directive"}}
    always_live = set(always_table["always"])
    candidates = {PRODUCT_PACKAGE}
    added = sorted(always_live - candidates)
    candidates |= always_live
    kept_always, _, _, _ = resolve_conflicts(candidates, always_table, set())
    check(
        "tag" in kept_always and added == ["tag"],
        "an [always] package joins a closure the diff never reached, and is reported as added",
    )
    # Not a licence to co-load: an [always] package that conflicts with the product still loses.
    conflicting = {
        "always": {"tag": "on by default"},
        "conflict": [
            {"a": PRODUCT_PACKAGE, "b": "tag", "kind": "hook-collision", "reason": "r", "evidence": "e"}
        ],
    }
    kept_conflict, excluded_conflict, _, _ = resolve_conflicts(
        {PRODUCT_PACKAGE, "tag"}, conflicting, set()
    )
    check(
        "tag" not in kept_conflict and [e["package"] for e in excluded_conflict] == ["tag"],
        "[always] does not override conflict ranking; the loser is still dropped and reported",
    )
    check(
        "on by default, not because this branch reached them" in render(
            {
                "base_ref": "origin/main",
                "merge_base": "0" * 12,
                "head": "1" * 12,
                "dirty": False,
                "changed_file_count": 0,
                "changed_outside_crates": 0,
                "seed_crates": [],
                "affected_crates": [],
                "artifacts": ["er_quickload.dll", "er_lockon_filter.dll"],
                "fallback": None,
                "added_by_default": ["er-lockon-filter"],
                "excluded": [],
                "unresolvable": [],
            }
        ),
        "the rendered report names a DLL that arrived from [always] rather than from the diff",
    )
    # The live table, against the real shipped set: the lock-on filter is on by default.
    check(
        "er-lockon-filter" in live.get("always", {})
        and "er-lockon-filter" not in live.get("opt_in_only", {}),
        "er-lockon-filter is declared [always] in the shipped table, and is no longer opt-in-only",
    )
    always_real = set(live.get("always", {})) & {package for package, _ in shipped_pairs()}
    kept_default, _, _, _ = resolve_conflicts(
        {PRODUCT_PACKAGE} | always_real, live, set()
    )
    check(
        "er-lockon-filter" in kept_default,
        "a product-only closure still loads er_lockon_filter.dll: it is on by default",
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
    parser.add_argument(
        "--without",
        dest="dropped",
        action="append",
        default=[],
        metavar="PACKAGE",
        help="force-EXCLUDE a shell (repeatable); applied after conflict ranking and reported "
        "in the excluded list, so the run block says what was withheld",
    )
    parser.add_argument(
        "--agent-driven",
        action="store_true",
        help="declare this run AGENT-DRIVEN, accepting a --with-pinned drives-input conflict "
        "(the input harness). The run is then recorded as one in which the player was NOT in "
        "control; no other conflict kind is affected.",
    )
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    try:
        result = compute(
            args.base,
            fetch=not args.no_fetch,
            pinned=set(args.pinned),
            dropped=set(args.dropped),
            agent_driven=args.agent_driven,
        )
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
