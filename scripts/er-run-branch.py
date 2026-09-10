#!/usr/bin/env python3
"""Launch Elden Ring with the DLLs this branch changed, on a random decoded character.

Pipeline, in order, each step refusing rather than guessing:

  1. Garbage-collect dead runs. Cleanup is guaranteed by this step, not by the reaper.
  2. Steam PREFLIGHT via scripts/steam-running.sh. With Steam down the game still boots, but
     into a different environment (wineprefix, save dir, account id), so the logs land
     elsewhere and the run is not representative.
  3. Closure -- scripts/er-dll-closure.py. Refuses on a conflict it cannot rank.
  4. Provenance -- scripts/er-dll-provenance.py per selected DLL. A DLL with no provenance,
     or whose recorded source hash no longer matches this tree, is stale and stops the run.
     Nothing here builds: this tool's contract is that the DLL is already fresh, and if it is
     not, it says so loudly.
  5. Save -- scripts/er-pick-save.py. Random, but DECODED FIRST: the character's name, level
     and slot are known and printed before anything launches (AGENTS.md's Autoload Identity
     Launch Gate). `--seed` reproduces a pick exactly.
  6. Stage -- a temp .me3 plus a DLL-adjacent sidecar toml. The game-directory er-quickload.toml
     is never written.
  7. Launch -- ~/Elden/launch.sh with ME3_PROFILE, detached into its own session, and with every
     DLL artifact redirected into this run's own directory (ARTIFACT_ENV). A game-directory log is
     single-slot; two launches and the run before last is gone.
  8. Testimony -- the block is printed only after the DLL says, in its own debug log, that it
     loaded and read this run's sidecar. Otherwise a failed block is printed and the run is
     cleaned up.
  9. REAP -- a detached reaper removes the staged files when the game exits. It removes what the
     run staged, never what the run WROTE: the artifact directory survives.

Why the block waits for the DLL rather than the window
------------------------------------------------------
"The process started" is a weak claim -- me3 spawns through Proton and a crashing game is
briefly alive. "The window is up" is a strong claim but minutes away, well past the shell
budget. The DLL's own `runtime-config: loaded ... sidecar=...` line lands at DllMain, within
seconds, and proves three things at once: the process is up, our DLL is in it, and it read
our config. So the block cannot be printed for a run that did not really happen -- which
matters because a copy-pasted block is a promise to whoever reads it.

The block deliberately claims nothing about the window, the world, or readiness. `--status`
re-checks those later, honestly, once they exist.

Usage:
    python3 scripts/er-run-branch.py                      # random save, ersc loaded
    python3 scripts/er-run-branch.py --seed 4242          # reproduce a pick
    python3 scripts/er-run-branch.py --vanilla            # no ersc; .sl2 saves only
    python3 scripts/er-run-branch.py --monitor DP-1       # move the ER window when it appears
    python3 scripts/er-run-branch.py --dry-run            # stage and report, launch nothing
    python3 scripts/er-run-branch.py --status <run-id>
    python3 scripts/er-run-branch.py --selftest
"""

from __future__ import annotations

import argparse
import json
import os
import random
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import er_run_lib  # noqa: E402
from er_artifact_env import ARTIFACT_ENV  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = REPO_ROOT / "scripts"
LAUNCHER = Path.home() / "Elden" / "launch.sh"
PROFILE_DIR = Path.home() / "Elden"
AUTOLOAD_LOG_NAME = "er-quickload-autoload-debug.log"
# me3's own stdout/stderr for the run, kept beside the run state. See the comment at the Popen
# that writes it: this used to be DEVNULL, which cost a diagnosis.
LAUNCHER_LOG_NAME = "me3-launcher.log"

# Every per-run artifact goes into this run'S own directory, keyed by the run id this tool already
# mints. A game-directory artifact is single-slot, not a log that accumulates: `er_game_base::log::
# begin_fresh_run` renames `<name>` to `<name>.prev` and truncates on the DLL's first write, one
# generation only. Two launches and the run before last is gone -- and several sessions launch
# concurrently here, so that is the normal case, not a race. Measured 2026-08-31: an 11:09 launch
# destroyed the 5.4 MB `er-quickload-continue-trace.log` belonging to the 09:07 run, whose evidence
# nobody had read.
#
# Copying the files out at teardown does not fix that, which is why this is a redirect: by teardown
# this run has already clobbered the previous one's file, and a run that crashes or is killed never
# reaches a teardown step at all -- exactly the run whose evidence matters most.
#
# The table itself lives in `scripts/er_artifact_env.py`, shared with every other Python launcher,
# because the original bug was a table with one line missing. Add a new artifact there, once;
# `scripts/er-artifact-redirect-audit.py` fails when a knob the DLLs honour is missing from it, and
# this tool's own selftest asserts `ARTIFACT_ENV` covers every knob the audit finds in the Rust.

# `AUTOLOAD_LOG_NAME` above belongs to this DLL and no other. The sidecar-testimony contract is only
# available when it is loaded, because it is the only shell that reads the sidecar at all.
PRODUCT_DLL_NAME = "er_quickload.dll"

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_NO_TESTIMONY = 4

# Each wait is one bounded slice, re-armed until a wall-clock deadline, so no single call
# approaches the 30s shell ceiling.
#
# The budget is wall-clock and not a count of slices. A slice ends on any inotify event in the
# game directory, and during boot that directory sees dozens of writes a second (every co-loaded
# DLL has its own log). Counting slices therefore burned a nominal 24-second budget in
# milliseconds and reported a perfectly healthy run as "silent" -- measured on a live launch
# where the DLL logged its config 9 seconds in, well inside the window that was supposed to be
# open.
TESTIMONY_SLICE_SECONDS = 4.0
# 90s, not 25s -- and the reason is not what an earlier version of this comment claimed.
#
# That version said "measured: a cold Proton start took 62 seconds from launch.sh to the first DLL
# log line". No such measurement was ever taken. What existed were two runs that printed
# `ELDEN RING DID NOT START` after waiting 25s while the game went on to boot normally, and a
# 25-second timeout bounds the start from below -- it says ">25s" and nothing whatsoever about 62.
# A number invented to justify a change was written down as evidence, which is worse than leaving
# the constant unexplained.
#
# Measured 2026-09-04, properly, over 11 runs: launch -> DLL attach is 3.3-4.1s, median 3.7s (the
# run id is the launch time and the crash-logging breadcrumb's mtime is the attach). So the boot is
# an order of magnitude faster than the retracted figure, and 25s was not too short for attach.
# What the two condemned runs actually hit was the witness looking in the wrong place: they loaded
# only shells that write `.txt`, and the glob was `*.log` (see `testimony_candidates`). The budget
# stays generous anyway because a slow first-run shader compile is real and a false
# `DID NOT START` is expensive, but it is a margin, not a measurement of the typical case.
# This budget is the wait for the first sign of life, not a runtime cap -- the run's own idle/stall
# backstop is `.auto/runtime_timeout_cap_seconds` (300s) and is untouched by this. It is also not a
# subprocess timeout, so it is outside `scripts/check-no-timeouts.py`'s 30s ceiling; `SUBPROCESS_TIMEOUT`
# below is the one that ceiling governs and it stays where it is. A launch waited on for this long
# must be run in the background by the caller, since an agent shell is capped at 30s.
TESTIMONY_BUDGET_SECONDS = 90.0
SUBPROCESS_TIMEOUT = 28


def game_dir() -> Path:
    return er_run_lib.game_dir()


def target_dir() -> Path:
    return REPO_ROOT / "target/x86_64-pc-windows-msvc/release"


def run_script(script: str, *args: str) -> tuple[int, str, str]:
    proc = subprocess.run(
        [sys.executable, str(SCRIPTS / script), *args],
        text=True,
        capture_output=True,
        timeout=SUBPROCESS_TIMEOUT,
        cwd=REPO_ROOT,
    )
    return proc.returncode, proc.stdout, proc.stderr


def steam_running() -> bool:
    """Ask the sanctioned helper. A bare `pgrep -x steam` false-negatives here and is guarded."""
    helper = SCRIPTS / "steam-running.sh"
    if not helper.is_file():
        return True
    proc = subprocess.run(["bash", str(helper)], capture_output=True, timeout=20)
    return proc.returncode == 0


def normalize_path(value: str) -> str:
    """Compare a Wine-reported path with a Linux one: `Z:\\home\\x` and `/home/x` are the same file."""
    text = value.strip().replace("\\", "/").lower()
    if len(text) > 1 and text[1] == ":":
        text = text[2:]
    return text


