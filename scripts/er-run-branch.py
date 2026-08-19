#!/usr/bin/env python3
"""Launch Elden Ring with the DLLs this branch changed, on a random decoded character.

Pipeline, in order, each step refusing rather than guessing:

  1. GARBAGE-COLLECT dead runs. Cleanup is guaranteed by this step, not by the reaper.
  2. STEAM PREFLIGHT via scripts/steam-running.sh. With Steam down the game still boots, but
     into a different environment (wineprefix, save dir, account id), so the logs land
     elsewhere and the run is not representative.
  3. CLOSURE -- scripts/er-dll-closure.py. Refuses on a conflict it cannot rank.
  4. PROVENANCE -- scripts/er-dll-provenance.py per selected DLL. A DLL with no provenance,
     or whose recorded source hash no longer matches this tree, is STALE and stops the run.
     Nothing here builds: this tool's contract is that the DLL is already fresh, and if it is
     not, it says so loudly.
  5. SAVE -- scripts/er-pick-save.py. Random, but DECODED FIRST: the character's name, level
     and slot are known and printed before anything launches (AGENTS.md's Autoload Identity
     Launch Gate). `--seed` reproduces a pick exactly.
  6. STAGE -- a temp .me3 plus a DLL-adjacent sidecar toml. The game-directory er-effects.toml
     is never written.
  7. LAUNCH -- ~/Elden/launch.sh with ME3_PROFILE, detached into its own session.
  8. TESTIMONY -- the block is printed only after the DLL says, in its own debug log, that it
     loaded and read THIS run's sidecar. Otherwise a FAILED block is printed and the run is
     cleaned up.
  9. REAP -- a detached reaper removes the staged files when the game exits.

WHY THE BLOCK WAITS FOR THE DLL RATHER THAN THE WINDOW
------------------------------------------------------
"The process started" is a weak claim -- me3 spawns through Proton and a crashing game is
briefly alive. "The window is up" is a strong claim but minutes away, well past the shell
budget. The DLL's own `runtime-config: loaded ... sidecar=...` line lands at DllMain, within
seconds, and proves three things at once: the process is up, OUR DLL is in it, and it read
OUR config. So the block cannot be printed for a run that did not really happen -- which
matters because a copy-pasted block is a promise to whoever reads it.

The block deliberately claims nothing about the window, the world, or readiness. `--status`
re-checks those later, honestly, once they exist.

Usage:
    python3 scripts/er-run-branch.py                      # random save, ersc loaded
    python3 scripts/er-run-branch.py --seed 4242          # reproduce a pick
    python3 scripts/er-run-branch.py --save default       # the active Steam user's own save
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

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = REPO_ROOT / "scripts"
LAUNCHER = Path.home() / "Elden" / "launch.sh"
PROFILE_DIR = Path.home() / "Elden"
AUTOLOAD_LOG_NAME = "er-effects-autoload-debug.log"
# The log above belongs to THIS DLL and no other. The sidecar-testimony contract is only
# available when it is loaded, because it is the only shell that reads the sidecar at all.
PRODUCT_DLL_NAME = "er_effects_rs.dll"

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_NO_TESTIMONY = 4

# Each wait is one bounded slice, re-armed until a wall-clock DEADLINE, so no single call
# approaches the 30s shell ceiling.
#
# The budget is wall-clock and NOT a count of slices. A slice ends on any inotify event in the
# game directory, and during boot that directory sees dozens of writes a second (every co-loaded
# DLL has its own log). Counting slices therefore burned a nominal 24-second budget in
# milliseconds and reported a perfectly healthy run as "silent" -- measured on a live launch
# where the DLL logged its config 9 seconds in, well inside the window that was supposed to be
# open.
TESTIMONY_SLICE_SECONDS = 4.0
TESTIMONY_BUDGET_SECONDS = 25.0
SUBPROCESS_TIMEOUT = 28


def game_dir() -> Path:
    steam = Path(os.environ.get("ME3_STEAM_DIR", Path.home() / ".local/share/Steam"))
    return steam / "steamapps/common/ELDEN RING/Game"


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
        #  * REPLACED -- new inode, so read the whole new file;
        #  * TRUNCATED IN PLACE -- same inode, size drops below our offset. Seeking to the old
        #    offset then lands past EOF and reads NOTHING, so the DLL's startup lines are
        #    invisible and the run is reported "silent" while it is in fact running perfectly.
        rotated = stat.st_ino != self.inode or stat.st_size < self.offset
        start = 0 if rotated else self.offset
        try:
            with self.path.open("rb") as handle:
                handle.seek(min(start, stat.st_size))
                data = handle.read()
        except OSError:
            return ""
        # A LINE IS EVIDENCE ONLY ONCE IT IS TERMINATED, and this cost a live run. The DLL's
        # `runtime-config: loaded` line is ~540 bytes and is not written atomically, so a read
        # landing mid-write returns a PREFIX -- "runtime-config: loaded '<game toml>'" with the
        # `sidecar=` field still unwritten. The caller cannot tell that from a DLL that genuinely
        # named no sidecar, so it declared a perfectly good run "the DLL IGNORED this run's
        # overlay" and told the reader not to cite it. Hand back only whole lines; the fragment
        # arrives complete on the next poll, milliseconds later.
        end = data.rfind(b"\n")
        if end < 0:
            return ""
        return data[: end + 1].decode("utf-8", errors="replace")


def await_testimony(log_path: Path, tail: LogTail, sidecar: Path, launcher_pid: int) -> dict:
    """Block until the DLL states it loaded THIS run's sidecar. Event-driven, never a sleep.

    Returns a verdict dict with `status`:
      confirmed      -- the DLL named OUR sidecar; this run is what it says it is.
      wrong-sidecar  -- the DLL loaded and logged, but named a different sidecar (or none).
                        Completely different from silence: the game IS running our DLL, it just
                        ignored the overlay, so the character on screen is NOT the one picked.
                        Conflating the two sends you hunting a launch failure that did not happen.
      silent         -- no `runtime-config: loaded` line at all within the window.
    """
    wanted = normalize_path(str(sidecar))
    seen: dict | None = None
    deadline = time.monotonic() + TESTIMONY_BUDGET_SECONDS
    with er_run_lib.DirectoryWatch(log_path.parent) as watch:
        while True:
            for line in tail.new_text().splitlines():
                if "runtime-config: loaded" not in line:
                    continue
                fields = parse_loaded_line(line)
                reported = normalize_path(fields.get("sidecar", ""))
                if reported and reported == wanted:
                    return {"status": "confirmed", "line": line.strip(), **fields}
                seen = {"status": "wrong-sidecar", "line": line.strip(), **fields}
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


def await_any_dll_log(game_dir_path: Path, launcher_pid: int, started_at: float) -> dict:
    """Weaker witness, for a run whose DLL set does not include the product shell.

    WHY THIS EXISTS. The sidecar line proves three things at once -- process up, our DLL in it,
    our config read -- but only `er_effects_rs.dll` writes it, because it is the only shell that
    reads the sidecar. Once the launcher/watchdog/guard work merged to main, the closure started
    selecting DLL sets that legitimately EXCLUDE that shell, and the gate went on waiting for a
    witness the run never loaded. It then condemned a perfectly healthy game: run
    br-20260817-184836-d6a7 printed `ELDEN RING DID NOT START` while `eldenring.exe` was up and
    the invasion DLL was heartbeating into its own log.

    So when the strong witness is unavailable, ask a weaker question honestly rather than a
    strong one wrongly: has ANY log next to the executable gained bytes since we launched? That
    proves the process is up and one of our shells is running in it. It does NOT prove which
    sidecar was read, and the caller must not claim that it does.

    Matching is by mtime rather than by a DLL-name -> log-name table on purpose: those names do
    not follow a convention (`er_net_effects_dll.dll` writes `er-net-effects.log`,
    `er_invasion_warp_dll.dll` writes `er-invasion-warp-dll.log`), so a table would be a second
    source of truth that silently rots every time a shell is added.
    """
    deadline = time.monotonic() + TESTIMONY_BUDGET_SECONDS
    with er_run_lib.DirectoryWatch(game_dir_path) as watch:
        while True:
            for log in sorted(game_dir_path.glob("*.log")):
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
    lines.append("")
    if save:
        lines += [
            f"  character     {save['name']}  RL{save['level']}  slot {save['slot']}",
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
        lines.append("  character     <active Steam user's default save>")
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
            "                existing. This run does not load er_effects_rs.dll, which is the\n"
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
        "  cleanup       automatic when the game exits; the next launch collects it otherwise.",
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


def preflight(args) -> tuple[dict, dict | None]:
    """Closure + provenance + save pick. Raises RuntimeError with a loud message on any refusal."""
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
    elif args.save != "default":
        save = decode_explicit_save(args.save)

    return closure, save


def decode_explicit_save(spec: str) -> dict:
    """Resolve `PATH[:SLOT]` into the same decoded shape a random pick produces.

    The decode is NOT optional. AGENTS.md's Autoload Identity Launch Gate requires the character
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
    if args.save == "default":
        gen_args.append("--save-default")
    else:
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

    log_path = game_dir() / AUTOLOAD_LOG_NAME
    tail = LogTail(log_path)

    if not LAUNCHER.is_file():
        raise RuntimeError(f"launcher not found: {LAUNCHER}")

    started = datetime.now(timezone.utc).isoformat(timespec="seconds")
    # Monotonic-free on purpose: compared against file mtimes, which are wall clock.
    launched_at = time.time()
    process = subprocess.Popen(
        ["bash", str(LAUNCHER)],
        env={**os.environ, "ME3_PROFILE": staged["profile"]},
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL,
        start_new_session=True,  # survives this shell, and this agent turn
        cwd=str(REPO_ROOT),
    )
    state.pid = process.pid
    state.save()

    # Which witness this run can actually produce. Asking for sidecar testimony from a DLL set
    # that does not contain the shell which writes it is how a healthy run gets condemned.
    loads_product_dll = any(
        Path(dll).name == PRODUCT_DLL_NAME for dll in staged["dlls"]
    )
    if loads_product_dll:
        testimony = await_testimony(log_path, tail, Path(staged["sidecar"]), process.pid)
    else:
        testimony = await_any_dll_log(log_path.parent, process.pid, launched_at)
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
                f"  {log_path if loads_product_dll else log_path.parent}",
            ]

        # Only tear down staged files if nothing is using them. A game still booting will read
        # the sidecar AFTER this point, and deleting it mid-boot both breaks that run and
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
                "save": save,
                "profile": staged["profile"],
                "sidecar": staged["sidecar"],
                "evidence_class": staged["evidence_class"],
                "testimony": testimony["line"],
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
        print(f"no such run: {run_id} (already cleaned up?)")
        return EXIT_ERROR
    alive = er_run_lib.process_alive(state.pid)
    game = er_run_lib.find_game_pids()
    print(
        json.dumps(
            {
                "run_id": run_id,
                "launcher_pid": state.pid,
                "launcher_alive": alive,
                "game_pids": game,
                "profile": state.profile,
                "meta": state.meta,
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
        normalize_path("Z:\\home\\banon\\x\\er_effects_rs.toml")
        == normalize_path("/home/banon/x/er_effects_rs.toml"),
        "a Wine Z:\\ path and its Linux path compare equal",
    )
    check(
        normalize_path("C:\\Other\\thing.toml") != normalize_path("/home/x/thing.toml"),
        "different files still compare unequal after normalisation",
    )

    line = (
        "runtime-config: loaded 'Z:/game/er-effects.toml' sidecar=Z:/t/er_effects_rs.toml "
        "save_file=Z:/c/ER0000.sl2 slot=3 method=<unset>"
    )
    fields = parse_loaded_line(line)
    check(fields.get("slot") == "3", "the slot is parsed out of the loaded line")
    check(
        normalize_path(fields.get("sidecar", "")) == normalize_path("/t/er_effects_rs.toml"),
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

        # The rotation that actually bit: TRUNCATION IN PLACE. Same inode, size back to ~0, so
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

        # A read landing mid-write must yield NOTHING rather than a prefix that parses as a
        # complete record with its later fields missing.
        partial = Path(raw) / "partial.log"
        partial.write_text("", encoding="utf-8")
        partial_tail = LogTail(partial)
        with partial.open("a", encoding="utf-8") as handle:
            handle.write("runtime-config: loaded 'S:/g/er-effects.toml' side")
        check(
            partial_tail.new_text() == "",
            "a half-written line is withheld until its newline arrives",
        )
        with partial.open("a", encoding="utf-8") as handle:
            handle.write("car=Z:/t/er_effects_rs.toml slot=4\n")
        check(
            "sidecar=Z:/t/er_effects_rs.toml" in partial_tail.new_text(),
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
            "dlls": [("er_effects_rs.dll", "c" * 64)],
            "ersc": "/game/ersc.dll",
            "excluded": [{"artifact": "er_loading_bar_dll.dll", "kind": "present-compositor"}],
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
            "sidecar": "/t/er_effects_rs.toml",
            "evidence_class": "explicit-save-source",
            "testimony": "runtime-config: loaded ...",
        }
    )
    check("ELDEN RING IS RUNNING" in block, "the block announces the run")
    check("Bonky Bean" in block and "RL139" in block, "the block names the decoded character")
    check("--seed 7" in block, "the block carries the seed to reproduce the pick")
    check("EXCLUDED er_loading_bar_dll.dll" in block, "the block names excluded DLLs")
    check(
        "NOT claimed" in block and "world loaded" in block,
        "the block states what it does NOT claim, so it cannot be over-read",
    )
    check(block.startswith("```") and block.rstrip().endswith("```"), "the block is copy-pasteable")

    failure = failed_block("r2", "no DLL testimony", ["pid 9 exited"])
    check("DID NOT START" in failure, "the failure block cannot be mistaken for success")
    check("ELDEN RING IS RUNNING" not in failure, "the failure block never contains the running banner")

    # The distinction the first live run exposed: a DLL that loaded and ignored the overlay is
    # NOT the same failure as a DLL that never spoke, and treating them alike sends you hunting
    # a launch failure that did not happen.
    with tempfile.TemporaryDirectory() as raw:
        log = Path(raw) / "auto.log"
        log.write_text("", encoding="utf-8")
        tail = LogTail(log)
        log.write_text(
            "runtime-config: loaded 'S:/g/er-effects.toml' sidecar=<none> "
            "save_file=Z:/other/ER0000.sl2 slot=0\n",
            encoding="utf-8",
        )
        verdict = await_testimony(log, tail, Path("/t/er_effects_rs.toml"), os.getpid())
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
            "runtime-config: loaded 'S:/g/er-effects.toml' sidecar=Z:/t/er_effects_rs.toml slot=3\n",
            encoding="utf-8",
        )
        verdict2 = await_testimony(log2, tail2, Path("/t/er_effects_rs.toml"), os.getpid())
        check(verdict2["status"] == "confirmed", "a matching sidecar line confirms the run")

        # THE FALSE NEGATIVE THAT CONDEMNED A RUNNING GAME, br-20260817-184836-d6a7. Once the
        # launcher/watchdog/guard commits merged to main, the closure legitimately stopped
        # selecting er_effects_rs.dll -- the only shell that writes a `runtime-config: loaded`
        # line. The gate kept waiting for it and printed ELDEN RING DID NOT START while
        # eldenring.exe was up and the invasion DLL was heartbeating into its own log.
        weak_dir = Path(raw) / "weakwitness"
        weak_dir.mkdir()
        launched = time.time()
        # A log that predates the launch must NOT count: it is last run's evidence.
        stale = weak_dir / "er-invasion-warp-dll.log"
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
            f"a run without er_effects_rs.dll confirms from any DLL's log (got {weak['status']})",
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
                "dirty": False, "dlls": [("er_invasion_warp_dll.dll", "c" * 64)],
                "ersc": None, "excluded": [], "save": None, "profile": "/p.me3",
                "sidecar": "/s.toml", "evidence_class": "x",
                "testimony": "er-net-effects.log written after launch", "witness": "weak",
            }
        )
        check(
            "sidecar was NOT verified" in weak_block,
            "the weak-witness block says outright that the sidecar was not verified",
        )

        # THE FALSE NEGATIVE THIS COSTS A RUN OVER, observed live on br-20260816-183410-949e:
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
            handle.write("runtime-config: loaded 'S:/g/er-effects.toml' side")

        def finish_line() -> None:
            observed_partial.wait(timeout=5)
            with split_log.open("a", encoding="utf-8") as handle:
                handle.write("car=Z:/t/er_effects_rs.toml save_file=Z:/c/ER0000.sl2 slot=4\n")

        writer = threading.Thread(target=finish_line, daemon=True)
        writer.start()
        try:
            verdict_split = await_testimony(
                split_log, split_tail, Path("/t/er_effects_rs.toml"), os.getpid()
            )
        finally:
            writer.join(timeout=2)
        check(
            verdict_split["status"] == "confirmed",
            f"a loaded line arriving in two writes confirms instead of condemning the run "
            f"(got {verdict_split['status']})",
        )

    # THE REGRESSION THAT COST A LIVE RUN. The game directory is written to constantly during
    # boot, so every inotify slice returns instantly. When the budget was a COUNT of slices it
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
                quiet_log, tail3, Path("/t/er_effects_rs.toml"), os.getpid()
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

    check(LAUNCHER.is_file(), f"the user launcher this delegates to exists ({LAUNCHER})")
    check((SCRIPTS / "er-run-reaper.py").is_file(), "the reaper it detaches exists")

    print("selftest:", "PASS" if ok else "FAIL")
    return EXIT_OK if ok else EXIT_ERROR


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--save",
        default="random",
        metavar="random|default|PATH[:SLOT]",
        help="random (default), default (the active Steam user's own container), or an explicit "
        "save path with an optional :SLOT. An explicit save is still DECODED and reported before "
        "launch -- naming a file is not the same as knowing which character is in it.",
    )
    parser.add_argument("--seed", type=int, help="reproduce an exact save pick")
    parser.add_argument("--vanilla", action="store_true", help="omit ersc.dll; draw .sl2 saves only")
    parser.add_argument("--monitor", help="Hyprland monitor to move the ER window to when it appears")
    parser.add_argument("--with", dest="pinned", action="append", default=[], metavar="PACKAGE")
    parser.add_argument("--no-fetch", action="store_true", help="skip refreshing origin/main")
    parser.add_argument("--skip-steam-check", action="store_true")
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
