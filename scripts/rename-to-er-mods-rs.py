#!/usr/bin/env python3
"""One-shot migration: `er-effects-rs` -> repo `er-mods-rs` + product crate `er-quickload`.

The repo outgrew its name. `er-effects-rs` described one feature -- named SpEffect calls --
which now lives in its own crate (`er-net-effects`) and is the smallest thing here. What the
repo actually holds is ~20 ME3-loadable DLLs plus the host-side Rust libraries they are built
from. So the name splits in two, and every reference has to be classified as one or the other:

    the repository   er-effects-rs  ->  er-mods-rs     (GitHub, clone dir, sibling-path prose)
    the product DLL  er-effects-rs  ->  er-quickload   (crate, er_quickload.dll, .toml, .log)

A single blind substitution cannot do this: `crates/er-effects-rs` and
`github.com/Banon-Labs/er-effects-rs` are the same 13 characters meaning different things.
Hence an ordered, longest-alternative-first alternation resolved in one pass (two passes would
let an earlier rewrite feed a later rule), with the context baked into the token itself.

Three classes must survive unchanged, and are identity-mapped so the alternation matches them
first and rewrites them to themselves:

  1. Beads issue IDs (`er-effects-rs-wncc`). Database keys for 2500+ memories and 100 issues.
     Renaming them orphans every cross-reference in the tree and in the Dolt database, which
     this pass does not touch.
  2. `er_effects_union_register`. The cross-DLL hook-union export, resolved by name through
     GetProcAddress by seven crates. A user running an already-released `er_invasion_warp.dll`
     next to a freshly built product DLL would silently lose union coordination and get two
     MinHook instances on one prologue. This is the exact class bd
     `rename-crate-name-vs-runtime-name-2026-08-25` measured going wrong last time.
  3. The DoltHub sync remote. It names an external repository that this rename does not move.

Usage: python3 scripts/rename-to-er-mods-rs.py [--apply] [--report-residue]
       Default is a dry run.
Env:   ER_MODS_REPO_ROOT, ELDEN_PROFILES_DIR (default ~/Elden) override the defaults.
"""

import os
import re
import subprocess
import sys

REPO_OLD, REPO_NEW = "er-effects-rs", "er-mods-rs"
CRATE_NEW, CRATE_NEW_U = "er-quickload", "er_quickload"

# Historical records: past runs must keep the names those runs actually used, and the beads
# export is a database dump, not source.
EXCLUDE_PATHS = {".auto/log.jsonl"}
EXCLUDE_PREFIXES = (".beads/",)

# Identity-mapped protections (class 2 and 3 above; class 1 is harvested at runtime).
PROTECT_LITERAL = [
    "er_effects_union_register",
    "doltremoteapi.dolthub.com/chozandrias/er-effects-rs",
]

# Repo-scoped references that are not a bare path: they name the repository inside a longer
# identifier, so they must be listed before the generic bd-ID protection would swallow them.
REPO_COMPOUND = [
    ("er-effects-rs-build-watermark", "er-mods-rs-build-watermark"),   # outbound User-Agent
    ("er-effects-rs-pi-disasm-color", "er-mods-rs-pi-disasm-color"),   # pi package name
]

# Repo-scoped: the token means "this checkout" / "this GitHub project".
REPO_CONTEXT = [
    ("Banon-Labs/er-effects-rs", f"Banon-Labs/{REPO_NEW}"),
    ("projects/er-effects-rs", f"projects/{REPO_NEW}"),
    # Windows/Wine checkout paths. The Rust literal holds doubled backslashes, the bare
    # script path a single one, so both separators are spelled out rather than matched
    # by a character class the escaping would have to survive.
    ("\\\\er-effects-rs", f"\\\\{REPO_NEW}"),
    ("\\er-effects-rs", f"\\{REPO_NEW}"),
    # Per-repo state directory outside the tree, and the outbound User-Agent that names
    # the project rather than the DLL doing the fetching.
    ("state}/er-effects-rs", f"state}}/{REPO_NEW}"),
    ("er-effects-rs build-import", f"{REPO_NEW} build-import"),
    ("-home-banon-projects-er-effects-rs", f"-home-banon-projects-{REPO_NEW}"),
    ("ER_EFFECTS_REPO_ROOT", "ER_MODS_REPO_ROOT"),
]

# Crate-scoped: the token means the product package.
CRATE_CONTEXT = [
    ("crates/er-effects-rs", f"crates/{CRATE_NEW}"),
    ("-p er-effects-rs", f"-p {CRATE_NEW}"),
]

# Product-scoped tails. `er-effects` is a prefix of many runtime filenames
# (er-effects.toml, er-effects-autoload-debug.log, er-effects-telemetry.json), so unlike the
# previous migration these deliberately carry no trailing boundary -- the whole point is to
# rewrite the longer names too.
PRODUCT_TAIL = [
    ("ER_EFFECTS_", "ER_QUICKLOAD_"),
    ("er_effects_rs", CRATE_NEW_U),
    ("er-effects-rs", CRATE_NEW),   # residual bare mentions: prose about the product DLL
    ("er_effects", CRATE_NEW_U),
    ("er-effects", CRATE_NEW),
]