def parse_loaded_line(line: str) -> dict[str, str]:
    """Pull `key=value` pairs out of a `runtime-config: loaded ...` line."""
    fields: dict[str, str] = {}
    for token in ("sidecar", "save_file", "slot"):
        marker = f"{token}="
        start = line.find(marker)
        if start < 0:
            continue
        rest = line[start + len(marker) :]
        # Values are space-separated; a path with spaces would break this, and the tokens we
        # need (sidecar, slot) are tool-generated and space-free by construction.
        fields[token] = rest.split(" ")[0]
    return fields


class LogTail:
    """Read only what a run appended, surviving the DLL's startup log rotation.

    The DLL renames `<log>` to `<log>.prev` at startup, so an offset alone would either miss
    the new file or replay the old one. Remembering the inode as well makes "everything since
    launch" exact in both cases.
    """

    def __init__(self, path: Path) -> None:
        self.path = path
        try:
            stat = path.stat()
            self.inode, self.offset = stat.st_ino, stat.st_size
        except OSError:
            self.inode, self.offset = None, 0

    def new_text(self) -> str:
        try:
            stat = self.path.stat()
        except OSError:
            return ""
        # Two distinct rotations to survive, and missing the second one cost a live run:
        #  * replaced -- new inode, so read the whole new file;
        #  * truncated in place -- same inode, size drops below our offset. Seeking to the old
        #    offset then lands past EOF and reads nothing, so the DLL's startup lines are
        #    invisible and the run is reported "silent" while it is in fact running perfectly.
        rotated = stat.st_ino != self.inode or stat.st_size < self.offset
        start = 0 if rotated else self.offset
        try:
            with self.path.open("rb") as handle:
                handle.seek(min(start, stat.st_size))
                data = handle.read()
        except OSError:
            return ""
        # A line is evidence only once it is terminated, and this cost a live run. The DLL's
        # `runtime-config: loaded` line is ~540 bytes and is not written atomically, so a read
        # landing mid-write returns a prefix -- "runtime-config: loaded '<game toml>'" with the
        # `sidecar=` field still unwritten. The caller cannot tell that from a DLL that genuinely
        # named no sidecar, so it declared a perfectly good run "the DLL ignored this run's
        # overlay" and told the reader not to cite it. Hand back only whole lines; the fragment
        # arrives complete on the next poll, milliseconds later.
        end = data.rfind(b"\n")
        if end < 0:
            return ""
        return data[: end + 1].decode("utf-8", errors="replace")


WatchSet = er_run_lib.WatchSet


def await_testimony(tails: list[LogTail], sidecar: Path, launcher_pid: int) -> dict:
    """Block until the DLL states it loaded this run's sidecar. Event-driven, never a sleep.

    Returns a verdict dict with `status`:
      confirmed      -- the DLL named our sidecar; this run is what it says it is.
      wrong-sidecar  -- the DLL loaded and logged, but named a different sidecar (or none).
                        Completely different from silence: the game is running our DLL, it just
                        ignored the overlay, so the character on screen is not the one picked.
                        Conflating the two sends you hunting a launch failure that did not happen.
      silent         -- no `runtime-config: loaded` line at all within the window.

    Several tails, on purpose. This run redirects the autoload debug log into its own artifact
    directory (see ARTIFACT_ENV), so that is where the testimony should land. But the redirect rides
    an environment variable through `launch.sh` -> me3 -> the Proton compat tool -> the game, and if
    any link drops it the DLL falls back to the game directory and logs there instead. Watching only
    the redirected path would then report a perfectly healthy run as silent -- the exact false
    negative this gate has produced twice before, for different reasons. So watch both, and let the
    verdict say which one spoke.
    """
    wanted = normalize_path(str(sidecar))
    seen: dict | None = None
    deadline = time.monotonic() + TESTIMONY_BUDGET_SECONDS
    with WatchSet([tail.path.parent for tail in tails]) as watch:
        while True:
            for tail in tails:
                for line in tail.new_text().splitlines():
                    if "runtime-config: loaded" not in line:
                        continue
                    fields = parse_loaded_line(line)
                    reported = normalize_path(fields.get("sidecar", ""))
                    if reported and reported == wanted:
                        return {
                            "status": "confirmed",
                            "line": line.strip(),
                            "log": str(tail.path),
                            **fields,
                        }
                    seen = {
                        "status": "wrong-sidecar",
                        "line": line.strip(),
                        "log": str(tail.path),
                        **fields,
                    }
            if seen:
                # The DLL has spoken and it did not name our file. Waiting longer cannot change
                # that -- it reads its config once, at DllMain.
                return seen
            if not er_run_lib.process_alive(launcher_pid):
                return {"status": "silent", "launcher_exited": True}
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return {"status": "silent", "launcher_exited": False}
            slice_seconds = min(TESTIMONY_SLICE_SECONDS, remaining)
            if not watch.available:
                # No inotify: fall back to the launcher-exit wait, which still blocks on an
                # event rather than spinning, and re-read afterwards.
                er_run_lib.wait_for_exit(launcher_pid, slice_seconds)
            else:
                watch.wait(slice_seconds)


def testimony_candidates(game_dir_path: Path):
    """Every file a shell might write to prove it is alive -- `.log` and `.txt`.

    `.log` alone was a false negative with teeth: `er_crash_logging.dll` writes only `.txt`
    (`er-crash-log.txt`, `er-crash-latest.txt`, `er-crash-modules.txt`), so a DLL set whose only
    logging shell was the crash logger could run for minutes, catch a fatal exception, write a full
    record -- and still be reported as `ELDEN RING DID NOT START`. The suffix a shell picks is not a
    contract anywhere; it is whatever its author typed.
    """
    for suffix in ("*.log", "*.txt"):
        yield from game_dir_path.glob(suffix)


def await_any_dll_log(watch_dirs, launcher_pid: int, started_at: float) -> dict:
    """Weaker witness, for a run whose DLL set does not include the product shell.

    Why this exists. The sidecar line proves three things at once -- process up, our DLL in it,
    our config read -- but only `er_quickload.dll` writes it, because it is the only shell that
    reads the sidecar. Once the launcher/watchdog/guard work merged to main, the closure started
    selecting DLL sets that legitimately exclude that shell, and the gate went on waiting for a
    witness the run never loaded. It then condemned a perfectly healthy game: run
    br-20260817-184836-d6a7 printed `ELDEN RING DID NOT START` while `eldenring.exe` was up and
    the invasion DLL was heartbeating into its own log.

    So when the strong witness is unavailable, ask a weaker question honestly rather than a
    strong one wrongly: has any log next to the executable gained bytes since we launched? That
    proves the process is up and one of our shells is running in it. It does not prove which
    sidecar was read, and the caller must not claim that it does.

    Matching is by mtime rather than by a DLL-name -> log-name table on purpose: those names do
    not follow a convention (`er_net_effects.dll` writes `er-net-effects.log`,
    `er_invasion_warp.dll` writes `er-invasion-warp.log`), so a table would be a second
    source of truth that silently rots every time a shell is added.

    Several directories, not one, and the reason is this tool's own success. Every artifact knob
    it sets moves a DLL's log out of the game directory and into the run directory -- which is the
    point -- so watching only the game directory means the better this redirect gets, the blinder
    this witness becomes. It went fully blind on 2026-09-04, the day `er_crash_logging` gained its
    knobs: a run whose only shell was the crash logger wrote all four of its files into the run
    directory, the game directory never changed, and this reported `ELDEN RING DID NOT START` for
    a game that was up and busy. Watch both, and let whichever one speaks be the witness.
    """
    if isinstance(watch_dirs, Path):
        watch_dirs = [watch_dirs]
    watch_dirs = [directory for directory in watch_dirs if directory is not None]
    deadline = time.monotonic() + TESTIMONY_BUDGET_SECONDS
    with WatchSet(watch_dirs) as watch:
        while True:
            candidates = sorted(
                candidate
                for directory in watch_dirs
                for candidate in testimony_candidates(directory)
            )
            for log in candidates:
                try:
                    if log.stat().st_mtime > started_at:
                        return {"status": "confirmed-weak", "log": log.name,
                                "line": f"{log.name} written after launch"}
                except OSError:
                    continue
            if not er_run_lib.process_alive(launcher_pid):
                return {"status": "silent", "launcher_exited": True}
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return {"status": "silent", "launcher_exited": False}
            slice_seconds = min(TESTIMONY_SLICE_SECONDS, remaining)
            if not watch.available:
                er_run_lib.wait_for_exit(launcher_pid, slice_seconds)
            else:
                watch.wait(slice_seconds)


