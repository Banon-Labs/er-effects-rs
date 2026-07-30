#!/usr/bin/env python3
"""A/B the profile-switch reload softlock (WorldBlockRes parks at phase 2 / mms_step=3).

ROOT CAUSE this exercises (bd reload-freeze-ROOTCAUSE-addfilecap-cachehit-never-enqueues-2026-07-30):
reloading the SAME map area back-to-back makes `FD4ResCapHolder::AddEntry` hit the surviving
`MsbFileCap` the skipped `_Common_Finalize` left alive. `CS::CSFileRepository::AddFileCap` then takes
its cache-hit branch, which returns early WITHOUT calling `PushFileCap` -- so no load is ever enqueued
and `FUN_1406157f0` (case 2) polls `FD4FileCap::loadState == 4` forever with no timeout.

The candidate fix is the existing opt-in Phase-3 outgoing-world teardown
(`er-effects-enable-outgoing-teardown.txt`), which routes the outgoing world through
`CS::InGameStep::_Common_Finalize` so the map FD4FileCap is released and the next `AddEntry` MISSES.

Both arms are DETERMINISTIC and fully agent-driven: load 1 is the configured autoload, and load 2 is
triggered by the product's own control file (`er-effects-switch-slot.txt`, polled every in-world
frame), which is the same code path the user's menu click reaches. No input harness, no menu nav.

  ARM off  -> expect FROZEN  (reproduces the bug)
  ARM on   -> expect LOADED  (oracle_common_finalize_count increments)

Usage:
  python3 scripts/run-reload-freeze-ab.py --teardown off --label control
  python3 scripts/run-reload-freeze-ab.py --teardown on  --label fix

Save safety: the configured source save is opened READ-ONLY by the DLL (staged into a private tree);
this script never writes it. It backs up and restores the game-dir er-effects.toml and the marker.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import re
import select
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GAME_DIR = Path(
    os.environ.get(
        "ER_GAME_DIR",
        str(Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game"),
    )
)
LAUNCHER = Path(os.environ.get("ER_LAUNCHER", str(Path.home() / "Elden/launch.sh")))
TELEMETRY = GAME_DIR / "er-effects-telemetry.json"
DEBUG_LOG = GAME_DIR / "er-effects-autoload-debug.log"
TOML = GAME_DIR / "er-effects.toml"
MARKER = GAME_DIR / "er-effects-enable-outgoing-teardown.txt"
SWITCH_SLOT = GAME_DIR / "er-effects-switch-slot.txt"
SWITCH_SAVE = GAME_DIR / "er-effects-switch-save-file.txt"

# The GAME runtime portion is bounded by the single canonical cap; never hardcode it here.
CAP_FILE = REPO / ".auto/runtime_timeout_cap_seconds"
BOOT_TIMEOUT_S = 180        # load1: boot -> player present
FREEZE_VERDICT_S = 45       # blk_35==2 held this long with no step change => FROZEN
LOAD2_TIMEOUT_S = 120       # load2: switch armed -> world or freeze verdict


class GameDirWatch:
    """Block until the DLL WRITES something, instead of sleeping on a guess.

    The repo bans sleep-as-synchronization (scripts/check-no-timeouts.py `python-sleep-or-wait-for`).
    The DLL rewrites er-effects-telemetry.json and appends the debug log every frame, so an inotify
    watch on the game dir is the deterministic readiness primitive for "new state exists": each wait
    returns as soon as the game produces evidence, and the budget is only a backstop for a dead game.
    """

    IN_MODIFY = 0x00000002
    IN_CREATE = 0x00000100
    IN_NONBLOCK = 0o4000

    def __init__(self, directory: Path) -> None:
        self._libc = ctypes.CDLL("libc.so.6", use_errno=True)
        self.fd = self._libc.inotify_init1(self.IN_NONBLOCK)
        if self.fd < 0:
            raise OSError("inotify_init1 failed")
        if self._libc.inotify_add_watch(
            self.fd, str(directory).encode(), self.IN_MODIFY | self.IN_CREATE
        ) < 0:
            raise OSError(f"inotify_add_watch failed on {directory}")

    def wait(self, budget_s: float) -> bool:
        """Return True when the game wrote something within the budget, False on a quiet timeout."""
        ready, _, _ = select.select([self.fd], [], [], budget_s)
        if not ready:
            return False
        try:
            os.read(self.fd, 1 << 16)
        except BlockingIOError:
            pass
        return True

    def close(self) -> None:
        try:
            os.close(self.fd)
        except Exception:
            pass


def quiet_wait(budget_s: float) -> None:
    """Timed wait with no file to watch (process-exit polling). select() with no fds, no sleep API."""
    select.select([], [], [], budget_s)


def cap_seconds() -> int:
    try:
        return int(CAP_FILE.read_text().strip())
    except Exception:
        return 300


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def win_path(p: Path) -> str:
    return "Z:" + str(p).replace("/", "\\")


def steam_running() -> bool:
    """Use the sanctioned helper; raw pgrep is guard-denied and false-negatives here."""
    helper = REPO / "scripts/steam-running.sh"
    if not helper.exists():
        return True
    # Explicit bound: every non-game op is hard-capped at 30s (scripts/check-no-timeouts.py).
    # The GAME launch below is a Popen and is bounded separately by the canonical runtime cap.
    try:
        r = subprocess.run(
            ["bash", "-c", f'source "{helper}" && steam_running'],
            capture_output=True,
            timeout=15,
        )
    except subprocess.TimeoutExpired:
        return False
    return r.returncode == 0


def er_pids() -> list[int]:
    out = []
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            if "eldenring.exe" in (d / "comm").read_text():
                out.append(int(d.name))
        except Exception:
            pass
    return out


def kill_er() -> None:
    """Kill ONLY eldenring.exe by exact comm match -- never a broad pattern."""
    for pid in er_pids():
        try:
            os.kill(pid, signal.SIGTERM)
        except Exception:
            pass
    for _ in range(20):
        if not er_pids():
            return
        quiet_wait(0.5)
    for pid in er_pids():
        try:
            os.kill(pid, signal.SIGKILL)
        except Exception:
            pass


def read_telemetry() -> dict:
    try:
        return json.loads(TELEMETRY.read_text())
    except Exception:
        return {}


ORACLE_RE = re.compile(
    r"\[\+(\d+)ms\].*SWITCH-ORACLE.*?epoch=(\d+).*?blk_35=(\d+)", re.S
)


def tail_oracle(fh) -> list[tuple[int, int, int, int]]:
    """Return (t_ms, epoch, blk_35, mms_step) for new SWITCH-ORACLE lines."""
    rows = []
    for line in fh.readlines():
        if "SWITCH-ORACLE" not in line:
            continue

        def g(pat):
            m = re.search(pat, line)
            return int(m.group(1)) if m else None

        t = g(r"\[\+(\d+)ms\]")
        ep = g(r"epoch=(\d+)")
        b35 = g(r"blk_35=(\d+)")
        st = g(r"mms_step=(\d+)")
        if t is not None and st is not None:
            rows.append((t, ep if ep is not None else -1, b35 if b35 is not None else -1, st))
    return rows


def rotate_outputs(art: Path) -> None:
    """Move the previous run's telemetry/log aside.

    MANDATORY, not hygiene: `oracle_player_present` from a PREVIOUS run reads as a satisfied
    precondition the instant we look, so load1 is declared complete before the game has even booted
    (observed 20260730-151858-control: "LOAD1 in world after 0.0s"). The DLL also RECREATES the debug
    log per process, so a byte-offset saved before launch points past the new file's content.
    """
    for src in (TELEMETRY, DEBUG_LOG):
        if src.exists():
            try:
                shutil.move(str(src), str(art / (src.name + ".pre-run")))
            except Exception:
                pass


def stage(save: Path, slot_a: int, teardown_on: bool) -> None:
    shutil.copyfile(TOML, TOML.with_suffix(".toml.ab-backup"))
    body = (
        f"save_file = '{win_path(save)}'\n"
        f"slot = {slot_a}\n"
        "autoupdate_preferred_picker_dir = false\n"
    )
    TOML.write_text(body)
    log(f"toml staged: save_file={win_path(save)} slot={slot_a}")

    for f in (SWITCH_SLOT, SWITCH_SAVE):
        if f.exists():
            f.unlink()
    log("cleared stale switch control files (CASE 1: same save, different slot)")

    if teardown_on:
        MARKER.write_text("enabled by run-reload-freeze-ab.py\n")
        log("MARKER ON  -> outgoing-world teardown (_Common_Finalize) ENABLED")
    else:
        if MARKER.exists():
            MARKER.unlink()
        log("MARKER OFF -> product default in-place reload (bug expected)")


def restore() -> None:
    bak = TOML.with_suffix(".toml.ab-backup")
    if bak.exists():
        shutil.copyfile(bak, TOML)
        bak.unlink()
    if MARKER.exists():
        MARKER.unlink()
    for f in (SWITCH_SLOT, SWITCH_SAVE):
        if f.exists():
            f.unlink()
    log("restored toml + removed marker/control files")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--teardown", choices=["on", "off"], required=True)
    ap.add_argument("--label", required=True)
    # Defaults reproduce the user's observed freeze EXACTLY: 139-STR slot 0 ('Ordinary Bean') and
    # 45-Slots slot 2 are BOTH c30=0x1c000000 (m28), i.e. same area back-to-back.
    # CASE 2 (cross-save) is required: driving CASE 1 (same file, different slot) instead takes the
    # world-teardown path into the quit-to-desktop hook and cleanly ExitProcess(0)s rather than
    # reloading (observed 20260730-152711-control3).
    ap.add_argument("--save", default=str(REPO / "save-files/139-STR/ER0000.sl2"))
    ap.add_argument("--switch-save", default=str(REPO / "save-files/45-Slots/ER0000.sl2"))
    ap.add_argument("--slot-a", type=int, default=0)
    ap.add_argument("--slot-b", type=int, default=2)
    args = ap.parse_args()

    save = Path(args.save)
    art = REPO / "target/reload-freeze-ab" / f"{time.strftime('%Y%m%d-%H%M%S')}-{args.label}"
    art.mkdir(parents=True, exist_ok=True)
    verdict = {"label": args.label, "teardown": args.teardown, "save": str(save),
               "slot_a": args.slot_a, "slot_b": args.slot_b, "cap_s": cap_seconds()}

    if not save.is_file():
        log(f"FATAL: save not found: {save}")
        return 2
    if not steam_running():
        log("FATAL: Steam is DOWN. Agent-owned ER probes require Steam (sanctioned helper).")
        return 2
    if er_pids():
        log("FATAL: an eldenring.exe is ALREADY running; refusing to disturb it.")
        return 2

    ro = not os.access(save, os.W_OK)
    log(f"save={save} read_only={ro} (read-only autoload source is the point of this run)")
    verdict["source_read_only"] = ro

    stage(save, args.slot_a, args.teardown == "on")
    rotate_outputs(art)  # must precede launch; see rotate_outputs docstring
    t0 = time.time()
    proc = subprocess.Popen(
        ["bash", str(LAUNCHER)],
        cwd=str(LAUNCHER.parent),
        stdout=open(art / "launcher.log", "w"),
        stderr=subprocess.STDOUT,
    )
    log(f"launched {LAUNCHER} pid={proc.pid}")
    watch = GameDirWatch(GAME_DIR)

    try:
        # ---- phase 1: load1 to world -------------------------------------------------
        # Gate on the FRESH debug log (epoch0 reaching MAP LOAD DONE), never on telemetry alone --
        # telemetry is a snapshot that a previous run can satisfy before this one boots.
        fh = None
        player = False
        last_key = None
        while time.time() - t0 < BOOT_TIMEOUT_S:
            if fh is None and DEBUG_LOG.exists():
                fh = open(DEBUG_LOG, "r", encoding="utf-8", errors="replace")
                log("debug log opened (fresh, recreated by this process)")
            if fh is not None:
                for (t, ep, b35, st) in tail_oracle(fh):
                    key = (ep, b35, st)
                    if key != last_key:
                        log(f"  load1 oracle epoch={ep} blk_35={b35} mms_step={st}")
                        last_key = key
            # LOAD1 COMPLETE == player in world with a real character. Do NOT gate on mms_step==20:
            # the autoload path parks at 18 (PLACING IN WORLD) and is MOVABLE there -- 18/fin0 is the
            # working state, not a stall (bd REFRAME-load2-mms18-fin0-IS-MOVABLE-not-stall). Demanding
            # 20 timed out a perfectly healthy load1 (run 20260730-152302-control2).
            # rotate_outputs() guarantees any telemetry present here was written by THIS process.
            tel = read_telemetry()
            name = str(tel.get("oracle_char_name", "") or "")
            if str(tel.get("oracle_player_present", "0")) not in ("0", "None", "", "False") and \
               name not in ("", "?", "None"):
                player = True
                log(f"load1 character in world: '{name}'")
                break
            if not er_pids() and time.time() - t0 > 30:
                log("game exited during load1")
                break
            watch.wait(2)
        verdict["load1_player_present"] = player
        verdict["load1_seconds"] = round(time.time() - t0, 1)
        if not player:
            log("FAIL: load1 never reached player-present; aborting")
            verdict["verdict"] = "LOAD1_FAILED"
            return finish(art, verdict, proc)
        log(f"LOAD1 in world after {verdict['load1_seconds']}s")

        # ---- phase 2: drive load2 via the product's own control file ------------------
        # Settle on EVIDENCE, not a nap: require consecutive DLL writes that still report the
        # player in world, so the switch is armed only once load1 is genuinely eligible.
        settled = 0
        while settled < 5 and time.time() - t0 < BOOT_TIMEOUT_S:
            if not watch.wait(3):
                break
            t = read_telemetry()
            settled = settled + 1 if str(t.get("oracle_player_present", "0")) not in ("0", "None", "", "False") else 0
        log(f"load1 settled ({settled} consecutive in-world telemetry writes)")
        # CASE 2 ORDER MATTERS: the target save FILE must be staged BEFORE the slot, because the slot
        # write is what arms the switch and own_load resolves the source at arm time.
        if args.switch_save:
            SWITCH_SAVE.write_text(win_path(Path(args.switch_save)) + "\n")
            log(f"WROTE er-effects-switch-save-file.txt = {win_path(Path(args.switch_save))}")
        SWITCH_SLOT.write_text(f"{args.slot_b}\n")
        t_switch = time.time()
        log(f"WROTE er-effects-switch-slot.txt = {args.slot_b}  (this is the menu click)")

        # ---- phase 3: watch for freeze or completion ----------------------------------
        # Reuse the SAME handle from phase 1 -- reopening would re-read load1's oracle lines and
        # mistake load1's own healthy progression for load2's.
        frozen_since = None
        cleared = False
        result = "TIMEOUT"
        while True:
            elapsed_total = time.time() - t0
            if elapsed_total > cap_seconds():
                log(f"hit canonical runtime cap {cap_seconds()}s")
                result = "CAP"
                break
            if time.time() - t_switch > LOAD2_TIMEOUT_S:
                result = "LOAD2_TIMEOUT"
                break
            if not er_pids():
                result = "GAME_EXITED"
                break

            for (t, ep, b35, st) in tail_oracle(fh):
                key = (ep, b35, st)
                if key != last_key:
                    log(f"  oracle epoch={ep} blk_35={b35} mms_step={st}")
                    last_key = key
                    frozen_since = time.time() if (b35 == 2 and st == 3) else None
                if ep is not None and ep >= 1 and b35 >= 3:
                    cleared = True
                elif b35 == 2 and st == 3 and frozen_since is None:
                    frozen_since = time.time()

            tel = read_telemetry()
            cf = tel.get("oracle_common_finalize_count")
            # LOAD2 CLEARED == the reload's WorldBlockRes advanced PAST the phase-2 freeze point.
            # Phase 2 is the infinite wait (FUN_1406157f0 has no timeout); anything >=3 means the
            # FD4FileCap completed and the block moved on. Again: not mms_step==20.
            # LATCH it: blk_35 is only present on SOME oracle lines (-1 = field absent), so testing
            # only the MOST RECENT sample scores a cleared reload as a timeout -- control4 reached
            # epoch1/blk_35=7 and was still called LOAD2_TIMEOUT because the last line had blk_35=-1.
            if cleared:
                log(f"LOAD2 advanced past the freeze (blk_35>=3 seen at epoch>=1)")
                result = "LOADED"
                break
            if frozen_since and time.time() - frozen_since > FREEZE_VERDICT_S:
                log(f"FROZEN: blk_35=2 / mms_step=3 held {FREEZE_VERDICT_S}s with no advance")
                result = "FROZEN"
                break
            verdict["last_common_finalize"] = cf
            watch.wait(2)

        verdict["verdict"] = result
        verdict["load2_seconds"] = round(time.time() - t_switch, 1)
        tel = read_telemetry()
        for k in ("oracle_common_finalize_count", "oracle_outgoing_teardown_done",
                  "oracle_outgoing_teardown_failsoft", "oracle_outgoing_teardown_wait_ticks",
                  "oracle_player_present"):
            verdict[k] = tel.get(k)
        log(f"VERDICT: {result}")
        return finish(art, verdict, proc)
    finally:
        kill_er()
        restore()


def finish(art: Path, verdict: dict, proc) -> int:
    kill_er()
    for src in (TELEMETRY, DEBUG_LOG):
        if src.exists():
            try:
                shutil.copyfile(src, art / src.name)
            except Exception:
                pass
    (art / "verdict.json").write_text(json.dumps(verdict, indent=2))
    log(f"artifacts: {art}")
    log(json.dumps(verdict, indent=2))
    return 0 if verdict.get("verdict") in ("LOADED", "FROZEN") else 1


if __name__ == "__main__":
    sys.exit(main())
