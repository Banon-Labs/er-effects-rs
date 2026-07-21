#!/usr/bin/env python3
"""ORACLE VERDICT emitter (user 2026-07-20): the agent must NOT hand-read logs to explain a run.

Given a run directory (or its telemetry-timeseries.jsonl), print ONE plain-language verdict that says,
per load epoch: the PHASE reached, whether the run was CONTAMINATED (the character moved without any
injected input -- ER accepts UNFOCUSED mouse/click input and agent-owned runs do not block it, so
incidental user movement corrupts the run), and the TEARDOWN reason (timeout window vs
semaphore-complete vs stall). The target shape is exactly the sentence the user asked for, e.g.:

  "I entered load2, the run was corrupted by mouse/click input the game accepted while unfocused
   (we did not block it), and I tore down at the prescribed timeout window."

Contamination is derived from oracle_did_move_frames > 0 while oracle_supplied_movement_input_frames
== 0 (movement with no injected input) and/or oracle_harness_move_verdict == 3 (contaminated). NOTE:
there is no focus/camera field in telemetry yet, so "while unfocused" is the *likely cause*, not a
measured fact, until a window-focus oracle is added -- the verdict says so explicitly.

Usage: python3 scripts/oracle-verdict.py <run-dir | timeseries.jsonl> [--observe-seconds N]
Exit: 0 always (it is a reporter, not a gate).
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

# mms (MoveMapStep step) milestones. 18 = MoveMapStep finalize (the load2 stall point); 19/20 = past
# finalize / world-ready. -1 = MoveMapStep not resolved (pre-world or a native path that doesn't set
# the product mms pointer).
MMS_FINALIZE_STUCK = 18
MMS_WORLD_READY_MIN = 19
# Movement below this many frames is noise (single-frame jitter / physics settle), not real locomotion.
MOVE_NOISE_FRAMES = 3


def load_rows(path: Path) -> list[dict]:
    ts = path / "telemetry-timeseries.jsonl" if path.is_dir() else path
    if not ts.exists():
        return []
    rows = []
    for line in ts.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if line:
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return sorted(rows, key=lambda r: r.get("t_ms", 0))


def _imax(rows: list[dict], key: str) -> int:
    vals: list[int] = []
    for r in rows:
        v = r.get(key)
        if isinstance(v, int):
            vals.append(v)
    return max(vals) if vals else -1


def _idelta(rows: list[dict], key: str) -> int:
    """Delta of a cumulative counter across this epoch's rows (how much it advanced during the epoch)."""
    vals: list[int] = []
    for r in rows:
        v = r.get(key)
        if isinstance(v, int):
            vals.append(v)
    return (max(vals) - min(vals)) if vals else 0


