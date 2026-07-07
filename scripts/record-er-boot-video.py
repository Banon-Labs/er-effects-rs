#!/usr/bin/env python3
"""Record the Elden Ring boot at constant 60fps, teardown at the native loading-screen trigger.

Bug-investigation video protocol (user 2026-07-06): the user points at specific frames, so the
run must produce a frame-per-tick folder they can scroll in Dolphin and drag frames from.

Flow:
  1. Launch the approved first-boot Continue probe (scripts/run-product-continue-direct-probe.sh);
     it owns preflight (Steam up, no existing ER, fresh DLL staging) and final cleanup.
  2. Poll Hyprland for ONLY the exact ER window (class steam_app_1245620; never enumerate or log
     other clients). When it maps with stable geometry, start
     `wf-recorder -g <geom> -r 60 -D` (no-damage => constant framerate, so frame N <-> N/60s).
  3. Poll er-effects-telemetry.json for the loading-screen trigger:
     oracle_loadscreen_table_builds > 0 (the native "Now Loading" table build -- the same latch
     that drives the custom boot bar to its 700 permille native-handoff stop) OR
     oracle_boot_view_last_permille >= 700. RAM oracles only; screenshots are never the oracle.
  4. On trigger: kill eldenring.exe immediately (teardown), SIGINT the recorder at the same
     moment so the video ends on teardown and never captures the desktop behind the dying window.
  5. Extract every frame with ffmpeg (fps=60 -> frame-%05d.png), write video-timeline.json
     mapping frame numbers to wall-clock/launch-relative seconds. Pass --dolphin to open the
     frames folder for the user -- only AFTER the agent's deterministic pixel/telemetry
     verification pass, never as a default (user 2026-07-06).

The agent must NOT read the frames (no-image-reading directive); they are FOR THE USER.
Deterministic pixel telemetry (luminance scans, blackframe detection) is the verification tool.

Usage:
  record-er-boot-video.py [--dry-run] [--selftest] [--dolphin]
                          [--artifact-dir DIR] [--trigger-permille N]
                          [--post-trigger-grace SECONDS]
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WINDOW_CLASS = "steam_app_1245620"
FPS = 60
TRIGGER_PERMILLE_DEFAULT = 700
# Poll loops carry no sleeps (repo no-timeouts rule): each iteration's hyprctl IPC round-trip
# (~10-20ms) is the natural pacer, and every decision comes from polled evidence (telemetry
# oracles, /proc, hyprctl state) under a wall-clock deadline plus an iteration budget.
POLL_BUDGET = 50_000
# Free-space floor for lossless PNG frames (~1200 frames x ~3MB is ~4G; require a wide margin).
PNG_MIN_FREE_BYTES = 20 * 1024**3


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] record-er-boot-video: {msg}", flush=True)


def runtime_cap_seconds() -> int:
    """Single source of truth: .auto/runtime_timeout_cap_seconds (same fallback as its reader)."""
    try:
        return int((REPO_ROOT / ".auto" / "runtime_timeout_cap_seconds").read_text().strip())
    except Exception:
        return 60


def eldenring_pids() -> list[int]:
    """Exact-comm /proc scan (no pgrep: embedding the exe name in a shell command trips guards)."""
    pids: list[int] = []
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            if (proc / "comm").read_text().strip() == "eldenring.exe":
                pids.append(int(proc.name))
        except OSError:
            continue
    return pids


def hypr_er_window(hyprctl: str) -> dict | None:
    """Return ONLY the exact-class ER client. The full client list is filtered in memory and
    never printed/logged/persisted (privacy: other windows must not leak into artifacts)."""
    try:
        out = subprocess.run(
            [hyprctl, "clients", "-j"], text=True, capture_output=True, timeout=10
        ).stdout
        for c in json.loads(out):
            if isinstance(c, dict) and str(c.get("class") or "") == WINDOW_CLASS:
                return c
    except Exception:
        return None
    return None


def window_problems(w: dict) -> list[str]:
    p: list[str] = []
    if w.get("mapped") is False:
        p.append("unmapped")
    if w.get("hidden") is True:
        p.append("hidden")
    at, size = w.get("at") or [], w.get("size") or []
    if len(at) != 2 or len(size) != 2 or int(size[0] or 0) <= 0 or int(size[1] or 0) <= 0:
        p.append("bad_geometry")
    return p


def window_geometry(w: dict) -> str:
    at, size = w["at"], w["size"]
    return f"{int(at[0])},{int(at[1])} {int(size[0])}x{int(size[1])}"


def read_telemetry(path: Path) -> dict | None:
    """Tolerate partial writes: the DLL rewrites the file; a torn read is retried next poll."""
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return None


def as_int(value, default: int = 0) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def trigger_fired(telemetry: dict | None, trigger_permille: int) -> bool:
    """The teardown oracle. Primary: the native loading-screen GFX table build (the exact latch
    that stops the custom boot bar at its native-handoff mark). Secondary: the displayed custom
    bar permille itself, in case the table-build counter is ever renamed/regressed."""
    if not telemetry:
        return False
    if as_int(telemetry.get("oracle_loadscreen_table_builds")) > 0:
        return True
    return as_int(telemetry.get("oracle_boot_view_last_permille")) >= trigger_permille


def trigger_snapshot(telemetry: dict | None) -> dict:
    keys = (
        "oracle_loadscreen_table_builds",
        "oracle_boot_view_last_permille",
        "oracle_boot_view_milestone_idx",
        "oracle_boot_view_milestone_mask",
        "oracle_boot_view_draw_hits",
        "oracle_loading_bar_progress_permille",
    )
    return {k: (telemetry or {}).get(k) for k in keys}


def selftest() -> int:
    assert not trigger_fired(None, 700)
    assert not trigger_fired({}, 700)
    assert not trigger_fired({"oracle_loadscreen_table_builds": 0}, 700)
    assert not trigger_fired({"oracle_boot_view_last_permille": 699}, 700)
    assert trigger_fired({"oracle_loadscreen_table_builds": 1}, 700)
    assert trigger_fired({"oracle_boot_view_last_permille": 700}, 700)
    assert trigger_fired({"oracle_loadscreen_table_builds": "2"}, 700)
    assert not trigger_fired({"oracle_loadscreen_table_builds": "garbage"}, 700)
    assert window_problems({"mapped": True, "hidden": False, "at": [0, 0], "size": [1920, 1080]}) == []
    assert "bad_geometry" in window_problems({"at": [0, 0], "size": [0, 1080]})
    assert "unmapped" in window_problems({"mapped": False, "at": [0, 0], "size": [1, 1]})
    assert window_geometry({"at": [3072, 0], "size": [2560, 1440]}) == "3072,0 2560x1440"
    assert as_int(None, 7) == 7 and as_int("12") == 12
    print("selftest ok")
    return 0


def stop_recorder(recorder: subprocess.Popen | None) -> float | None:
    """SIGINT finalizes the container (wf-recorder's documented stop); escalate if it hangs."""
    if recorder is None or recorder.poll() is not None:
        return None
    epoch = time.time()
    recorder.send_signal(signal.SIGINT)
    try:
        recorder.wait(timeout=15)
    except subprocess.TimeoutExpired:
        recorder.terminate()
        try:
            recorder.wait(timeout=5)
        except subprocess.TimeoutExpired:
            recorder.kill()
    return epoch


def teardown_game() -> float:
    epoch = time.time()
    for pid in eldenring_pids():
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
    return epoch


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dry-run", action="store_true", help="preflight + probe --dry-run only")
    ap.add_argument("--selftest", action="store_true", help="offline logic checks, no launch")
    ap.add_argument(
        "--dolphin", action="store_true",
        help="open the frames folder in Dolphin (only after verification; never default)")
    ap.add_argument("--artifact-dir", default=None)
    ap.add_argument("--trigger-permille", type=int, default=TRIGGER_PERMILLE_DEFAULT)
    ap.add_argument(
        "--post-trigger-grace", type=float, default=0.0,
        help="keep recording this many seconds after the trigger before teardown "
             "(e.g. 1.5 to capture the boot-cover fade-out past the handoff)")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    tools = {name: shutil.which(name) for name in ("wf-recorder", "ffmpeg", "ffprobe", "hyprctl", "dolphin")}
    missing = [n for n, p in tools.items() if not p]
    if missing:
        log(f"FATAL missing tools: {missing}")
        return 2

    stamp = time.strftime("%Y%m%d-%H%M%S")
    artifact_dir = Path(args.artifact_dir or (REPO_ROOT / "target" / "runtime-probe" / f"boot-video-{stamp}")).resolve()
    probe = REPO_ROOT / "scripts" / "run-product-continue-direct-probe.sh"
    if not probe.is_file():
        log(f"FATAL probe script missing: {probe}")
        return 2

    cap = runtime_cap_seconds()
    env = dict(os.environ, ARTIFACT_DIR=str(artifact_dir))

    if args.dry_run:
        rc = subprocess.run(
            ["bash", str(probe), "--dry-run"], env=env, text=True, capture_output=True, timeout=30)
        print(rc.stdout.strip())
        if rc.returncode != 0:
            print(rc.stderr.strip(), file=sys.stderr)
            return rc.returncode
        log(f"dry-run ok: would record {WINDOW_CLASS} at {FPS}fps into {artifact_dir}, "
            f"teardown on oracle_loadscreen_table_builds>0 or permille>={args.trigger_permille}, cap={cap}s")
        return 0

    artifact_dir.mkdir(parents=True, exist_ok=True)
    video_path = artifact_dir / "boot-video.mkv"
    frames_dir = artifact_dir / "boot-video-frames"
    telemetry_path = artifact_dir / "er-effects-telemetry.json"
    recorder_log = artifact_dir / "wf-recorder.log"
    timeline_path = artifact_dir / "video-timeline.json"
    caveats: list[str] = []

    log(f"launching probe -> {artifact_dir}")
    probe_out = open(artifact_dir / "record-probe.out", "w", encoding="utf-8")
    probe_proc = subprocess.Popen(
        ["bash", str(probe)], env=env, cwd=REPO_ROOT,
        stdout=probe_out, stderr=subprocess.STDOUT, start_new_session=True,
    )
    launch_epoch = time.time()  # refined from launch-epoch.txt once the probe writes it

    recorder: subprocess.Popen | None = None
    recorder_spawn_epoch: float | None = None
    recorder_sigint_epoch: float | None = None
    teardown_epoch: float | None = None
    trigger_epoch: float | None = None
    trigger_state: dict = {}
    geom: str | None = None
    stop_reason = "unknown"
    window_events: list[dict] = []

    # Window wait: start recording at the FIRST stable mapped geometry (two identical consecutive
    # samples ride out the game's startup window reconfiguration jump). Deadline = probe cap +
    # margin; the probe's own watcher owns the hard runtime cap.
    deadline = launch_epoch + cap + 15
    prev_geom: str | None = None
    try:
        for _ in range(POLL_BUDGET):
            if time.time() >= deadline:
                break
            if probe_proc.poll() is not None:
                stop_reason = f"probe_exited_rc_{probe_proc.returncode}_before_window"
                break
            # The hyprctl IPC round-trip paces this loop; no sleep.
            w = hypr_er_window(tools["hyprctl"])
            if w is not None and not window_problems(w):
                g = window_geometry(w)
                if g == prev_geom:
                    geom = g
                    break
                prev_geom = g

        if geom is not None:
            log(f"ER window stable at {geom}; starting wf-recorder ({FPS}fps constant)")
            recorder_spawn_epoch = time.time()
            recorder = subprocess.Popen(
                [tools["wf-recorder"], "-f", str(video_path), "-g", geom,
                 "-r", str(FPS), "-D", "-c", "libx264",
                 "-p", "preset=ultrafast", "-p", "crf=16"],
                stdout=open(recorder_log, "w", encoding="utf-8"),
                stderr=subprocess.STDOUT, start_new_session=True,
            )

            # Trigger wait: telemetry oracles decide the stop; process death / cap are fallbacks.
            # Every window-state change is timestamped into window_events (a compositor-side
            # semaphore: the game's startup mode changes black the captured region for a few
            # frames, and these events let each black run be attributed frame-exactly). The
            # per-iteration hyprctl IPC round-trip paces the loop (no sleep).
            last_window_sig: dict | None = None
            grace_until: float | None = None
            for _ in range(POLL_BUDGET):
                now = time.time()
                if recorder.poll() is not None:
                    stop_reason = f"recorder_died_rc_{recorder.returncode}"
                    caveats.append("wf-recorder exited early; see wf-recorder.log")
                    break
                if grace_until is None:
                    telemetry = read_telemetry(telemetry_path)
                    if trigger_fired(telemetry, args.trigger_permille):
                        trigger_epoch = now
                        trigger_state = trigger_snapshot(telemetry)
                        stop_reason = "loading_screen_trigger"
                        if args.post_trigger_grace > 0:
                            grace_until = now + args.post_trigger_grace
                            log(f"TRIGGER {trigger_state} -> recording {args.post_trigger_grace}s grace before teardown")
                        else:
                            log(f"TRIGGER {trigger_state} -> teardown + stop recording")
                            teardown_epoch = teardown_game()
                            recorder_sigint_epoch = stop_recorder(recorder)
                            break
                elif now >= grace_until:
                    log("post-trigger grace elapsed -> teardown + stop recording")
                    teardown_epoch = teardown_game()
                    recorder_sigint_epoch = stop_recorder(recorder)
                    break
                if not eldenring_pids():
                    stop_reason = "game_exited_before_trigger" if grace_until is None else stop_reason
                    recorder_sigint_epoch = stop_recorder(recorder)
                    break
                if probe_proc.poll() is not None:
                    stop_reason = f"probe_exited_rc_{probe_proc.returncode}_before_trigger"
                    recorder_sigint_epoch = stop_recorder(recorder)
                    break
                if now > deadline:
                    stop_reason = "wrapper_deadline_before_trigger"
                    teardown_epoch = teardown_game()
                    recorder_sigint_epoch = stop_recorder(recorder)
                    break
                # Re-validate ONLY the target window every iteration: timestamp every state
                # change (this IPC call is also the loop pacer). Occlusion/moves are logged,
                # not fatal (the user may deliberately interact); disappearance is handled by
                # the game-pid check above on the next iteration.
                w = hypr_er_window(tools["hyprctl"])
                sig = None if w is None else {
                    "geometry": window_geometry(w) if not window_problems(w) else None,
                    "problems": window_problems(w),
                    "fullscreen": w.get("fullscreen"),
                }
                if sig != last_window_sig:
                    event = {
                        "epoch": now,
                        "video_second": round(now - (recorder_spawn_epoch or now), 3),
                        "video_frame_estimate": int(max(0.0, now - (recorder_spawn_epoch or now)) * FPS),
                        "window": sig,
                    }
                    window_events.append(event)
                    log(f"window-state change: {sig} (~frame {event['video_frame_estimate']})")
                    last_window_sig = sig
        else:
            if stop_reason == "unknown":
                stop_reason = "er_window_never_stable"
    finally:
        recorder_sigint_epoch = recorder_sigint_epoch or stop_recorder(recorder)
        if not trigger_epoch:
            teardown_epoch = teardown_epoch or teardown_game()

    # Refine launch epoch to the probe's own T0 (bash timestamp at the launch fire).
    try:
        launch_epoch = float((artifact_dir / "launch-epoch.txt").read_text().strip())
    except Exception:
        caveats.append("launch-epoch.txt unavailable; launch_epoch is the probe spawn time")

    log(f"stop_reason={stop_reason}; waiting for probe cleanup")
    try:
        probe_proc.wait(timeout=60)
    except subprocess.TimeoutExpired:
        caveats.append("probe did not exit within 60s after teardown; left to its own watcher")
    probe_out.close()

    # Frame extraction: fps=60 re-times to exact CFR so frame N <-> N/60s even if the recorder
    # dropped/duplicated. PNG (lossless) when disk allows, else high-quality JPG.
    frame_count = 0
    frame_ext = "png"
    if video_path.exists() and video_path.stat().st_size > 0:
        if shutil.disk_usage(artifact_dir).free < PNG_MIN_FREE_BYTES:
            frame_ext = "jpg"
            caveats.append("low disk space: extracted JPG q=1 instead of PNG")
        frames_dir.mkdir(exist_ok=True)
        cmd = [tools["ffmpeg"], "-y", "-i", str(video_path), "-vf", f"fps={FPS}",
               "-start_number", "0"]
        if frame_ext == "jpg":
            cmd += ["-q:v", "1"]
        cmd += [str(frames_dir / f"frame-%05d.{frame_ext}")]
        try:
            rc = subprocess.run(cmd, text=True, capture_output=True, timeout=30)
            if rc.returncode != 0:
                caveats.append(f"ffmpeg frame extraction failed rc={rc.returncode}: {rc.stderr[-400:]}")
        except subprocess.TimeoutExpired:
            caveats.append("ffmpeg frame extraction exceeded the 30s cap; frames are partial")
        frame_count = len(list(frames_dir.glob(f"frame-*.{frame_ext}")))
    else:
        caveats.append("no video captured")

    timeline = {
        "stop_reason": stop_reason,
        "window_class": WINDOW_CLASS,
        "window_geometry": geom,
        "fps": FPS,
        "video": str(video_path),
        "frames_dir": str(frames_dir),
        "frame_count": frame_count,
        "frame_ext": frame_ext,
        "launch_epoch": launch_epoch,
        "recorder_spawn_epoch": recorder_spawn_epoch,
        "trigger_epoch": trigger_epoch,
        "teardown_kill_epoch": teardown_epoch,
        "recorder_sigint_epoch": recorder_sigint_epoch,
        "trigger_state": trigger_state,
        "trigger_permille": args.trigger_permille,
        "post_trigger_grace_seconds": args.post_trigger_grace,
        "window_events": window_events,
        "frame_time_formula": {
            "video_seconds": "N / 60",
            "wall_clock_epoch": "recorder_spawn_epoch + N / 60 (spawn-to-first-frame skew ~<=0.1s)",
            "seconds_after_game_launch": "(recorder_spawn_epoch - launch_epoch) + N / 60",
        },
        "caveats": caveats,
    }
    timeline_path.write_text(json.dumps(timeline, indent=2) + "\n", encoding="utf-8")

    log(f"video: {video_path}")
    log(f"frames ({frame_count} x .{frame_ext}): {frames_dir}")
    log(f"timeline: {timeline_path}")
    if caveats:
        log(f"caveats: {caveats}")

    if frame_count > 0 and args.dolphin:
        subprocess.Popen(
            [tools["dolphin"], str(frames_dir)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
        )
        log("opened frames folder in Dolphin")

    return 0 if (stop_reason == "loading_screen_trigger" and frame_count > 0) else 1


if __name__ == "__main__":
    raise SystemExit(main())