def harvest_bd_ids(root):
    """Every `er-effects-rs-<suffix>` form present in the tree, minus the two compounds that
    are repo names rather than issue keys. Harvested rather than hard-coded so a fixture ID
    invented inside a .rego test (`er-effects-rs-1`) is protected the same as a real one."""
    out = subprocess.run(
        ["git", "-C", root, "grep", "-ohIE", r"er-effects-rs-[a-zA-Z0-9]+"],
        capture_output=True, text=True, timeout=30,
    ).stdout.split()
    compounds = {old for old, _ in REPO_COMPOUND}
    ids = set()
    for tok in out:
        if any(tok.startswith(c) for c in compounds):
            continue
        ids.add(tok)
    return sorted(ids)


def token_map(root):
    m = [(t, t) for t in PROTECT_LITERAL]
    m += [(i, i) for i in harvest_bd_ids(root)]
    m += REPO_COMPOUND + REPO_CONTEXT + CRATE_CONTEXT + PRODUCT_TAIL
    # Longest first so a specific context always beats the bare tail it contains.
    return sorted(m, key=lambda kv: -len(kv[0]))


def build_regex(mapping):
    return re.compile("|".join(re.escape(t) for t, _ in mapping))


def repo_files(root):
    out = subprocess.run(
        ["git", "-C", root, "ls-files"], capture_output=True, text=True, check=True,
        timeout=30,
    ).stdout.splitlines()
    keep = [
        f for f in out
        if f not in EXCLUDE_PATHS and not f.startswith(EXCLUDE_PREFIXES)
    ]
    return [os.path.join(root, f) for f in keep]


# Launchers live beside the profiles and hard-code the same absolute DLL path.
PROFILE_SUFFIXES = (".me3", ".sh", ".fish")


def profile_files(profiles_dir):
    """Top level of the profiles dir only. Subdirectories there hold captured run logs and
    SHA256SUMS attestations, which are historical records of runs that really did load a DLL
    by its old name -- rewriting those would forge the evidence."""
    if not os.path.isdir(profiles_dir):
        return []
    return [
        os.path.join(profiles_dir, f)
        for f in sorted(os.listdir(profiles_dir))
        if f.endswith(PROFILE_SUFFIXES)
        and os.path.isfile(os.path.join(profiles_dir, f))
    ]


def rewrite(paths, rx, table, apply_changes):
    changed, total = [], 0
    for p in paths:
        try:
            with open(p, encoding="utf-8") as fh:
                text = fh.read()
        except (UnicodeDecodeError, IsADirectoryError, FileNotFoundError):
            continue
        new, n = rx.subn(lambda mo: table[mo.group(0)], text)
        # subn counts identity rewrites too; only report real edits.
        if new != text:
            changed.append((p, sum(1 for _ in rx.finditer(text))))
            total += n
            if apply_changes:
                with open(p, "w", encoding="utf-8") as fh:
                    fh.write(new)
    return changed, total


def main() -> int:
    args = sys.argv[1:]
    if "--help" in args or "-h" in args:
        print(__doc__)
        return 0
    apply_changes = "--apply" in args
    unknown = [a for a in args if a not in ("--apply", "--report-residue")]
    if unknown:
        print(f"ERROR: unrecognised argument(s): {' '.join(unknown)}")
        return 2

    root = os.environ.get(
        "ER_MODS_REPO_ROOT",
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    )
    profiles_dir = os.environ.get(
        "ELDEN_PROFILES_DIR", os.path.join(os.path.expanduser("~"), "Elden")
    )

    mapping = token_map(root)
    table = dict(mapping)
    rx = build_regex(mapping)
    protected = sum(1 for k, v in mapping if k == v)
    print(f"tokens: {len(mapping)} ({protected} identity-protected)")

    if apply_changes:
        src = os.path.join(root, f"crates/{REPO_OLD}")
        if os.path.isdir(src):
            subprocess.run(
                ["git", "-C", root, "mv", f"crates/{REPO_OLD}", f"crates/{CRATE_NEW}"],
                check=True, timeout=30,
            )
            print(f"  git mv crates/{REPO_OLD} -> crates/{CRATE_NEW}")

    paths = repo_files(root) + profile_files(profiles_dir)
    changed, total = rewrite(paths, rx, table, apply_changes)

    verb = "rewrote" if apply_changes else "would rewrite"
    print(f"{verb} {total} references in {len(changed)} files")
    for p, n in sorted(changed, key=lambda x: -x[1])[:20]:
        print(f"  {n:5d}  {os.path.relpath(p, root)}")
    if len(changed) > 20:
        print(f"  ... {len(changed) - 20} more files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
