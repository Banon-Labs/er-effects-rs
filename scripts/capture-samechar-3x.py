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
import contextlib
import json
import subprocess
import sys
import threading
import time
from pathlib import Path

RENDER_READY_DWELL_SECONDS = 5.0  # goal SS4 hard render gate dwell
POLL_SECONDS = 1.0
TARGET_FINAL_EPOCH = 3  # 4 loads total = fresh_deser 0..3
FINAL_LOAD_DWELL_SECONDS = (
    14.0  # after the 4th load appears, give the 60-frame move-probe time to run
)
BOOT_TIMEOUT_SECONDS = (
    110.0  # if no in-world player by here, the boot failed -> tear down, don't idle
)
LOADING_PROGRESS_STALL_SECONDS = 10.0

# Interruptible poll wait (never set) -- the sanctioned no-bare-sleep pace primitive (see
# scripts/multi-load-proof-monitor.py); real synchronization is the telemetry-file readiness checks.
_POLL_WAIT = threading.Event()

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


def as_int(value: object, default: int = -1) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, (float, str, bytes, bytearray)):
        try:
            return int(value)
        except (OverflowError, ValueError):
            return default
    return default


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
        "oracle_fake_loading_field_c",
        "oracle_fake_loading_field_10",
        "oracle_loading_bar_enabled",
        "oracle_loading_screen_last_this",
        "oracle_loading_screen_last_data",
        "oracle_loading_bar_current_frame",
        "oracle_loading_bar_max_frame",
        "oracle_loading_bar_progress_permille",
        "oracle_loading_bar_current_terminal",
        "oracle_loading_bar_final_hits",
        "oracle_loading_bar_update_hits",
        "oracle_loading_screen_close_sent",
        "oracle_loading_screen_close_sent_hits",
        "oracle_load_in_progress_b80",
        "oracle_stepfinish_request_code",
        "oracle_stepfinish_mms_state",
        "oracle_stepfinish_finalize_substate_12a",
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
        note.write_text(
            f"loading-screen-portrait capture failed: {exc}\n", encoding="utf-8"
        )