def epoch_verdict(epoch: int, rows: list[dict]) -> str:
    present = any(r.get("oracle_player_present") for r in rows)
    names = {r.get("oracle_char_name") for r in rows if r.get("oracle_char_name")}
    char = next(iter(names), "?") if names else "?"
    mms_max = _imax(rows, "oracle_stepfinish_mms_state")
    did_move = _imax(rows, "oracle_did_move_frames")
    supplied = _imax(rows, "oracle_supplied_movement_input_frames")
    probe_move = _imax(rows, "oracle_move_probe_moved_frames")
    verdict_code = _imax(rows, "oracle_harness_move_verdict")
    play_live = any(r.get("oracle_play_time_live") for r in rows)

    label = f"load{epoch + 1}" if epoch == 0 else f"reload #{epoch}"
    # PHASE
    if mms_max >= MMS_WORLD_READY_MIN:
        phase = f"COMPLETED to world-ready (MoveMapStep reached {mms_max})"
    elif mms_max == MMS_FINALIZE_STUCK:
        phase = "ENTERED but STUCK at MoveMapStep finalize (mms=18, never advanced to 19)"
    elif mms_max >= 0:
        phase = f"mid-load (MoveMapStep reached {mms_max}, not finalize)"
    elif present:
        phase = "player present but MoveMapStep pointer never resolved (native/telemetry-only path)"
    else:
        phase = "never reached a loaded world (no player, mms unresolved)"
    parts = [
        f"{label} (epoch {epoch}): {phase}."
        f" player_present={present} char={char!r} play_time_live={play_live}."
    ]
    # CONTAMINATION -- RawInput RECEPTION is authoritative: did the GAME receive user mouse/keyboard
    # input? The harness injects via the direct-memory inputmgr (never RawInput), so any RawInput event
    # is the USER's. This covers mouse-look (camera) input that char-position cannot see.
    has_rawinput = any("oracle_rawinput_mouse_move_events" in r for r in rows)
    moved = max(did_move, probe_move)
    if has_rawinput:
        mmove = _idelta(rows, "oracle_rawinput_mouse_move_events")
        mbtn = _idelta(rows, "oracle_rawinput_mouse_button_events")
        keys = _idelta(rows, "oracle_rawinput_key_events")
        total = mmove + mbtn + keys
        if total > 0:
            parts.append(
                f"    CONTAMINATION (authoritative -- the game RECEIVED user input): {mmove} mouse-move"
                f" + {mbtn} mouse-button + {keys} keyboard RawInput events this epoch. The harness injects"
                f" via the direct-memory inputmgr (never RawInput), so these are the USER's input reaching"
                f" the game -> this epoch is CORRUPTED and its stall/finish signal is INVALID."
            )
        else:
            parts.append(
                "    CLEAN: the game received 0 user RawInput events (mouse+keyboard) this epoch"
                " -> NOT contaminated (authoritative; covers mouse-look/camera input that char-position"
                " misses)."
            )
    elif verdict_code == 3 or (moved > MOVE_NOISE_FRAMES and supplied == 0):
        parts.append(
            f"    LIKELY CONTAMINATION: character moved ~{moved} frames with {supplied} injected frames"
            f" (old build without the RawInput oracle). Zero injection means the movement was the user's;"
            f" this epoch is UNTRUSTWORTHY."
        )
    else:
        blind = (
            " (old build: no RawInput oracle -- this only tracks CHARACTER POSITION, so mouse-look/camera"
            " contamination is INVISIBLE)"
        )
        trust = (
            " and this load did NOT complete, so its stall signal is UNTRUSTWORTHY until re-run on a"
            " RawInput-oracle build."
            if mms_max < MMS_WORLD_READY_MIN
            else ""
        )
        parts.append(
            f"    no CHAR movement without injection (moved ~{moved}, injected {supplied}){blind}{trust}."
        )
    return "\n".join(parts)


def teardown_reason(rows: list[dict], observe_seconds: float | None) -> str:
    if not rows:
        return "no telemetry -- the process died before writing any (early boot crash)."
    last_ms = rows[-1].get("t_ms", 0)
    last_mms = rows[-1].get("oracle_stepfinish_mms_state")
    if observe_seconds and last_ms >= observe_seconds * 1000 - 6000:
        return (
            f"reached the OBSERVE WINDOW (~{observe_seconds:.0f}s, last sample {last_ms / 1000:.0f}s)"
            f" -- torn down at the prescribed timeout, NOT on a completion/stall semaphore"
            f" (last mms={last_mms})."
        )
    return (
        f"ended at {last_ms / 1000:.0f}s (last mms={last_mms}) -- before the observe window;"
        f" likely a semaphore teardown or the process exited."
    )


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    observe_seconds = None
    for a in sys.argv[1:]:
        if a.startswith("--observe-seconds"):
            observe_seconds = float(a.split("=", 1)[1]) if "=" in a else None
    if not args:
        print(__doc__)
        return 0
    path = Path(args[0])
    rows = load_rows(path)
    run = path.name if path.is_dir() else path.parent.name
    print(f"ORACLE VERDICT (run {run}):")
    if not rows:
        print(f"  {teardown_reason(rows, observe_seconds)}")
        return 0
    ep_set: set[int] = set()
    for r in rows:
        v = r.get("system_quit_continue_confirm_fresh_deser_count")
        if isinstance(v, int):
            ep_set.add(v)
    epochs = sorted(ep_set)
    for e in epochs:
        er = [r for r in rows if r.get("system_quit_continue_confirm_fresh_deser_count") == e]
        print("  " + epoch_verdict(e, er).replace("\n", "\n  "))
    print(f"  TEARDOWN: {teardown_reason(rows, observe_seconds)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
