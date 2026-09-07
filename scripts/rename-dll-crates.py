#!/usr/bin/env python3
"""One-shot migration: remove `-dll` / `_dll` from every crate and build artifact.

19 ME3-loadable shell crates were named `er-<feature>-dll` and 12 of them produced an
artifact literally called `er_<feature>_dll.dll`. Nine of those shells sit beside a
library crate of the same base name, so the shell could not simply drop the suffix --
the name was taken. This migration resolves that the other way round:

    er-telemetry       (library)  ->  er-telemetry-core
    er-telemetry-dll   (shell)    ->  er-telemetry        ->  er_telemetry.dll

The ten shells with no sibling just drop the suffix. The result is that every
ME3-loadable artifact is `er_<feature>.dll` with no suffix at all, and no package,
lib target, or exported symbol carries `_dll`.

Substitution is a single pass with a longest-alternative-first alternation. Two passes
(or a naive per-token sed) would rename `er-telemetry-dll` -> `er-telemetry` and then
`er-telemetry` -> `er-telemetry-core`, silently collapsing the shell into the library.
The lookarounds also refuse to match a longer name that merely starts with a renamed
one, so `er-build-import-runtime` and `er-invasion-warp-telemetry` are left alone.

Usage: python3 scripts/rename-dll-crates.py [--apply] [--profiles-dir DIR]
       Default is a dry run that prints the file/line counts it would change.
Env:   ER_MODS_REPO_ROOT, ELDEN_PROFILES_DIR (default ~/Elden) override the defaults.
"""

import os
import re
import subprocess
import sys

# 9 shells that have a sibling library crate of the same base name. The library takes a
# -core suffix; the shell takes the plain name.
PAIRS = [
    "build-import", "build-watermark", "crash-logging", "invasion-warp",
    "loading-bar", "loading-portrait", "quit-menu", "save-picker", "telemetry",
]

# 10 shells with no sibling: they simply drop the -dll suffix.
STANDALONE = [
    "better-refills", "charm-enemies", "death-persist", "input-harness",
    "inventory-sort", "net-effects", "player-name-filter", "reload-trace",
    "save-disable", "seamless-bugfixes",
]

# Files renamed because their name embeds a crate name that ceases to exist.
FILE_RENAMES = [
    ("scripts/deploy-er-net-effects-dll.ps1", "scripts/deploy-er-net-effects.ps1"),
    ("scripts/check-reload-trace-dll-policy.py", "scripts/check-reload-trace-policy.py"),
    (".auto/reload_trace_dll_policy.rego", ".auto/reload_trace_policy.rego"),
]

# Non-crate identifiers that also carry the suffix and must move with their crate.
EXTRA = [
    ("reload_trace_dll_policy", "reload_trace_policy"),   # rego filename in prose/paths
    ("auto.reload_trace_dll", "auto.reload_trace"),       # rego package
    ("check-reload-trace-dll-policy", "check-reload-trace-policy"),
    ("deploy-er-net-effects-dll", "deploy-er-net-effects"),
]

SKIP_DIRS = {".git", "target", ".worktrees", "node_modules"}


def token_map():
    """Ordered (token, replacement) pairs, most specific first."""
    m = list(EXTRA)
    # 1. exported host-stub symbols: er_X_dll_host_stub -> er_X_host_stub
    for b in PAIRS + STANDALONE:
        u = b.replace("-", "_")
        m.append((f"er_{u}_dll_host_stub", f"er_{u}_host_stub"))
    # 2. shell package/dir names: er-X-dll -> er-X
    for b in PAIRS + STANDALONE:
        m.append((f"er-{b}-dll", f"er-{b}"))
    # 3. shell lib idents / artifact stems: er_X_dll -> er_X
    for b in PAIRS + STANDALONE:
        u = b.replace("-", "_")
        m.append((f"er_{u}_dll", f"er_{u}"))
    # 4. library package/dir names: er-X -> er-X-core (pairs only)
    for b in PAIRS:
        m.append((f"er-{b}", f"er-{b}-core"))
    # 5. library idents: er_X -> er_X_core (pairs only)
    for b in PAIRS:
        u = b.replace("-", "_")
        m.append((f"er_{u}", f"er_{u}_core"))
    return m


def build_regex(mapping):
    # `[\w-]` on both sides: a hyphen is part of a crate name, so `er-invasion-warp`
    # must not match inside `er-invasion-warp-telemetry`. An underscore is already \w.
    alts = "|".join(re.escape(t) for t, _ in mapping)
    return re.compile(rf"(?<![\w-])(?:{alts})(?![\w-])")


def repo_files(root):
    out = subprocess.run(
        ["git", "-C", root, "ls-files"], capture_output=True, text=True, check=True,
        timeout=30,
    ).stdout.splitlines()
    return [os.path.join(root, f) for f in out]


def profile_files(profiles_dir):
    if not os.path.isdir(profiles_dir):
        return []
    found = []
    for dirpath, dirnames, filenames in os.walk(profiles_dir):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        found += [os.path.join(dirpath, f) for f in filenames if f.endswith(".me3")]
    return found


def rewrite(paths, rx, table, apply_changes):
    changed, total = [], 0
    for p in paths:
        try:
            with open(p, encoding="utf-8") as fh:
                text = fh.read()
        except (UnicodeDecodeError, IsADirectoryError, FileNotFoundError):
            continue
        new, n = rx.subn(lambda m: table[m.group(0)], text)
        if n:
            changed.append((p, n))
            total += n
            if apply_changes:
                with open(p, "w", encoding="utf-8") as fh:
                    fh.write(new)
    return changed, total


def git_mv(root, src, dst):
    if not os.path.exists(os.path.join(root, src)):
        print(f"  SKIP (absent): {src}")
        return
    subprocess.run(["git", "-C", root, "mv", src, dst], check=True, timeout=30)
    print(f"  {src} -> {dst}")


def main() -> int:
    args = sys.argv[1:]
    if "--help" in args or "-h" in args:
        print(__doc__)
        return 0
    apply_changes = "--apply" in args
    unknown = [a for a in args if a not in ("--apply",)]
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

    mapping = token_map()
    table = dict(mapping)
    rx = build_regex(mapping)

    if apply_changes:
        # Directory moves first, so the text pass sees the final paths. Libraries before
        # shells: `er-telemetry` must vacate the name before `er-telemetry-dll` takes it.
        print("== git mv: library crates -> -core ==")
        for b in PAIRS:
            git_mv(root, f"crates/er-{b}", f"crates/er-{b}-core")
        print("== git mv: shell crates -> drop -dll ==")
        for b in PAIRS + STANDALONE:
            git_mv(root, f"crates/er-{b}-dll", f"crates/er-{b}")
        print("== git mv: files naming a retired crate ==")
        for src, dst in FILE_RENAMES:
            git_mv(root, src, dst)

    paths = repo_files(root) + profile_files(profiles_dir)
    changed, total = rewrite(paths, rx, table, apply_changes)

    verb = "rewrote" if apply_changes else "would rewrite"
    print(f"\n{verb} {total} references in {len(changed)} files")
    for p, n in sorted(changed, key=lambda x: -x[1])[:25]:
        print(f"  {n:5d}  {os.path.relpath(p, root)}")
    if len(changed) > 25:
        print(f"  ... {len(changed) - 25} more files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