def teardown() -> None:
    for image in ("eldenring.exe", "me3.exe"):
        with contextlib.suppress(subprocess.TimeoutExpired):
            subprocess.run(
                ["taskkill.exe", "/F", "/IM", image],
                capture_output=True,
                text=True,
                timeout=15,
            )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--game-dir",
        type=Path,
        required=True,
        help="dir with er-effects-telemetry.json + logs",
    )
    ap.add_argument("--artifact-dir", type=Path, required=True)
    ap.add_argument(
        "--max-seconds",
        type=float,
        required=True,
        help="runtime cap (.auto/runtime_timeout_cap_seconds)",
    )
    ap.add_argument("--report", type=Path, required=True)
    ap.add_argument(
        "--require-reload-move",
        action="store_true",
        help="success requires a RELOAD (deser>=1) to prove movement, not just load1 (the full-sequence goal)",
    )
    ap.add_argument(
        "--require-reload-settled",
        action="store_true",
        help=(
            "success requires a RELOAD (deser>=1) to prove movement AND finish native MoveMap "
            "handoff (requestCode==2, MoveMapStep absent); catches load2 moving under stale mms18"
        ),
    )
    args = ap.parse_args()

    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    telemetry_path = args.game_dir / "er-effects-telemetry.json"

    # Per-epoch record: first-seen ts, the max/settled snapshot, whether render-ready was ever held.
    epochs: dict[int, dict] = {}
    portrait_captured = False
    first_present_at: float | None = None
    start = time.monotonic()
    last_log = 0.0
    result = "TIMEOUT_NO_LOAD3"
    max_nav_stage = (
        0  # highest sq_repro_state reached = how far the driver ever drove the menu
    )
    max_activate = (
        0  # profile-load confirm activations = number of reload ATTEMPTS started
    )
    last_loading_progress_signature: tuple | None = None
    last_loading_progress_at: float | None = None
    loading_stall_signature: tuple | None = None
    loading_stall_source: str | None = None
    loading_stall_seconds = 0.0
    last_bar_epoch: tuple[int, int] | None = None
    last_terminal_bar_fields: tuple | None = None

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
            if present and first_present_at is None:
                first_present_at = now

            # User-directed teardown guard (2026-07-19): if the loading bar stops making observable
            # progress for >10s, preserve artifacts and fail immediately instead of consuming the full
            # runtime cap. Primary signal is the explicit RAM loading-bar progress oracle plus the native
            # post-bar handoff semaphores, not a screenshot or broad cover-visible proxy. This deliberately
            # keeps progressing after the visible bar reaches its nominal final frame (e.g. 11/11 or
            # frame=max): if requestCode, MoveMapStep, close-sent, fake-loading, player presence, or
            # movement proof still change, the loading-progress signature changes too. The coarse native-load
            # signature is used only as a fallback when the loading-bar oracle is unavailable.
            request_code = as_int(s.get("oracle_stepfinish_request_code"))
            mms_state = as_int(s.get("oracle_stepfinish_mms_state"))
            bar_enabled = as_int(s.get("oracle_loading_bar_enabled"), 0) > 0
            bar_current_frame = as_int(s.get("oracle_loading_bar_current_frame"))
            bar_progress_permille = as_int(
                s.get("oracle_loading_bar_progress_permille")
            )
            bar_max_frame = as_int(s.get("oracle_loading_bar_max_frame"))
            bar_progress_available = bar_enabled and (
                bar_current_frame >= 0 or bar_progress_permille >= 0
            )
            current_epoch = (as_int(deser, 0), as_int(activate, 0))
            handoff_progress_fields = (
                ("close_sent", as_int(s.get("oracle_loading_screen_close_sent"), 0)),
                ("request_code", request_code),
                ("mms_state", mms_state),
                (
                    "finalize12a",
                    as_int(s.get("oracle_stepfinish_finalize_substate_12a")),
                ),
                ("load_in_progress_b80", as_int(s.get("oracle_load_in_progress_b80"))),
                ("now_loading", as_int(s.get("oracle_now_loading"))),
                ("fake_cover", int(fake_cover)),
                ("fake_field_c", as_int(s.get("oracle_fake_loading_field_c"))),
                ("fake_field_10", as_int(s.get("oracle_fake_loading_field_10"))),
                ("player_present", int(present)),
                ("can_move", int(can_move)),
                ("moved_frames", as_int(s.get("oracle_move_probe_moved_frames"), 0)),
            )
            boot_playable_before_reload = (
                current_epoch == (0, 0) and present and can_move
            )
            if boot_playable_before_reload:
                loading_active = False
                loading_progress_signature = (
                    "boot_playable_before_reload",
                    current_epoch,
                )
            elif bar_progress_available:
                loading_active = True
                bar_terminal = as_int(s.get("oracle_loading_bar_current_terminal"), 0)
                live_bar_fields = (
                    ("deser", current_epoch[0]),
                    ("activate", current_epoch[1]),
                    ("bar_frame", bar_current_frame),
                    ("bar_progress_permille", bar_progress_permille),
                    ("bar_this", as_int(s.get("oracle_loading_screen_last_this"))),
                    ("bar_data", as_int(s.get("oracle_loading_screen_last_data"))),
                    ("bar_max_frame", bar_max_frame),
                    ("bar_terminal_current", bar_terminal),
                )
                if bar_terminal:
                    last_bar_epoch = current_epoch
                    last_terminal_bar_fields = live_bar_fields
                else:
                    last_bar_epoch = None
                    last_terminal_bar_fields = None
                loading_progress_signature = (
                    "loading_bar_plus_semaphores",
                    *live_bar_fields,
                    *handoff_progress_fields,
                )
            elif (
                last_bar_epoch == current_epoch and last_terminal_bar_fields is not None
            ):
                loading_active = True
                loading_progress_signature = (
                    "loading_bar_terminal_plus_semaphores",
                    *last_terminal_bar_fields,
                    *handoff_progress_fields,
                )
            else:
                loading_active = bool(fake_cover or s.get("oracle_now_loading")) and (
                    as_int(activate, 0) > 0
                    or as_int(deser, 0) >= 1
                    or request_code >= 0
                    or mms_state >= 0
                )
                loading_progress_signature = (
                    "native_load_fallback",
                    as_int(deser, 0),
                    as_int(activate, 0),
                    as_int(nav_stage, 0),
                    request_code,
                    mms_state,
                    as_int(s.get("oracle_stepfinish_finalize_substate_12a")),
                    as_int(s.get("oracle_now_loading")),
                    int(fake_cover),
                    as_int(s.get("oracle_fake_loading_field_c")),
                    as_int(s.get("oracle_fake_loading_field_10")),
                )
            loading_progress_source = str(loading_progress_signature[0])
            if loading_active:
                if loading_progress_signature != last_loading_progress_signature:
                    last_loading_progress_signature = loading_progress_signature
                    last_loading_progress_at = now
                elif last_loading_progress_at is not None:
                    loading_stall_seconds = now - last_loading_progress_at
                    if loading_stall_seconds > LOADING_PROGRESS_STALL_SECONDS:
                        loading_stall_signature = loading_progress_signature
                        loading_stall_source = loading_progress_source
                        result = "LOADING_PROGRESS_STALLED_10S"
                        break
            else:
                last_loading_progress_signature = None
                last_loading_progress_at = None
                loading_stall_seconds = 0.0

            ep = epochs.setdefault(
                int(deser),
                {
                    "first_seen": elapsed,
                    "ever_ready": False,
                    "ever_moved": False,
                    "last": None,
                },
            )
            ep["last"] = s
            if render_ready:
                ep["ever_ready"] = True
            if can_move:
                ep["ever_moved"] = True

            # Mandatory portrait capture at the frozen-load view: a reload in progress (deser>=1),
            # cover up, not yet render-ready -- the exact moment the user sees the failure.
            if (
                not portrait_captured
                and deser >= 1
                and fake_cover
                and not render_ready
                and present
            ):
                capture_portrait(args.artifact_dir)
                portrait_captured = True

            # Success = a load proves movement (can_move latched: >=60 consecutive frames of injected
            # motion). For the full sequence (--require-reload-move) it must be a RELOAD (deser>=1) --
            # the user's "third time they can move" -- so load1 moving does NOT end the run; the driver
            # keeps going through the reloads. Stricter --require-reload-settled also requires the native
            # MoveMap handoff to finish (requestCode==2 and no live MoveMapStep), because load2 can now
            # prove movement while the loading/render handoff is still stuck at requestCode=1/mms18.
            reload_epoch = as_int(deser) >= 1
            reload_move = can_move and reload_epoch
            reload_settled = (
                reload_move
                and as_int(s.get("oracle_stepfinish_request_code")) == 2
                and as_int(s.get("oracle_stepfinish_mms_state")) == -1
            )
            if args.require_reload_settled:
                if reload_settled:
                    result = "RELOAD_SETTLED"
                    break
            elif can_move and (not args.require_reload_move or reload_epoch):
                result = "MOVEMENT_PROVEN"
                break
            # TEARDOWN ON UNEXPECTED FAILURE (user 2026-07-18): if the boot never reaches an in-world
            # player within the boot budget, do NOT idle to the cap -- fail fast and tear down.
            if first_present_at is None and elapsed >= BOOT_TIMEOUT_SECONDS:
                result = "BOOT_TIMEOUT_NO_INWORLD"
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
                    f"finalize12a={s.get('oracle_stepfinish_finalize_substate_12a')} "
                    f"fake_cover={fake_cover} switch_idx={s.get('sq_repro_switch_index')} "
                    f"havok={s.get('oracle_havok_pos')}",
                    flush=True,
                )
        _POLL_WAIT.wait(POLL_SECONDS)

    # Snapshot artifacts before teardown clears live state.
    for name in (
        "er-effects-telemetry.json",
        "er-effects-autoload-debug.log",
        "er-reload-trace.log",
    ):
        src = args.game_dir / name
        if src.exists():
            with contextlib.suppress(OSError):
                (args.artifact_dir / name).write_bytes(src.read_bytes())

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
    ]
    if loading_stall_signature is not None:
        lines.extend(
            [
                "## Loading progress stall guard",
                f"- source: {loading_stall_source}",
                f"- stalled_for_seconds: {loading_stall_seconds:.1f}",
                f"- stale_progress_signature: {loading_stall_signature}",
                "",
            ]
        )
    lines.extend(
        [
            "## Driver progress (attempt vs complete)",
            f"- reload ATTEMPTS started (profile_load_activate_count): **{max_activate}**",
            f"- reload COMPLETIONS committed (max fresh_deser epoch): **{completions}**",
            f"- furthest menu-nav stage the driver reached (max sq_repro_state): **{max_nav_label}** ({max_nav_stage})",
            "  - reaching CONFIRM(5) = a load was attempted; stalling below it (e.g. TO_PROFILE(3)) = the",
            "    driver never drove the menu far enough to START the next load.",
            "",
            "## Per-load (fresh_deser epoch) settled signature",
            "",
        ]
    )
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
            f"finalize12a: {s.get('oracle_stepfinish_finalize_substate_12a')}  "
            f"now_loading: {s.get('oracle_now_loading')}  "
            f"fake_cover: {s.get('oracle_fake_loading_any_visible')}"
        )
        lines.append(
            f"- havok_pos: {s.get('oracle_havok_pos')}  play_time_ms: {s.get('oracle_play_time_ms')}"
        )
        lines.append("")
    success_results = {"MOVEMENT_PROVEN", "RELOAD_SETTLED"}
    if result == "RELOAD_SETTLED":
        verdict = "PASS (a reload PROVED movement and native MoveMap settled: requestCode==2, mms=-1)"
    elif result == "MOVEMENT_PROVEN":
        verdict = "PASS (a load PROVED movement: >=60 frames of injected motion)"
    else:
        verdict = (
            "FAIL / incomplete (required movement/settled reload proof was not reached)"
        )
    lines.append(f"## Verdict: {verdict}")
    args.report.write_text("\n".join(lines), encoding="utf-8")
    print("\n".join(lines))
    return 0 if result in success_results else 1


if __name__ == "__main__":
    sys.exit(main())
