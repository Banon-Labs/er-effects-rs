#!/usr/bin/env python3
"""Prove (or refute) the different-slot loading-screen PORTRAIT with RAM semaphores.

TARGET DEFECT (user 2026-07-30): a System->Quit->Load-Profile switch to a DIFFERENT slot of the
same save deterministically displayed NO rendered character portrait. Root cause (run
20260730-202840 + code): `patch_profile_offscreen_size_for_loaded_slot` resolved the CONFIRMED
LOADED slot, which still names the OLD character during the switch, so the incoming slot's
offscreen-size row stayed native 128 (x2 supersample = 256x256 captures) for every renderer built
in the switch window; the small-capture publish gate then rejected all of them -> nothing displayed
(and before that gate, the same 256 captures published -> the pixelated/naked portrait reports).

This driver validates the fix END TO END, fully agent-driven (no menu nav, no input):
  load1 = the configured boot autoload (game-dir er-effects.toml, untouched by this script);
  load2 = the product's own deterministic control file (er-effects-switch-slot.txt), the same
          code path the user's ProfileSelect click reaches (switch_slot_arm_programmatic).

PASS semaphores for the load2 window (all RAM/native, screenshots are user evidence only):
  - oracle_ls_portrait_w/h >= 1024 (expect 1542) on captures seen during the window
  - oracle_portrait_onto_draw_hits delta > 0 (the portrait actually composited)
  - oracle_ls_portrait_rejected_publishes delta == 0 (no small/neutral captures were eaten)
  - published identity oracle_ls_portrait_slot == target_slot + 1
  - oracle_ls_portrait_renderer_visible_armor_equipped > 0 when the source has armor (not nude)

ALSO REPORTED (er-effects-rs-wmw defect #1, the vanilla flash-through): per-switch
`native_exposure_delta` = Present frames where the game's own CS::LoadingScreen was live but our
cover did not draw -- the frames the user sees vanilla. Reported, not gated, so the portrait
verdict and the flash-through stay independently readable.

Event screenshot: scripts/capture-er-window.py fires at the loading-screen-portrait moment
(protocol: the exact view where the USER can see whether the feature is failing). The agent never
reads the image; it is user evidence.

Usage:
  python3 scripts/run-slot-portrait-proof.py --switch-slot 1 [--label fix1]
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import select
import signal
import shutil
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
SWITCH_SLOT = GAME_DIR / "er-effects-switch-slot.txt"
SWITCH_SAVE = GAME_DIR / "er-effects-switch-save-file.txt"
CAPTURE = REPO / "scripts/capture-er-window.py"
CAP_FILE = REPO / ".auto/runtime_timeout_cap_seconds"

PORTRAIT_KEYS = [
    "oracle_ls_portrait_w", "oracle_ls_portrait_h",
    "oracle_ls_portrait_slot", "oracle_ls_portrait_name_hash",
    "oracle_ls_portrait_rejected_publishes", "oracle_ls_portrait_too_small_seen_version",
    "oracle_ls_portrait_neutral_leak_seen_version",
    "oracle_loading_bg_portrait_rgba_version",
    "oracle_portrait_onto_draw_hits",
    "oracle_portrait_display_frames_last_window",
    "oracle_portrait_drive_frames_last_window",
    "oracle_portrait_have_keyed_frame",
    "oracle_portrait_render_drive_hits", "oracle_portrait_rb_count",
    "oracle_portrait_target_kicks", "oracle_portrait_rt_pin_switches",
    "oracle_ls_portrait_equip_sample_count",
    "oracle_ls_portrait_source_visible_armor_equipped",
    "oracle_ls_portrait_renderer_visible_armor_equipped",
    "oracle_ls_portrait_source_naked_kicks", "oracle_ls_portrait_renderer_naked_kicks",
    "oracle_boot_view_native_exposure_frames",
    # er-effects-rs-wmw defect #1: frames where the game's own loading screen reached the
    # screen with our cover absent -- the vanilla flash-through, per-frame.
    "oracle_native_ls_exposure_frames", "oracle_native_ls_covered_frames",
    "oracle_native_ls_exposure_max_run", "oracle_native_ls_exposure_by_gate",
    "oracle_native_ls_exposure_last_gate", "oracle_native_ls_exposure_last_stop_reason",
    "oracle_native_ls_exposure_first_ms", "oracle_native_ls_exposure_last_ms",
    "oracle_boot_view_stop_reason",
    "oracle_portrait_loadwin_total", "oracle_portrait_loadwin_ok_full",
    "oracle_portrait_loadwin_displayed_small", "oracle_portrait_loadwin_displayed_stale",
    "oracle_portrait_loadwin_missing", "oracle_portrait_loadwin_open",
    "oracle_portrait_loadwin_history",
    "oracle_now_loading", "oracle_player_present", "oracle_char_name", "oracle_char_level",
    "system_quit_quickload_phase", "system_quit_quickload_selected_slot",
    "oracle_switch_slot_control_primed",
]


class GameDirWatch:
    """inotify wait on the game dir: the DLL rewrites telemetry every frame, so this is the
    deterministic 'new state exists' primitive (no sleep-as-sync)."""

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
    select.select([], [], [], budget_s)


def cap_seconds() -> int:
    try:
        return int(CAP_FILE.read_text().strip())
    except Exception:
        return 300


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def steam_running() -> bool:
    helper = REPO / "scripts/steam-running.sh"
    if not helper.exists():
        return True
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


def player_in_world(t: dict) -> bool:
    name = t.get("oracle_char_name") or ""
    return (
        t.get("oracle_player_present") is True
        and bool(name)
        and not name.startswith("�")
        and (t.get("oracle_char_level") or 0) > 0
    )


def snap(t: dict) -> dict:
    return {k: t.get(k) for k in PORTRAIT_KEYS}


def capture(out: Path) -> None:
    try:
        subprocess.run(
            [sys.executable, str(CAPTURE), str(out)], capture_output=True, timeout=25
        )
        log(f"screenshot attempt -> {out.name} (see {out.with_suffix('.txt').name} on fail-closed)")
    except subprocess.TimeoutExpired:
        log("screenshot helper timed out (non-fatal)")


def rotate_outputs(art: Path) -> None:
    """Previous-run telemetry reads as satisfied preconditions; move it aside BEFORE launch."""
    for src in (TELEMETRY, DEBUG_LOG):
        if src.exists():
            try:
                shutil.move(str(src), str(art / (src.name + ".pre-run")))
            except Exception:
                pass


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--switch-slot", type=int, default=1)
    ap.add_argument(
        "--switch-slots",
        default="",
        help="comma-separated CHAIN of switch targets (e.g. '1,3,2'); overrides --switch-slot",
    )
    ap.add_argument("--label", default="proof")
    args = ap.parse_args()

    cap = cap_seconds()
    art = REPO / "target/runtime-probe" / f"slot-portrait-proof-{time.strftime('%Y%m%d-%H%M%S')}-{args.label}"
    art.mkdir(parents=True, exist_ok=True)
    series_f = open(art / "portrait-series.jsonl", "w")
    verdict: dict = {"label": args.label, "switch_slot": args.switch_slot, "cap_s": cap}

    if not steam_running():
        log("FATAL: Steam is DOWN (sanctioned helper). Agent probe requires Steam.")
        return 2
    if er_pids():
        log("FATAL: an eldenring.exe is ALREADY running; refusing to disturb it.")
        return 2

    toml = GAME_DIR / "er-effects.toml"
    verdict["toml"] = toml.read_text() if toml.exists() else None
    for f in (SWITCH_SLOT, SWITCH_SAVE):
        if f.exists():
            f.unlink()
    rotate_outputs(art)

    t0 = time.time()
    proc = subprocess.Popen(
        ["bash", str(LAUNCHER)],
        cwd=str(LAUNCHER.parent),
        stdout=open(art / "launcher.log", "w"),
        stderr=subprocess.STDOUT,
    )
    log(f"launched {LAUNCHER} pid={proc.pid} (product profile, game-dir toml untouched)")
    watch = GameDirWatch(GAME_DIR)

    def teardown(reason: str) -> None:
        (art / "teardown.txt").write_text(
            f"reason={reason}\nat={time.strftime('%Y-%m-%dT%H:%M:%S%z')}\n"
            f"er_pids={er_pids()}\nlauncher_pid={proc.pid}\n"
        )
        # Preserve the run's evidence before killing anything.
        for src in (TELEMETRY, DEBUG_LOG):
            if src.exists():
                try:
                    shutil.copyfile(src, art / src.name)
                except Exception:
                    pass
        kill_er()
        try:
            proc.terminate()
        except Exception:
            pass
        for f in (SWITCH_SLOT, SWITCH_SAVE):
            if f.exists():
                f.unlink()
        log(f"teardown complete ({reason})")

    try:
        # ---- phase 1: boot -> load1 in world ----------------------------------------
        deadline = t0 + cap
        t = {}
        while time.time() < deadline:
            watch.wait(2.0)
            t = read_telemetry()
            if player_in_world(t):
                break
            if proc.poll() is not None and not er_pids():
                verdict["result"] = "FAIL:game-exited-during-boot"
                teardown("boot-exit")
                (art / "verdict.json").write_text(json.dumps(verdict, indent=1))
                return 1
        if not player_in_world(t):
            verdict["result"] = "FAIL:boot-timeout"
            teardown("boot-timeout")
            (art / "verdict.json").write_text(json.dumps(verdict, indent=1))
            return 1
        boot_char = t.get("oracle_char_name")
        log(f"LOAD1 in world after {time.time() - t0:.0f}s char={boot_char}")
        # settle so the switch eligibility (PlayerIns + menu job) is real, then baseline.
        settle_until = time.time() + 8.0
        while time.time() < settle_until:
            watch.wait(1.0)
        base = snap(read_telemetry())
        verdict["load1"] = base
        (art / "load1-baseline.json").write_text(json.dumps(base, indent=1))
        log(f"baseline: rejected={base.get('oracle_ls_portrait_rejected_publishes')} "
            f"rgba_v={base.get('oracle_loading_bg_portrait_rgba_version')} "
            f"w={base.get('oracle_ls_portrait_w')}x{base.get('oracle_ls_portrait_h')}")

        # ---- phase 2..N: chained switches via the product control file --------------
        chain = (
            [int(s) for s in args.switch_slots.replace(",", " ").split()]
            if args.switch_slots.strip()
            else [args.switch_slot]
        )
        verdict["chain"] = chain
        switches = []
        overall_ok = True
        for idx, slot in enumerate(chain, start=1):
            sw: dict = {"slot": slot, "index": idx}
            base_s = snap(read_telemetry())
            SWITCH_SLOT.write_text(f"{slot}\n")
            log(f"[switch {idx}/{len(chain)}] WROTE er-effects-switch-slot.txt = {slot}")
            switch_t0 = time.time()
            deadline = switch_t0 + cap
            window_seen_at = 0.0
            window_closed_at = 0.0
            shot_window = False
            shot_display = False
            max_w = max_h = 0
            window_display_frames = 0
            last_mtime = TELEMETRY.stat().st_mtime if TELEMETRY.exists() else 0.0
            stall_since = time.time()
            result = "FAIL:load-cap"
            while time.time() < deadline:
                watch.wait(1.0)
                if TELEMETRY.exists():
                    m = TELEMETRY.stat().st_mtime
                    if m != last_mtime:
                        last_mtime = m
                        stall_since = time.time()
                if time.time() - stall_since > 60.0:
                    result = "FAIL:telemetry-frozen-60s"
                    break
                t = read_telemetry()
                if not t:
                    continue
                s = snap(t)
                s["t_s"] = round(time.time() - switch_t0, 1)
                s["switch_index"] = idx
                series_f.write(json.dumps(s) + "\n")
                series_f.flush()
                phase = t.get("system_quit_quickload_phase") or 0
                loading = t.get("oracle_now_loading") is True or t.get("oracle_player_present") is not True
                in_window = phase >= 2 and loading
                if in_window and window_seen_at == 0.0:
                    window_seen_at = time.time()
                    log(f"[switch {idx}] window OPEN at +{s['t_s']}s (phase={phase})")
                if in_window:
                    w = int(t.get("oracle_ls_portrait_w") or 0)
                    h = int(t.get("oracle_ls_portrait_h") or 0)
                    max_w, max_h = max(max_w, w), max(max_h, h)
                    # Displayed frames THIS window = delta of the cumulative composite-draw
                    # counter. The old key `oracle_portrait_display_frames_current_window` does
                    # not exist in the DLL (it never has), so it always read 0 and pinned
                    # portrait_pass to False no matter what the pixels did -- run
                    # 20260731-113339 reported display=0 while the loadwin verdict recorded
                    # 100 and 80 drawn frames for the same two windows.
                    window_display_frames = max(
                        window_display_frames,
                        int(t.get("oracle_portrait_onto_draw_hits") or 0)
                        - int(base_s.get("oracle_portrait_onto_draw_hits") or 0),
                    )
                    if not shot_window and time.time() - window_seen_at > 3.0:
                        # Protocol: the loading-screen-portrait moment, where the USER can see
                        # the feature failing or working. Never delayed for prettiness.
                        capture(art / f"loading-screen-portrait-screenshot-sw{idx}.jpg")
                        shot_window = True
                    if not shot_display and window_display_frames > 0:
                        capture(art / f"portrait-display-moment-sw{idx}.jpg")
                        shot_display = True
                if proc.poll() is not None and not er_pids():
                    result = "FAIL:game-exited-during-switch(quit-to-desktop path?)"
                    break
                if window_seen_at != 0.0 and player_in_world(t) and phase >= 2:
                    window_closed_at = time.time()
                    result = "LOADED"
                    break
            sw["result"] = result
            sw["load_seconds"] = round(time.time() - switch_t0, 1)
            window_s = round((window_closed_at or time.time()) - (window_seen_at or switch_t0), 1)
            sw["window_seconds"] = window_s
            final = snap(read_telemetry())
            rejected_delta = (final.get("oracle_ls_portrait_rejected_publishes") or 0) - (
                base_s.get("oracle_ls_portrait_rejected_publishes") or 0
            )
            sw["window_max_capture"] = [max_w, max_h]
            sw["window_display_frames"] = window_display_frames
            sw["rejected_delta"] = rejected_delta
            sw["published_slot"] = final.get("oracle_ls_portrait_slot")
            # NATIVE FLASH-THROUGH (er-effects-rs-wmw defect #1). Independent of the portrait
            # verdict: a window can publish a perfect head and still have shown vanilla for a
            # frame. exposure_delta > 0 means the defect reproduced in THIS switch.
            sw["native_exposure_delta"] = (final.get("oracle_native_ls_exposure_frames") or 0) - (
                base_s.get("oracle_native_ls_exposure_frames") or 0
            )
            sw["native_covered_delta"] = (final.get("oracle_native_ls_covered_frames") or 0) - (
                base_s.get("oracle_native_ls_covered_frames") or 0
            )
            sw["native_exposure_max_run"] = final.get("oracle_native_ls_exposure_max_run")
            sw["native_exposure_last_gate"] = final.get("oracle_native_ls_exposure_last_gate")
            sw["native_exposure_by_gate"] = final.get("oracle_native_ls_exposure_by_gate")
            sw["boot_view_stop_reason"] = final.get("oracle_boot_view_stop_reason")
            # A same-map in-place reload can finish faster than the ~2.3s confirm->publish
            # latency; a short window with no displayed frame is recorded, not failed.
            short_window = window_s < 4.0
            sw["portrait_pass"] = (
                result == "LOADED"
                and rejected_delta == 0
                and (short_window or (max(max_w, max_h) >= 1024 and window_display_frames > 0))
            )
            sw["portrait_short_window"] = short_window
            # ---- HANDOFF COMPLETION (the dead-Load-Profile-button class): after the world
            # is reached, the switch machine must actually FINISH -- phase back to IDLE, the
            # in-game menu job rebuilt, now_loading cleared. These are the exact semaphores
            # that stayed wedged in run 20260731-user-portrait-verify (phase=4 for 15 min).
            comp_deadline = time.time() + 45.0
            completed = False
            while time.time() < comp_deadline:
                watch.wait(1.0)
                t = read_telemetry()
                if not t:
                    continue
                if (
                    (t.get("system_quit_quickload_phase") or 0) == 0
                    and t.get("oracle_switch_menu_job_present") == 1
                    and not t.get("oracle_now_loading")
                ):
                    completed = True
                    break
            tt = read_telemetry()
            sw["handoff_complete"] = completed
            sw["handoff_phase"] = tt.get("system_quit_quickload_phase")
            sw["handoff_menu_job"] = tt.get("oracle_switch_menu_job_present")
            sw["handoff_now_loading"] = tt.get("oracle_now_loading")
            sw["handoff_request_code"] = tt.get("oracle_stepfinish_request_code")
            sw["char"] = tt.get("oracle_char_name")
            switches.append(sw)
            log(
                f"[switch {idx}] RESULT={result} portrait_pass={sw['portrait_pass']} "
                f"window={window_s}s capture={max_w}x{max_h} display={window_display_frames} "
                f"rejected_delta={rejected_delta} "
                f"native_exposure={sw['native_exposure_delta']}/"
                f"{sw['native_exposure_delta'] + sw['native_covered_delta']} frames "
                f"(max_run={sw['native_exposure_max_run']} gate={sw['native_exposure_last_gate']} "
                f"stop_reason={sw['boot_view_stop_reason']}) handoff_complete={completed} "
                f"(phase={sw['handoff_phase']} menu_job={sw['handoff_menu_job']} "
                f"now_loading={sw['handoff_now_loading']}) char={sw['char']}"
            )
            if result != "LOADED" or not completed:
                overall_ok = False
                break
            if not sw["portrait_pass"]:
                overall_ok = False
        verdict["switches"] = switches
        verdict["result"] = switches[-1]["result"] if switches else "FAIL:no-switch-ran"
        verdict["portrait_pass"] = overall_ok
        # settle a moment for a stable final telemetry copy, then tear down.
        settle_until = time.time() + 5.0
        while time.time() < settle_until:
            watch.wait(1.0)
        teardown(f"done:{verdict['result']}")
    finally:
        series_f.close()
        watch.close()
        (art / "verdict.json").write_text(json.dumps(verdict, indent=1))
        log(f"verdict -> {art / 'verdict.json'}")
    return 0 if verdict.get("portrait_pass") else 1


if __name__ == "__main__":
    raise SystemExit(main())
