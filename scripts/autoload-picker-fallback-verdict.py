#!/usr/bin/env python3
"""RAM verdict for the autoload -> save-picker fallback (user 2026-08-26).

Scores one runtime run purely from `oracle_*` telemetry. No screenshots, no user
adjudication: the run either shows the dead end becoming a picker and the pick
superseding it, or it does not.

The feature under test: when the autoload finds its Continue slot unloadable it must
reject the save, arm the picker late (post-boot), and let the pick supersede the bad
selection. The old behavior spun on that branch forever, so "picker armed at boot"
is not this feature -- arming must happen strictly after the first rejection, which
is why every gate below is ordered against the rejection row.

Usage:
    python3 scripts/autoload-picker-fallback-verdict.py <telemetry.json>
    python3 scripts/autoload-picker-fallback-verdict.py --selftest
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_common import load_rows  # noqa: E402

# The autoload's own count of "Continue slot fingerprinted empty-like -> rejected".
# Emitted by the escalation branch in product_continue.rs. Absent on a DLL built before
# the feature, which is reported as UNPROVEN rather than silently passing.
REJECT_FIELD = "oracle_autoload_empty_slot_rejections"


def _rows_from(path: Path) -> list[dict]:
    """Accept both shapes the runtime writes.

    `er-quickload-telemetry.json` as the DLL leaves it on disk is one dict -- the last
    snapshot, not a timeseries. The watcher-collected artifact is a list of per-poll
    dicts. Passing the live file to `load_rows` yields zero rows and the run scores
    UNPROVEN for a reason that has nothing to do with the feature, which is exactly the
    kind of false negative that gets read as "the fix did not work".
    """
    rows = load_rows(path)
    if rows:
        return rows
    try:
        blob = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (OSError, ValueError):
        return []
    if isinstance(blob, dict):
        return [blob]
    return [r for r in blob if isinstance(r, dict)] if isinstance(blob, list) else []


def _num(row: dict, key: str, default: int = 0) -> int:
    v = row.get(key, default)
    if isinstance(v, bool):
        return int(v)
    return v if isinstance(v, (int, float)) else default


def _truthy(row: dict, key: str) -> bool:
    v = row.get(key)
    return v is True or (isinstance(v, (int, float)) and v > 0)


def _first_index(rows: list[dict], pred) -> int | None:
    for i, r in enumerate(rows):
        if pred(r):
            return i
    return None


def evaluate(rows: list[dict]) -> tuple[list[tuple[str, str, str]], str]:
    """Return ([(gate, verdict, detail)], overall). Verdict is PASS/FAIL/UNPROVEN."""
    gates: list[tuple[str, str, str]] = []
    if not rows:
        return ([("rows", "UNPROVEN", "no telemetry rows")], "UNPROVEN")

    # --- Gate 0: no MessageBoxDialog, ever. AGENTS.md: any msgbox is a hard trigger.
    worst_box = max(_num(r, "oracle_msgbox_total_builds") for r in rows)
    gates.append((
        "no_messagebox",
        "PASS" if worst_box == 0 else "FAIL",
        f"oracle_msgbox_total_builds max={worst_box} (must be 0)",
    ))

    # --- Gate 1: the dead end was actually reached (else the run never tested anything).
    if REJECT_FIELD not in rows[-1] and not any(REJECT_FIELD in r for r in rows):
        gates.append((
            "dead_end_reached", "UNPROVEN",
            f"{REJECT_FIELD} absent -- DLL predates the feature; nothing to score",
        ))
        return gates, "UNPROVEN"
    rej_i = _first_index(rows, lambda r: _num(r, REJECT_FIELD) > 0)
    if rej_i is None:
        gates.append((
            "dead_end_reached", "UNPROVEN",
            f"{REJECT_FIELD} never rose -- the slot loaded fine, so the fallback was never exercised",
        ))
        return gates, "UNPROVEN"
    t_rej = rows[rej_i].get("t_ms")
    gates.append((
        "dead_end_reached", "PASS",
        f"slot rejected as empty-like at t={t_rej}ms ({REJECT_FIELD}={_num(rows[rej_i], REJECT_FIELD)})",
    ))

    # --- Gate 2: picker armed late. Arming at boot is the old no-save path, not this feature.
    if len(rows) == 1:
        # One snapshot has no ordering to read, and `armed` is a live flag that the pick
        # clears -- so a run that armed, drew and was picked reports armed=0, identical to
        # one that never armed at all. Judge it by the cumulative draw counter instead and
        # say plainly that the ordering half is unproven rather than inventing a verdict.
        drew = _num(rows[0], "oracle_save_picker_overlay_draw_hits")
        gates.append((
            "picker_armed_late", "PASS" if drew > 0 else "FAIL",
            f"snapshot: draw_hits={drew} "
            + ("(picker owned the screen at some point)" if drew > 0
               else "(picker never composited -- it never armed)")
            + "; arm-vs-rejection ORDERING is unproven from a single snapshot -- "
              "read the escalation timestamps in er-quickload-autoload-debug.log",
        ))
        gates.append((
            "picker_drew", "PASS" if drew > 0 else "FAIL",
            f"oracle_save_picker_overlay_draw_hits={drew}",
        ))
        return _tail_gates(rows, gates)

    armed_at_boot = _truthy(rows[0], "oracle_save_picker_overlay_armed")
    arm_i = _first_index(rows, lambda r: _truthy(r, "oracle_save_picker_overlay_armed"))
    if armed_at_boot:
        gates.append((
            "picker_armed_late", "FAIL",
            "picker was already armed on the first row -- that is the boot no-save path, "
            "not the post-rejection fallback; re-run with a save the autoload will accept at boot",
        ))
    elif arm_i is None:
        gates.append((
            "picker_armed_late", "FAIL",
            "oracle_save_picker_overlay_armed never became true after the rejection -- "
            "the dead end is still a dead end",
        ))
    elif arm_i < rej_i:
        gates.append((
            "picker_armed_late", "FAIL",
            f"picker armed at row {arm_i} BEFORE the rejection at row {rej_i} -- ordering wrong",
        ))
    else:
        gates.append((
            "picker_armed_late", "PASS",
            f"armed at t={rows[arm_i].get('t_ms')}ms, after the rejection at t={t_rej}ms",
        ))

    # --- Gate 3: it actually composited. Armed != on screen; draw hits are the pixels.
    base_draw = _num(rows[rej_i], "oracle_save_picker_overlay_draw_hits")
    peak_draw = max(_num(r, "oracle_save_picker_overlay_draw_hits") for r in rows[rej_i:])
    gates.append((
        "picker_drew", "PASS" if peak_draw > base_draw else "FAIL",
        f"oracle_save_picker_overlay_draw_hits {base_draw} -> {peak_draw} after rejection "
        f"({'composited' if peak_draw > base_draw else 'NEVER drew -- armed but invisible'})",
    ))

    return _tail_gates(rows, gates)


def _tail_gates(rows: list[dict], gates: list) -> tuple[list, str]:
    """Gates 4-6, shared by timeseries and snapshot mode (all cumulative counters)."""
    # --- Gate 4: a pick was taken.
    picks = max(_num(r, "oracle_save_picker_overlay_pick_count") for r in rows)
    rejects = max(_num(r, "oracle_save_picker_overlay_pick_reject_count") for r in rows)
    gates.append((
        "pick_taken", "PASS" if picks >= 1 else "FAIL",
        f"pick_count={picks} pick_reject_count={rejects}"
        + ("" if picks >= 1 else " -- no save was chosen"),
    ))

    # --- Gate 5: the pick superseded the bad selection and a real character loaded.
    # stats compositing is the loaded-character oracle: stats and picker are mutually
    # exclusive by construction, so stats drawing proves the picker released and the
    # game thread built readable lines from an actual character.
    stats_built = max(_num(r, "oracle_stats_text_built") for r in rows)
    stats_drew = max(_num(r, "oracle_overlay_stats_draw_hits") for r in rows)
    name = next((r.get("oracle_char_name") for r in reversed(rows)
                 if r.get("oracle_char_name")), None)
    ok5 = stats_built > 0 and stats_drew > 0
    gates.append((
        "supersede_loaded_character", "PASS" if ok5 else "FAIL",
        f"stats_text_built={stats_built} overlay_stats_draw_hits={stats_drew} char_name={name!r}"
        + ("" if ok5 else " -- no real character rendered after the pick"),
    ))

    # --- Gate 5b: the title-time deserializer must never have run.
    # 0x14067b290 has exactly one native caller, CS::MoveMapStep::DoSaveStuff, reachable
    # only from the in-world MoveMapStep::Update. Calling it from the boot title reads the
    # save stream from position=32 and dispatches gaitemInsTable[-1] -> AV at 0x67141a.
    # Four separate attempts to gate its preconditions each moved the fault one step later
    # without removing it, so the only passing value is zero: the picked save must reach the
    # world down the native Continue path, exactly like the default save already does.
    deser_field = "oracle_title_time_deser_calls"
    if any(deser_field in r for r in rows):
        calls = max(_num(r, deser_field) for r in rows)
        gates.append((
            "no_title_time_deser",
            "PASS" if calls == 0 else "FAIL",
            f"{deser_field}={calls} (must be 0 -- a picked save routed through the "
            "title-time deserialize instead of the native Continue path)",
        ))
    else:
        gates.append((
            "no_title_time_deser", "UNPROVEN",
            f"{deser_field} absent -- DLL predates the routing fix; cannot tell which "
            "branch the picked save took",
        ))

    # --- Gate 5c: the boot check validated the container the runtime actually opens.
    # The 2026-08-26 root cause: under Seamless the game opens ER0000.co2, but
    # default_save_file_for_steam_id64 falls back to ER0000.sl2 when the .co2 holds no
    # character -- so boot reported default-user-save (+98ms) on a container ersc.dll never
    # reads. missing_save_selection_pending() was therefore false, should_hold_save_check never
    # held, the save-data job passed through at +14s with a blank ProfileSummary, and the picker
    # armed ~1076s later against a title that had already spent its menu-open attempts. Every
    # downstream symptom this file gates on descends from that one mismatch, which is why it gets
    # a gate of its own: it cost two runs while staying completely invisible.
    match_field = "oracle_boot_save_container_matches_runtime"
    if any(match_field in r for r in rows):
        matched = max(_num(r, match_field) for r in rows)
        gates.append((
            "boot_container_matches_runtime",
            "PASS" if matched == 1 else "FAIL",
            f"{match_field}={matched} "
            + ("(boot validated the container the runtime opens)" if matched == 1
               else "(MISMATCH -- boot accepted a container the runtime will not read; the "
                    "save-check hold cannot arm and the boot runs on with a blank summary)"),
        ))
    else:
        gates.append((
            "boot_container_matches_runtime", "UNPROVEN",
            f"{match_field} absent -- DLL predates the boot-container fix",
        ))

    # --- Gate 6: world readiness.
    #
    # This gate used to require `oracle_can_move`, and that was wrong. That oracle latches only
    # after >=60 consecutive frames of injected-forward havok motion -- its own comment calls it
    # the "input-causes-movement gate". It proves the movement-injection harness works. A run
    # where a human loads a character and walks around never arms that probe, so `can_move` stays
    # false and `move_probe_moved_frames` stays 0 no matter how thoroughly the load succeeded.
    # Demanding it here failed every honest user-driven run and would have blocked a PR whose
    # load was demonstrably fine (2026-08-26: char_name="Ordinary Bean", grounded, real havok
    # position, game clock running -- and can_move false).
    #
    # What actually distinguishes "in the world" from "still loading", without assuming anyone
    # injected input: the player exists, physics owns it, and the game clock is running.
    present = any(_truthy(r, "oracle_player_present") for r in rows)
    grounded = any(_truthy(r, "oracle_grounded") for r in rows)
    clock = any(_truthy(r, "oracle_play_time_live") for r in rows)
    can_move = any(_truthy(r, "oracle_can_move") for r in rows)
    ok6 = present and (grounded or clock)
    gates.append((
        "world_ready", "PASS" if ok6 else "FAIL",
        f"player_present={present} grounded={grounded} play_time_live={clock}"
        + (f" (can_move={can_move}: injected-probe signal, not required)" if not can_move else ""),
    ))

    verdicts = [v for _, v, _ in gates]
    overall = "FAIL" if "FAIL" in verdicts else ("UNPROVEN" if "UNPROVEN" in verdicts else "PASS")
    return gates, overall


def _report(gates, overall) -> None:
    width = max(len(g) for g, _, _ in gates)
    for gate, verdict, detail in gates:
        print(f"  [{verdict:8s}] {gate:<{width}}  {detail}")
    print(f"\nVERDICT: {overall}")


def _selftest() -> int:
    base = {"t_ms": 0, "oracle_msgbox_total_builds": 0}

    def row(t, **kw):
        r = dict(base)
        r["t_ms"] = t
        r.update(kw)
        return r

    failures = []

    # 1. A DLL without the feature scores UNPROVEN, never pass.
    g, o = evaluate([row(0), row(100)])
    if o != "UNPROVEN":
        failures.append(f"old-DLL run should be UNPROVEN, got {o}")

    # 2. The old dead-end behavior (rejects, never arms) must fail.
    g, o = evaluate([row(0, **{REJECT_FIELD: 0}), row(50, **{REJECT_FIELD: 3})])
    if o != "FAIL":
        failures.append(f"spin-forever run should FAIL, got {o}")

    # 3. The boot no-save path must not be mistaken for the feature.
    g, o = evaluate([
        row(0, **{REJECT_FIELD: 0}, oracle_save_picker_overlay_armed=True),
        row(50, **{REJECT_FIELD: 1}, oracle_save_picker_overlay_armed=True),
    ])
    if o != "FAIL":
        failures.append(f"boot-armed picker should FAIL (not the feature), got {o}")
    if not any(gate == "picker_armed_late" and v == "FAIL" for gate, v, _ in g):
        failures.append("boot-armed picker should fail specifically on picker_armed_late")

    # 4. Armed but never composited must fail (armed != visible).
    g, o = evaluate([
        row(0, **{REJECT_FIELD: 0}),
        row(50, **{REJECT_FIELD: 1}),
        row(90, **{REJECT_FIELD: 1}, oracle_save_picker_overlay_armed=True,
            oracle_save_picker_overlay_draw_hits=0),
    ])
    if not any(gate == "picker_drew" and v == "FAIL" for gate, v, _ in g):
        failures.append("armed-but-never-drew should fail picker_drew")

    # 5. A MessageBoxDialog anywhere is a hard fail even on an otherwise good run.
    good = [
        row(0, **{REJECT_FIELD: 0}),
        row(50, **{REJECT_FIELD: 1}),
        row(90, **{REJECT_FIELD: 1}, oracle_save_picker_overlay_armed=True,
            oracle_save_picker_overlay_draw_hits=12),
        row(200, **{REJECT_FIELD: 1}, oracle_save_picker_overlay_pick_count=1,
            oracle_stats_text_built=5, oracle_overlay_stats_draw_hits=30,
            oracle_char_name="angrE", oracle_player_present=True,
            oracle_grounded=True, oracle_play_time_live=True),
    ]
    # A run that is green on every other gate but predates the routing counter is
    # UNPROVEN, not PASS: without oracle_title_time_deser_calls there is no way to tell
    # whether the picked save reached the world natively or through the title-time
    # deserialize that crashes. Scenario 8 below is the same run with the counter at 0.
    g, o = evaluate(good)
    if o != "UNPROVEN":
        failures.append(f"good run lacking the deser counter should be UNPROVEN, got {o}: {g}")
    boxed = [dict(r) for r in good]
    boxed[2]["oracle_msgbox_total_builds"] = 1
    g, o = evaluate(boxed)
    if o != "FAIL":
        failures.append(f"msgbox run should FAIL, got {o}")

    # 6. Reached the picker but never got a character back = fail, not pass.
    half = [dict(r) for r in good]
    half[3] = row(200, **{REJECT_FIELD: 1}, oracle_save_picker_overlay_pick_count=1)
    g, o = evaluate(half)
    if o != "FAIL":
        failures.append(f"picked-but-never-loaded should FAIL, got {o}")

    # 7. The routing regression: everything else green, but the title-time deser ran.
    routed = [dict(r) for r in good]
    for r in routed:
        r["oracle_title_time_deser_calls"] = 0
    routed[3]["oracle_title_time_deser_calls"] = 1
    g, o = evaluate(routed)
    if o != "FAIL":
        failures.append(f"title-time deser run should FAIL, got {o}")
    if not any(gate == "no_title_time_deser" and v == "FAIL" for gate, v, _ in g):
        failures.append("title-time deser should fail specifically on no_title_time_deser")

    # 8. Same run with the counter at 0 passes the new gate.
    clean = [dict(r) for r in good]
    for r in clean:
        r["oracle_title_time_deser_calls"] = 0
    # Clean routing but no boot-container field yet: UNPROVEN, not pass. Scenario 11 is the
    # same run with the container field present and matching, which is the real all-green case.
    g, o = evaluate(clean)
    if o != "UNPROVEN":
        failures.append(f"clean-routing run lacking the container field should be UNPROVEN, got {o}")
    if not any(gate == "no_title_time_deser" and v == "PASS" for gate, v, _ in g):
        failures.append("clean-routing run should still PASS the deser gate")

    # 9. A DLL without the counter must not silently pass that gate.
    g, o = evaluate(good)
    if not any(gate == "no_title_time_deser" and v == "UNPROVEN" for gate, v, _ in g):
        failures.append("absent deser counter should be UNPROVEN, not PASS")

    # 10. The root cause: boot validated a container the runtime does not open.
    mism = [dict(r) for r in clean]
    for r in mism:
        r["oracle_boot_save_container_matches_runtime"] = 0
    g, o = evaluate(mism)
    if o != "FAIL":
        failures.append(f"boot container mismatch should FAIL, got {o}")
    if not any(gate == "boot_container_matches_runtime" and v == "FAIL" for gate, v, _ in g):
        failures.append("mismatch should fail specifically on boot_container_matches_runtime")

    # 11. Same run with the container matching passes.
    okc = [dict(r) for r in clean]
    for r in okc:
        r["oracle_boot_save_container_matches_runtime"] = 1
    g, o = evaluate(okc)
    if o != "PASS":
        failures.append(f"matching-container run should PASS, got {o}: {g}")

    # 12. A human-driven run: character in world, but nothing injected input, so can_move is
    # false and move_probe_moved_frames is 0. This must pass -- it is the shape of every run the
    # user actually performs, and the old gate failed all of them.
    human = [dict(r) for r in okc]
    human[3]["oracle_can_move"] = False
    human[3]["oracle_move_probe_moved_frames"] = 0
    g, o = evaluate(human)
    if o != "PASS":
        failures.append(f"human-driven run (can_move false) should PASS, got {o}: {g}")

    # 13. ...but a run with no player at all still fails.
    noplayer = [dict(r) for r in okc]
    noplayer[3]["oracle_player_present"] = False
    noplayer[3]["oracle_grounded"] = False
    noplayer[3]["oracle_play_time_live"] = False
    g, o = evaluate(noplayer)
    if o != "FAIL":
        failures.append(f"run with no player should FAIL, got {o}")

    if failures:
        print("SELFTEST FAILED:")
        for f in failures:
            print("  -", f)
        return 1
    print("selftest: 13 scenarios OK (missing-field, spin-forever, boot-armed, "
          "armed-not-drawn, msgbox, picked-but-not-loaded, title-deser-ran, "
          "clean-routing, deser-counter-absent, boot-container-mismatch, "
          "boot-container-ok, human-driven-run, no-player)")
    return 0


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__)
        return 2
    if argv[1] == "--selftest":
        return _selftest()
    path = Path(argv[1])
    if not path.exists():
        print(f"no telemetry at {path}")
        return 2
    rows = _rows_from(path)
    print(f"{path} -- {len(rows)} row(s){' (single snapshot)' if len(rows) == 1 else ''}\n")
    gates, overall = evaluate(rows)
    _report(gates, overall)
    return 0 if overall == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
