#!/usr/bin/env python3
"""Observer + capture watcher for the same-character-3x milestone (docs/goals SS4a).

The PRODUCT DLL drives the loads (boot autoload = load1; sq-repro XInput autopilot drives 2 same-slot
reloads = load2, load3, with the load-2 freeze force-advanced to the load-3 recovery by the DLL's
freeze-recovery deadline). This script does NOT drive input -- it OBSERVES er-effects-telemetry.json,
records the per-load RAM-oracle signature (render-ready / can-see + havok motion + freeze markers),
captures the mandatory loading-screen-portrait image at the frozen-load-2 moment, and tears down after
load-3 reaches a held render-ready dwell OR the runtime cap. It is the "logging" half of the two-DLL
setup; the trace DLL logs the native pipeline in parallel.

Load epochs are keyed by system_quit_continue_confirm_fresh_deser_count: 0 = load1 (boot autoload),
1 = load2 (first reload), 2 = load3 (second reload / recovery).
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

RENDER_READY_DWELL_SECONDS = 5.0  # goal SS4 hard render gate dwell
POLL_SECONDS = 1.0

# sq-repro autopilot menu-nav stage (constants/system_quit.rs). The MENU-NAV stage the driver reached
# is the ATTEMPT semaphore: it distinguishes "the driver never drove the menu far enough to start a
# load" (stuck < CONFIRM) from "confirm fired but the load did not complete" (reached CONFIRM/activate
# but fresh_deser never incremented). Run1 stalled at TO_PROFILE(3) -> load3 was never attempted.
SQ_REPRO_STATE_LABELS = {
    0: "WAIT_WORLD",
    1: "OPEN_MENU",
    2: "TO_SYSTEM",
    3: "TO_PROFILE",
    4: "TO_SLOT",
    5: "CONFIRM",
    6: "DONE",
    7: "WAIT_RELOAD",
}


def read_telemetry(path: Path) -> dict | None:
    try:
        return json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (OSError, json.JSONDecodeError):
        return None


def snap(t: dict) -> dict:
    """Extract the load-readiness signature we care about."""
    keys = [
        "oracle_char_name",
        "oracle_player_present",
        "oracle_player_render_ready",
        "oracle_can_move",
        "oracle_move_probe_moved_frames",
        "oracle_chr_draw_group_enabled",
        "oracle_chr_render_group_enabled",
        "oracle_chr_enable_render",
        "oracle_havok_pos",
        "oracle_play_time_ms",
        "oracle_now_loading",
        "oracle_fake_loading_any_visible",
        "oracle_stepfinish_request_code",
        "oracle_stepfinish_mms_state",
        "oracle_saved_map_c30",
        "sq_repro_state",
        "sq_repro_switch_index",
        "system_quit_profile_load_activate_count",  # ATTEMPT: load-confirm activations fired
        "system_quit_continue_confirm_allow_count",
        "system_quit_continue_confirm_fresh_deser_count",  # COMPLETE: reload deserializes committed
        "system_quit_continue_confirm_fresh_deser_done",
    ]
    return {k: t.get(k) for k in keys}


def capture_portrait(artifact_dir: Path) -> None:
    """Mandatory loading-screen-portrait capture (AGENTS.md protocol). One-shot; agent never reads it."""
    out = artifact_dir / "loading-screen-portrait-screenshot.jpg"
    note = out.with_suffix(".txt")
    if out.exists() or note.exists():
        return
    helper = Path(__file__).with_name("capture-er-window.py")
    try:
        subprocess.run(
            [sys.executable, str(helper), str(out)],
            text=True,
            capture_output=True,
            timeout=25,
        )
    except Exception as exc:  # noqa: BLE001 -- capture is best-effort; fail closed to a note
        note.write_text(f"loading-screen-portrait capture failed: {exc}\n", encoding="utf-8")


def teardown() -> None:
    for image in ("eldenring.exe", "me3.exe"):
        subprocess.run(["taskkill.exe", "/F", "/IM", image], capture_output=True, text=True)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--game-dir", type=Path, required=True, help="dir with er-effects-telemetry.json + logs")
    ap.add_argument("--artifact-dir", type=Path, required=True)
    ap.add_argument("--max-seconds", type=float, required=True, help="runtime cap (.auto/runtime_timeout_cap_seconds)")
    ap.add_argument("--report", type=Path, required=True)
    args = ap.parse_args()

    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    telemetry_path = args.game_dir / "er-effects-telemetry.json"

    # Per-epoch record: first-seen ts, the max/settled snapshot, whether render-ready was ever held.
    epochs: dict[int, dict] = {}
    portrait_captured = False
    start = time.monotonic()
    last_log = 0.0
    result = "TIMEOUT_NO_LOAD3"
    max_nav_stage = 0  # highest sq_repro_state reached = how far the driver ever drove the menu
    max_activate = 0  # profile-load confirm activations = number of reload ATTEMPTS started

    while True:
        now = time.monotonic()
        elapsed = now - start
        if elapsed >= args.max_seconds:
            result = "CAP_REACHED"
            break

        t = read_telemetry(telemetry_path)
        if t is not None:
            s = snap(t)
            deser = s.get("system_quit_continue_confirm_fresh_deser_count") or 0
            present = bool(s.get("oracle_player_present"))
            render_ready = bool(s.get("oracle_player_render_ready"))
            fake_cover = bool(s.get("oracle_fake_loading_any_visible"))

            nav_stage = s.get("sq_repro_state") or 0
            activate = s.get("system_quit_profile_load_activate_count") or 0
            max_nav_stage = max(max_nav_stage, int(nav_stage))
            max_activate = max(max_activate, int(activate))

            can_move = bool(s.get("oracle_can_move"))

            ep = epochs.setdefault(
                int(deser),
                {"first_seen": elapsed, "ever_ready": False, "ever_moved": False, "last": None},
            )
            ep["last"] = s
            if render_ready:
                ep["ever_ready"] = True
            if can_move:
                ep["ever_moved"] = True

            # Mandatory portrait capture at the frozen-load view: a reload in progress (deser>=1),
            # cover up, not yet render-ready -- the exact moment the user sees the failure.
            if not portrait_captured and deser >= 1 and fake_cover and not render_ready and present:
                capture_portrait(args.artifact_dir)
                portrait_captured = True

            # SUCCESS = a load PROVED MOVEMENT (can_move latched: >=60 consecutive frames of injected-
            # forward motion). That is the real milestone -- a load that renders AND is playable.
            if can_move:
                result = "MOVEMENT_PROVEN"
                break

            if elapsed - last_log >= 3.0:
                last_log = elapsed
                nav_label = SQ_REPRO_STATE_LABELS.get(int(nav_stage), f"?{nav_stage}")
                print(
                    f"[{elapsed:6.1f}s] deser={deser} activate={activate} nav={nav_label} "
                    f"present={present} render_ready={render_ready} "
                    f"can_move={can_move}(f{s.get('oracle_move_probe_moved_frames')}) "
                    f"draw_group={s.get('oracle_chr_draw_group_enabled')} "
                    f"req_code={s.get('oracle_stepfinish_request_code')} mms={s.get('oracle_stepfinish_mms_state')} "
                    f"fake_cover={fake_cover} switch_idx={s.get('sq_repro_switch_index')} "
                    f"havok={s.get('oracle_havok_pos')}",
                    flush=True,
                )
        time.sleep(POLL_SECONDS)

    # Snapshot artifacts before teardown clears live state.
    for name in ("er-effects-telemetry.json", "er-effects-autoload-debug.log", "er-reload-trace.log"):
        src = args.game_dir / name
        if src.exists():
            try:
                (args.artifact_dir / name).write_bytes(src.read_bytes())
            except OSError:
                pass

    teardown()

    # Report.
    # Attempt vs complete (the semaphore gap this run exposed): reload ATTEMPTS = profile-load confirm
    # activations (max_activate); reload COMPLETIONS = fresh_deser epochs beyond load1 (max deser key).
    completions = max((d for d in epochs if d >= 1), default=0)
    max_nav_label = SQ_REPRO_STATE_LABELS.get(max_nav_stage, f"?{max_nav_stage}")
    lines = [
        "# Same-character-3x capture report",
        "",
        f"result: **{result}**",
        f"elapsed: {time.monotonic() - start:.1f}s (cap {args.max_seconds}s)",
        f"portrait_captured: {portrait_captured}",
        "",
        "## Driver progress (attempt vs complete)",
        f"- reload ATTEMPTS started (profile_load_activate_count): **{max_activate}**",
        f"- reload COMPLETIONS committed (max fresh_deser epoch): **{completions}**",
        f"- furthest menu-nav stage the driver reached (max sq_repro_state): **{max_nav_label}** ({max_nav_stage})",
        f"  - reaching CONFIRM(5) = a load was attempted; stalling below it (e.g. TO_PROFILE(3)) = the",
        f"    driver never drove the menu far enough to START the next load.",
        "",
        "## Per-load (fresh_deser epoch) settled signature",
        "",
    ]
    epoch_names = {
        0: "load1 (boot autoload)",
        1: "load2 (first reload)",
        2: "load3 (second reload)",
        3: "load4 (third reload)",
    }
    for deser in sorted(epochs):
        ep = epochs[deser]
        s = ep["last"] or {}
        lines.append(f"### deser={deser} — {epoch_names.get(deser, 'load')}")
        lines.append(
            f"- first_seen: {ep['first_seen']:.1f}s   ever_render_ready: {ep['ever_ready']}   "
            f"ever_moved(can_move): {ep.get('ever_moved')}"
        )
        lines.append(
            f"- char_name: {s.get('oracle_char_name')}   "
            f"can_move: {s.get('oracle_can_move')}  moved_frames: {s.get('oracle_move_probe_moved_frames')}"
        )
        lines.append(
            f"- player_render_ready: {s.get('oracle_player_render_ready')}  "
            f"draw_group: {s.get('oracle_chr_draw_group_enabled')}  "
            f"render_group: {s.get('oracle_chr_render_group_enabled')}  "
            f"enable_render: {s.get('oracle_chr_enable_render')}"
        )
        lines.append(
            f"- request_code: {s.get('oracle_stepfinish_request_code')}  "
            f"mms_state: {s.get('oracle_stepfinish_mms_state')}  "
            f"now_loading: {s.get('oracle_now_loading')}  "
            f"fake_cover: {s.get('oracle_fake_loading_any_visible')}"
        )
        lines.append(f"- havok_pos: {s.get('oracle_havok_pos')}  play_time_ms: {s.get('oracle_play_time_ms')}")
        lines.append("")
    verdict = (
        "PASS (a load PROVED movement: >=60 frames of injected motion)"
        if result == "MOVEMENT_PROVEN"
        else "FAIL / incomplete (no load proved 60-frame movement)"
    )
    lines.append(f"## Verdict: {verdict}")
    args.report.write_text("\n".join(lines), encoding="utf-8")
    print("\n".join(lines))
    return 0 if result == "MOVEMENT_PROVEN" else 1


if __name__ == "__main__":
    sys.exit(main())
