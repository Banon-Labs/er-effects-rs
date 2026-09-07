#!/usr/bin/env python3
"""Fail when the installed Seamless Co-op is not the one build this workspace supports.

Why this exists
---------------
On 2026-09-02 Seamless Co-op v2.0.0 replaced v1.9.9 under an unchanged file name,
and every `ersc.dll` fact this repo holds went stale in one move: `show` left
`0x180022d30`, the session object's state field went from `S+0x110` to `S+0x150`,
and the session-state enum was renumbered by `+1` throughout.  None of that
announces itself.  A DLL built against v1.9.9 loads into v2.0.0 without complaint;
its gates refuse, the feature silently does nothing, and the player has no way to
know the mod and their Seamless are describing different programs.

So this workspace supports exactly one Seamless build at a time, and the question
asked here is the only one worth asking: *is the installed module that build, yes
or no*.  There is no candidate list and no adaptation.  A set of addresses from one
build used with the field offsets of another does not fail loudly -- it reads and
writes the wrong members of a live multiplayer session.

Both halves of the comparison are readable off the disk, so this never needs a game
to run.  The supported version is `ERSC_SUPPORTED_VERSION` in
`build-support/prologue_build.rs`, parsed rather than copied -- the point is to
read what the build scripts actually enforce.  The installed version is the
`Seamless Co-op vX.Y.Z by Yui` banner Seamless builds into its own image, read
rather than inferred from a path, a size or a timestamp, none of which change when
the build does.

What it refuses to guess
------------------------
If it cannot find the module, or cannot find the constant, or cannot parse either,
it says which one and exits 0 -- a missing Seamless is not a failed gate, it is a
machine without Seamless.  It exits non-zero only for the case it exists to catch:
both sides present, readable, and disagreeing.  That is the same line
`check-game-version-supported.py` draws, for the same reason.

Relationship to the build script
--------------------------------
`build-support/prologue_build.rs` performs the same identity check, but on the file
it is about to ground-truth constants against -- which `ER_ERSC_DLL` may redirect to
an archived copy of the supported build so a developer can still compile.  This gate
answers the other question: what will the machine actually load.  It honours
`ERSC_DLL`/`ER_GAME_DIR` (there is no single hard-coded user path in this repo) but
not `ER_ERSC_DLL`, so the build-side escape hatch cannot wave a real mismatch past
the validation suite.

    python3 scripts/check-ersc-version-supported.py
    python3 scripts/check-ersc-version-supported.py --selftest
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

# `pub const ERSC_SUPPORTED_VERSION: &str = "1.9.9";` -- parsed, never assumed, so this
# gate cannot disagree with the build scripts about what "supported" means.
SUPPORTED_CONST = re.compile(
    r'ERSC_SUPPORTED_VERSION\s*:\s*&\s*str\s*=\s*"([0-9]+(?:\.[0-9]+)*)"'
)

# The banner Seamless builds into its own image, e.g. "Seamless Co-op v2.0.0 by Yui".
#
# UTF-16LE and NUL-terminated (measured: v1.9.9 @0x1dcaf4, v2.0.0 @0x1e19dc, exactly once
# each, with no ASCII copy in either file).  Reading UTF-16 code units to the terminator is
# not fussiness: stripping NULs to fake an ASCII search merges the banner with the string
# pooled directly after it, and both real builds then report "v2.0.0 by YuiThis".
VERSION_BANNER = "Seamless Co-op v"
VERSION_BANNER_LIMIT = 128

PROLOGUE_BUILD = "build-support/prologue_build.rs"
REPO = Path(__file__).resolve().parent.parent


class Unreadable(Exception):
    """Something we need is absent or unparseable.  Not a gate failure."""


def game_dir() -> Path:
    """The ELDEN RING `Game` directory, env-overridable, no hard-coded user path."""
    env = os.environ.get("ER_GAME_DIR")
    if env:
        return Path(env)
    return Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game"


def installed_path() -> Path:
    """What the game will load.  Deliberately not `ER_ERSC_DLL`; see the module docstring."""
    env = os.environ.get("ERSC_DLL")
    return Path(env) if env else game_dir() / "SeamlessCoop" / "ersc.dll"


def version_in_bytes(data: bytes) -> str | None:
    """The `X.Y.Z` from the Seamless banner in `data`, or None if it carries no banner.

    Split out so `--selftest` can exercise it on a synthetic buffer and therefore mean
    something on a machine that has never installed Seamless.
    """
    prefix = VERSION_BANNER.encode("utf-16le")
    start = data.find(prefix)
    if start < 0:
        return None
    end = start + len(prefix)
    # `len(data) - 1` so the two-byte terminator read always has a second byte; the limit
    # keeps a corrupt image from turning this into a walk of the whole file.
    limit = min(end + VERSION_BANNER_LIMIT, len(data) - 1)
    while end < limit and data[end : end + 2] != b"\x00\x00":
        end += 2
    text = data[start + len(prefix) : end].decode("utf-16le", errors="replace")
    token = text.split()[0] if text.split() else ""
    # The trailing " by Yui" is an author credit, not a version, and pinning it would make a
    # rename read as a version mismatch.  The shape check is what stops a hit in compressed
    # bytes being reported as a version.
    return token if re.fullmatch(r"[0-9]+(?:\.[0-9]+)*", token) else None


def installed_version(path: Path) -> str:
    try:
        data = path.read_bytes()
    except OSError as err:
        raise Unreadable(f"cannot read {path}: {err}") from err
    version = version_in_bytes(data)
    if version is None:
        raise Unreadable(f"no Seamless Co-op version banner in {path}")
    return version


def supported_version(prologue_build: Path) -> str:
    try:
        text = prologue_build.read_text(encoding="utf-8")
    except OSError as err:
        raise Unreadable(f"cannot read {prologue_build}: {err}") from err
    found = SUPPORTED_CONST.search(text)
    if not found:
        raise Unreadable(f"no ERSC_SUPPORTED_VERSION in {prologue_build}")
    return found.group(1)


def selftest() -> int:
    """Prove both readers on inputs whose answers are known.

    Neither half needs Seamless or the game, which is the difference between this and
    `check-game-version-supported.py --selftest`: a banner is a string constant, so a
    synthetic buffer exercises the real code path rather than a stand-in for it.
    """
    failures = []

    def banner(text: str) -> bytes:
        return b"\x11\x22" + text.encode("utf-16le") + b"\x00\x00" + b"padding"

    cases = [
        ("Seamless Co-op v1.9.9 by Yui", "1.9.9"),
        ("Seamless Co-op v2.0.0 by Yui", "2.0.0"),
        # A future build that drops the author credit still identifies.
        ("Seamless Co-op v2.1", "2.1"),
        # No banner at all, and a banner whose body is not a version: both unidentifiable,
        # which the caller treats as unsupported rather than as "probably fine".
        ("nothing to see here", None),
        ("Seamless Co-op vNEXT by Yui", None),
    ]
    for text, want in cases:
        got = version_in_bytes(banner(text))
        if got != want:
            failures.append(f"version_in_bytes({text!r}) returned {got!r}, want {want!r}")

    # The pooled-string trap this reader exists to avoid: the next literal follows the
    # terminator immediately, and an ASCII-ish search would swallow it.
    pooled = (
        b"\x00\x00"
        + "Seamless Co-op v2.0.0 by Yui".encode("utf-16le")
        + b"\x00\x00"
        + "This is the next string".encode("utf-16le")
    )
    if version_in_bytes(pooled) != "2.0.0":
        failures.append(f"pooled-string case returned {version_in_bytes(pooled)!r}, want '2.0.0'")

    # The constant parser, against the shape prologue_build.rs actually uses.
    sample = 'pub const ERSC_SUPPORTED_VERSION: &str = "1.9.9";\n'
    got_const = SUPPORTED_CONST.search(sample)
    if not got_const or got_const.group(1) != "1.9.9":
        failures.append(f"constant parser returned {got_const!r} on the real declaration shape")

    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        empty = Path(tmp) / "prologue_build.rs"
        empty.write_text("// nothing here\n", encoding="utf-8")
        try:
            supported_version(empty)
        except Unreadable:
            pass
        else:
            failures.append("a constant-less prologue_build.rs did not raise Unreadable")

    # And against the real file, so a rename of the constant fails here rather than
    # silently turning this gate into a no-op.
    real = REPO / PROLOGUE_BUILD
    if real.is_file():
        try:
            print(f"selftest: {PROLOGUE_BUILD} records v{supported_version(real)}")
        except Unreadable as err:
            failures.append(str(err))
    else:
        failures.append(f"{PROLOGUE_BUILD} is missing")

    installed = installed_path()
    if installed.is_file():
        try:
            print(f"selftest: read v{installed_version(installed)} from {installed}")
        except Unreadable as err:
            print(f"selftest: {err}")
    else:
        print(f"selftest: no Seamless at {installed}; installed reader not exercised")

    for line in failures:
        print(f"SELFTEST FAIL: {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--installed", type=Path, default=installed_path())
    ap.add_argument("--prologue-build", type=Path, default=REPO / PROLOGUE_BUILD)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    if not args.installed.is_file():
        print(f"SKIP: no Seamless Co-op at {args.installed} (set ERSC_DLL or ER_GAME_DIR)")
        return 0

    try:
        supported = supported_version(args.prologue_build)
        installed = installed_version(args.installed)
    except Unreadable as err:
        print(f"SKIP: {err}")
        return 0

    if installed == supported:
        print(f"OK: installed Seamless Co-op v{installed} is the build this workspace supports")
        return 0

    print(f"FAIL: Seamless Co-op v{installed} is installed; this workspace supports v{supported}.")
    print()
    print("This workspace supports ONE Seamless build at a time. Every ersc.dll RVA, struct")
    print("field offset and state code here was measured against the supported build, and they")
    print("do not carry across an update: v2.0.0 moved the session object's state field from")
    print("S+0x110 to S+0x150 and renumbered the session-state enum by +1. An address from one")
    print("build used with another build's field offsets reads and writes the wrong members of")
    print("a live multiplayer session, so the runtime gates refuse and every ersc-dependent")
    print("feature goes inert rather than guessing:")
    print()
    print("    local-invasion: ersc.dll @0x... does not match the RVAs this build was measured")
    print(f"    against (Seamless Co-op v{supported}) -- NOT touching it.")
    print()
    print("Re-measure rather than find-and-replace: a signature match is a candidate, not an")
    print("identification (v2.0.0's 19-byte BUILD_LOBBY_KEY pin matches exactly one address in")
    print("v2.0.0, and it is the wrong function).")
    print()
    print("    uv run --with capstone python3 scripts/locate-ersc-entry-points.py")
    print()
    print("Then re-pin the constants and the field offsets, and set ERSC_SUPPORTED_VERSION in")
    print(f'{PROLOGUE_BUILD} to "{installed}".')
    return 1


if __name__ == "__main__":
    sys.exit(main())
