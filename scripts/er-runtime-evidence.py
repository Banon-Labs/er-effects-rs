#!/usr/bin/env python3
"""Which run, if any, proves that the code at a commit actually executed.

One implementation, three readers
---------------------------------
`.cupcake/signals/runtime_evidence_for_head.sh` (the verdict a `git push` is gated on),
`.cupcake/signals/runtime_evidence_note.sh` (the sentence the denial quotes) and
`scripts/check-runtime-evidence.sh` (the pre-push hook) all ask the same question. They used
to answer it with three separate copies of the same scan, and copies drift: the note said one
log was to blame while the verdict was computed from another, which is how a refusal sent an
agent hunting a log that had nothing to do with it.

Where a run leaves its evidence
-------------------------------
Every shell in this workspace opens its own log with one identity line:

    build git=e0e3a2e11e80+dirty module=er_quit_rows.dll base=0x6ffff9de0000 pe=0x6aa4c5a9 (...)

Two directories hold those logs and both count:

  * the run root, `~/.cache/er-me3-runs/br-*/`, which only `scripts/er-run-branch.py` writes;
  * the `ELDEN RING/Game` directory, where a `~/Elden/launch.sh` run writes instead -- the way
    the user launches. Scanning only the first is why a push proven by a live run was refused
    on 2026-09-11: the evidence existed, in the other directory, unread.

The sha on that line, never a file timestamp, is what ties a run to code. An earlier version
compared mtimes and called a log written by a two-commit-old build "evidence", because the
process was still running and still writing.

When `+dirty` is still evidence
-------------------------------
`+dirty` says the tree carried uncommitted changes when that shell was built. That does not by
itself mean the shell contains any of them: this workspace routinely has several agents editing
unrelated crates, so a tree is nearly always dirty somewhere, and refusing every such log makes
the guard unpassable for a change that did run.

A dirty log is accepted only when four separate facts line up, each of which refuses on its own:

  1. the log names the commit being pushed -- so nothing has been committed since that build;
  2. the artifact on disk carries the same `PE` timestamp the running module reported, which is
     the link identity of that exact binary;
  3. `scripts/er-dll-provenance.py verify` passes, so the compiled dependency closure recorded
     at build time is still byte-identical to the working tree;
  4. `git status` reports nothing uncommitted anywhere in that closure, so the working tree
     copy of it is the committed copy.

Together those say: the binary that ran was compiled from source identical to what the commit
contains. A fifth requirement keeps the accept honest about scope -- every crate the push
changes must be inside the closure of some accepted dirty log, so the evidence covers the code
being pushed rather than merely coexisting with it.

Usage:
    python3 scripts/er-runtime-evidence.py --head <sha> [--exhaustive] < changed-paths
    python3 scripts/er-runtime-evidence.py --selftest

Reads the pushed diff's paths on stdin, one per line; without them no dirty log can be
accepted, because requirement five cannot be checked.

Writes to stdout, in this order:

    note <one sentence naming what ran, for the human message>
    match <where> <sha>                 only when a run proves the commit
    candidate <sha> <where>             clean builds the caller may carry forward from

Exit status: 0 a run proves it, 1 none does, 2 neither log directory exists (unmeasurable,
which callers treat as "allow" -- a check that cannot see must not invent a verdict).
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import pathlib
import re
import struct
import subprocess
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent

BUILD_LINE = re.compile(
    r"^build git=(?P<sha>[0-9a-f]+)(?P<dirty>\+dirty)?"
    r"(?:\s+module=(?P<module>\S+))?"
    r"(?:[^\n]*?\bpe=0x(?P<pe>[0-9a-fA-F]+))?"
)

EXIT_MATCH = 0
EXIT_NO_MATCH = 1
EXIT_UNMEASURABLE = 2

# How many distinct clean build shas to collect before giving up on the run root. The caller
# turns each one into a reverse-dependency walk, so an uncapped list costs it minutes; and an
# older sha reaches the tip across strictly more commits, so if the newest cannot carry the
# diff forward, an older one cannot either.
CANDIDATES = 2

GIT_TIMEOUT_SECONDS = 20


_LOADED: dict[str, object] = {}


def load_script(name: str, filename: str):
    """Import a sibling script under a dashed file name, once per process.

    Registered in `sys.modules` before it executes, because `@dataclass` resolves its own
    module out of that table: `scripts/er_run_lib.py` raises `AttributeError` on `NoneType`
    partway through import without it, which a caller then reads as "the game directory does
    not exist" rather than "the module failed to load".
    """
    if name in _LOADED:
        return _LOADED[name]
    spec = importlib.util.spec_from_file_location(name, REPO_ROOT / "scripts" / filename)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {filename}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    except BaseException:
        del sys.modules[name]
        raise
    _LOADED[name] = module
    return module


class Entry:
    """One log's identity line, with where it was found."""

    def __init__(self, path: pathlib.Path, where: str, found: re.Match, mtime: int) -> None:
        self.path = path
        self.where = where
        self.sha = found.group("sha")
        self.dirty = bool(found.group("dirty"))
        self.module = found.group("module") or ""
        self.pe = int(found.group("pe"), 16) if found.group("pe") else None
        self.mtime = mtime

    def names(self, sha: str) -> bool:
        return self.sha.startswith(sha) or sha.startswith(self.sha)

    def described(self) -> str:
        if self.module and not self.module.startswith("<"):
            return f"{self.path.name} ({self.module})"
        return self.path.name


