#!/usr/bin/env python3
"""Agent-owned self-driving System->Quit->Load-Profile repro run + monitor.

Arms the controller-free XInput harness in PROFILE-LOAD-SWITCH mode via two game-dir
marker files, deploys the freshly-built DLL, launches ER through me3 (the same offline
path me3_live_launch uses), and watches the DLL debug log for:
  - the sq-repro autopilot phase transitions (proves the harness self-drives, no human input)
  - the SWITCH-ORACLE step-3 (WORLD RES WAIT) stall (blk_ls=0x0) = the repro
  - LOADED_STABLE (world entered = no stall / a fix worked)
  - game exit (crash/close)

The REAL sync signal is the RAM-derived SWITCH-ORACLE semaphore in the log; the wall-clock
CAP is only a safety backstop (NOT the primary sync). Tears the game down immediately on a
verdict (tear-down-on-insight) and removes the markers so the next manual launch is normal.

Usage: python3 scripts/sq-repro-selfdrive-monitor.py <log_start_offset> [cap_seconds]
"""
import ctypes
import os
import re
import select
import shutil
import subprocess
import sys
import time

GAMEDIR = "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"
LOG = os.path.join(GAMEDIR, "er-effects-autoload-debug.log")
ME3 = "/mnt/c/Users/choza/AppData/Local/garyttierney/me3/bin/me3.exe"
BUILT_DLL = "/home/choza/projects/er-effects-rs/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll"
DEPLOY_DIR_WSL = "/mnt/c/Users/choza/er-effects-live"
DEPLOY_DLL_WSL = os.path.join(DEPLOY_DIR_WSL, "er_effects_rs.dll")
DEPLOY_DLL_WIN = r"C:\Users\choza\er-effects-live\er_effects_rs.dll"
MARKERS = [
    os.path.join(GAMEDIR, "er-effects-system-quit-repro.txt"),
    # Select the PROFILE-LOAD-SWITCH mode. NOT er-effects-system-quit-allow-profile-load.txt: that
    # opt-in makes the ProfileSelect slot-activate FORWARD to the guarded native load instead of
    # DIRECT-ARMING the save-safe switch, so quickload_phase never advances and the switch never runs.
    os.path.join(GAMEDIR, "er-effects-system-quit-load-switch.txt"),
    # STAY-ACTIVE: force ER's input-accept flag every tick (headless launch leaves the window
    # unfocused). Combined with the foreground-force + SendInput, this lets keyboard input route.
    os.path.join(GAMEDIR, "er-effects-stay-active.txt"),
]
START_OFFSET = int(sys.argv[1]) if len(sys.argv) > 1 else 0
CAP_SECONDS = int(sys.argv[2]) if len(sys.argv) > 2 else 360
POLL = 1.0  # bound the progress-idle watchdog check to ~1s even if the log briefly goes quiet
ME3_STDOUT = "/tmp/sq-repro-me3-stdout.txt"  # data artifact (allowed in /tmp)

# Deterministic readiness (repo no-sleep policy): block on inotify game-dir file-change events with a
# bounded select() timeout as the hard safety cap -- never a bare sleep. The DLL debug log grows inside
# GAMEDIR, so an inotify MODIFY/CREATE event is the real readiness signal (new log bytes to scan); POLL
# only bounds how long we block before re-checking process liveness / the RAM-derived semaphore.
_IN_MODIFY = 0x00000002
_IN_CREATE = 0x00000100
_IN_MOVED_TO = 0x00000080
_IN_CLOSE_WRITE = 0x00000008
_inotify_fd = -1
try:
    _libc = ctypes.CDLL("libc.so.6", use_errno=True)
    _inotify_fd = _libc.inotify_init1(0)
    if _inotify_fd >= 0:
        _libc.inotify_add_watch(
            _inotify_fd,
            GAMEDIR.encode("utf-8"),
            _IN_MODIFY | _IN_CREATE | _IN_MOVED_TO | _IN_CLOSE_WRITE,
        )