def running_block(context: dict) -> str:
    save = context.get("save")
    lines = [
        "```",
        "================ ELDEN RING IS RUNNING ================",
        f"  run           {context['run_id']}",
        f"  pid           {context['pid']} (me3 launcher)",
        f"  started       {context['started']}",
        "",
        f"  branch        {context['branch']} @ {context['head'][:12]}"
        + ("   WORKING TREE DIRTY" if context["dirty"] else ""),
        f"  base          {context['base_ref']} -> {context['merge_base'][:12]}",
        "",
        "  DLLs loaded (fresh, provenance-verified against this tree):",
    ]
    for name, sha in context["dlls"]:
        lines.append(f"    {name:34} {sha[:16]}")
    if context.get("ersc"):
        lines.append(f"    {'ersc.dll (game install)':34} referenced, not bundled")
    for entry in context.get("excluded", []):
        lines.append(f"    EXCLUDED {entry['artifact']:25} {entry['kind']} -- not tested in this run")
    # The mirror of the line above. A shell the diff never reached is in this process because
    # `scripts/me3-dll-conflicts.toml [always]` says it is on by default, and a reader deciding
    # what this run proved needs to know which half of the profile the branch actually selected.
    for package in context.get("added_by_default", []):
        lines.append(f"    DEFAULT-ON {package:23} not selected by this branch's changes")
    # A run that loads the input harness is one the player is not driving. The closure only lets
    # that through when --agent-driven declared it (kind `drives-input`), and the declaration is
    # worthless if the artifact does not carry it: AGENTS.md forbids claiming the user is in
    # control of a self-driving probe, and this block is what someone reads later to decide what
    # the run proved.
    for entry in context.get("accepted_conflicts", []):
        lines.append(
            f"    AGENT-DRIVEN {entry['package']:22} {entry['kind']} -- the PLAYER IS NOT IN CONTROL"
        )
    lines.append("")
    if save:
        lines += [
            f"  character     {save['name']}  RL{save['level']}"
            + (
                f"  weapon +{save['matchmaking_weapon_level']}"
                if save.get("matchmaking_weapon_level") is not None
                else "  weapon +?"
            )
            + f"  slot {save['slot']}",
            f"  save          {save['save_file']}",
            f"  container     .{save['container']}"
            + ("   SOURCE WRITABLE" if save.get("source_writable") else "   source read-only"),
            # An explicit --save has no seed, and "--seed None" is not a command anyone can run.
            # The rerun hint has to name whatever actually determined this character.
            (
                f"  seed          {save['seed']}   (rerun: --seed {save['seed']})"
                if save.get("seed") is not None
                else f"  chosen by     --save (rerun: --save '{save['save_file']}:{save['slot']}')"
            ),
        ]
    else:
        # UNREACHABLE by construction: every save path is decoded now (see `--save`). Kept as a
        # refusal rather than deleted, because the thing this block used to print --
        # "<active Steam user's default save>" -- was a PLACEHOLDER standing where the Autoload
        # Identity Launch Gate requires a real name and slot, and it read like a value.
        raise RuntimeError(
            "launch block has no decoded character -- refusing to print an identity placeholder"
        )
    lines += [
        "",
        f"  profile       {context['profile']}",
        f"  sidecar       {context['sidecar']}",
        f"  evidence      {context['evidence_class']}",
        "",
        # A block that overstates its own evidence is worse than no block, so the wording
        # tracks which witness was actually obtained rather than always claiming the strong one.
        (
            "  PROVEN BY     the DLL's own log line, not by the process existing:"
            if context.get("witness") != "weak"
            else "  PROVEN BY     one of this run's DLLs writing its log, not by the process\n"
            "                existing. This run does not load er_quickload.dll, which is the\n"
            "                only shell that reports a sidecar, so the sidecar was NOT verified:"
        ),
        f"    {context['testimony'][:96]}",
        "",
        (
            "  NOT claimed   window visible / world loaded / player able to move."
            if context.get("witness") != "weak"
            else "  NOT claimed   window visible / world loaded / player able to move / which\n"
            "                save the DLLs actually read."
        ),
        f"                re-check later: scripts/er-run-branch.py --status {context['run_id']}",
        f"  me3 log       {context.get('launcher_log', '(not captured)')}",
        # The artifact dir OUTLIVES the run on purpose. Cleanup removes the files this run staged
        # (profile, sidecar, run.json); it does not touch what the run wrote, and the directory
        # survives because it is not empty. That is what makes the run before last still readable.
        f"  artifacts     {context.get('artifact_dir', '(none)')}",
        f"                {len(ARTIFACT_ENV)} DLL log/telemetry paths redirected here, so the next",
        "                launch cannot overwrite them the way a game-directory log is overwritten.",
        "  cleanup       staged files only, when the game exits; the artifacts above are kept.",
        "=======================================================",
        "```",
    ]
    return "\n".join(lines)


def failed_block(run_id: str, reason: str, detail: list[str]) -> str:
    lines = [
        "```",
        "============ ELDEN RING DID NOT START ============",
        f"  run     {run_id}",
        f"  reason  {reason}",
    ]
    # No trailing "staged files were removed" line here: whether they were removed depends on
    # whether the process is still alive, and the caller states which. A block that always
    # claimed removal contradicted itself the first time the keep-alive path fired.
    lines.extend(f"  {line}" for line in detail)
    lines += ["==================================================", "```"]
    return "\n".join(lines)


