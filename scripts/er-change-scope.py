#!/usr/bin/env python3
"""What does this diff actually require us to RE-check?

`scripts/er-dll-closure.py` already answers "which ME3-loadable DLLs does this branch
affect" for a runtime question -- which cdylibs must be loaded for a launch to be testing
them. This module asks the CI/pre-push question instead: which gate work is this diff
capable of invalidating, and which is provably untouched by it.

It REUSES that tool'S walk rather than copying it. `path_dependents()`, `affected_packages()`,
`changed_paths()` and `resolve_base()` are imported from it, so the diff base
(`merge-base(origin/main, HEAD)` -> working tree, plus untracked files) and the reverse
dependency closure are the same in both tools by construction. A second implementation of the
walk is the drift this repo keeps closing.

What it deliberately does *not* reuse, and why
----------------------------------------------
`er-dll-closure.py`'s `packages` output is post-processed by three rules that are about
loading a process, and every one of them is wrong here:

  * the conflict table drops `er-loading-portrait` when the product is present, because two
    Present hooks in one process corrupt the frame. Nothing is loaded here -- CI compiles.
    Honouring it would mean a change to that crate is never compiled.
  * `opt_in_only` withholds a gameplay mod nobody consented to. Consent is a launch concept;
    a mod you did not ask to play still has to build.
  * "the product is never optional" unions `er-quickload` in unconditionally, because the
    launcher's testimony step waits for its log line. That would put the single most expensive
    package in every doc-only diff.

So selection here reads `affected_crates` -- the raw reverse-dependency closure -- and
intersects it with the shipped cdylib list from `scripts/me3-dll-list.py`. Nothing else.

Fail open, always, and say so
-----------------------------
Every uncertainty selects more work, never less, and names the reason:

  * no changed paths at all (a push to `main`, where merge-base == head) -> everything.
    This is the property that makes selective PR checking safe: every merge to main still
    runs the whole suite, so a crate nobody's PR touched cannot rot unobserved.
  * a Rust build input that is not inside a single crate directory (root `Cargo.toml`,
    `Cargo.lock`, `.cargo/`, `build-support/`, `data/`, `.github/`) -> everything. Crate-local
    reasoning is only valid when every build-affecting path belongs to a crate.
  * a git failure, an unresolvable base, `ER_SCOPE_ALL=1` -> everything (the callers treat a
    non-zero exit as "run it all").

The one thing that can subtract work is an explicit list of paths that cannot reach cargo,
and it is asserted rather than asserted-by-assumption: `--selftest` reads every build script in
the tree, extracts the repo-relative paths they actually open, and fails if any of them falls
inside the non-build set. `crates/er-game-base/build.rs` reading `docs/recon/*.tsv` is exactly
why `docs/` carries an exception rather than being waved through wholesale.

Usage:
    python3 scripts/er-change-scope.py                 # human-readable
    python3 scripts/er-change-scope.py --json
    python3 scripts/er-change-scope.py --matrix         # GitHub Actions matrix (all shells)
    python3 scripts/er-change-scope.py --summary-md     # markdown accounting table
    python3 scripts/er-change-scope.py --check-sh-skips # line<tab>reason for scripts/check.sh
    python3 scripts/er-change-scope.py --rust-touched   # exit 0 if cargo work is required
    python3 scripts/er-change-scope.py --rev <sha>      # diff a pushed tip instead of the tree
    python3 scripts/er-change-scope.py --selftest

Exit status: 0 fine, 1 the caller must fail open and run everything.
`--rust-touched` is a PREDICATE mode: 0 = cargo work required, 3 = provably none required.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CLOSURE = REPO_ROOT / "scripts" / "er-dll-closure.py"
CHECK_SH = REPO_ROOT / "scripts" / "check.sh"
PORTABILITY = REPO_ROOT / "scripts" / "ci-gate-portability.py"

EXIT_OK = 0
EXIT_FAIL_OPEN = 1
EXIT_NO_CARGO_WORK = 3

# Paths whose change cannot alter what cargo compiles. Kept short on purpose: everything not
# named here is treated as build-affecting, so a new directory nobody classified fails safe.
# `--selftest` proves no build script reads anything under these.
NON_BUILD_PREFIXES = (
    "scripts/",
    "docs/",
    ".beads/",
    ".auto/",
    ".cupcake/",
    ".claude/",
    ".worktrees/",
    "save-files/",
    "vendor-archive/",
)
# ...and the holes in them. `crates/er-game-base/build.rs` opens four TSVs under `docs/recon/`
# (VERIFIED_MAP, quarantine, FUNCTION_MAP, DATA_MAP) and generates `address_map_1170.rs` from
# them, so `docs/` as a blanket exemption would let an address-map edit skip every compile.
BUILD_INPUT_EXCEPTIONS = ("docs/recon/",)
# A markdown file anywhere is prose. No build script reads one; `--selftest` re-proves it.
NON_BUILD_SUFFIXES = (".md",)

# `.github/` is deliberately absent from the non-build set even though cargo never reads it:
# `.github/workflows/check.yml` carries FROMSOFTWARE_RS_REV, the sibling-checkout pin that
# decides which game builds the DLLs survive. A change there changes what CI compiles against,
# so it must select everything.

# The gates that shell out to the Rust TOOLCHAIN. A step running one of these is skipped only
# when nothing that can reach cargo changed -- never on a per-package basis, because each of
# them is whole-workspace by construction:
#
#   check-rust-build.sh          links all 26 me3 shells and re-attests their provenance
#                                sidecars. A partial relink with a full re-attestation would
#                                make `er-dll-provenance.py verify` claim bytes it did not
#                                build, and the launchers gate on that -- so it is all or
#                                nothing, and the only safe subset is the empty one.
#   check-committed-compiles.sh  clippies the committed tree in a temp worktree; its subject is
#                                the commit, not a package.
#   check-save-disable-warnings.py  runs the compiler per crate to read its warning set.
#
# The list is small and explicit, and `--selftest` keeps it from falling behind: it scans every
# script `check.sh` invokes for a real toolchain invocation and fails on one that is not
# declared here. A gate that quietly grew a compile step becomes a red gate, not a silent skip.
RUST_TOOLCHAIN_GATES = {
    "check-rust-build.sh": "links every me3 shell and re-attests provenance; all-or-nothing",
    "check-committed-compiles.sh": "compiles the committed tree; its subject is the commit",
    "check-save-disable-warnings.py": "invokes the compiler per crate to read its warnings",
}

# `cargo fmt` is never skipped. It is workspace-wide by nature, costs seconds, and formatting
# drift is exactly the class of damage a scoped run would let through.
ALWAYS_RUN_CARGO = ("fmt",)

DASH_P = re.compile(r"-p\s+([A-Za-z0-9_-]+)")
GIT_TIMEOUT_SECONDS = 25


class ScopeError(RuntimeError):
    """Something the caller must answer by running everything."""


def _load(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    # Registered before exec: `ci-gate-portability.py` defines a @dataclass, and dataclasses
    # resolves annotations through `sys.modules[cls.__module__]`. Without this line that lookup
    # returns None and the import dies in the decorator, nowhere near the real cause.
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def closure_module():
    return _load(CLOSURE, "er_dll_closure")


# --- the predicate -------------------------------------------------------------------------


def is_build_input(path: str) -> bool:
    """True when changing `path` can change what cargo compiles."""
    if any(path.startswith(prefix) for prefix in BUILD_INPUT_EXCEPTIONS):
        return True
    if any(path.startswith(prefix) for prefix in NON_BUILD_PREFIXES):
        return False
    if any(path.endswith(suffix) for suffix in NON_BUILD_SUFFIXES):
        return False
    return True


# --- prose is not a build input ---------------------------------------------------------------
#
# `is_build_input` decides on the `path` alone, which cannot tell a comment edit from a code edit.
# Measured 2026-09-07 on the comment-caps branch: 1,026 changed .rs/.py/.sh files, 1,021 of them
# comment-only, and four of the five real ones sat outside crates/ -- so the select-everything rule
# fired and all 28 DLLs were rebuilt and retested for a diff that cannot change a single byte of
# compiled output.
#
# So the paths that reach cargo are filtered once more, by CONTENT: a .rs file whose code is
# byte-identical with its comments blanked cannot change what cargo compiles.
#
# One comment is code, and is deliberately kept in the comparison: a ``` block inside a doc comment
# is a rustdoc example, and `cargo test` compiles and runs it. `prose_spans` already excludes
# fenced blocks, so blanking exactly the spans it returns leaves every doctest line in place and an
# edit to one reads as a real change.
#
# Everything else fails closed. A file with no base version (added), one that cannot be read, one
# whose extension the comment scanner does not know: all stay build inputs.


def comment_scanner():
    return _load(REPO_ROOT / "scripts" / "check-comment-caps.py", "er_comment_caps")


def code_projection(text: str, suffix: str, scanner) -> str | None:
    """`text` with prose blanked, or None when the scanner has no dialect for `suffix`."""
    dialect = scanner.SCANNED_SUFFIXES.get(suffix)
    if dialect is None:
        return None
    prose = {line for line, _ in scanner.prose_spans(text, dialect)}
    return "\n".join("" if n in prose else body for n, body in enumerate(text.splitlines(), 1))


def prose_only_paths(paths: list[str], base: str, head: str | None, scanner) -> set[str]:
    """The subset of `paths` whose code is unchanged once comments are blanked."""
    inert: set[str] = set()
    for path in paths:
        suffix = Path(path).suffix
        try:
            before = _blob(f"{base}:{path}")
            after = _blob(f"{head}:{path}") if head else Path(path).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError, ScopeError):
            continue  # unreadable or newly added: not provably inert, so it stays a build input
        left = code_projection(before, suffix, scanner)
        right = code_projection(after, suffix, scanner)
        if left is not None and left == right:
            inert.add(path)
    return inert


def _blob(spec: str) -> str:
    out = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "show", spec],
        capture_output=True,
        text=True,
        timeout=GIT_TIMEOUT_SECONDS,
    )
    if out.returncode != 0:
        raise ScopeError(f"cannot read {spec}")
    return out.stdout


def crate_of(path: str) -> str | None:
    parts = Path(path).parts
    return parts[1] if len(parts) >= 2 and parts[0] == "crates" else None


# --- the scope -----------------------------------------------------------------------------


def compute(base_ref: str = "origin/main", fetch: bool = False, revs: tuple[str, ...] = ()) -> dict:
    """The whole answer. Raises ScopeError when the caller must fail open.

    `fetch` defaults to false, unlike er-dll-closure.py, and that is a deliberate difference:
    this runs on every push and inside check.sh, where a network round trip is a cost nobody
    asked for. A stale `origin/main` widens the diff (an older base has more changed paths),
    which selects more work -- the safe direction. CI does not need it either: actions/checkout
    with fetch-depth 0 has already fetched.
    """
    dll = closure_module()
    merge_base, head = dll.resolve_base(base_ref, fetch)

    if revs:
        # The pre-push SHAPE: ask about the commits being pushed, not about the working tree.
        # A dirty tree is not what reaches origin, and the hook's question is about what does.
        changed: set[str] = set()
        for rev in revs:
            rev_base = dll.git("merge-base", base_ref, rev).strip()
            changed.update(
                line.strip()
                for line in dll.git("diff", "--name-only", rev_base, rev).splitlines()
                if line.strip()
            )
        changed_paths = sorted(changed)
    else:
        changed_paths = dll.changed_paths(merge_base)

    pairs = dll.shipped_pairs()
    artifact_of = dict(pairs)
    shipped = [package for package, _ in pairs]

    select_all_reason: str | None = None
    if os.environ.get("ER_SCOPE_ALL") == "1":
        select_all_reason = "ER_SCOPE_ALL=1 in the environment"
    elif not changed_paths:
        select_all_reason = (
            f"no path differs from {base_ref} ({merge_base[:12]}). That is a push to main, or an "
            f"empty branch -- and it is why selective PR checking is safe: every merge runs "
            f"everything."
        )

    build_inputs = [path for path in changed_paths if is_build_input(path)]
    # Then drop the ones that are provably prose. `revs` mode compares two commits; the default
    # compares the merge base against the working tree, which is what the caller is about to
    # validate.
    prose_only = prose_only_paths(build_inputs, merge_base, revs[-1] if revs else None, comment_scanner())
    build_inputs = [path for path in build_inputs if path not in prose_only]
    outside = sorted({path for path in build_inputs if crate_of(path) is None})
    if select_all_reason is None and outside:
        select_all_reason = (
            "a Rust build input outside crates/ changed, so crate-local reasoning does not hold: "
            + ", ".join(outside[:6])
            + ("" if len(outside) <= 6 else f" (+{len(outside) - 6} more)")
        )

    seeds = {crate for path in build_inputs if (crate := crate_of(path))}
    affected = dll.affected_packages(seeds, dll.path_dependents()) if seeds else set()

    select_all = select_all_reason is not None
    rust_touched = select_all or bool(build_inputs)

    dlls = []
    for package in shipped:
        if select_all:
            selected, reason = True, "everything is selected: " + select_all_reason
        elif package in affected:
            via = sorted(seeds & {package}) or sorted(seeds)
            reason = (
                "changed directly"
                if package in seeds
                else "depends (transitively) on changed crate(s): " + ", ".join(via[:4])
            )
            selected = True
        else:
            selected = False
            reason = (
                "no changed file feeds this DLL"
                if rust_touched
                else "this diff touches nothing cargo compiles"
            )
        dlls.append(
            {
                "package": package,
                "artifact": f"{artifact_of[package]}.dll",
                "selected": selected,
                "reason": reason,
            }
        )

    return {
        "base_ref": base_ref,
        "merge_base": merge_base,
        "head": head,
        "revs": list(revs),
        "changed_file_count": len(changed_paths),
        "build_input_count": len(build_inputs),
        "prose_only_count": len(prose_only),
        "non_build_changed": sorted(set(changed_paths) - set(build_inputs))[:20],
        "select_all": select_all,
        "select_all_reason": select_all_reason,
        "rust_touched": rust_touched,
        "seed_crates": sorted(seeds),
        "affected_crates": sorted(affected),
        "dlls": dlls,
        "selected_packages": [entry["package"] for entry in dlls if entry["selected"]],
    }


# --- check.sh step classification ----------------------------------------------------------


def classify_step(text: str) -> tuple[str, list[str]]:
    """('global' | 'package' | 'rust-toolchain', packages) for one check.sh step line."""
    stripped = text.strip()
    head = stripped.split(" ", 1)[0]
    if head == "cargo":
        words = stripped.split()
        if len(words) > 1 and words[1] in ALWAYS_RUN_CARGO:
            return "global", []
        packages = DASH_P.findall(stripped)
        # A cargo step with no `-p` is whole-workspace; it survives exactly as long as anything
        # cargo compiles is in the diff.
        return ("package", packages) if packages else ("rust-toolchain", [])
    if head in ("python3", "bash"):
        for name in RUST_TOOLCHAIN_GATES:
            if f"scripts/{name}" in stripped:
                return "rust-toolchain", []
    return "global", []


def check_sh_skips(scope: dict) -> list[tuple[int, str]]:
    """(line, reason) for every check.sh step this diff cannot invalidate."""
    portability = _load(PORTABILITY, "ci_gate_portability")
    affected = set(scope["affected_crates"])
    out: list[tuple[int, str]] = []
    for step in portability.parse_steps(CHECK_SH):
        kind, packages = classify_step(step.text)
        if kind == "global" or scope["select_all"]:
            continue
        if kind == "rust-toolchain" and not scope["rust_touched"]:
            out.append(
                (
                    step.line,
                    "NOT SELECTED FOR THIS DIFF -- nothing that reaches cargo changed, so this "
                    "compile gate has nothing new to compile",
                )
            )
        elif kind == "package" and not (set(packages) & affected):
            out.append(
                (
                    step.line,
                    "NOT SELECTED FOR THIS DIFF -- "
                    + ", ".join(packages)
                    + " is outside the reverse-dependency closure of the changed crates",
                )
            )
    return out


# --- renderers -----------------------------------------------------------------------------


def render(scope: dict) -> str:
    selected = [d for d in scope["dlls"] if d["selected"]]
    lines = [
        f"base            {scope['base_ref']} -> {scope['merge_base'][:12]}",
        f"changed         {scope['changed_file_count']} paths "
        f"({scope['build_input_count']} can reach cargo)",
        f"rust work       {'REQUIRED' if scope['rust_touched'] else 'NOT REQUIRED'}",
        f"seeds           {', '.join(scope['seed_crates']) or '(none)'}",
        f"DLLs selected   {len(selected)} of {len(scope['dlls'])}",
    ]
    if scope["select_all_reason"]:
        lines.append(f"  ^ {scope['select_all_reason']}")
    lines.append("")
    for entry in scope["dlls"]:
        mark = "SELECTED    " if entry["selected"] else "not selected"
        lines.append(f"  {mark}  {entry['artifact']:<28} {entry['reason']}")
    return "\n".join(lines)


def summary_md(scope: dict) -> str:
    selected = [d for d in scope["dlls"] if d["selected"]]
    rows = [
        "## Per-DLL selection for this diff",
        "",
        f"`{scope['changed_file_count']}` changed path(s), `{scope['build_input_count']}` of "
        f"which can reach cargo. **{len(selected)} of {len(scope['dlls'])}** DLLs selected.",
        "",
        f"`{scope.get('prose_only_count', 0)}` changed source file(s) were comment-only and are "
        "not counted above: with prose blanked their code is byte-identical, and rustdoc "
        "```` ``` ```` blocks are kept in that comparison because a doctest is compiled.",
        "",
        "A DLL marked *not selected* was **NOT checked here**. That is not a pass.",
        "",
        "| DLL | verdict | why |",
        "| --- | --- | --- |",
    ]
    for entry in scope["dlls"]:
        verdict = "SELECTED" if entry["selected"] else "**NOT SELECTED**"
        rows.append(f"| `{entry['artifact']}` | {verdict} | {entry['reason']} |")
    return "\n".join(rows)


def matrix(scope: dict) -> dict:
    """A GitHub Actions matrix carrying every shipped shell, selected or not.

    The row count is constant, on purpose. A matrix computed down to the winners makes an
    absent DLL indistinguishable from a DLL that does not exist -- the same defect check.sh's
    not run state exists to refuse. `matrix` is not available in `jobs.<id>.if` (GitHub's
    context table), but it is available in `jobs.<id>.name`, so the verdict goes in the name
    and the heavy steps carry `if: matrix.selected == 'yes'`.
    """
    include = []
    for entry in scope["dlls"]:
        selected = entry["selected"]
        include.append(
            {
                "package": entry["package"],
                "artifact": entry["artifact"],
                "selected": "yes" if selected else "no",
                "reason": entry["reason"],
                "label": entry["package"]
                if selected
                else f"{entry['package']} -- NOT SELECTED (not affected by this diff)",
            }
        )
    return {"include": include}


# --- selftest ------------------------------------------------------------------------------


def _build_script_paths() -> list[tuple[str, str]]:
    """(build script, repo-relative path it opens) for every literal that resolves to a file."""
    out = []
    scripts = sorted(REPO_ROOT.glob("crates/*/build.rs")) + sorted(
        (REPO_ROOT / "build-support").glob("*.rs")
    )
    literal = re.compile(r'"((?:\.\./)*[A-Za-z0-9_./-]+\.[A-Za-z0-9]+)"')
    for script in scripts:
        text = script.read_text(encoding="utf-8", errors="replace")
        for match in literal.finditer(text):
            raw = match.group(1)
            resolved = (script.parent / raw).resolve()
            try:
                rel = resolved.relative_to(REPO_ROOT).as_posix()
            except ValueError:
                continue
            if resolved.is_file():
                out.append((script.relative_to(REPO_ROOT).as_posix(), rel))
    return out


TOOLCHAIN_CALL = re.compile(
    r"""(?x)
    (?:^|[;&|(]\s*|\$\(\s*)ca rgo\s        # a shell command position
    | \[\s*["']ca rgo["']                  # a python argv list
    | ["']ca rgo["']\s*,                   # ...or its first element
    """.replace(
        "ca rgo", "cargo"
    )
)


def _strip_shell_noise(text: str) -> str:
    """Blank comments and quoted spans so prose about the toolchain is not read as a call."""
    out = []
    for raw in text.split("\n"):
        line = raw.split("#", 1)[0] if raw.lstrip().startswith("#") else raw
        line = re.sub(r'"[^"]*"', '""', line)
        line = re.sub(r"'[^']*'", "''", line)
        out.append(line)
    return "\n".join(out)


def selftest() -> int:
    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
        print(f"  {'ok  ' if condition else 'FAIL'} {label}")

    # --- the predicate ---------------------------------------------------------------------
    check(is_build_input("crates/er-hook/src/lib.rs"), "a crate source is a build input")
    check(is_build_input("Cargo.lock"), "Cargo.lock is a build input")
    check(is_build_input("data/effects.json"), "data/ is a build input (embedded at compile time)")
    check(is_build_input(".github/workflows/check.yml"), "the CI pin file is a build input")
    check(
        is_build_input("docs/recon/rva-map-1162-to-1170.data.tsv"),
        "docs/recon is a build input -- er-game-base's build.rs reads it",
    )
    check(not is_build_input("scripts/check.sh"), "a shell script is not a build input")
    check(not is_build_input("docs/plans/whatever.md"), "a doc is not a build input")
    check(not is_build_input("README.md"), "a root markdown file is not a build input")

    # --- prose is not a build input ---------------------------------------------------------
    scanner = comment_scanner()

    def code(text: str) -> str | None:
        return code_projection(text, ".rs", scanner)

    body = "fn f() -> u8 {\n    1\n}\n"
    check(
        code("// one wording\n" + body) == code("// another wording entirely\n" + body),
        "a comment edit leaves the code projection identical",
    )
    check(
        code("/// doc\n" + body) != code("/// doc\nfn f() -> u8 {\n    2\n}\n"),
        "a code edit changes the projection",
    )
    # The one comment that is code: rustdoc compiles and runs it.
    fence_a = "/// Example.\n///\n/// ```\n/// assert_eq!(f(), 1);\n/// ```\n" + body
    fence_b = "/// Example.\n///\n/// ```\n/// assert_eq!(f(), 2);\n/// ```\n" + body
    check(code(fence_a) != code(fence_b), "a doctest edit is a code change, not prose")
    check(
        code(fence_a) == code("/// Reworded.\n///\n/// ```\n/// assert_eq!(f(), 1);\n/// ```\n" + body),
        "...while the prose around the same doctest is not",
    )
    check(code_projection("x", ".toml", scanner) is None, "an unknown suffix has no projection")
    check(
        prose_only_paths(["crates/nope/src/does-not-exist.rs"], "HEAD", None, scanner) == set(),
        "a path with no base version fails closed and stays a build input",
    )

    # --- Anti-drift 1: no build script reads anything the predicate waves through ----------
    literals = _build_script_paths()
    check(len(literals) >= 4, f"build scripts name {len(literals)} real repo path(s) to audit")
    leaks = sorted({rel for _, rel in literals if not is_build_input(rel)})
    check(
        not leaks,
        "every path a build script opens is classified as a build input"
        + ("" if not leaks else f" -- LEAKED: {', '.join(leaks)}"),
    )
    check(
        any(rel.startswith("docs/recon/") for _, rel in literals),
        "the docs/recon exception is load-bearing, not decorative (a build script reads it)",
    )

    # --- Anti-drift 2: every compile-invoking gate in check.sh is declared ------------------
    portability = _load(PORTABILITY, "ci_gate_portability")
    undeclared = []
    seen_declared = set()
    for step in portability.parse_steps(CHECK_SH):
        match = re.search(r"\$repo_root/scripts/([A-Za-z0-9_.-]+)", step.text)
        if not match or step.text.strip().split(" ", 1)[0] not in ("python3", "bash"):
            continue
        name = match.group(1)
        target = REPO_ROOT / "scripts" / name
        if not target.is_file():
            continue
        if name in RUST_TOOLCHAIN_GATES:
            seen_declared.add(name)
            continue
        body = _strip_shell_noise(target.read_text(encoding="utf-8", errors="replace"))
        if TOOLCHAIN_CALL.search(body):
            undeclared.append(name)
    check(
        not undeclared,
        "no check.sh gate invokes the toolchain without being declared in RUST_TOOLCHAIN_GATES"
        + ("" if not undeclared else f" -- UNDECLARED: {', '.join(sorted(set(undeclared)))}"),
    )
    check(
        seen_declared == set(RUST_TOOLCHAIN_GATES),
        "every declared toolchain gate is still a step in check.sh "
        f"(missing: {', '.join(sorted(set(RUST_TOOLCHAIN_GATES) - seen_declared)) or 'none'})",
    )

    # --- step classification ---------------------------------------------------------------
    kind, packages = classify_step('cargo test --manifest-path "$r/Cargo.toml" -p er-gfx')
    check((kind, packages) == ("package", ["er-gfx"]), "a -p step is package-scoped")
    kind, packages = classify_step("cargo fmt --all -- --check")
    check(kind == "global", "cargo fmt is never skipped")
    kind, _ = classify_step('cargo check --manifest-path "$r/Cargo.toml" --all-targets')
    check(kind == "rust-toolchain", "a cargo step with no -p is whole-workspace")
    kind, _ = classify_step('bash "$repo_root/scripts/check-rust-build.sh"')
    check(kind == "rust-toolchain", "the shell-linking gate is whole-workspace")
    kind, _ = classify_step('python3 "$repo_root/scripts/check-fnv1a-owner.py"')
    check(kind == "global", "an ordinary static gate is global and always runs")

    # --- selection, over synthetic scopes ---------------------------------------------------
    def scope_of(**kwargs) -> dict:
        base = {
            "select_all": False,
            "rust_touched": True,
            "affected_crates": [],
            "seed_crates": [],
        }
        base.update(kwargs)
        return base

    skips = dict(check_sh_skips(scope_of(rust_touched=False)))
    real = {s.line: classify_step(s.text)[0] for s in portability.parse_steps(CHECK_SH)}
    globals_skipped = [line for line in skips if real.get(line) == "global"]
    check(not globals_skipped, "a no-Rust diff never skips a global gate")
    check(
        all(real.get(line) in ("package", "rust-toolchain") for line in skips),
        "a no-Rust diff skips only cargo-shaped work",
    )
    check(
        any(real.get(line) == "rust-toolchain" for line in skips),
        "...and it really does skip the compile gates (non-vacuity)",
    )
    check(
        all("NOT SELECTED FOR THIS DIFF" in reason for reason in skips.values()),
        "every skip reason says NOT SELECTED, so it can never read as a pass",
    )

    everything = dict(check_sh_skips(scope_of(select_all=True)))
    check(not everything, "select_all skips nothing at all")

    gfx = dict(check_sh_skips(scope_of(affected_crates=["er-gfx"])))
    gfx_lines = [
        s.line
        for s in portability.parse_steps(CHECK_SH)
        if classify_step(s.text) == ("package", ["er-gfx"])
    ]
    check(bool(gfx_lines), "check.sh really has an er-gfx-only step to reason about")
    check(
        all(line not in gfx for line in gfx_lines),
        "the step for an affected crate is NOT skipped",
    )

    print("selftest:", "PASS" if ok else "FAIL")
    return EXIT_OK if ok else EXIT_FAIL_OPEN


# --- cli -----------------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="origin/main")
    parser.add_argument(
        "--fetch",
        action="store_true",
        help="refresh the base ref first (off by default: a stale base only widens the diff)",
    )
    parser.add_argument(
        "--rev",
        dest="revs",
        action="append",
        default=[],
        metavar="SHA",
        help="diff this commit against the base instead of the working tree (repeatable)",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--json", action="store_true")
    mode.add_argument("--matrix", action="store_true")
    mode.add_argument("--summary-md", action="store_true")
    mode.add_argument("--check-sh-skips", action="store_true")
    mode.add_argument("--rust-touched", action="store_true")
    mode.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    try:
        scope = compute(args.base, fetch=args.fetch, revs=tuple(args.revs))
    except Exception as err:  # noqa: BLE001 -- every failure has the same answer: run everything
        print(
            f"er-change-scope: cannot determine scope ({type(err).__name__}: {err}).\n"
            "Failing OPEN: the caller must run everything.",
            file=sys.stderr,
        )
        return EXIT_FAIL_OPEN

    if args.check_sh_skips:
        for line, reason in check_sh_skips(scope):
            print(f"{line}\t{reason}")
    elif args.rust_touched:
        return EXIT_OK if scope["rust_touched"] else EXIT_NO_CARGO_WORK
    elif args.json:
        print(json.dumps(scope, indent=2))
    elif args.matrix:
        print(json.dumps(matrix(scope)))
    elif args.summary_md:
        print(summary_md(scope))
    else:
        print(render(scope))
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