except OSError:
    _inotify_fd = -1


def wait_for_change(timeout):
    # Return when a game-dir file changes (log append) OR the timeout safety cap elapses, then the loop
    # re-reads the log + re-checks process liveness. Readiness is the inotify event; the cap only bounds it.
    watch = [_inotify_fd] if _inotify_fd >= 0 else []
    ready, _, _ = select.select(watch, [], [], timeout)
    if ready:
        try:
            os.read(_inotify_fd, 65536)  # drain events; we re-read the log from the saved offset anyway
        except OSError:
            pass


def game_alive():
    try:
        out = subprocess.run(
            ["tasklist.exe", "/FI", "IMAGENAME eq eldenring.exe", "/NH"],
            capture_output=True, text=True, timeout=10,
        ).stdout.lower()
        return "eldenring.exe" in out
    except (OSError, subprocess.SubprocessError):
        return False


def kill(name):
    try:
        subprocess.run(["taskkill.exe", "/F", "/IM", name], capture_output=True, timeout=15)
    except (OSError, subprocess.SubprocessError):
        pass


def main():
    for m in MARKERS:
        with open(m, "w") as f:
            f.write("agent-owned self-driving System->Quit->Load-Profile step-3 repro\n")
    os.makedirs(DEPLOY_DIR_WSL, exist_ok=True)
    shutil.copyfile(BUILT_DLL, DEPLOY_DLL_WSL)
    me3_out = open(ME3_STDOUT, "w")
    p = subprocess.Popen(
        [ME3, "launch", "-g", "eldenring", "-n", DEPLOY_DLL_WIN],
        stdin=subprocess.PIPE, stdout=me3_out, stderr=subprocess.STDOUT,
    )
    print(f"launched me3 pid={p.pid}; markers armed; watching log from offset {START_OFFSET}", flush=True)

    # Shared semaphore-progress watchdog (bd runtime-teardown-semaphore-progress-watchdog): the
    # monitor's OLD stall detector keyed on blk_ls=0, which the 2-arg getter fix made non-null, so it
    # no longer fired and every run went to the full cap. Replace it with the agreed THREE-condition
    # teardown.
    from semaphore_watchdog import (  # pyright: ignore[reportMissingImports]
        ProgressWatchdog,
        CONTINUE,
        TEARDOWN_TERMINAL,
        TEARDOWN_STALL,
        TEARDOWN_CAP,
    )

    t0 = time.time()
    offset = START_OFFSET
    seen = {k: 0 for k in ("in_world", "open_menu", "ingametop", "optionsetting",
                           "profileselect", "to_slot", "confirm")}
    step3_hits = 0
    step3_first = None
    loaded_stable_frames = 0
    mms_step_hwm = 0
    blk35_hwm = 0
    last_oracle = ""
    step3_oracle = ""
    game_seen = False
    verdict = None

    # THREE-condition teardown (user directive 2026-07-17):
    #  (1) LOADED_STABLE (world entered + held)  -> terminal success (+small flush delay)
    #  (2) no world-load PROGRESS for ~1s after the second-load confirm -> progress-idle stall
    #  (3) CAP seconds -> hard backstop
    # Progress = MONOTONIC high-water marks only, never liveness: loaded_stable_frames, max mms_step,
    # and the max block load-phase blk_35 (which cycles 0/2/7 while wedged, so we track its HIGH-water
    # so oscillation is not mistaken for progress). Armed only after the second-load confirm so boot /
    # first-autoload coarse phases cannot false-stall.
    watchdog = ProgressWatchdog(
        idle_window_seconds=1.5,
        teardown_delay_seconds=1.0,
        hard_cap_seconds=float(CAP_SECONDS),
        arm_predicate=lambda t: t.get("confirm", 0) >= 1,
        terminal_predicate=lambda t: t.get("stable_frames", 0) >= 300,
        progress_keys=("stable_frames", "mms_step_hwm", "blk35_hwm"),
    )

    while True:
        now = time.time()
        elapsed = now - t0
        if game_alive():
            game_seen = True
        try:
            sz = os.path.getsize(LOG)
        except OSError:
            sz = offset
        if sz > offset:
            with open(LOG, "rb") as f:
                f.seek(offset)
                data = f.read()
            offset = sz
            for line in data.decode("utf-8", "replace").splitlines():
                if "in-world settled" in line:
                    seen["in_world"] = 1; seen["open_menu"] = 1
                if "IngameTop opened" in line:
                    seen["ingametop"] = 1
                if "OptionSetting opened" in line:
                    seen["optionsetting"] = 1
                if "ProfileSelect opened" in line:
                    seen["profileselect"] = 1
                if "-> TO_SLOT" in line or "-> CONFIRM" in line:
                    seen["to_slot"] = 1
                if "load CONFIRMED" in line or "-> CONFIRM" in line:
                    seen["confirm"] = 1
                if "SWITCH-ORACLE" in line:
                    last_oracle = line.strip()
                    m = re.search(r"mms_step=(\d+)", line)
                    if m:
                        mms_step_hwm = max(mms_step_hwm, int(m.group(1)))
                        # legacy blk_ls=0 counter kept only for the report, not teardown
                        blk = re.search(r"blk_ls=0x([0-9a-fA-F]+)", line)
                        if int(m.group(1)) == 3 and blk and blk.group(1).rstrip("0") == "":
                            step3_hits += 1
                            if step3_first is None:
                                step3_first = round(elapsed, 1)
                                step3_oracle = line.strip()
                    p35 = re.search(r"blk_35=(-?\d+)", line)
                    if p35:
                        blk35_hwm = max(blk35_hwm, int(p35.group(1)))
                    st = re.search(r"stable_frames=(\d+)", line)
                    if st:
                        loaded_stable_frames = max(loaded_stable_frames, int(st.group(1)))
        if game_seen and not game_alive():
            verdict = "GAME EXITED (crash or close)"
            break
        decision = watchdog.observe(
            {
                "confirm": seen["confirm"],
                "stable_frames": loaded_stable_frames,
                "mms_step_hwm": mms_step_hwm,
                "blk35_hwm": blk35_hwm,
            },
            now,
        )
        if decision == TEARDOWN_TERMINAL:
            verdict = "WORLD ENTERED (LOADED_STABLE) -- second load reached world readiness"
            break
        if decision == TEARDOWN_STALL:
            verdict = (
                f"PROGRESS-IDLE STALL: no world-load progress for {watchdog.idle_window_seconds}s "
                f"after confirm (blk35_hwm={blk35_hwm} mms_step_hwm={mms_step_hwm} "
                f"stable={loaded_stable_frames}) -- {watchdog.last_reason}"
            )
            break
        if decision == TEARDOWN_CAP:
            verdict = f"CAP {CAP_SECONDS}s reached (no terminal semaphore, no idle-stall)"
            break
        if decision != CONTINUE:
            verdict = f"watchdog: {decision}"
            break
        wait_for_change(POLL)

    kill("eldenring.exe")
    kill("me3.exe")
    try:
        p.terminate()
    except OSError:
        pass
    for m in MARKERS:
        try:
            os.remove(m)
        except OSError:
            pass
    me3_out.close()

    print("=" * 70, flush=True)
    print("VERDICT:", verdict, flush=True)
    print("elapsed_s:", round(time.time() - t0, 1), flush=True)
    print("harness phases seen:", seen, flush=True)
    print("step3_hits:", step3_hits, "first_at_s:", step3_first, flush=True)
    print("max stable_frames:", loaded_stable_frames, flush=True)
    if step3_oracle:
        print("STEP3 ORACLE:", step3_oracle[-460:], flush=True)
    elif last_oracle:
        print("LAST ORACLE:", last_oracle[-460:], flush=True)
    print("markers removed; game torn down", flush=True)


if __name__ == "__main__":
    main()