def stale_launchers() -> list[tuple[int, str]]:
    """Live `me3 launch` processes, as `(pid, profile)`.

    A launcher OUTLIVES the game it started. Measured 2026-09-09: three launches in a row failed
    with `no DLL testimony`, each after the full 90-second budget, because a previous run's `me3`
    was still alive holding the Wine prefix -- and `er-teardown.py --status` said `no eldenring.exe`
    and `clean-exit`, which reads as nothing running. So the condition is invisible to the game
    check and has to be asked for separately.

    Reads `/proc` directly rather than shelling out to a process-name tool, which this workspace
    forbids for reasons recorded in the Steam-detection guard.
    """
    found: list[tuple[int, str]] = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            argv = (entry / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue  # exited between listing and reading; not our business
        args_text = [part.decode(errors="replace") for part in argv if part]
        if len(args_text) < 2 or Path(args_text[0]).name != "me3":
            continue
        if "launch" not in args_text:
            continue
        profile = "<unnamed>"
        for flag in ("-p", "--profile"):
            if flag in args_text:
                index = args_text.index(flag)
                if index + 1 < len(args_text):
                    profile = args_text[index + 1]
        found.append((int(entry.name), profile))
    return found


def game_pid() -> int | None:
    """The live `eldenring.exe`, or `None`.

    Asked so the stale-launcher refusal can describe what is actually there. The refusal used to
    assert that the game had already exited, because that is the case the check was written for --
    but a launcher is also alive during a perfectly healthy run, and then the sentence told the
    reader the opposite of the truth and invited them to tear down a session someone was playing.
    Observed 2026-09-09 against pid 1574375, up and burning cpu, while the message said it was gone.

    Same `/proc` walk as `stale_launchers`, and for the same reason: this workspace forbids
    process-name tools.
    """
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            comm = (entry / "comm").read_text().strip()
        except OSError:
            continue
        if comm == "eldenring.exe":
            return int(entry.name)
    return None


def stale_launcher_state() -> str:
    """What the live launcher currently has under it, said as observed rather than assumed."""
    pid = game_pid()
    if pid is None:
        return (
            "Its game has already exited, so `er-teardown.py --status` reports `no eldenring.exe` "
            "and looks clean."
        )
    return (
        f"A game is still running under it: eldenring.exe pid {pid}. This is a live session, not "
        "a leftover -- clearing it ends whatever is on screen right now. If that session is "
        "someone's, leave it alone; the launch you are attempting would have taken it down."
    )


def preflight(args) -> tuple[dict, dict | None]:
    """Closure + provenance + save pick. Raises RuntimeError with a loud message on any refusal."""
    # Before anything expensive: a launcher from an earlier run still holds the prefix, and the
    # only symptom downstream is ninety seconds of silence followed by the wrong diagnosis.
    if not getattr(args, "allow_stale_launcher", False):
        held = stale_launchers()
        if held:
            raise RuntimeError(
                "REFUSING TO LAUNCH -- a previous run's me3 launcher is still alive and holding "
                "the Wine prefix:\n"
                + "\n".join(f"  pid {pid}  profile {profile}" for pid, profile in held)
                + "\n\n"
                + stale_launcher_state()
                + "\nClear it first:  python3 scripts/er-teardown.py --reason stale-launcher"
            )

    if not args.skip_steam_check and not steam_running():
        raise RuntimeError(
            "Steam is not running. Start it (it needs an interactive login), or pass "
            "--skip-steam-check to accept a non-representative environment."
        )

    closure_args = ["--json"]
    if args.no_fetch:
        closure_args.append("--no-fetch")
    for package in args.pinned:
        closure_args += ["--with", package]
    for package in getattr(args, "dropped", []):
        closure_args += ["--without", package]
    if getattr(args, "agent_driven", False):
        closure_args.append("--agent-driven")
    code, out, err = run_script("er-dll-closure.py", *closure_args)
    if code == 2:
        raise RuntimeError(f"the changed DLLs cannot share one profile:\n{out or err}")
    if code != 0:
        raise RuntimeError(f"closure failed: {err.strip() or out.strip()}")
    closure = json.loads(out)

    stale: list[str] = []
    for package, artifact in zip(closure["packages"], closure["artifacts"]):
        dll = target_dir() / artifact
        code, out, err = run_script(
            "er-dll-provenance.py", "verify", "--package", package, "--artifact", str(dll)
        )
        if code != 0:
            stale.append((err or out).strip())
    if stale:
        raise RuntimeError(
            "REFUSING TO LAUNCH -- staged DLLs do not match this source tree:\n"
            + "\n".join(stale)
            + "\n\nRebuild them:  scripts/er-build-dlls.sh "
            + " ".join(closure["packages"])
        )

    save = None
    if args.save == "random":
        pick_args = ["--json", "--container", "sl2" if args.vanilla else "both"]
        if args.seed is not None:
            pick_args += ["--seed", str(args.seed)]
        code, out, err = run_script("er-pick-save.py", *pick_args)
        if code != 0:
            raise RuntimeError(f"no save could be picked: {err.strip() or out.strip()}")
        save = json.loads(out)
    else:
        save = decode_explicit_save(args.save)

    return closure, save


def decode_explicit_save(spec: str) -> dict:
    """Resolve `PATH[:SLOT]` into the same decoded shape a random pick produces.

    The decode is not optional. AGENTS.md's Autoload Identity Launch Gate requires the character
    and slot to be known from current save evidence before a launch that will autoload -- and
    naming a file proves neither. A path whose named slot holds no character is refused here
    rather than discovered on a loading screen.
    """
    path, _, slot_text = spec.rpartition(":")
    if not path:
        path, slot_text = spec, ""
    slot = None
    if slot_text:
        try:
            slot = int(slot_text)
        except ValueError as err:
            raise RuntimeError(f"bad slot in --save {spec!r}: {err}") from err

    source = Path(path).expanduser().resolve()
    if not source.is_file():
        raise RuntimeError(f"--save names a file that does not exist: {source}")

    code, out, err = run_script(
        "er-pick-save.py", "--json", "--all", "--root", str(source.parent)
    )
    if code != 0:
        raise RuntimeError(f"could not decode {source}: {err.strip() or out.strip()}")
    targets = [t for t in json.loads(out)["targets"] if Path(t["save_file"]) == source]
    if slot is not None:
        targets = [t for t in targets if t["slot"] == slot]
    if not targets:
        where = f"{source} slot {slot}" if slot is not None else str(source)
        raise RuntimeError(
            f"no occupied character at {where}. Nothing will autoload, so this launch is refused."
        )
    chosen = targets[0]
    return {**chosen, "seed": None, "draws": 0, "eligible_files": 1,
            "occupied_slots_in_file": len(targets), "corpus_root": str(source.parent)}


def launch(args) -> int:
    collected = er_run_lib.collect_dead_runs()
    for run_id, removed in collected:
        print(f"[gc] collected dead run {run_id}: removed {len(removed)} staged file(s)")

    closure, save = preflight(args)

    run_id = f"br-{datetime.now(timezone.utc):%Y%m%d-%H%M%S}-{random.randrange(16**4):04x}"
    profile_path = PROFILE_DIR / f"{run_id}.me3"

    gen_args = [
        "--closure", "-",
        "--run-id", run_id,
        "--profile", str(profile_path),
        "--target-dir", str(target_dir()),
        "--json",
    ]
    closure_file = er_run_lib.RUN_STATE_ROOT / run_id / "closure.json"
    closure_file.parent.mkdir(parents=True, exist_ok=True)
    closure_file.write_text(json.dumps(closure), encoding="utf-8")
    gen_args[1] = str(closure_file)
    if args.vanilla:
        gen_args.append("--vanilla")
    if args.disable_arxan:
        gen_args.append("--disable-arxan")
    save_file = closure_file.with_name("save.json")
    save_file.write_text(json.dumps(save), encoding="utf-8")
    gen_args += ["--save", str(save_file)]

    code, out, err = run_script("er-gen-me3-profile.py", *gen_args)
    if code != 0:
        raise RuntimeError(f"could not stage the profile: {err.strip() or out.strip()}")
    staged = json.loads(out)

    state = er_run_lib.RunState(
        run_id=run_id,
        pid=0,
        profile=staged["profile"],
        remove_paths=staged["remove_paths"] + [str(closure_file), str(closure_file.with_name("save.json"))],
        meta={"branch": args.branch, "evidence_class": staged["evidence_class"]},
    )
    state.save()

    if args.dry_run:
        print(json.dumps({**staged, "run_id": run_id, "save": save}, indent=2))
        print("\n--dry-run: staged only, nothing launched. Remove with:")
        print(f"  python3 scripts/er-run-reaper.py --run-id {run_id}")
        return EXIT_OK

    # This run's own artifact directory: the same per-run directory that already holds run.json,
    # closure.json and me3-launcher.log, now also the destination for every DLL artifact.
    artifact_dir = er_run_lib.RUN_STATE_ROOT / run_id
    artifact_dir.mkdir(parents=True, exist_ok=True)
    harness_drive = getattr(args, "harness_drive", None)
    if harness_drive:
        # Both markers are required and neither is sufficient. `er-harness-drive-mode.txt` picks the
        # phase table; `er-harness-force-drive.txt` is what stops `resolve_mode()` from returning
        # Passive because the product DLL is present. They resolve beside the DLL's log (me3 launches
        # the game with an arbitrary CWD, so a bare relative path silently answers false), which is
        # this directory.
        (artifact_dir / "er-harness-drive-mode.txt").write_text(harness_drive, encoding="utf-8")
        (artifact_dir / "er-harness-force-drive.txt").write_text("1", encoding="utf-8")
    artifact_env = {env: str(artifact_dir / name) for env, name in ARTIFACT_ENV.items()}
    # Named once, for the watchers. `scripts/er-user-session-watch.py` and the other observers used
    # to look in the game directory unconditionally; with the redirect in place that directory holds
    # nothing for this run, and a watcher that finds nothing reports a healthy session as silent.
    artifact_env["ER_RUN_ARTIFACT_DIR"] = str(artifact_dir)

    # The markers must be named, not just written. `redirected_artifact_path` falls back to the game
    # directory when its env var is unset, so markers dropped in this run's directory are invisible
    # unless the variable points at them -- measured on br-20260905-040013-1038, where both files were
    # present in the artifact directory and the harness still logged `drive: mode='passive' phases=0`.
    # These two are deliberately not in ARTIFACT_ENV: that set is the DLL's output redirect and its
    # selftest asserts an exact match against what the DLLs honour, while these are inputs.
    if harness_drive:
        artifact_env["ER_HARNESS_DRIVE_MODE_PATH"] = str(
            artifact_dir / "er-harness-drive-mode.txt"
        )
        artifact_env["ER_HARNESS_FORCE_DRIVE_PATH"] = str(
            artifact_dir / "er-harness-force-drive.txt"
        )
        # The live command file (crates/er-input-harness/src/repl.rs). Same reason as the two above,
        # and it bites harder here: the whole point of the command loop is to interrogate a session
        # that is already up, so a command written into this run's directory that the harness reads
        # from the game directory instead is a question that silently never gets asked.
        artifact_env["ER_HARNESS_CMD_PATH"] = str(artifact_dir / "er-harness-cmd.txt")

    # Both candidate homes for the DLL's testimony -- the redirect, and the game directory it falls
    # back to if the environment does not survive the launch chain. See `await_testimony`.
    log_paths = [artifact_dir / AUTOLOAD_LOG_NAME, game_dir() / AUTOLOAD_LOG_NAME]
    tails = [LogTail(path) for path in log_paths]

    if not LAUNCHER.is_file():
        raise RuntimeError(f"launcher not found: {LAUNCHER}")

    started = datetime.now(timezone.utc).isoformat(timespec="seconds")
    # Monotonic-free on purpose: compared against file mtimes, which are wall clock.
    launched_at = time.time()
    # Keep me3's output. It used to go to DEVNULL, and on 2026-08-29 that was the difference
    # between diagnosing a run and guessing at it: the game exited ~2.2s in with no coredump, no
    # OOM, no fatal record and no exit-hook stamp -- meaning nothing faulted and something asked
    # the process to stop. The only witness to that is me3's own stdout/stderr, and it was being
    # thrown away. Discarding the loader's output on a probe whose entire purpose is evidence is
    # a contradiction; this file is small, per-run, and reaped with the rest.
    launcher_log = er_run_lib.RUN_STATE_ROOT / run_id / LAUNCHER_LOG_NAME
    launcher_log.parent.mkdir(parents=True, exist_ok=True)
    launcher_out = launcher_log.open("wb")
    process = subprocess.Popen(
        # `-o`: offline/solo, no Seamless. launch.sh now includes ersc.dll by default
        # (2026-08-24); this probe predates that and wants the plain quicksave profile
        # with ER_QUICKLOAD_SAVE_MODE_HINT=vanilla, so it asks for it explicitly.
        ["bash", str(LAUNCHER), "-o"],
        env={**os.environ, "ME3_PROFILE": staged["profile"], **artifact_env},
        stdout=launcher_out,
        stderr=subprocess.STDOUT,
        stdin=subprocess.DEVNULL,
        start_new_session=True,  # survives this shell, and this agent turn
        cwd=str(REPO_ROOT),
    )
    launcher_out.close()  # the child holds its own duplicate of the descriptor
    state.pid = process.pid
    state.save()

    # Which witness this run can actually produce. Asking for sidecar testimony from a DLL set
    # that does not contain the shell which writes it is how a healthy run gets condemned.
    loads_product_dll = any(
        Path(dll).name == PRODUCT_DLL_NAME for dll in staged["dlls"]
    )
    if loads_product_dll:
        testimony = await_testimony(tails, Path(staged["sidecar"]), process.pid)
    else:
        testimony = await_any_dll_log(
            [artifact_dir, game_dir()], process.pid, launched_at
        )
    if testimony["status"] not in ("confirmed", "confirmed-weak"):
        alive = er_run_lib.process_alive(process.pid)
        if testimony["status"] == "wrong-sidecar":
            reason = "the DLL loaded but IGNORED this run's overlay"
            detail = [
                f"expected sidecar  {staged['sidecar']}",
                f"DLL reported      sidecar={testimony.get('sidecar', '<absent>')}",
                f"DLL loaded save   {testimony.get('save_file', '<unset>')} slot={testimony.get('slot', '<unset>')}",
                "",
                "The game IS running this DLL -- it just did not read the staged sidecar, so the",
                "character in front of you is NOT the one this run picked. Do not cite this run.",
            ]
        else:
            reason = "no DLL testimony"
            detail = [
                f"launcher pid {process.pid} "
                + ("is alive but silent" if alive else "exited before saying anything"),
                (
                    f"waited {TESTIMONY_BUDGET_SECONDS:.0f}s (wall clock) for a "
                    f"'runtime-config: loaded' line in"
                    if loads_product_dll
                    else f"waited {TESTIMONY_BUDGET_SECONDS:.0f}s (wall clock) for ANY DLL log to "
                    f"be written in"
                ),
            ] + (
                # Name both watched paths. The DLL writes to the redirect when the environment
                # survives the launch chain and to the game directory when it does not, and a
                # failure report that names one of them leaves the reader guessing which.
                [f"  {path}" for path in log_paths]
                if loads_product_dll
                else [f"  {game_dir()}"]
            )

        # Only tear down staged files if nothing is using them. A game still booting will read
        # the sidecar after this point, and deleting it mid-boot both breaks that run and
        # destroys the evidence needed to explain the failure. When the process is alive the
        # reaper takes ownership and cleans up on exit, exactly as for a confirmed run.
        if alive:
            detail += [
                "",
                f"Process {process.pid} is STILL RUNNING -- staged files kept so it is not pulled",
                f"out from under a booting game. Cleanup on exit; force now with:",
                f"  python3 scripts/er-run-reaper.py --run-id {run_id}",
            ]
            _spawn_reaper(run_id, args.monitor)
        else:
            state.cleanup()
            detail += ["", "Process is gone; staged files were removed."]

        print(failed_block(run_id, reason, detail))
        return EXIT_NO_TESTIMONY

    _spawn_reaper(run_id, args.monitor)

    print(
        running_block(
            {
                "run_id": run_id,
                "pid": process.pid,
                "started": started,
                "branch": args.branch,
                "head": closure["head"],
                "merge_base": closure["merge_base"],
                "base_ref": closure["base_ref"],
                "dirty": closure["dirty"],
                "dlls": [
                    (Path(dll).name, _sha(Path(dll))) for dll in staged["dlls"]
                ],
                "ersc": staged["ersc"],
                "excluded": closure.get("excluded", []),
                "added_by_default": closure.get("added_by_default", []),
                "accepted_conflicts": closure.get("accepted_conflicts", []),
                "save": save,
                "profile": staged["profile"],
                "sidecar": staged["sidecar"],
                "evidence_class": staged["evidence_class"],
                "testimony": testimony["line"],
                "launcher_log": str(launcher_log),
                "artifact_dir": str(artifact_dir),
                "witness": "weak" if testimony["status"] == "confirmed-weak" else "strong",
            }
        )
    )
    return EXIT_OK


def _sha(path: Path) -> str:
    import hashlib

    return hashlib.sha256(path.read_bytes()).hexdigest()


def _spawn_reaper(run_id: str, monitor: str | None) -> None:
    """Detach the reaper into its own session so it outlives this shell -- and this agent turn."""
    subprocess.Popen(
        [sys.executable, str(SCRIPTS / "er-run-reaper.py"), "--run-id", run_id]
        + (["--place-monitor", monitor] if monitor else []),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL,
        start_new_session=True,
    )


def status(run_id: str) -> int:
    state = er_run_lib.RunState.load(er_run_lib.RUN_STATE_ROOT / run_id / "run.json")
    if state is None:
        # `run.json` is removed by cleanup; the artifacts are not. Reporting "no such run" over a
        # directory full of a finished run's evidence would hide exactly what the redirect exists to
        # keep -- and a finished run is the normal case when someone comes back to read it.
        finished = er_run_lib.RUN_STATE_ROOT / run_id
        if finished.is_dir():
            print(
                json.dumps(
                    {
                        "run_id": run_id,
                        "state": "finished (staged files cleaned up; artifacts kept)",
                        "artifact_dir": str(finished),
                        "artifacts": [
                            {"name": path.name, "bytes": path.stat().st_size}
                            for path in sorted(finished.iterdir())
                            if path.is_file()
                        ],
                    },
                    indent=2,
                )
            )
            return EXIT_OK
        print(f"no such run: {run_id}")
        return EXIT_ERROR
    alive = er_run_lib.process_alive(state.pid)
    game = er_run_lib.find_game_pids()
    artifact_dir = er_run_lib.RUN_STATE_ROOT / run_id
    artifacts = sorted(
        (path.name, path.stat().st_size)
        for path in artifact_dir.iterdir()
        if path.is_file()
    ) if artifact_dir.is_dir() else []
    print(
        json.dumps(
            {
                "run_id": run_id,
                "launcher_pid": state.pid,
                "launcher_alive": alive,
                "game_pids": game,
                "profile": state.profile,
                "meta": state.meta,
                "artifact_dir": str(artifact_dir),
                # Names and sizes: "the file exists" and "the file has anything in it" are different
                # claims, and a zero-byte log is the signature of a redirect that reached the DLL
                # while the run died before writing.
                "artifacts": [{"name": name, "bytes": size} for name, size in artifacts],
            },
            indent=2,
        )
    )
    return EXIT_OK


def selftest() -> int:
    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
        print(("  ok   " if condition else "  FAIL ") + label)

    check(
        normalize_path("Z:\\home\\banon\\x\\er_quickload.toml")
        == normalize_path("/home/banon/x/er_quickload.toml"),
        "a Wine Z:\\ path and its Linux path compare equal",
    )
    check(
        normalize_path("C:\\Other\\thing.toml") != normalize_path("/home/x/thing.toml"),
        "different files still compare unequal after normalisation",
    )

    line = (
        "runtime-config: loaded 'Z:/game/er-quickload.toml' sidecar=Z:/t/er_quickload.toml "
        "save_file=Z:/c/ER0000.sl2 slot=3 method=<unset>"
    )
    fields = parse_loaded_line(line)
    check(fields.get("slot") == "3", "the slot is parsed out of the loaded line")
    check(
        normalize_path(fields.get("sidecar", "")) == normalize_path("/t/er_quickload.toml"),
        "the sidecar path is parsed and normalises to the staged path",
    )
    check(parse_loaded_line("runtime-config: loaded 'x'") == {}, "a line with no fields yields none")

    import tempfile

    with tempfile.TemporaryDirectory() as raw:
        log = Path(raw) / "a.log"
        log.write_text("old line\n", encoding="utf-8")
        tail = LogTail(log)
        check(tail.new_text() == "", "a tail started now sees none of the pre-existing log")
        with log.open("a", encoding="utf-8") as handle:
            handle.write("new line\n")
        check(tail.new_text().strip() == "new line", "the tail sees only what was appended after")

        # The DLL rotates the log at startup; the tail must read the replacement from byte 0.
        log.rename(Path(raw) / "a.log.prev")
        log.write_text("fresh run line\n", encoding="utf-8")
        check(
            "fresh run line" in tail.new_text(),
            "after a log rotation the tail reads the new file from the start",
        )

        # The rotation that actually bit: TRUNCATION in place. Same inode, size back to ~0, so
        # the recorded offset now sits past EOF. Seeking there reads nothing and the run gets
        # reported "silent" while the DLL is logging normally.
        big = Path(raw) / "trunc.log"
        big.write_text("x" * 5000 + "\n", encoding="utf-8")
        trunc_tail = LogTail(big)
        check(trunc_tail.new_text() == "", "a tail on a large log starts with nothing new")
        with big.open("w", encoding="utf-8") as handle:  # truncate, same inode
            handle.write("line after in-place truncation\n")
        check(
            "line after in-place truncation" in trunc_tail.new_text(),
            "an IN-PLACE truncation is detected and re-read from byte 0, not silently skipped",
        )

        # A read landing mid-write must yield nothing rather than a prefix that parses as a
        # complete record with its later fields missing.
        partial = Path(raw) / "partial.log"
        partial.write_text("", encoding="utf-8")
        partial_tail = LogTail(partial)
        with partial.open("a", encoding="utf-8") as handle:
            handle.write("runtime-config: loaded 'S:/g/er-quickload.toml' side")
        check(
            partial_tail.new_text() == "",
            "a half-written line is withheld until its newline arrives",
        )
        with partial.open("a", encoding="utf-8") as handle:
            handle.write("car=Z:/t/er_quickload.toml slot=4\n")
        check(
            "sidecar=Z:/t/er_quickload.toml" in partial_tail.new_text(),
            "the completed line is delivered whole on the next read",
        )

    block = running_block(
        {
            "run_id": "r1",
            "pid": 42,
            "started": "now",
            "branch": "feat/x",
            "head": "a" * 40,
            "merge_base": "b" * 40,
            "base_ref": "origin/main",
            "dirty": False,
            "dlls": [("er_quickload.dll", "c" * 64)],
            "ersc": "/game/ersc.dll",
            "excluded": [{"artifact": "er_loading_bar.dll", "kind": "present-compositor"}],
            "save": {
                "name": "Bonky Bean",
                "level": 139,
                "slot": 0,
                "save_file": "/c/ER0000.sl2",
                "container": "sl2",
                "source_writable": False,
                "seed": 7,
            },
            "profile": "/p.me3",
            "sidecar": "/t/er_quickload.toml",
            "evidence_class": "explicit-save-source",
            "testimony": "runtime-config: loaded ...",
        }
    )
    check("ELDEN RING IS RUNNING" in block, "the block announces the run")
    check("Bonky Bean" in block and "RL139" in block, "the block names the decoded character")
    check("--seed 7" in block, "the block carries the seed to reproduce the pick")
    check("EXCLUDED er_loading_bar.dll" in block, "the block names excluded DLLs")
    check(
        "NOT claimed" in block and "world loaded" in block,
        "the block states what it does NOT claim, so it cannot be over-read",
    )
    check(block.startswith("```") and block.rstrip().endswith("```"), "the block is copy-pasteable")

    failure = failed_block("r2", "no DLL testimony", ["pid 9 exited"])
    check("DID NOT START" in failure, "the failure block cannot be mistaken for success")

    # The stale-launcher reader, against this process rather than a fixture: it must find the
    # live `me3 launch` processes and nothing else, and it must survive a pid that exits
    # mid-scan. Asserting the shape rather than a count, because whether a launcher happens to
    # be up while the selftest runs is not this function's business.
    for pid, profile in stale_launchers():
        check(isinstance(pid, int) and pid > 0, f"a launcher pid must be a pid: {pid!r}")
        check(isinstance(profile, str) and profile, f"a profile must be named: {profile!r}")
        check(er_run_lib.process_alive(pid), f"a reported launcher must be alive: {pid}")
    check(
        "me3" not in {Path(sys.argv[0]).name},
        "the reader must not be able to match the selftest's own process",
    )
    check("ELDEN RING IS RUNNING" not in failure, "the failure block never contains the running banner")

    # The distinction the first live run exposed: a DLL that loaded and ignored the overlay is
    # not the same failure as a DLL that never spoke, and treating them alike sends you hunting
    # a launch failure that did not happen.
    with tempfile.TemporaryDirectory() as raw:
        log = Path(raw) / "auto.log"
        log.write_text("", encoding="utf-8")
        tail = LogTail(log)
        log.write_text(
            "runtime-config: loaded 'S:/g/er-quickload.toml' sidecar=<none> "
            "save_file=Z:/other/ER0000.sl2 slot=0\n",
            encoding="utf-8",
        )
        verdict = await_testimony([tail], Path("/t/er_quickload.toml"), os.getpid())
        check(
            verdict["status"] == "wrong-sidecar",
            f"a loaded-but-different-sidecar line is 'wrong-sidecar', not silence (got {verdict['status']})",
        )
        check(
            verdict.get("save_file") == "Z:/other/ER0000.sl2",
            "the wrong-sidecar verdict carries the save the DLL ACTUALLY loaded",
        )

        log2 = Path(raw) / "match.log"
        log2.write_text("", encoding="utf-8")
        tail2 = LogTail(log2)
        log2.write_text(
            "runtime-config: loaded 'S:/g/er-quickload.toml' sidecar=Z:/t/er_quickload.toml slot=3\n",
            encoding="utf-8",
        )
        verdict2 = await_testimony([tail2], Path("/t/er_quickload.toml"), os.getpid())
        check(verdict2["status"] == "confirmed", "a matching sidecar line confirms the run")

        # The redirect'S own failure mode. The artifacts now go to this run's directory via
        # ER_QUICKLOAD_*_PATH, which has to survive launch.sh -> me3 -> the compat tool -> the game.
        # If any link drops the environment the DLL falls back to the game directory, and a gate
        # watching only the redirected path would call a healthy run silent. Both are watched, and
        # the verdict must name whichever one actually spoke.
        redirected = Path(raw) / "run-dir"
        redirected.mkdir()
        fallback_dir = Path(raw) / "game-dir"
        fallback_dir.mkdir()
        for which, home in (("redirect", redirected), ("fallback", fallback_dir)):
            live = home / AUTOLOAD_LOG_NAME
            live.write_text("", encoding="utf-8")
            other = (fallback_dir if home is redirected else redirected) / AUTOLOAD_LOG_NAME
            other.write_text("", encoding="utf-8")
            both = [LogTail(redirected / AUTOLOAD_LOG_NAME), LogTail(fallback_dir / AUTOLOAD_LOG_NAME)]
            live.write_text(
                "runtime-config: loaded 'S:/g/er-quickload.toml' "
                "sidecar=Z:/t/er_quickload.toml slot=3\n",
                encoding="utf-8",
            )
            verdict_either = await_testimony(both, Path("/t/er_quickload.toml"), os.getpid())
            check(
                verdict_either["status"] == "confirmed"
                and verdict_either.get("log") == str(live),
                f"testimony written to the {which} path confirms, and the verdict names that path",
            )

        # The false negative that condemned a running game, br-20260817-184836-d6a7. Once the
        # launcher/watchdog/guard commits merged to main, the closure legitimately stopped
        # selecting er_quickload.dll -- the only shell that writes a `runtime-config: loaded`
        # line. The gate kept waiting for it and printed ELDEN RING did not start while
        # eldenring.exe was up and the invasion DLL was heartbeating into its own log.
        weak_dir = Path(raw) / "weakwitness"
        weak_dir.mkdir()
        launched = time.time()
        # A log that predates the launch must not count: it is last run's evidence.
        stale = weak_dir / "er-invasion-warp.log"
        stale.write_text("from a previous run\n", encoding="utf-8")
        os.utime(stale, (launched - 600, launched - 600))
        fresh_written = threading.Event()

        def write_fresh() -> None:
            (weak_dir / "er-net-effects.log").write_text("hello\n", encoding="utf-8")
            fresh_written.set()

        writer_weak = threading.Thread(target=write_fresh, daemon=True)
        writer_weak.start()
        try:
            weak = await_any_dll_log(weak_dir, os.getpid(), launched)
        finally:
            writer_weak.join(timeout=2)
        check(
            weak["status"] == "confirmed-weak",
            f"a run without er_quickload.dll confirms from any DLL's log (got {weak['status']})",
        )
        check(
            weak.get("log") == "er-net-effects.log",
            f"the FRESH log is the witness, not the stale one (got {weak.get('log')})",
        )

        # And the block must not borrow the strong claim when only the weak witness was had.
        weak_block = running_block(
            {
                "run_id": "r-weak", "pid": 1, "started": "now", "branch": "b",
                "head": "a" * 40, "merge_base": "b" * 40, "base_ref": "origin/main",
                "dirty": False, "dlls": [("er_invasion_warp.dll", "c" * 64)],
                "ersc": None, "excluded": [],
                # A decoded save is now mandatory in every block (`--save default`, the one mode
                # that had none, was removed 2026-09-04). This case is about the witness being
                # weak, not the identity being unknown, so it carries a real decoded character.
                "save": {
                    "name": "Selftest", "level": 1, "slot": 0, "container": "sl2",
                    "save_file": "/corpus/ER0000.sl2", "seed": 1, "source_writable": False,
                },
                "profile": "/p.me3",
                "sidecar": "/s.toml", "evidence_class": "x",
                "testimony": "er-net-effects.log written after launch", "witness": "weak",
            }
        )
        check(
            "sidecar was NOT verified" in weak_block,
            "the weak-witness block says outright that the sidecar was not verified",
        )

        # The false negative this costs a run over, observed live on br-20260816-183410-949e:
        # the real loaded line is ~540 bytes and does not land in one write. Reading the first
        # write yields `runtime-config: loaded '<game toml>'` with no `sidecar=` yet, which is
        # indistinguishable from a DLL that named no sidecar -- so the launcher condemned a run
        # that was loading exactly the character it had picked. `wrong-sidecar` is terminal by
        # design (the DLL reads its config once), which is precisely why the input must be a
        # whole line before it is judged.
        split_log = Path(raw) / "split.log"
        split_log.write_text("", encoding="utf-8")
        observed_partial = threading.Event()

        class SignallingTail(LogTail):
            """Fires the instant the partial line has been read, so the completing write is
            ordered by an observed event rather than by a sleep the timing gate would reject."""

            def new_text(self) -> str:
                text = super().new_text()
                observed_partial.set()
                return text

        split_tail = SignallingTail(split_log)
        with split_log.open("a", encoding="utf-8") as handle:
            handle.write("runtime-config: loaded 'S:/g/er-quickload.toml' side")

        def finish_line() -> None:
            observed_partial.wait(timeout=5)
            with split_log.open("a", encoding="utf-8") as handle:
                handle.write("car=Z:/t/er_quickload.toml save_file=Z:/c/ER0000.sl2 slot=4\n")

        writer = threading.Thread(target=finish_line, daemon=True)
        writer.start()
        try:
            verdict_split = await_testimony(
                [split_tail], Path("/t/er_quickload.toml"), os.getpid()
            )
        finally:
            writer.join(timeout=2)
        check(
            verdict_split["status"] == "confirmed",
            f"a loaded line arriving in two writes confirms instead of condemning the run "
            f"(got {verdict_split['status']})",
        )

    # The regression that cost a live run. The game directory is written to constantly during
    # boot, so every inotify slice returns instantly. When the budget was a count of slices it
    # was consumed in milliseconds and a healthy run was declared silent. The budget must be
    # wall-clock: a directory churning with events must not shorten it.
    with tempfile.TemporaryDirectory() as raw:
        busy_dir = Path(raw)
        quiet_log = busy_dir / "never-written.log"
        quiet_log.write_text("", encoding="utf-8")
        tail3 = LogTail(quiet_log)

        stop = threading.Event()

        def churn() -> None:
            counter = 0
            while not stop.is_set():
                (busy_dir / f"noise-{counter % 8}.tmp").write_text(str(counter), encoding="utf-8")
                counter += 1
                stop.wait(0.01)

        global TESTIMONY_BUDGET_SECONDS
        previous_budget, TESTIMONY_BUDGET_SECONDS = TESTIMONY_BUDGET_SECONDS, 2.0
        worker = threading.Thread(target=churn, daemon=True)
        worker.start()
        try:
            started_at = time.monotonic()
            verdict3 = await_testimony(
                [tail3], Path("/t/er_quickload.toml"), os.getpid()
            )
            elapsed = time.monotonic() - started_at
        finally:
            stop.set()
            worker.join(timeout=2)
            TESTIMONY_BUDGET_SECONDS = previous_budget

        check(verdict3["status"] == "silent", "a genuinely silent log is reported silent")
        check(
            elapsed >= 1.8,
            f"a directory churning with events still consumes the FULL wall-clock budget "
            f"(waited {elapsed:.2f}s of 2.0s)",
        )

    # The artifact redirect, checked against the audit's view of what the DLLs actually honour.
    # Two lists that must agree, so neither can rot alone: the audit reads the knobs out of the Rust
    # sources, and this is the launcher that has to set them. A knob added to the DLLs and not here
    # means that artifact silently goes back to the single-slot game directory.
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "er_artifact_redirect_audit", SCRIPTS / "er-artifact-redirect-audit.py"
    )
    audit_module = importlib.util.module_from_spec(spec)
    # Registered before execution: `@dataclass` resolves its own module out of `sys.modules` while
    # the class body runs, and an unregistered module makes that lookup return None.
    sys.modules[spec.name] = audit_module
    spec.loader.exec_module(audit_module)
    honoured = {knob.env for knob in audit_module.discover_knobs()}
    check(
        len(honoured) >= 6,
        f"the audit found the DLLs' redirect knobs to compare against ({len(honoured)})",
    )
    check(
        honoured == set(ARTIFACT_ENV),
        "ARTIFACT_ENV redirects EVERY knob the DLLs honour "
        f"(missing here: {sorted(honoured - set(ARTIFACT_ENV))}; "
        f"here but not honoured: {sorted(set(ARTIFACT_ENV) - honoured)})",
    )
    check(
        len(set(ARTIFACT_ENV.values())) == len(ARTIFACT_ENV),
        "no two knobs are pointed at the same filename, which would have them overwrite each other",
    )

    # Two consecutive runs, end to end, with no game: the real ARTIFACT_ENV, the real per-run
    # directory, the real `RunState.cleanup`, and a stand-in writer that reproduces the DLL's
    # rotation (`er_game_base::log::begin_fresh_run`: drop `<name>.prev`, rename, truncate).
    # The claim under test is the only one that matters -- after run 2, run 1 is still readable.
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        game_dir_stub = root / "Game"
        game_dir_stub.mkdir()
        previous_root, er_run_lib.RUN_STATE_ROOT = er_run_lib.RUN_STATE_ROOT, root

        def fake_dll_write(env: dict[str, str], marker: str) -> None:
            """One process's worth of writing, through the same rotation the DLL performs."""
            for path_text in env.values():
                path = Path(path_text)
                path.parent.mkdir(parents=True, exist_ok=True)
                previous = path.with_name(path.name + ".prev")
                previous.unlink(missing_ok=True)
                if path.exists():
                    path.rename(previous)
                path.write_text(f"{marker}\n", encoding="utf-8")

        try:
            run_dirs = []
            for marker in ("RUN-1 EVIDENCE", "RUN-2 EVIDENCE"):
                run_id = f"br-selftest-{marker[4]}"
                run_dir = er_run_lib.RUN_STATE_ROOT / run_id
                run_dir.mkdir(parents=True, exist_ok=True)
                fake_dll_write(
                    {env: str(run_dir / name) for env, name in ARTIFACT_ENV.items()}, marker
                )
                staged = root / f"{run_id}.me3"
                staged.write_text("profileVersion = \"v1\"\n", encoding="utf-8")
                state = er_run_lib.RunState(
                    run_id=run_id, pid=999_999_997, profile=str(staged),
                    remove_paths=[str(staged)],
                )
                state.save()
                state.cleanup()  # the reaper's job, run immediately
                run_dirs.append(run_dir)

            first = (run_dirs[0] / AUTOLOAD_LOG_NAME).read_text(encoding="utf-8").strip()
            second = (run_dirs[1] / AUTOLOAD_LOG_NAME).read_text(encoding="utf-8").strip()
            check(
                first == "RUN-1 EVIDENCE" and second == "RUN-2 EVIDENCE",
                f"after two runs BOTH are readable in their own directories (got {first!r}, "
                f"{second!r})",
            )
            check(
                all(
                    (run_dirs[0] / name).is_file() for name in ARTIFACT_ENV.values()
                ),
                f"all {len(ARTIFACT_ENV)} of run 1's artifacts survived run 2 and both cleanups",
            )
            check(
                not (run_dirs[0] / f"{AUTOLOAD_LOG_NAME}.prev").exists(),
                "run 1's file was never rotated, because run 2 never wrote to its path",
            )

            # The same two runs without the redirect -- the bug, reproduced, so the check above is
            # measuring the fix rather than an accident of the fixture.
            shared = {env: str(game_dir_stub / name) for env, name in ARTIFACT_ENV.items()}
            fake_dll_write(shared, "RUN-1 EVIDENCE")
            fake_dll_write(shared, "RUN-2 EVIDENCE")
            fake_dll_write(shared, "RUN-3 EVIDENCE")
            surviving = (game_dir_stub / f"{AUTOLOAD_LOG_NAME}.prev").read_text(encoding="utf-8")
            check(
                "RUN-1 EVIDENCE" not in surviving,
                "unredirected, run 1 is GONE two launches later -- the failure being fixed",
            )
        finally:
            er_run_lib.RUN_STATE_ROOT = previous_root

    redirect_block = running_block(
        {
            "run_id": "r3", "pid": 1, "started": "now", "branch": "b", "head": "a" * 40,
            "merge_base": "b" * 40, "base_ref": "origin/main", "dirty": False,
            "dlls": [("er_quickload.dll", "c" * 64)], "ersc": None, "excluded": [],
            # Every block carries a decoded character now; `--save default`, the one mode that
            # produced a block without one, was removed 2026-09-04.
            "save": {
                "name": "Selftest", "level": 1, "slot": 0, "container": "sl2",
                "save_file": "/corpus/ER0000.sl2", "seed": 1, "source_writable": False,
            },
            "profile": "/p.me3", "sidecar": "/s.toml", "evidence_class": "x",
            "testimony": "runtime-config: loaded ...",
            "artifact_dir": "/cache/er-me3-runs/r3",
        }
    )
    check(
        "/cache/er-me3-runs/r3" in redirect_block,
        "the block tells the reader where this run's artifacts are",
    )
    check(
        "artifacts above are kept" in redirect_block,
        "the block states that cleanup does NOT remove the artifacts, so they can be relied on",
    )

    check(LAUNCHER.is_file(), f"the user launcher this delegates to exists ({LAUNCHER})")
    check((SCRIPTS / "er-run-reaper.py").is_file(), "the reaper it detaches exists")

    print("selftest:", "PASS" if ok else "FAIL")
    return EXIT_OK if ok else EXIT_ERROR


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--save",
        default="random",
        metavar="random|PATH[:SLOT]",
        help="random (default) or an explicit save path with an optional :SLOT. Either way the "
        "save is DECODED and its character name, level and slot are reported before launch -- "
        "naming a file is not the same as knowing which character is in it. `default` was REMOVED "
        "on 2026-09-04 (user directive): it was the one mode that launched without decoding "
        "anything, so the block printed a placeholder where AGENTS.md's Autoload Identity Launch "
        "Gate requires a real identity, and nobody knew which character was loading until it "
        "appeared on screen. Use `random` -- WHICH slot does not matter, but KNOWING it does.",
    )
    parser.add_argument("--seed", type=int, help="reproduce an exact save pick")
    parser.add_argument("--vanilla", action="store_true", help="omit ersc.dll; draw .sl2 saves only")
    parser.add_argument("--monitor", help="Hyprland monitor to move the ER window to when it appears")
    parser.add_argument(
        "--harness-drive",
        metavar="MODE",
        choices=(
            "boot",
            "reload",
            "reload2",
            "full",
            "menureload",
            "menuchain",
            "probe",
            "equip",
            "inv",
        ),
        help=(
            "arm er-input-harness to drive the MENU with real key events instead of standing down. "
            "The harness resolves Passive whenever the product DLL is loaded, which is every run here, "
            "so its whole menu drive -- OpenPauseMenu, NavToOptionSetting, TabToQuit -- has been dead "
            "code in every launch (`drive: mode='passive' phases=0`). This writes the two markers that "
            "override that, into the run's artifact directory where the DLL resolves them beside its "
            "log. Use it whenever a run has to reproduce something through the menu: a switch armed by "
            "a load armed without the menu proves nothing about a menu bug -- and since 2026-09-05 "
            "there is no such arm left, so loads 2..N happen HERE or not at all. `menureload` "
            "drives one; `menuchain` drives three, which is what a load-3 defect needs."
        ),
    )
    parser.add_argument(
        "--agent-driven",
        action="store_true",
        help="declare that the AGENT drives this run's input, which lets a --with-pinned "
        "er-input-harness through the drives-input conflict. The run block then says outright "
        "that the player was not in control, so no such run can be mistaken for a user-driven "
        "one. Required by AGENTS.md's 2026-07-22 order that the agent drive every input; without "
        "it the closure refuses the harness, which is the correct default.",
    )
    parser.add_argument("--with", dest="pinned", action="append", default=[], metavar="PACKAGE")
    parser.add_argument(
        "--without",
        dest="dropped",
        action="append",
        default=[],
        metavar="PACKAGE",
        help="exclude a shell the closure would otherwise load (repeatable). The run block "
        "lists it under EXCLUDED with reason `withheld`, so an A/B pair cannot be confused "
        "for two identical runs. Needed for the param-patching class: any DLL that mutates a "
        "param row moves the Seamless lobby key and silently drops you out of matchmaking.",
    )
    parser.add_argument(
        "--disable-arxan",
        action="store_true",
        help="ask me3 to disable Arxan anti-tamper. DIAGNOSTIC A/B ONLY: me3 logs "
        "`arxan detected: true` and leaves it ARMED by default, so an ordinary run here carries "
        "live anti-tamper. Turning it off changes what the game IS, so a run with this flag is "
        "never product proof -- it answers one question: does the fault survive without Arxan?",
    )
    parser.add_argument("--no-fetch", action="store_true", help="skip refreshing origin/main")
    parser.add_argument("--skip-steam-check", action="store_true")
    parser.add_argument(
        "--allow-stale-launcher",
        action="store_true",
        help="launch even though another me3 launcher is alive (it will probably fail)",
    )
    parser.add_argument("--dry-run", action="store_true", help="stage everything, launch nothing")
    parser.add_argument("--status", metavar="RUN_ID")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if args.status:
        return status(args.status)

    try:
        args.branch = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            timeout=20,
        ).stdout.strip()
        return launch(args)
    except RuntimeError as err:
        print(f"\ner-run-branch: {err}\n", file=sys.stderr)
        return EXIT_ERROR
    except subprocess.TimeoutExpired as err:
        print(f"\ner-run-branch: a step exceeded its {SUBPROCESS_TIMEOUT}s bound: {err}\n", file=sys.stderr)
        return EXIT_ERROR


if __name__ == "__main__":
    sys.exit(main())
