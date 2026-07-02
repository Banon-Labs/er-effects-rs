#!/usr/bin/env python3
"""Fail-fast oracle: did the System-Quit->Load-Profile character SWITCH load the
character of the PICKED slot, or the wrong (usually original/most-recent) one?

Motivation (2026-07-02, user-reported + ground-truth confirmed): after selecting a
DIFFERENT save slot in the in-world System->Quit->Load-Profile menu, the world
reloads the ORIGINAL most-recent character instead of the picked slot's character.
The picked slot is recorded (`system_quit_quickload_selected_slot`) but the actual
title-time load commits via native Continue (loads GameMan+0xac0 = the most-recent
slot), so the pick never reaches the load.

This oracle turns that into a run-stopping semaphore so a probe fails FAST instead
of burning the whole runtime cap and eyeballing a screenshot:

  * EXPECTED identity = the PICKED slot decoded from the staged save (the same save
    the probe copies in), via the proven `save-slot-oracle.py` decoder.
  * OBSERVED identity = the loaded character read from RAM by the DLL and published
    in telemetry (`oracle_char_name`, `oracle_saved_map_c30`, `oracle_char_runes`,
    `oracle_char_level`, `oracle_char_stats`). These are CS::PlayerGameData reads.
  * The check ARMS only after the switch has handed off (a return-title was
    requested) AND a real, complete character is resident -- so it never fires on
    the FIRST (correct) load or mid-teardown.

Verdict + exit code:
  0  -> match, OR not-yet-armed (keep waiting)          [no failure]
  2  -> ARMED and the loaded character is NOT the picked slot's character
        (fail fast: switch loaded the wrong character)  [FAILURE]
  1  -> usage / IO / decode error

The loaded side is a RAM semaphore; the expected side is the exact save staged into
the run. Screenshots are never consulted. Designed to be polled against the live
`er-effects-telemetry.json` (rewritten every ~250ms) during a no-teardown probe, or
run once post-hoc against a captured telemetry file.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

_SCRIPT_DIR = Path(__file__).resolve().parent


def _load_save_decoder():
    """Import the hyphen-named save-slot-oracle.py as a module (proven decoder)."""
    path = _SCRIPT_DIR / "save-slot-oracle.py"
    spec = importlib.util.spec_from_file_location("save_slot_oracle", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load save decoder from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _as_int(value: Any, default: int = -1) -> int:
    try:
        if isinstance(value, bool):
            return int(value)
        if isinstance(value, str):
            return int(value, 0)
        return int(value)
    except (TypeError, ValueError):
        return default


def _name_empty_like(value: Any) -> bool:
    if not isinstance(value, str):
        return True
    stripped = value.strip()
    return stripped == "" or stripped == "_"


def decode_slot_identity(decoder, save_bytes: bytes, save_path: Path, slot: int) -> dict[str, Any]:
    """Decode one save slot into the identity subset the telemetry oracle exposes."""
    decoded = decoder.decode_save_slot(save_bytes, save_path, slot).get("decoded_fields") or {}
    return {
        "slot": slot,
        "name": decoded.get("name"),
        "name_len": decoded.get("name_len"),
        "level": decoded.get("level"),
        "health": decoded.get("health"),
        "runes": decoded.get("runes"),
        "stats": decoded.get("stats"),
        "saved_map_c30": decoded.get("saved_map_c30"),
        "empty_like": decoded.get("name_empty_like", True),
    }


def observed_identity(telemetry: dict[str, Any]) -> dict[str, Any]:
    return {
        "name": telemetry.get("oracle_char_name"),
        "name_len": _as_int(telemetry.get("oracle_char_name_len")),
        "level": _as_int(telemetry.get("oracle_char_level")),
        "health": _as_int(telemetry.get("oracle_char_current_hp")),
        "runes": _as_int(telemetry.get("oracle_char_runes")),
        "stats": telemetry.get("oracle_char_stats"),
        "saved_map_c30": _as_int(telemetry.get("oracle_saved_map_c30")),
    }


# Strong discriminators: name + saved map block + runes. Two distinct real slots in
# a multi-slot save differ on at least one of these (verified for the 25-invades
# gold save: e.g. slot 4 'Speed Bean' c30=0x200b0000 runes=994451292 vs slot 5
# 'Patches' c30=0xe000000 runes=994431680). Level/stats are corroborating only
# (many slots share level 139), so they are NOT used to declare a mismatch.
def identity_matches(expected: dict[str, Any], observed: dict[str, Any]) -> bool:
    return bool(
        observed.get("name") == expected.get("name")
        and _as_int(observed.get("name_len"), -1) == _as_int(expected.get("name_len"), -2)
        and _as_int(observed.get("saved_map_c30"), -1) == _as_int(expected.get("saved_map_c30"), -2)
        and _as_int(observed.get("runes"), -1) == _as_int(expected.get("runes"), -2)
    )


def stable_world_loaded(telemetry: dict[str, Any]) -> bool:
    """A real character is resident in a STABLE, finished-loading world -- not a
    loading screen and not the lingering pre-teardown character mid-transition.

    Judging identity only here is what makes this a reliable fix-gate: during the
    switch's return-title->reload window the original character is briefly still
    resident (world tearing down) and `oracle_now_loading` is 1; if we judged then,
    a CORRECT fix whose reload is still on the loading screen would false-fail.
    Requiring `oracle_now_loading == 0` means we only score the FINAL loaded world."""
    player_seen = telemetry.get("oracle_player_present") is True or telemetry.get("player_available") is True
    not_loading = _as_int(telemetry.get("oracle_now_loading"), 1) == 0
    loaded_signal = (
        telemetry.get("oracle_block_id_valid") is True
        or isinstance(telemetry.get("oracle_havok_pos"), list)
        or _as_int(telemetry.get("oracle_saved_map_c30"), -1) not in (-1, 0)
    )
    real_name = not _name_empty_like(telemetry.get("oracle_char_name")) and _as_int(telemetry.get("oracle_char_level"), 0) > 0
    return bool(player_seen and not_loading and loaded_signal and real_name)


def switch_handed_off(telemetry: dict[str, Any]) -> bool:
    """The switch requested a return-to-title (so the SECOND load is the one under
    test). Before this, the resident character is the first/correct load and must
    NOT be judged against the picked slot."""
    return (
        _as_int(telemetry.get("system_quit_quickload_return_title_request_count"), 0) > 0
        or _as_int(telemetry.get("system_quit_return_title_final_functor_call_count"), 0) > 0
    )


def evaluate(save_path: Path, telemetry: dict[str, Any]) -> dict[str, Any]:
    decoder = _load_save_decoder()
    save_bytes = save_path.read_bytes()

    picked = _as_int(telemetry.get("system_quit_quickload_selected_slot"), -1)
    handed_off = switch_handed_off(telemetry)
    stable = stable_world_loaded(telemetry)
    observed = observed_identity(telemetry)

    # Decode every real slot so the failure can name the ACTUALLY-loaded slot, and so
    # a visual-cursor/profile-id mapping surprise is visible rather than assumed.
    all_slots = [decode_slot_identity(decoder, save_bytes, save_path, s) for s in range(10)]
    matched_loaded_slots = [s["slot"] for s in all_slots if identity_matches(s, observed)]

    expected = None
    if 0 <= picked < 10:
        expected = decode_slot_identity(decoder, save_bytes, save_path, picked)

    armed = bool(handed_off and stable and expected is not None)
    is_match = bool(expected is not None and identity_matches(expected, observed))
    wrong_character = bool(armed and not is_match)
    correct_character = bool(armed and is_match)

    if not armed:
        reason = "not_armed_waiting"
    elif is_match:
        reason = "correct_character_loaded"
    else:
        reason = "wrong_character_loaded"

    return {
        "armed": armed,
        "handed_off": handed_off,
        "stable_world_loaded": stable,
        "picked_slot": picked,
        "expected": expected,
        "observed": observed,
        "match": is_match,
        "wrong_character": wrong_character,
        "correct_character": correct_character,
        "actual_loaded_slots": matched_loaded_slots,
        "reason": reason,
        "save_path": str(save_path),
    }


def _selftest() -> int:
    """Validate both branches against synthetic telemetry derived from THIS save,
    so the oracle is proven before it gates any run."""
    import tempfile

    save_env = None
    for cand in (
        _SCRIPT_DIR.parent / "save-files" / "25-Invades-patches" / "ER0000.sl2",
    ):
        if cand.is_file():
            save_env = cand
            break
    if save_env is None:
        print("selftest: gold save not found; skipping", file=sys.stderr)
        return 0

    decoder = _load_save_decoder()
    save_bytes = save_env.read_bytes()
    s4 = decode_slot_identity(decoder, save_bytes, save_env, 4)
    s5 = decode_slot_identity(decoder, save_bytes, save_env, 5)

    def telem_for(identity: dict[str, Any], picked: int, now_loading: int = 0) -> dict[str, Any]:
        return {
            "system_quit_quickload_selected_slot": picked,
            "system_quit_quickload_return_title_request_count": 1,
            "oracle_now_loading": now_loading,
            "oracle_player_present": True,
            "oracle_block_id_valid": True,
            "oracle_char_name": identity["name"],
            "oracle_char_name_len": identity["name_len"],
            "oracle_char_level": identity["level"],
            "oracle_char_current_hp": identity["health"],
            "oracle_char_runes": identity["runes"],
            "oracle_char_stats": identity["stats"],
            "oracle_saved_map_c30": identity["saved_map_c30"],
        }

    failures = []
    # Case A: picked slot 4 but slot 5 loaded in a STABLE world -> wrong_character (the real bug).
    a = evaluate(save_env, telem_for(s5, picked=4))
    if not (a["armed"] and a["wrong_character"] and not a["match"] and a["actual_loaded_slots"] == [5]):
        failures.append(f"A wrong-char not detected: {a}")
    # Case B: picked slot 4 and slot 4 loaded in a stable world -> correct_character (fix passes).
    b = evaluate(save_env, telem_for(s4, picked=4))
    if not (b["armed"] and b["correct_character"] and b["match"] and not b["wrong_character"]):
        failures.append(f"B correct-char not detected: {b}")
    # Case C: not handed off yet (first load) -> not armed, never a failure.
    c_tel = telem_for(s5, picked=4)
    c_tel["system_quit_quickload_return_title_request_count"] = 0
    c_tel["system_quit_return_title_final_functor_call_count"] = 0
    c = evaluate(save_env, c_tel)
    if c["armed"] or c["wrong_character"]:
        failures.append(f"C armed before handoff: {c}")
    # Case D: handed off but still on the loading screen (now_loading=1) -> NOT armed,
    # so a correct fix mid-reload can't be false-failed on the lingering original char.
    d = evaluate(save_env, telem_for(s5, picked=4, now_loading=1))
    if d["armed"] or d["wrong_character"]:
        failures.append(f"D armed while still loading: {d}")

    if failures:
        for f in failures:
            print("selftest FAIL:", f, file=sys.stderr)
        return 1
    print("selftest OK: wrong-character, match, and pre-handoff cases all correct")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--save", type=Path, help="staged ER0000.sl2 (the save copied into the run)")
    parser.add_argument("--telemetry", type=Path, help="er-effects-telemetry.json to evaluate")
    parser.add_argument("--json", action="store_true", help="emit the full verdict as JSON")
    parser.add_argument("--selftest", action="store_true", help="validate the oracle against the gold save and exit")
    args = parser.parse_args(argv[1:])

    if args.selftest:
        return _selftest()

    if not args.save or not args.telemetry:
        parser.error("--save and --telemetry are required (or use --selftest)")
    if not args.save.is_file():
        print(f"[switch-oracle] save not found: {args.save}", file=sys.stderr)
        return 1
    if not args.telemetry.is_file():
        print(f"[switch-oracle] telemetry not found: {args.telemetry}", file=sys.stderr)
        return 1
    try:
        telemetry = json.loads(args.telemetry.read_text(encoding="utf-8", errors="replace"))
    except Exception as exc:  # partial write during live polling -> treat as not-yet-ready
        print(f"[switch-oracle] telemetry not readable yet ({exc})", file=sys.stderr)
        return 0
    if not isinstance(telemetry, dict):
        print("[switch-oracle] telemetry is not a JSON object", file=sys.stderr)
        return 1

    verdict = evaluate(args.save, telemetry)
    if args.json:
        print(json.dumps(verdict, indent=2, sort_keys=True))
    else:
        exp = verdict["expected"] or {}
        obs = verdict["observed"]
        print(
            f"[switch-oracle] {verdict['reason']} armed={verdict['armed']} "
            f"picked_slot={verdict['picked_slot']} "
            f"expected='{exp.get('name')}'(c30={exp.get('saved_map_c30')}) "
            f"loaded='{obs.get('name')}'(c30={obs.get('saved_map_c30')}) "
            f"actual_loaded_slots={verdict['actual_loaded_slots']}"
        )
    # Exit codes: 2 = wrong character loaded (FAIL, stop the run); 0 = correct
    # character loaded (SUCCESS, stop the run); 10 = not armed yet (keep polling).
    if verdict["wrong_character"]:
        return 2
    if verdict["correct_character"]:
        return 0
    return 10


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