# ------------------------------------------------------------------------------------------
# Where the logs are
# ------------------------------------------------------------------------------------------


def run_root() -> pathlib.Path:
    """The branch-launch run root. `ER_ME3_RUN_ROOT` overrides it, as in every caller."""
    default = pathlib.Path.home() / ".cache" / "er-me3-runs"
    return pathlib.Path(os.environ.get("ER_ME3_RUN_ROOT", default))


def game_dir() -> pathlib.Path | None:
    """The `ELDEN RING/Game` directory, where a launcher run's DLLs write their logs.

    `ER_GAME_DIR` wins, as in the other tools that read game-directory artifacts; otherwise the
    path comes from `er_run_lib.game_dir`, the one owner of that layout in this repo, so the
    `ME3_STEAM_DIR` override and the default Steam root are not spelled a second time here.
    """
    explicit = os.environ.get("ER_GAME_DIR")
    if explicit:
        return pathlib.Path(explicit)
    try:
        return pathlib.Path(load_script("er_run_lib", "er_run_lib.py").game_dir())
    except Exception:  # noqa: BLE001 -- a signal that cannot resolve a path must not crash
        return None


def read_identity(path: pathlib.Path, where: str) -> Entry | None:
    try:
        mtime = int(path.stat().st_mtime)
        with path.open(encoding="utf-8", errors="replace") as handle:
            first = handle.readline()
    except OSError:
        return None
    found = BUILD_LINE.match(first)
    if not found:
        return None
    return Entry(path, where, found, mtime)


def collect(head: str, exhaustive: bool) -> tuple[list[Entry], list[str]]:
    """Every identity line worth reading, newest source first, plus the roots that existed.

    The run root is walked newest directory first and stops once enough distinct clean shas
    have been seen, because it holds hundreds of runs and this is read on every Bash tool call.
    The game directory is a single slot -- one log per shell, overwritten each launch -- so it
    is always read whole, which also means a match there can never be missed by the cap.
    """
    entries: list[Entry] = []
    roots: list[str] = []

    runs = run_root()
    if runs.is_dir():
        roots.append(str(runs))
        distinct: set[str] = set()
        try:
            directories = sorted((d for d in runs.iterdir() if d.is_dir()), reverse=True)
        except OSError:
            directories = []
        for run in directories:
            if not exhaustive and len(distinct) >= CANDIDATES:
                break
            for artifact in sorted(run.glob("er-*.log")):
                entry = read_identity(artifact, f"{run.name}/{artifact.name}")
                if entry is None:
                    continue
                entries.append(entry)
                if not entry.dirty:
                    distinct.add(entry.sha)

    game = game_dir()
    if game is not None and game.is_dir():
        roots.append(str(game))
        for artifact in sorted(game.glob("er-*.log")):
            entry = read_identity(artifact, f"game-dir/{artifact.name}")
            if entry is not None:
                entries.append(entry)

    entries.sort(key=lambda entry: entry.mtime, reverse=True)
    return entries, roots


# ------------------------------------------------------------------------------------------
# The dirty-log proof
# ------------------------------------------------------------------------------------------


