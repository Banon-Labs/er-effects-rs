#!/usr/bin/env python3
"""Record the validated Elden Ring window for a human-driven session, until the game exits.

Companion to record-er-window-wf.py (whose window-validation logic is reused via import):
- waits for the stable, focused, sane-geometry class=steam_app_1245620 window (fail-closed);
- records that exact geometry with wf-recorder at the requested fps (constant framerate, -D);
- stops the recording the moment no eldenring.exe process exists in /proc (the human quit),
  or at --max-seconds as a hard cap (the game is NEVER touched; only the recording stops);
- then extracts every frame (native rate, -vsync 0) to <out_dir>/frames/ as JPEG q2
  (a multi-minute 60fps PNG dump would be tens of GB; the .mkv is kept for lossless re-extract);
- writes <out_dir>/drive-recording-result.json with the full outcome.

This is a diagnostic/evidence recorder only: it owns no game teardown and must not be used
as a run-stopping oracle.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("record_er_window_wf", HERE / "record-er-window-wf.py")
assert _spec and _spec.loader
_wf_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_wf_mod)


def eldenring_running() -> bool:
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            with open(f"/proc/{pid}/comm") as f:
                if f.read().strip() == "eldenring.exe":
                    return True
        except OSError:
            continue
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out_dir", type=Path)
    parser.add_argument("--fps", type=float, default=60.0)
    parser.add_argument("--window-timeout", type=float, default=240.0)
    parser.add_argument("--max-seconds", type=float, default=3600.0,
                        help="hard recording cap; the game itself is never terminated")
    parser.add_argument("--stop-grace", type=float, default=20.0,
                        help="seconds to let wf-recorder finalize after SIGINT")
    args = parser.parse_args()

    wf = shutil.which("wf-recorder")
    ffmpeg = shutil.which("ffmpeg")
    if not wf or not ffmpeg:
        raise SystemExit("missing wf-recorder or ffmpeg")
    args.out_dir.mkdir(parents=True, exist_ok=True)

    w, proof = _wf_mod.wait_for_stable_window(
        timeout_s=args.window_timeout, samples=8, interval_s=0.15, require_focus=True
    )
    at = list(map(int, w["at"]))
    size = list(map(int, w["size"]))
    geom = f"{at[0]},{at[1]} {size[0]}x{size[1]}"
    video = args.out_dir / f"wf-{args.fps:g}fps.mkv"
    (args.out_dir / "drive-recording-request.json").write_text(json.dumps({
        "window": _wf_mod.summarize(w),
        "stability_proof": proof,
        "geometry": geom,
        "fps": args.fps,
        "max_seconds": args.max_seconds,
        "output": str(video),
    }, indent=2))

    # preset=ultrafast keeps 60fps 1440p-class encoding comfortably real-time on CPU;
    # stdout/stderr go to files, never pipes (a filled pipe would stall wf-recorder mid-drive).
    cmd = [wf, "-g", geom, "-r", f"{args.fps:g}", "-D", "-p", "preset=ultrafast", "-f", str(video)]
    with open(args.out_dir / "wf-recorder.out", "w") as out_f, \
         open(args.out_dir / "wf-recorder.err", "w") as err_f:
        proc = subprocess.Popen(cmd, stdout=out_f, stderr=err_f)
        start = time.time()
        stop_reason = "unknown"
        while True:
            time.sleep(0.5)
            if proc.poll() is not None:
                stop_reason = "recorder_died"
                break
            if not eldenring_running():
                stop_reason = "game_exited"
                break
            if time.time() - start >= args.max_seconds:
                stop_reason = "max_seconds_cap"
                break
        if proc.poll() is None:
            proc.send_signal(signal.SIGINT)
            try:
                proc.wait(args.stop_grace)
            except subprocess.TimeoutExpired:
                proc.terminate()
                try:
                    proc.wait(5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
    recorded_seconds = round(time.time() - start, 3)

    frames_dir = args.out_dir / "frames"
    frame_count = 0
    extract_rc: int | None = None
    if video.exists() and video.stat().st_size > 0:
        if frames_dir.exists():
            shutil.rmtree(frames_dir)
        frames_dir.mkdir(parents=True)
        extract = subprocess.run(
            [ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", str(video),
             "-vsync", "0", "-qscale:v", "2", str(frames_dir / "frame-%06d.jpg")],
            text=True, capture_output=True,
        )
        extract_rc = extract.returncode
        if extract.stderr:
            (args.out_dir / "ffmpeg-extract.err").write_text(extract.stderr)
        frame_count = sum(1 for p in frames_dir.iterdir() if p.name.startswith("frame-"))

    result = {
        "stop_reason": stop_reason,
        "recorded_seconds": recorded_seconds,
        "wf_recorder_returncode": proc.returncode,
        "video": str(video),
        "video_bytes": video.stat().st_size if video.exists() else 0,
        "frames_dir": str(frames_dir),
        "frame_count": frame_count,
        "extract_returncode": extract_rc,
        "geometry": geom,
        "window": _wf_mod.summarize(w),
    }
    (args.out_dir / "drive-recording-result.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result))
    ok = result["video_bytes"] > 0 and frame_count > 0 and stop_reason in ("game_exited", "max_seconds_cap")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