def git(*args: str) -> str | None:
    try:
        proc = subprocess.run(
            ["git", *args],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            check=False,
            timeout=GIT_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return proc.stdout if proc.returncode == 0 else None


def artifact_dir() -> pathlib.Path:
    """Where a build leaves its DLLs, matching `scripts/er-build-dlls.sh`."""
    target = os.environ.get("ER_BUILD_TARGET", "x86_64-pc-windows-msvc")
    return REPO_ROOT / "target" / target / "release"


def pe_timestamp(artifact: pathlib.Path) -> int | None:
    """The `COFF TimeDateStamp` of a `PE` file: its link identity, four bytes after the signature.

    This is the value a loaded module reports on its own identity line, so comparing the two is
    what ties the binary on disk to the binary that ran.
    """
    try:
        with artifact.open("rb") as handle:
            head = handle.read(0x400)
    except OSError:
        return None
    if len(head) < 0x40 or head[:2] != b"MZ":
        return None
    offset = struct.unpack_from("<I", head, 0x3C)[0]
    if offset + 12 > len(head) or head[offset : offset + 4] != b"PE\0\0":
        return None
    return struct.unpack_from("<I", head, offset + 8)[0]


def package_for(module: str) -> str | None:
    """The cargo package that produces `module`, from the shipped-shell list.

    Four crates override `[lib] name`, so swapping dashes for underscores is not the map.
    """
    try:
        pairs = load_script("me3_dll_list", "me3-dll-list.py").dll_pairs()
    except Exception:  # noqa: BLE001 -- an unreadable list means "cannot prove", not a crash
        return None
    stem = module[:-4] if module.endswith(".dll") else module
    for package, artifact in pairs:
        if artifact == stem:
            return package
    return None


def closure_members(package: str) -> list[str] | None:
    """The in-repo crates whose source compiles into `package`."""
    try:
        provenance = load_script("er_dll_provenance", "er-dll-provenance.py")
        members, _external = provenance.forward_closure(package)
    except Exception:  # noqa: BLE001
        return None
    return members


def provenance_is_fresh(package: str, artifact: pathlib.Path) -> tuple[bool, str]:
    """Does the record beside `artifact` still describe this source tree?"""
    try:
        provenance = load_script("er_dll_provenance", "er-dll-provenance.py")
        code, failures = provenance.verify(package, artifact)
    except Exception as err:  # noqa: BLE001
        return False, f"its provenance record could not be read ({err})"
    if code == 0 and not failures:
        return True, ""
    return False, failures[0] if failures else "its provenance record does not match"


_DIRTY_CRATES: list[set[str] | None] = []


def dirty_crate_names() -> set[str] | None:
    """Crates carrying anything uncommitted: modified, staged, or untracked and not ignored.

    Read with `-z` so a path needing quotes cannot be mis-split, and matched by pattern rather
    than by field position so a rename's second record counts too. Over-collecting here only
    ever refuses more. Read once per process, because one scan asks it per candidate log and a
    run writes 26 of them.
    """
    if _DIRTY_CRATES:
        return _DIRTY_CRATES[0]
    out = git("status", "--porcelain", "-z", "--", "crates")
    if out is None:
        answer = None
    else:
        answer = {m.group(1) for m in re.finditer(r"crates/([A-Za-z0-9_.+-]+)/", out)}
    _DIRTY_CRATES.append(answer)
    return answer


def crates_in(paths: list[str]) -> set[str]:
    """The crate directories a list of repo-relative paths touches."""
    found = set()
    for path in paths:
        match = re.match(r"crates/([^/]+)/", path)
        if match:
            found.add(match.group(1))
    return found


def dirty_build_is_committed(entry: Entry) -> tuple[list[str] | None, str]:
    """Prove the binary that wrote this dirty log was compiled from committed source.

    Returns `(closure, "")` when it holds and `(None, reason)` when it does not. The reason is
    written to be read by whoever hit the refusal, so it names the artifact and the crate.
    """
    if not entry.module or entry.module.startswith("<"):
        return None, "its log names no module, so the artifact it came from cannot be found"
    if entry.pe is None:
        return None, (
            "its log carries no pe stamp, so the binary it came from cannot be tied to it"
        )

    package = package_for(entry.module)
    if package is None:
        return None, f"{entry.module} is not a shell this workspace builds"

    artifact = artifact_dir() / entry.module
    if not artifact.is_file():
        where = artifact
        try:
            where = artifact.relative_to(REPO_ROOT)
        except ValueError:
            pass
        return None, f"nothing is built at {where}"

    on_disk = pe_timestamp(artifact)
    if on_disk is None:
        return None, f"{entry.module} on disk could not be read as a pe file"
    if on_disk != entry.pe:
        return None, (
            f"{entry.module} has been relinked since that run "
            f"(pe 0x{on_disk:08x} on disk, 0x{entry.pe:08x} in the log)"
        )

    members = closure_members(package)
    if members is None:
        return None, f"the dependency closure of {package} could not be resolved"

    dirty = dirty_crate_names()
    if dirty is None:
        return None, "git could not report what is uncommitted"
    overlap = sorted(dirty.intersection(members))
    if overlap:
        return None, (
            f"{overlap[0]} is uncommitted and compiles into {entry.module}, "
            "so that binary is not this commit"
        )

    # Last, because it is the only expensive step: it hashes every file in the closure. The
    # cheap facts above reject nearly everything first, which keeps this signal's cost where its
    # header promises when a run is being re-read on every Bash tool call.
    fresh, why = provenance_is_fresh(package, artifact)
    if not fresh:
        return None, f"{why.rstrip('.')} -- rebuild and rerun"

    return members, ""


# ------------------------------------------------------------------------------------------
# The scan
# ------------------------------------------------------------------------------------------


def scan(head: str, changed: list[str], exhaustive: bool) -> tuple[int, list[str]]:
    """Answer for `head`. Returns (exit status, output lines)."""
    entries, roots = collect(head, exhaustive)
    if not roots:
        return EXIT_UNMEASURABLE, [
            f"note no run root at {run_root()} and no game directory at {game_dir()} "
            "-- cannot measure"
        ]

    for entry in entries:
        if not entry.dirty and entry.names(head):
            return EXIT_MATCH, [
                f"note {entry.described()} was built from {head} and ran",
                f"match {entry.where} {entry.sha}",
            ]

    # No clean log names the commit. A dirty one still can, when its own sources were committed
    # and its closure covers everything being pushed. Both halves are checked here because
    # either alone would accept a run that never executed the code in the push.
    wanted = crates_in(changed)
    covered: set[str] = set()
    proven: list[Entry] = []
    refusals: list[tuple[Entry, str]] = []
    seen: set[tuple[str, int | None]] = set()
    for entry in entries:
        if not (entry.dirty and entry.names(head)):
            continue
        key = (entry.module, entry.pe)
        if key in seen:
            continue
        seen.add(key)
        members, why = dirty_build_is_committed(entry)
        if members is None:
            refusals.append((entry, why))
            continue
        proven.append(entry)
        covered.update(members)

    if proven and wanted and wanted.issubset(covered):
        first = proven[0]
        return EXIT_MATCH, [
            f"note {first.described()} was built from {head} with its own sources committed, "
            "and ran",
            f"match {first.where} {first.sha}",
        ]

    lines = [f"note {no_match_note(head, entries, proven, wanted, covered, refusals)}"]
    emitted: set[str] = set()
    for entry in entries:
        if entry.dirty or entry.sha in emitted:
            continue
        emitted.add(entry.sha)
        lines.append(f"candidate {entry.sha} {entry.where}")
        if len(emitted) >= CANDIDATES and not exhaustive:
            break
    return EXIT_NO_MATCH, lines


def no_match_note(
    head: str,
    entries: list[Entry],
    proven: list[Entry],
    wanted: set[str],
    covered: set[str],
    refusals: list[tuple[Entry, str]],
) -> str:
    """One sentence saying why nothing proved `head`, naming what to do about it."""
    if proven:
        missing = sorted(wanted - covered) or ["nothing this push changes"]
        return (
            f"{proven[0].described()} ran this commit's own code, but {missing[0]} "
            "is not compiled into any shell that ran -- run one that is"
        )
    if refusals:
        entry, why = refusals[0]
        return f"{entry.described()} ran code built at {head}, but {why}"
    if not entries:
        return "no log in either the run root or the game directory carries a build line"
    newest = entries[0]
    # The dirt is named but not blamed: what refused this log is the sha, and an earlier version
    # of this sentence said only "was built from a DIRTY tree", which reads as the reason and
    # sent the agent looking for uncommitted files instead of for a run at the right commit.
    aside = " (with work uncommitted at the time)" if newest.dirty else ""
    return (
        f"the newest log {newest.described()} was built from {newest.sha[:8]}{aside}, not {head}"
    )


# ------------------------------------------------------------------------------------------
# Selftest
# ------------------------------------------------------------------------------------------


def selftest() -> int:  # noqa: C901 -- a list of cases reads better than a table of lambdas
    import tempfile

    failures = 0

    def check(condition: bool, label: str) -> None:
        nonlocal failures
        if condition:
            print(f"  ok    {label}")
        else:
            print(f"  FAIL  {label}")
            failures += 1

    def run(head: str, changed: list[str] | None = None) -> tuple[int, list[str]]:
        return scan(head, changed or [], exhaustive=True)

    identity = "build git={sha}{dirty} module={module} base=0x1 pe=0x{pe} (t)\nmore\n"

    with tempfile.TemporaryDirectory() as raw:
        tmp = pathlib.Path(raw)
        runs = tmp / "runs"
        game = tmp / "game"
        (runs / "br-good").mkdir(parents=True)
        game.mkdir()
        os.environ["ER_ME3_RUN_ROOT"] = str(runs)
        os.environ["ER_GAME_DIR"] = str(game)

        # The gap this file was written for: a run launched the way the user launches writes
        # into the game directory, and only the run root used to be read.
        (game / "er-quit-rows-debug.log").write_text(
            identity.format(sha="deadbeef1234", dirty="", module="er_quit_rows.dll", pe="6aa4c5a9"),
            encoding="utf-8",
        )
        code, lines = run("deadbeef1234")
        check(code == EXIT_MATCH, "a clean game-directory log naming the commit is evidence")
        check(
            any(line.startswith("match game-dir/er-quit-rows-debug.log") for line in lines),
            "the match names the game-directory log it came from",
        )
        code, _ = run("deadbeef")
        check(code == EXIT_MATCH, "an abbreviated commit sha matches the log's full sha")

        code, lines = run("0e0842402b18")
        check(code == EXIT_NO_MATCH, "a game-directory log for a different sha is refused")
        check(
            any("not 0e0842402b18" in line for line in lines),
            "the refusal says which build the log carried instead",
        )

        # The run root still counts, and a match there is found with the game directory present.
        (runs / "br-good" / "er-invasion-warp.log").write_text(
            identity.format(
                sha="aaaaaaaaaaaa", dirty="", module="er_invasion_warp.dll", pe="6a000001"
            ),
            encoding="utf-8",
        )
        code, lines = run("aaaaaaaaaaaa")
        check(code == EXIT_MATCH, "a clean run-root log naming the commit is still evidence")

        # A dirty log with no artifact behind it proves nothing, and says why.
        (game / "er-focus-input.log").write_text(
            identity.format(
                sha="bbbbbbbbbbbb", dirty="+dirty", module="er_focus_input.dll", pe="6a000002"
            ),
            encoding="utf-8",
        )
        code, lines = run("bbbbbbbbbbbb", ["crates/er-focus-input/src/lib.rs"])
        check(code == EXIT_NO_MATCH, "a dirty log whose artifact cannot be checked is refused")
        check(
            any("er_focus_input.dll" in line and "note" in line for line in lines),
            "the refusal names the module that wrote the log, not only the file name",
        )

        del os.environ["ER_ME3_RUN_ROOT"]
        del os.environ["ER_GAME_DIR"]
        os.environ["ER_ME3_RUN_ROOT"] = str(tmp / "absent")
        os.environ["ER_GAME_DIR"] = str(tmp / "absent")
        code, _ = run("deadbeef1234")
        check(
            code == EXIT_UNMEASURABLE,
            "with neither directory present the answer is unmeasurable",
        )
        del os.environ["ER_ME3_RUN_ROOT"]
        del os.environ["ER_GAME_DIR"]

    # The dirty-log proof, with its collaborators replaced so the cases are exact. Each fact is
    # removed in turn from an otherwise passing set, because a proof nobody has watched refuse
    # is not a proof.
    entry = Entry(
        pathlib.Path("/tmp/er-quit-rows-debug.log"),
        "game-dir/er-quit-rows-debug.log",
        BUILD_LINE.match(
            "build git=deadbeef1234+dirty module=er_quit_rows.dll base=0x1 pe=0x6aa4c5a9 (t)"
        ),
        0,
    )
    saved = (
        globals()["package_for"],
        globals()["artifact_dir"],
        globals()["pe_timestamp"],
        globals()["provenance_is_fresh"],
        globals()["closure_members"],
        globals()["dirty_crate_names"],
    )
    try:
        with tempfile.TemporaryDirectory() as raw:
            fake = pathlib.Path(raw)
            (fake / "er_quit_rows.dll").write_bytes(b"not really a pe")
            globals()["package_for"] = lambda module: "er-quit-rows"
            globals()["artifact_dir"] = lambda: fake
            globals()["pe_timestamp"] = lambda artifact: 0x6AA4C5A9
            globals()["provenance_is_fresh"] = lambda package, artifact: (True, "")
            globals()["closure_members"] = lambda package: ["er-quit-rows", "er-quit-menu-core"]
            globals()["dirty_crate_names"] = lambda: {"er-input-harness"}

            members, why = dirty_build_is_committed(entry)
            check(
                members == ["er-quit-rows", "er-quit-menu-core"] and not why,
                "a dirty build whose own closure is committed proves the commit ran",
            )

            globals()["dirty_crate_names"] = lambda: {"er-quit-menu-core"}
            members, why = dirty_build_is_committed(entry)
            check(
                members is None and "er-quit-menu-core" in why,
                "uncommitted source inside the closure refuses, naming the crate",
            )

            globals()["dirty_crate_names"] = lambda: {"er-input-harness"}
            globals()["pe_timestamp"] = lambda artifact: 0x11111111
            members, why = dirty_build_is_committed(entry)
            check(
                members is None and "relinked" in why,
                "an artifact relinked since the run refuses: it is not the binary that ran",
            )

            globals()["pe_timestamp"] = lambda artifact: 0x6AA4C5A9
            globals()["provenance_is_fresh"] = lambda package, artifact: (
                False,
                "SOURCE MOVED since this DLL was built",
            )
            members, why = dirty_build_is_committed(entry)
            check(
                members is None and "SOURCE MOVED" in why,
                "source that moved after the build refuses, whatever git status says",
            )

            globals()["provenance_is_fresh"] = lambda package, artifact: (True, "")
            globals()["dirty_crate_names"] = lambda: None
            members, why = dirty_build_is_committed(entry)
            check(members is None, "git failing to report leaves the dirty log unproven")

            # And the scope requirement, through the scan: the proof holds, but the push
            # changes a crate that is not compiled into the shell that ran.
            globals()["dirty_crate_names"] = lambda: {"er-input-harness"}
            with tempfile.TemporaryDirectory() as raw2:
                tmp2 = pathlib.Path(raw2)
                game2 = tmp2 / "game"
                game2.mkdir(parents=True)
                (game2 / "er-quit-rows-debug.log").write_text(
                    identity.format(
                        sha="deadbeef1234",
                        dirty="+dirty",
                        module="er_quit_rows.dll",
                        pe="6aa4c5a9",
                    ),
                    encoding="utf-8",
                )
                os.environ["ER_ME3_RUN_ROOT"] = str(tmp2 / "absent")
                os.environ["ER_GAME_DIR"] = str(game2)

                code, lines = run("deadbeef1234", ["crates/er-quit-rows/src/lib.rs"])
                check(
                    code == EXIT_MATCH,
                    "a dirty log covering the changed crate is accepted as evidence",
                )
                code, lines = run("deadbeef1234", ["crates/er-invasion-warp/src/lib.rs"])
                check(
                    code == EXIT_NO_MATCH
                    and any("er-invasion-warp" in line for line in lines),
                    "a dirty log that does not compile the changed crate is refused",
                )
                code, lines = run("deadbeef1234", [])
                check(
                    code == EXIT_NO_MATCH,
                    "with no changed paths to check against, a dirty log is not accepted",
                )
                del os.environ["ER_ME3_RUN_ROOT"]
                del os.environ["ER_GAME_DIR"]
    finally:
        (
            globals()["package_for"],
            globals()["artifact_dir"],
            globals()["pe_timestamp"],
            globals()["provenance_is_fresh"],
            globals()["closure_members"],
            globals()["dirty_crate_names"],
        ) = saved

    # The pe reader against a real artifact, when one is built: the fake above proves the
    # comparison, not the parser.
    real = artifact_dir() / "er_crash_logging.dll"
    if real.is_file():
        stamp = pe_timestamp(real)
        check(isinstance(stamp, int) and stamp > 0, f"a real DLL yields a pe stamp (0x{stamp:08x})")
    else:
        print(f"  skip  no built DLL at {real} -- the pe parser is exercised by the fake only")

    print("er-runtime-evidence selftest:", "PASS" if failures == 0 else f"{failures} failure(s)")
    return 0 if failures == 0 else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="Runtime evidence for a commit.")
    parser.add_argument("--head", help="the commit to look for")
    parser.add_argument(
        "--exhaustive",
        action="store_true",
        help="read every run directory rather than stopping at the newest few",
    )
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if not args.head:
        parser.error("--head is required")

    changed = []
    if not sys.stdin.isatty():
        changed = [line.strip() for line in sys.stdin.read().splitlines() if line.strip()]

    code, lines = scan(args.head, changed, args.exhaustive)
    for line in lines:
        print(line)
    return code


if __name__ == "__main__":
    sys.exit(main())
